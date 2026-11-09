//! **World income multipliers** (#141, lane `econ-mult`): the server owner
//! scales each ISK faucet with one number, set at launch, so a closed world
//! earns at the pace they choose (for example, to reach the 1B alliance fee).
//!
//! - **Faucets only.** A multiplier scales ISK *created* from nowhere: an NPC
//!   bounty, an agent mission reward, an NPC buy order's price, an insurance
//!   payout. ISK that moves between players (market fills between
//!   characters, contracts, trades, bounty-pool prizes, corporation tax) is
//!   never scaled. A sink (a fee, a premium, a tax) is never scaled either.
//! - **Four knobs and a master.** `bounty`, `mission`, `npc_buy` and
//!   `insurance` each scale one faucet; `isk` multiplies all four. The
//!   amount a faucet pays is `listed x specific x isk` ([`EconScale::bounty`]
//!   and so on). Every knob defaults to 1.0, where the payout is the exact
//!   old one: the product of two 1.0 values is exactly 1.0 and a multiply by
//!   it changes no float.
//! - **Read once**, at startup, from `--bounty-scale` etc. or the `EVE_*`
//!   variable ([`EconScale::from_flags_or_env`]); a bad value is an error,
//!   never a silent 1.0. It lives in a world slot ([`SLOT`]) like
//!   `game_clock`, set by `Server::bind`; [`of`] reads it.
//! - **A zero is allowed** (the faucet is switched off). The range is
//!   `0..=`[`MAX_SCALE`] **[A]**: 1000 is far above any sane tuning; it
//!   would be falsified by an owner who wants more.
//! - **Caps where a faucet could loop.** An NPC bid scaled up stops at
//!   `max(its 1.0 bid, the type's retail ask)` (`econ_exchange::scaled_bid`,
//!   shared by the book and the offline trader); a raised `npc_buy` also
//!   lifts the refine guard under Tier A asks by the same factor; an
//!   insurance payout (the sum of a loss's shares) stops at a full policy's
//!   price (`service::insurance::scaled_shares`). A dial under 1 is never capped.
//! - **`npc_buy` scales Tier A bids only.** Tier B bids (loot and faction
//!   items, where LP-store goods sell) stay unscaled **[I]**: kill LP is a
//!   fixed amount per ISK destroyed, so a raised loot bid would turn LP into
//!   ISK at the dial.
//! - **Known display mismatch.** The client shows unscaled numbers: the
//!   insurance quote (`GetInsurancePrice`), the mission offer's ISK and the
//!   NPC bounty attributes. Only the wallet sees the scaled amount;
//!   `is_modified` still reflects only the system multiplier.
//! - **What a scaled bounty does not change**: the dynamic bounty system's
//!   pool drains by the *unscaled* amount (the multiplier is the owner's
//!   dial, not a market event); the ESS bank's cut comes out of the scaled
//!   amount, so a scaled bounty scales the ESS payout with it.

use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};

/// The largest value a multiplier may take **[A]**.
pub const MAX_SCALE: f64 = 1000.0;

/// The flags and their fallback variables, in [`EconScale`] field order.
pub const FLAGS: [(&str, &str); 5] = [
    ("--bounty-scale", "EVE_BOUNTY_SCALE"),
    ("--mission-scale", "EVE_MISSION_SCALE"),
    ("--npc-buy-scale", "EVE_NPC_BUY_SCALE"),
    ("--insurance-scale", "EVE_INSURANCE_SCALE"),
    ("--isk-scale", "EVE_ISK_SCALE"),
];

/// The multipliers, each `1.0` by default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EconScale {
    /// NPC bounties, live and offline-settled (`npc::bounty::book_kill`).
    pub bounty: f64,
    /// Agent mission ISK rewards (`mission_rewards::grant`).
    pub mission: f64,
    /// What NPC buy orders pay (`econ_exchange::all_orders`, the trader loop).
    pub npc_buy: f64,
    /// Insurance payouts, not premiums (`service::insurance::pay_out`).
    pub insurance: f64,
    /// The master: multiplies every one above.
    pub isk: f64,
}

