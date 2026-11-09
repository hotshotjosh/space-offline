//! **Would the server take the real fits?** (REAL-FITS-SEED, issue #21.)
//! A tool, not a test: it reads the real table and the real content, so its
//! output stays under `out/`.
//!
//! ```text
//! eve-fitcheck [--content DIR] [--fits FILE] [--fit-skills]
//! ```
//!
//! Runs every fit of `FILE` (default `../out/bots/fits.tsv`, for the build
//! container whose working directory is `<repo>/server`) through
//! [`eve_bot::real_fits::check`] over the content's numbers: the server's
//! rack, rig and subsystem rules, and, apart from them, CPU and power on the
//! paper engine at the doctrine skills (`offline_cpu`, `offline_power`: the
//! server fits such a module and it stays offline). Prints one
//! tab-separated line per fit with a module or a drone:
//! `hull count class reasons builder hull_name`, where `reasons` is
//! comma-separated ([`eve_bot::real_fits::Reason::word`]) or `-`, `builder`
//! is the doctrine builder's verdict (`ok` or why `doctrine_real::try_build`
//! drops it), and `class` the bot's hull class or `group<id>`. The fit's
//! hash is left out on purpose: no row identifies a killmail.
//! The doctrine skills are `doctrine_fit::skills_for` at the doctrine level
//! (IV): the parts' own requirements and every fit-budget and rigging skill
//! (CPU Management, Power Grid Management among them). `--fit-skills` asks
//! for all of them at V instead **[A]**; falsified by a flown killmail fit
//! the engine shows over budget at V.
//! `tools/killfits.py --check` runs this (`--release`) in the build container and
//! summarises it.

use eve_bot::doctrine::{DOCTRINE_LEVEL, Makeable};
use eve_bot::doctrine_fit::{Library, Limits, profile_of, skills_for};
use eve_bot::real_fits::{HullInfo, ModuleInfo, PartKind, RealFit, Rules, check};
use eve_server::content::Content;
use eve_server::fitting::{hull_allows, hull_in};
use eve_server::typedata::{self, Installed};
use eve_server::{rigs, subsystems};
use std::path::Path;
use std::process::ExitCode;

const ATTR_MAX_GROUP_FITTED: i64 = 1544;
const ATTR_MAX_TYPE_FITTED: i64 = 2431;
const CATEGORY_SUBSYSTEM: i64 = 32;

struct World<'a> {
    content: &'a Content,
    inst: &'a Installed,
    lib: &'a Library<'a>,
    /// The level `skills_for` trains every skill it returns to.
    level: i64,
}

impl Rules for World<'_> {
    fn hull(&self, t: i64) -> Option<HullInfo> {
        let fit = *hull_in(self.content, t)?;
        let item = typedata::item_in(self.content, t)?;
        let rig = rigs::hull_rigs_in(self.content, t)?;
        Some(HullInfo {
            fit,
            group_id: item.group_id,
            category_id: item.category_id,
            rig_size: rig.size,
            calibration: rig.calibration,
            modular: subsystems::is_modular_group(item.group_id),
        })
    }

    fn module(&self, t: i64) -> Option<ModuleInfo> {
        let item = typedata::item_in(self.content, t)?;
        // A rack module's limits are read as `fitting::plan_fit_in` does
        // (`attribute_in`), a rig's as `rigs::plan_fit` does (`own_attribute_in`,
        // #177).
        let rig = rigs::rig_in(self.content, t);
        let limit = |a| {
            let v = if rig.is_some() {
                typedata::own_attribute_in(self.content, t, a)
            } else {
                typedata::attribute_in(self.content, t, a)
            };
            v.filter(|v| v.is_finite() && *v > 0.0).map(|v| v as i64)
        };
        let found = eve_server::modules::find_in(self.content, t);
        let kind = if rig.is_some() {
            PartKind::Rig
        } else if item.category_id == CATEGORY_SUBSYSTEM {
            PartKind::Subsystem
        } else if let Some(m) = found {
            PartKind::Rack(m.rack)
        } else {
            PartKind::Unfittable
        };
        let read = |a: i64| typedata::attribute_in(self.content, t, a).unwrap_or(0.0) as i64;
        let (sm, hm) = (subsystems::SLOT_MODIFIERS, subsystems::HARDPOINT_MODIFIERS);
        Some(ModuleInfo {
            kind,
            hardpoint: found.and_then(|m| m.hardpoint),
            group_id: item.group_id,
            volume: item.volume,
            max_group: limit(ATTR_MAX_GROUP_FITTED),
            max_type: limit(ATTR_MAX_TYPE_FITTED),
            rig_size: rig.map_or(0, |r| r.size),
            upgrade_cost: rig.map_or(0.0, |r| r.cost),
            subsystem_slot: subsystems::slot_flag(self.content, t),
            fits_ship: subsystems::fits_ship_type(self.content, t),
            subsystem_mods: [
                read(sm[0].1),
                read(sm[1].1),
                read(sm[2].1),
                read(hm[0].1),
                read(hm[1].1),
            ],
        })
    }

    fn allows(&self, module: i64, hull_group: i64, hull_type: i64) -> bool {
        hull_allows(
            |a| typedata::own_attribute_in(self.content, module, a),
            hull_group,
            hull_type,
        )
    }

    fn power(&self, fit: &RealFit) -> Option<(bool, bool)> {
        let charge = fit.charges.first().map(|c| c.1);
        let types: Vec<i64> = std::iter::once(fit.hull)
            .chain(fit.modules.iter().map(|m| m.0))
            .chain(charge)
            .collect();
        let skills = skills_for(self.inst.table(), &types, self.level);
        let p = profile_of(self.lib, fit.hull, &fit.modules, charge, &skills)?;
        Some((p.cpu_ok, p.pg_ok))
    }
}

fn run() -> Result<(), String> {
    let (mut content, mut fits) = (
        "../out/content".to_owned(),
        "../out/bots/fits.tsv".to_owned(),
    );
    let mut fit_skills = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |n: &str| args.next().ok_or_else(|| format!("{n} needs a value"));
        match a.as_str() {
            "--content" => content = value("--content")?,
            "--fits" => fits = value("--fits")?,
            "--fit-skills" => fit_skills = true,
            _ => {
                return Err(
                    "usage: eve-fitcheck [--content DIR] [--fits FILE] [--fit-skills]".into(),
                );
            }
        }
    }
    let table = eve_bot::real_fits::load(Path::new(&fits))?
        .ok_or_else(|| format!("{fits}: no such file"))?;
    let c = eve_bot::hulls::open(Path::new(&content))?;
    let inst = c
        .try_get::<Installed>()
        .ok_or("the content has no type data")?;
    let lib = Library::build(inst, &Makeable::all(), &|_| 0.0, Limits::default());
    let world = World {
        content: &c,
        inst,
        lib: &lib,
        level: if fit_skills { 5 } else { DOCTRINE_LEVEL },
    };
    println!("#level {}", world.level);
    println!("#hull\tcount\tclass\treasons\tbuilder\thull_name");
    let mut empty = 0u64;
    for fits in table.by_hull.values() {
        for f in fits {
            if f.modules.is_empty() && f.drones.is_empty() {
                empty += 1;
                continue;
            }
            let reasons = check(&world, f);
            let reasons = if reasons.is_empty() {
                "-".to_owned()
            } else {
                reasons
                    .iter()
                    .map(|r| r.word())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let builder = match eve_bot::doctrine_real::try_build(&lib, f) {
                Ok(_) => "ok".to_owned(),
                Err(d) => d.name().replace(' ', "_"),
            };
            let class = match lib.hulls.iter().find(|h| h.type_id == f.hull) {
                Some(h) => format!("{:?}", h.class),
                None => typedata::item_in(&c, f.hull)
                    .map_or("unknown".to_owned(), |i| format!("group{}", i.group_id)),
            };
            let name = typedata::item_in(&c, f.hull).map_or("?", |i| i.name);
            println!(
                "{}\t{}\t{class}\t{reasons}\t{builder}\t{name}",
                f.hull, f.count
            );
        }
    }
    eprintln!("eve-fitcheck: {empty} empty fits skipped");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("eve-fitcheck: {e}");
            ExitCode::FAILURE
        }
    }
}