impl EconScale {
    /// Every faucet at its old rate.
    pub const ONE: EconScale = EconScale {
        bounty: 1.0,
        mission: 1.0,
        npc_buy: 1.0,
        insurance: 1.0,
        isk: 1.0,
    };

    /// One multiplier's text: a finite number in `0..=MAX_SCALE`.
    pub fn parse(text: &str) -> Result<f64, String> {
        let x: f64 = text
            .trim()
            .parse()
            .map_err(|_| format!("{text:?} is not a number"))?;
        if !x.is_finite() || !(0.0..=MAX_SCALE).contains(&x) {
            return Err(format!("{text:?} is outside 0..={MAX_SCALE}"));
        }
        // "-0" parses as -0.0: a zero is a zero.
        Ok(if x == 0.0 { 0.0 } else { x })
    }

    /// The multipliers from the five flag values (in [`FLAGS`] order) and the
    /// five variable values: the flag when given, else the variable when set
    /// and not empty, else 1.0. A bad value names its source.
    pub fn from_flags_or_env(
        flags: [Option<&str>; 5],
        env: [Option<&str>; 5],
    ) -> Result<EconScale, String> {
        let mut v = [1.0; 5];
        for i in 0..5 {
            let (flag, var) = FLAGS[i];
            let (text, from) = match (flags[i], env[i]) {
                (Some(f), _) => (f, flag),
                (None, Some(e)) if !e.trim().is_empty() => (e, var),
                _ => continue,
            };
            v[i] = EconScale::parse(text).map_err(|e| format!("{from}: {e}"))?;
        }
        Ok(EconScale {
            bounty: v[0],
            mission: v[1],
            npc_buy: v[2],
            insurance: v[3],
            isk: v[4],
        })
    }

    /// [`Self::from_flags_or_env`] reading the real environment.
    pub fn from_cli(flags: [Option<&str>; 5]) -> Result<EconScale, String> {
        let mut env: [Option<String>; 5] = Default::default();
        for (slot, (_, var)) in env.iter_mut().zip(FLAGS) {
            *slot = env_text(var, std::env::var(var))?;
        }
        EconScale::from_flags_or_env(flags, [0, 1, 2, 3, 4].map(|i| env[i].as_deref()))
    }

    /// The bounty factor: `bounty x isk`.
    pub fn bounty(&self) -> f64 {
        self.bounty * self.isk
    }
    /// The mission reward factor: `mission x isk`.
    pub fn mission(&self) -> f64 {
        self.mission * self.isk
    }
    /// The NPC buy-order factor: `npc_buy x isk`.
    pub fn npc_buy(&self) -> f64 {
        self.npc_buy * self.isk
    }
    /// The insurance payout factor: `insurance x isk`.
    pub fn insurance(&self) -> f64 {
        self.insurance * self.isk
    }

    /// One `key=value` line for a log or a report (`tools/econ-report.py`).
    pub fn summary(&self) -> String {
        format!(
            "bounty={} mission={} npc_buy={} insurance={} isk={}",
            self.bounty, self.mission, self.npc_buy, self.insurance, self.isk
        )
    }
}

/// An ISK amount at a dial: `amount` itself at exactly 1.0 (bit-identical),
/// else `amount x factor` rounded DOWN to the cent, so a scaled faucet never
/// books a fraction of a cent.
pub fn scaled_isk(amount: f64, factor: f64) -> f64 {
    if factor == 1.0 {
        amount
    } else {
        (amount * factor * 100.0 + 1e-6).floor() / 100.0
    }
}

/// One variable's read: unset is `None`; a value that is not Unicode is an
/// error naming the variable, never a silent 1.0.
fn env_text(var: &str, read: Result<String, std::env::VarError>) -> Result<Option<String>, String> {
    match read {
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{var}: the value is not valid Unicode"))
        }
    }
}

impl Default for EconScale {
    fn default() -> Self {
        EconScale::ONE
    }
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "econ_scale",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(EconScale::ONE)
}

/// The multipliers of this world.
pub fn of(reg: &Registry) -> EconScale {
    *reg.ext::<EconScale>()
}

/// Set the world's multipliers (`Server::bind`, at startup).
pub fn set(reg: &mut Registry, scale: EconScale) {
    *reg.ext_mut::<EconScale>() = scale;
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: [Option<&str>; 5] = [None; 5];

    #[test]
    fn the_default_is_one_everywhere_and_changes_no_amount() {
        let s = EconScale::from_flags_or_env(NONE, NONE).unwrap();
        assert_eq!(s, EconScale::ONE);
        for f in [s.bounty(), s.mission(), s.npc_buy(), s.insurance()] {
            assert_eq!(f, 1.0);
            assert_eq!(1234.56 * f, 1234.56);
        }
    }

    #[test]
    fn scaled_isk_is_exact_at_one_and_whole_cents_otherwise() {
        assert_eq!(scaled_isk(1234.567, 1.0), 1234.567);
        assert_eq!(scaled_isk(10.01, 0.5), 5.0);
        assert_eq!(scaled_isk(10.0, 0.0), 0.0);
        // A product just under a whole cent in floats is that cent.
        assert_eq!(scaled_isk(0.29, 100.0), 29.0);
        assert_eq!(scaled_isk(0.57, 10.0), 5.7);
        let x = scaled_isk(333.33, 2.5);
        assert_eq!((x * 100.0).fract(), 0.0);
    }

    #[test]
    fn a_flag_beats_its_variable_and_the_master_multiplies_the_specific() {
        let s = EconScale::from_flags_or_env(
            [Some("2.5"), None, None, None, Some("2")],
            [Some("9"), Some("3"), Some(""), None, None],
        )
        .unwrap();
        assert_eq!(s.bounty, 2.5);
        assert_eq!(s.mission, 3.0);
        assert_eq!(s.npc_buy, 1.0);
        assert_eq!(s.bounty(), 5.0);
        assert_eq!(s.mission(), 6.0);
        assert_eq!(s.npc_buy(), 2.0);
        assert_eq!(s.insurance(), 2.0);
    }

    #[test]
    fn a_bad_value_is_an_error_naming_its_source() {
        let over = format!("{}", MAX_SCALE + 0.5);
        for bad in ["abc", "", "NaN", "inf", "-1", "-0.5", &over, "1e9"] {
            let e = EconScale::from_flags_or_env([Some(bad), None, None, None, None], NONE);
            assert!(e.unwrap_err().starts_with("--bounty-scale:"), "{bad}");
        }
        let e = EconScale::from_flags_or_env(NONE, [None, None, None, None, Some("x")]);
        assert!(e.unwrap_err().starts_with("EVE_ISK_SCALE:"));
        assert_eq!(EconScale::parse("0"), Ok(0.0));
        assert_eq!(EconScale::parse(&format!(" {MAX_SCALE} ")), Ok(MAX_SCALE));
        assert!(EconScale::parse("-0").unwrap().is_sign_positive());
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_variable_is_an_error_naming_it() {
        use std::os::unix::ffi::OsStringExt;
        let bad = std::ffi::OsString::from_vec(vec![0xff, 0xfe]);
        let e = env_text("EVE_ISK_SCALE", Err(std::env::VarError::NotUnicode(bad)));
        assert!(e.unwrap_err().starts_with("EVE_ISK_SCALE:"));
        assert_eq!(env_text("X", Err(std::env::VarError::NotPresent)), Ok(None));
        assert_eq!(env_text("X", Ok("2".into())), Ok(Some("2".into())));
    }

    #[test]
    fn the_registry_slot_defaults_to_one_and_takes_a_set() {
        let mut reg = crate::testkit::registry_with(|_| {});
        assert_eq!(of(&reg), EconScale::ONE);
        let mut s = EconScale::ONE;
        s.mission = 4.0;
        set(&mut reg, s);
        assert_eq!(of(&reg).mission(), 4.0);
    }
}
