//! Mercenary den activities (MERC-2, issue #84): the "Mercenary Tactical
//! Operations" (MTOs) a den offers its owner. Wire contract:
//! `docs/re/sov-gateway.md` §2.7; the proto shapes are
//! `eve_public/sovereignty/mercenaryden/activity/*` (`out/gw-protos.txt`
//! 5335-5393), the client's own reading of them
//! `sovereignty/mercenaryden/client/{request_messenger,notice_listener,
//! repository}.py` and `data/activity.py` **[V]**.
//!
//! **State.** One row of kind [`KIND`] per activity, keyed by its uuid in hex:
//!
//! ```text
//! mercact <uuid hex> -> den template started expiry site_id
//! ```
//!
//! plus two control rows in the same kind that no activity key can equal (a
//! key of two parts, or the word `site`): `gen <den> -> next_at count`, the
//! den's generation clock, and `site -> count`, the counter the personal
//! sites' ids are drawn from. Times are unix seconds on the wall clock, as
//! the den's.
//!
//! **Generation** ([`tick_at`]): every [`GEN_S`] a den that works makes one
//! activity from the template table ([`ActivityTable`]), up to the owner's
//! [`capacity_for`]; a clock that runs late does what an early one would have.
//! The owner gets an `AddedNotice`. An activity not started by
//! `generated + `[`LIFETIME_S`] goes (`RemovedNotice`).
//!
//! **Start** ([`start`]): the client branches on the status
//! (`request_messenger.py:116-125` **[V]**): 404 `UnknownActivity` (no such
//! activity, not the pilot's, or run out), 409 `ActivityAlreadyStarted`, 403
//! `ActivityValidationFailed` (the den is off, the pilot is at capacity, or the
//! dungeon cannot be opened). On success the template's dungeon opens as a
//! **personal site** for the owner: the shape of an escalation (`escal` row,
//! [`crate::escalation`]) **[I]**, in the den's own system, and the pilot's
//! journal offers it as an Expeditions transmission with a Warp To. Anyone who
//! clears the site completes the activity **[A]**: the client's text may
//! say others can join, but no client file we hold says a non-owner's clear
//! pays the owner's den. Falsified by a TQ MTO completed by a non-owner
//! paying nobody. Omega gating of MTOs is not handled. The hook is
//! [`completed`], called from `combat_site::on_complete`.
//!
//! **Completion** ([`completed`]): the den's levels are settled, then moved by
//! the template's two impacts (never below 0), its hold gains the infomorph
//! bonus (up to its cargo cap), the owner gets `EvolutionStateChangedNotice`
//! and `CompletedNotice`, and the activity goes. Nothing else is paid.
//!
//! The rewards are the template's and nothing more: no ISK, no items from
//! nowhere; an infomorph bonus is a count added to a den that already holds
//! the item type (it is taken out through the usual extraction).

use super::*;
use crate::gateway::get_bytes;
use crate::sov_proto::put_int32;

/// The delta kind (`delta_kinds::KINDS`, `("mercact", "merc_den")`).
pub const KIND: &str = "mercact";

/// The template table, relative to `--authored`.
pub const FILE: &str = "sov/merc_den_activities.tsv";
/// Its header.
pub const HEADER: &str = "template\tname_msg\tdescription_msg\tdungeon\tdev_impact\tanarchy_impact\tinfomorph_bonus\tduration_s\twhy";
const SHIPPED: &str = include_str!("../../../../../content/sov/merc_den_activities.tsv");

const PKG: &str = "eve_public.sovereignty.mercenaryden.activity.api.";

/// Seconds between two generations at a den. **[A]**: the client shows a
/// "next activity" timer and no period; six hours is the spec's guess.
/// Falsified by an owner who finds dens flooded or starved of activities.
pub const GEN_S: u64 = 21_600;

/// Seconds an activity stays on offer before it is started. **[A]**: the
/// client shows an `expiry` and no rule. Falsified by the same.
pub const LIFETIME_S: u64 = 86_400;

/// Activities one trained level of the den-management skill allows a pilot
/// to run at once, and a den to hold. **[A]**: the client reads only the
/// total (`get_activity_limit_for_character`, `activity_cards.py:72-77`).
/// Falsified by the My Dens window showing another limit on TQ. A den holding
/// at most `capacity` activities (as well as the pilot running that many) is
/// also **[A]**: falsified by a client showing more offers than the limit.
pub const PER_LEVEL: u32 = 2;

/// The most sites tried for a free id before a start is refused.
const SITE_TRIES: u64 = 512;

// ---------------------------------------------------------------------------
// The template table (authored; content/sov/merc_den_activities.tsv)
// ---------------------------------------------------------------------------

/// One template: what an activity says, runs and pays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template {
    /// The client's localization message ids (`GetByMessageID`).
    pub name_msg: u32,
    pub desc_msg: u32,
    /// The dungeon the site runs.
    pub dungeon: i64,
    pub dev_impact: i32,
    pub anarchy_impact: i32,
    pub infomorph_bonus: u32,
    /// Seconds a started activity stays open.
    pub duration_s: u64,
}

/// The templates, by number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityTable {
    rows: BTreeMap<u32, Template>,
}

impl ActivityTable {
    pub fn new(rows: Vec<(u32, Template)>) -> ActivityTable {
        ActivityTable {
            rows: rows.into_iter().collect(),
        }
    }

    pub fn get(&self, n: u32) -> Option<&Template> {
        self.rows.get(&n)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn numbers(&self) -> Vec<u32> {
        self.rows.keys().copied().collect()
    }
}

/// One row of the table. Ids and the duration are positive.
pub fn parse_template_row(r: &crate::authored::Row) -> Result<(u32, Template), String> {
    let small = |i: usize| -> Result<u32, String> {
        u32::try_from(r.i64(i)?).map_err(|_| format!("column {} is out of range", i + 1))
    };
    let signed = |i: usize| -> Result<i32, String> {
        i32::try_from(r.i64(i)?).map_err(|_| format!("column {} is out of range", i + 1))
    };
    let t = Template {
        name_msg: small(1)?,
        desc_msg: small(2)?,
        dungeon: r.i64(3)?,
        dev_impact: signed(4)?,
        anarchy_impact: signed(5)?,
        infomorph_bonus: small(6)?,
        duration_s: u64::try_from(r.i64(7)?).map_err(|_| "duration_s is negative".to_owned())?,
    };
    if t.name_msg == 0 || t.desc_msg == 0 {
        return Err("a message id of 0 names no text".into());
    }
    if t.dungeon <= 0 {
        return Err("dungeon is not a positive id".into());
    }
    if t.duration_s == 0 {
        return Err("duration_s must be above 0".into());
    }
    Ok((small(0)?, t))
}

fn load_table(src: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let (rows, _) = file.parse_rows_logged(parse_template_row);
    Ok(Box::new(ActivityTable::new(rows)))
}

/// An invented dungeon id, never a real one.
pub const FIXTURE_DUNGEON: i64 = 990_840_001;
/// A second invented dungeon id.
pub const FIXTURE_DUNGEON2: i64 = 990_840_002;

/// Invented rows for tests, never the shipped numbers: two templates over two
/// invented dungeons and invented message ids.
pub fn fixture_table() -> ActivityTable {
    ActivityTable::new(vec![
        (
            1,
            Template {
                name_msg: 990_841_001,
                desc_msg: 990_841_002,
                dungeon: FIXTURE_DUNGEON,
                dev_impact: 7,
                anarchy_impact: -4,
                infomorph_bonus: 6,
                duration_s: 1_000,
            },
        ),
        (
            2,
            Template {
                name_msg: 990_841_003,
                desc_msg: 990_841_004,
                dungeon: FIXTURE_DUNGEON2,
                dev_impact: 2,
                anarchy_impact: 0,
                infomorph_bonus: 0,
                duration_s: 2_000,
            },
        ),
    ])
}

/// Registration row for `content_tables::TABLES` (coordinator-only).
pub const TABLE: crate::content::TableSpec = crate::content::TableSpec {
    name: "sov mercenary den activities",
    load: load_table,
    fixture: || Box::new(fixture_table()) as crate::content::Boxed,
};

fn template_of(reg: &Registry, n: u32) -> Option<Template> {
    reg.content().try_get::<ActivityTable>()?.get(n).copied()
}

/// Whether a template can open: its dungeon is in the bundle and the
/// transmission has a text for it (`escalation::journal_text`).
fn usable(reg: &Registry, t: &Template) -> bool {
    reg.content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|d| d.dungeons.get(&t.dungeon))
        .is_some_and(|d| crate::escalation::journal_text(d, d).is_some())
}

// ---------------------------------------------------------------------------
// The row
// ---------------------------------------------------------------------------

/// One activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    /// The uuid the client holds (`Identifier.uuid`, 16 bytes).
    pub id: [u8; 16],
    pub den: i64,
    pub template: u32,
    pub started: bool,
    pub expiry: u64,
    /// The personal site, 0 until started.
    pub site: i64,
}

/// The uuid of the `n`th activity of `den`: the two numbers, big-endian, so
/// no two activities share one and the id says where it came from.
fn uuid_of(den: i64, n: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&(den as u64).to_be_bytes());
    out[8..].copy_from_slice(&n.to_be_bytes());
    out
}

/// The uuid as the key spells it.
pub fn hex(id: &[u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// The uuid a key or a GM line spells; `None` unless 32 hex digits.
pub fn unhex(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn parse_row(key: &[String], v: &[String]) -> Option<Activity> {
    let [k] = key else { return None };
    let id = unhex(k)?;
    let [den, template, started, expiry, site] = v else {
        return None;
    };
    Some(Activity {
        id,
        den: den.parse().ok()?,
        template: template.parse().ok()?,
        started: started.parse::<u8>().ok()? != 0,
        expiry: expiry.parse().ok()?,
        site: site.parse().ok()?,
    })
}

/// Every activity, in uuid order. The control rows are not activities.
pub fn all(d: &Deltas) -> Vec<Activity> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| parse_row(k, v))
        .collect()
}

/// The activity `id`.
pub fn by_id(d: &Deltas, id: &[u8; 16]) -> Option<Activity> {
    let key = vec![hex(id)];
    parse_row(&key, d.of(KIND).get(&key)?)
}

/// The activities of `den`.
pub fn of_den(d: &Deltas, den: i64) -> Vec<Activity> {
    all(d).into_iter().filter(|a| a.den == den).collect()
}

/// Write one row.
pub fn put(d: &mut Deltas, a: &Activity) {
    d.put(
        KIND,
        vec![hex(&a.id)],
        vec![
            a.den.to_string(),
            a.template.to_string(),
            u8::from(a.started).to_string(),
            a.expiry.to_string(),
            a.site.to_string(),
        ],
    );
}

fn clock_key(den: i64) -> Vec<String> {
    vec!["gen".to_owned(), den.to_string()]
}

/// `(next generation at, activities made so far)` of a den.
fn clock_of(d: &Deltas, den: i64) -> Option<(u64, u64)> {
    let v = d.of(KIND).get(&clock_key(den))?;
    Some((v.first()?.parse().ok()?, v.get(1)?.parse().ok()?))
}

fn put_clock(d: &mut Deltas, den: i64, next: u64, count: u64) {
    d.put(
        KIND,
        clock_key(den),
        vec![next.to_string(), count.to_string()],
    );
}

/// When `den` makes its next activity, for `GetForMercenaryDen`.
pub fn next_generation(d: &Deltas, den: i64, now: u64) -> u64 {
    clock_of(d, den).map_or(now + GEN_S, |c| c.0)
}

// ---------------------------------------------------------------------------
// Capacity
// ---------------------------------------------------------------------------

/// The activities a pilot may run at once, and each of their dens may hold:
/// the trained level of the den-management skill times [`PER_LEVEL`].
pub fn capacity_for(reg: &Registry, who: i64) -> u32 {
    u32::try_from(crate::drone_skills::level_of(
        reg,
        who,
        SKILL_DEN_MANAGEMENT,
    ))
    .unwrap_or(0)
    .min(SKILL_TOP)
    .saturating_mul(PER_LEVEL)
}

fn started_by(reg: &Registry, who: i64) -> usize {
    all(reg.deltas())
        .into_iter()
        .filter(|a| a.started && by_id_den(reg, a.den).is_some_and(|d| d.owner == who))
        .count()
}

fn by_id_den(reg: &Registry, den: i64) -> Option<Den> {
    super::by_id(reg.deltas(), den)
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

fn uuid_msg(id: &[u8; 16]) -> Vec<u8> {
    let mut m = Vec::new();
    put_bytes(&mut m, 1, id);
    m
}

/// `activitytemplate.Definition`.
pub fn definition_bytes(t: &Template) -> Vec<u8> {
    let mut m = Vec::new();
    put_bytes(&mut m, 1, &id(u64::from(t.name_msg)));
    put_bytes(&mut m, 2, &id(u64::from(t.desc_msg)));
    put_bytes(&mut m, 3, &id(t.dungeon as u64));
    put_int32(&mut m, 4, t.dev_impact);
    put_int32(&mut m, 5, t.anarchy_impact);
    put_uint(&mut m, 6, u64::from(t.infomorph_bonus));
    m
}

/// `activity.Attributes`.
pub fn attributes_bytes(a: &Activity, t: &Template, den: &Den) -> Vec<u8> {
    let mut m = Vec::new();
    put_bytes(&mut m, 1, &definition_bytes(t));
    put_bytes(&mut m, 2, &id(a.den as u64));
    put_bool(&mut m, 3, a.started);
    put_bytes(&mut m, 4, &timestamp(a.expiry));
    put_bytes(&mut m, 5, &id(den.system as u64));
    m
}

/// A `...Response.Activity`: `{ 1 id, 2 attributes }` (the same in
/// `GetAllResponse` and `GetForMercenaryDenResponse`).
fn entry_bytes(reg: &Registry, a: &Activity) -> Option<Vec<u8>> {
    let t = template_of(reg, a.template)?;
    let den = by_id_den(reg, a.den)?;
    let mut m = Vec::new();
    put_bytes(&mut m, 1, &uuid_msg(&a.id));
    put_bytes(&mut m, 2, &attributes_bytes(a, &t, &den));
    Some(m)
}

fn notice(reg: &mut Registry, name: &str, payload: Vec<u8>, to: i64) {
    crate::gateway_out::queue(reg, &format!("{PKG}{name}"), payload, Target::Character(to));
}

fn announce_added(reg: &mut Registry, a: &Activity, den: &Den) {
    let Some(t) = template_of(reg, a.template) else {
        return;
    };
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &uuid_msg(&a.id));
    put_bytes(&mut p, 2, &attributes_bytes(a, &t, den));
    put_bytes(&mut p, 3, &id(den.system as u64));
    notice(reg, "AddedNotice", p, den.owner);
}

fn announce_removed(reg: &mut Registry, a: &Activity, owner: i64) {
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(a.den as u64));
    put_bytes(&mut p, 2, &uuid_msg(&a.id));
    notice(reg, "RemovedNotice", p, owner);
}

// ---------------------------------------------------------------------------
// Generation and expiry (the clock)
// ---------------------------------------------------------------------------

/// Make one activity for `den` at `at`: a template picked by `(den, count)`
/// among the usable ones. `None` when none is usable.
fn make(reg: &mut Registry, den: &Den, count: u64, at: u64) -> Option<Activity> {
    let numbers: Vec<u32> = {
        let table = reg.content().try_get::<ActivityTable>()?;
        table
            .numbers()
            .into_iter()
            .filter(|n| table.get(*n).is_some_and(|t| usable(reg, t)))
            .collect()
    };
    if numbers.is_empty() {
        return None;
    }
    let template = numbers[(mix(den.id, count) % numbers.len() as u64) as usize];
    let a = Activity {
        id: uuid_of(den.id, count),
        den: den.id,
        template,
        started: false,
        expiry: at + LIFETIME_S,
        site: 0,
    };
    put(reg.deltas_mut(), &a);
    announce_added(reg, &a, den);
    Some(a)
}

/// Take an activity out of the world: its row, and the site it opened if it
/// was started; the owner is told.
fn drop_activity(reg: &mut Registry, a: &Activity, owner: Option<i64>) {
    reg.deltas_mut().del(KIND, &[hex(&a.id)]);
    // Only a site that is still this owner's own: the escalation module frees
    // an id when it expires, and another escalation may have taken it since.
    if owner.is_some_and(|o| owns_site(reg, a, o)) {
        crate::site::vacate(reg, a.site);
        crate::escalation::completed(reg, a.site);
    }
    if let Some(owner) = owner {
        announce_removed(reg, a, owner);
    }
}

/// Whether `a`'s site is still the personal site it opened for `owner`: the
/// `escal` row exists, shows only `owner` and runs the template's dungeon.
/// The escalation module frees a site id when its row expires and may hand
/// the id to another pilot's escalation, so a started activity must not act
/// on a site it no longer owns. With the template gone (content reloaded) the
/// dungeon cannot be compared, so the row's single owner decides: the site is
/// still dropped with its activity.
fn owns_site(reg: &Registry, a: &Activity, owner: i64) -> bool {
    let dungeon = template_of(reg, a.template).map(|t| t.dungeon);
    a.started
        && a.site != 0
        && crate::escalation::get(reg, a.site)
            .is_some_and(|e| e.chars == [owner] && dungeon.is_none_or(|d| e.dungeon == d))
}

/// A den went (its item is gone): its activities go with it, and its clock.
pub fn den_removed(reg: &mut Registry, den: &Den) {
    for a in of_den(reg.deltas(), den.id) {
        drop_activity(reg, &a, Some(den.owner));
    }
    reg.deltas_mut().del(KIND, &clock_key(den.id));
}

/// The activity clock at `now`: what has run out goes, then every den that
/// works makes what its clock owes, up to its capacity.
pub fn tick_at(reg: &mut Registry, now: u64) {
    for a in all(reg.deltas()) {
        let den = by_id_den(reg, a.den);
        // A started activity whose site row is gone (the escalation module
        // ran it out first) is over.
        let site_gone = a.started && crate::escalation::get(reg, a.site).is_none();
        if den.is_none() || template_of(reg, a.template).is_none() || a.expiry <= now || site_gone {
            drop_activity(reg, &a, den.map(|d| d.owner));
        }
    }
    for den in super::all(reg.deltas()) {
        let (mut next, mut count) = clock_of(reg.deltas(), den.id).unwrap_or((now + GEN_S, 0));
        let before = (next, count);
        if !den.enabled {
            // A den that is off makes nothing; its clock starts afresh when
            // it works again.
            if next <= now {
                next = now + GEN_S;
            }
        } else {
            let cap = capacity_for(reg, den.owner) as usize;
            let mut have = of_den(reg.deltas(), den.id).len();
            // Generations already out of time are not made late.
            if next + LIFETIME_S <= now {
                next += ((now - LIFETIME_S - next) / GEN_S + 1) * GEN_S;
            }
            while next <= now {
                if have >= cap {
                    next = skip_past(next, now);
                    break;
                }
                match make(reg, &den, count, next) {
                    Some(_) => {
                        have += 1;
                        count += 1;
                        next += GEN_S;
                    }
                    None => {
                        next = skip_past(next, now);
                        break;
                    }
                }
            }
        }
        if (next, count) != before || clock_of(reg.deltas(), den.id).is_none() {
            put_clock(reg.deltas_mut(), den.id, next, count);
        }
    }
}

/// The first time on the clock's grid (`next` plus whole periods) after `now`.
fn skip_past(next: u64, now: u64) -> u64 {
    next + ((now - next) / GEN_S + 1) * GEN_S
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

/// Why a start was refused: the client's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRefused {
    /// 404: no such activity, not the pilot's, or run out.
    Unknown,
    /// 409
    AlreadyStarted,
    /// 403
    Validation,
}

impl StartRefused {
    pub fn status(self) -> u32 {
        match self {
            StartRefused::Unknown => 404,
            StartRefused::AlreadyStarted => 409,
            StartRefused::Validation => 403,
        }
    }
}

/// Seconds to blue time, scaled as an escalation's (`--time-scale`).
fn span(reg: &Registry, secs: u64) -> i64 {
    crate::game_clock::span(reg, secs as i64 * 10_000_000)
}

/// The same span in whole wall seconds (rounded up), for the activity's own
/// expiry: one clock with the site's, so the two end together.
fn span_secs(reg: &Registry, secs: u64) -> u64 {
    (span(reg, secs).max(0) as u64).div_ceil(10_000_000)
}

/// Open the template's dungeon as a personal site of `owner` in `den`'s
/// system: an `escal` row in the escalation's shape (so its tick places and
/// expires it and the journal lists it), with a free site id, and the pilot's
/// transmission. The site id, or `None` when the dungeon is unknown or has no
/// text, the system holds no site slot or no id is free.
fn open_site(reg: &mut Registry, den: &Den, t: &Template, owner: i64) -> Option<i64> {
    let dest = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()?
        .dungeons
        .get(&t.dungeon)?
        .clone();
    let journal = crate::escalation::journal_text(&dest, &dest)?;
    let rank = reg
        .ext::<crate::population::Population>()
        .prepared()
        .sys_rank(den.system)?;
    let key = vec!["site".to_owned()];
    let mut n = reg
        .deltas()
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0);
    let mut found = None;
    for _ in 0..SITE_TRIES {
        let cand = crate::escalation::site_id_of(rank, n)?;
        n += 1;
        if crate::escalation::get(reg, cand).is_none()
            && reg.ext::<crate::site::Sites>().get(cand).is_none()
        {
            found = Some(cand);
            break;
        }
    }
    let site_id = found?;
    reg.deltas_mut().put(KIND, key, vec![n.to_string()]);
    let created = crate::pyutil::filetime_now();
    let expires = created + span(reg, t.duration_s);
    // The row `escalation::Esc` reads back (a test holds the two together).
    reg.deltas_mut().put(
        crate::escalation::KIND,
        vec![site_id.to_string()],
        vec![
            den.system.to_string(),
            t.dungeon.to_string(),
            t.dungeon.to_string(),
            journal.to_string(),
            created.to_string(),
            expires.to_string(),
            crate::escalation::rung(&dest).to_string(),
            owner.to_string(),
        ],
    );
    reg.schedule(
        std::time::Duration::ZERO,
        crate::escalation::PathMessage {
            instance_id: site_id,
            to: owner,
        },
    );
    Some(site_id)
}

/// **`StartRequest`** for pilot `who`: the activity's attributes now.
pub fn start(
    reg: &mut Registry,
    who: i64,
    uuid: &[u8; 16],
    now: u64,
) -> Result<Vec<u8>, StartRefused> {
    let mut a = by_id(reg.deltas(), uuid).ok_or(StartRefused::Unknown)?;
    let den = by_id_den(reg, a.den)
        .filter(|d| d.owner == who)
        .ok_or(StartRefused::Unknown)?;
    let t = template_of(reg, a.template).ok_or(StartRefused::Unknown)?;
    if a.expiry <= now {
        return Err(StartRefused::Unknown);
    }
    if a.started {
        return Err(StartRefused::AlreadyStarted);
    }
    if !den.enabled || started_by(reg, who) >= capacity_for(reg, who) as usize {
        return Err(StartRefused::Validation);
    }
    let site = open_site(reg, &den, &t, who).ok_or(StartRefused::Validation)?;
    let previous = a.expiry;
    a.started = true;
    a.site = site;
    a.expiry = now + span_secs(reg, t.duration_s);
    put(reg.deltas_mut(), &a);
    let attributes = attributes_bytes(&a, &t, &den);
    if a.expiry != previous {
        let mut p = Vec::new();
        put_bytes(&mut p, 1, &uuid_msg(&a.id));
        put_bytes(&mut p, 2, &attributes);
        put_bytes(&mut p, 3, &timestamp(previous));
        notice(reg, "ExpiryChangedNotice", p, who);
    }
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &uuid_msg(&a.id));
    put_bytes(&mut p, 2, &attributes);
    notice(reg, "StartedNotice", p, who);
    Ok(attributes)
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

fn moved(level: u64, impact: i32) -> u64 {
    (level as i64).saturating_add(i64::from(impact)).max(0) as u64
}

/// **The completion hook** (`combat_site::on_complete`): the site `site_id`
/// was cleared; if it was a started activity's, the activity completes.
/// Whether it did (the caller then rolls no escalation step).
pub fn completed(w: &mut Registry, site_id: i64) -> bool {
    completed_at(w, site_id, now())
}

/// [`completed`] at `now`. Whether an activity claimed the site (completed or
/// consumed).
pub fn completed_at(w: &mut Registry, site_id: i64, now: u64) -> bool {
    if w.deltas().of(KIND).is_empty() {
        return false;
    }
    // The activity that still owns this site and has not run out: a recycled
    // site id completes nobody's activity, and a stale row for the same id
    // does not shadow the live one.
    let Some(a) = all(w.deltas()).into_iter().find(|a| {
        a.started
            && a.site == site_id
            && a.expiry > now
            && by_id_den(w, a.den).is_some_and(|d| owns_site(w, a, d.owner))
    }) else {
        return false;
    };
    crate::escalation::completed(w, a.site);
    // A row `finish` consumed claims the site even when it paid nothing (its
    // template or den definition is gone): the caller rolls no step for it.
    finish(w, &a, now) != Finish::Untouched
}

/// What [`finish`] did with the activity row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finish {
    /// The den was paid and the row consumed.
    Completed,
    /// The row was consumed and nothing paid: the template or the den's
    /// definition is gone.
    Dropped,
    /// The row is still there (the den is gone).
    Untouched,
}

/// Complete `a`: the den's levels and hold, the notices, the row.
fn finish(reg: &mut Registry, a: &Activity, now: u64) -> Finish {
    let owner = by_id_den(reg, a.den).map(|d| d.owner);
    let Some(t) = template_of(reg, a.template) else {
        drop_activity(reg, a, owner);
        return Finish::Dropped;
    };
    let Some(def) = by_id_den(reg, a.den).and_then(|d| def_for(reg, &d)) else {
        drop_activity(reg, a, owner);
        return Finish::Dropped;
    };
    // Settle first: the infomorphs the old levels made, then the levels.
    let Some(mut den) = settle(reg, a.den, now) else {
        return Finish::Untouched;
    };
    let dev = moved(den.dev_level(&def, now), t.dev_impact);
    let anarchy = moved(den.anarchy_level(&def, now), t.anarchy_impact);
    (den.dev_base, den.dev_at) = (dev, now);
    (den.anarchy_base, den.anarchy_at) = (anarchy, now);
    den.infomorphs = den
        .infomorphs
        .saturating_add(t.infomorph_bonus)
        .min(def.cargo_cap);
    if den.seen != 0 {
        // Told below, once.
        den.seen = den.stages_code(&def, now);
    }
    super::put(reg.deltas_mut(), &den);
    reg.deltas_mut().del(KIND, &[hex(&a.id)]);
    announce_evolution(reg, &den, now);
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(den.id as u64));
    put_bytes(&mut p, 2, &uuid_msg(&a.id));
    put_int32(&mut p, 3, t.dev_impact);
    put_int32(&mut p, 4, t.anarchy_impact);
    put_uint(&mut p, 5, u64::from(t.infomorph_bonus));
    notice(reg, "CompletedNotice", p, den.owner);
    Finish::Completed
}

// ---------------------------------------------------------------------------
// The gateway arm
// ---------------------------------------------------------------------------

fn read_reply(reg: &Registry, rest: &str, ty: &str, body: &[u8], who: i64, now: u64) -> Reply {
    match rest {
        "GetAllRequest" => {
            let mut out = Vec::new();
            for a in all(reg.deltas()) {
                if by_id_den(reg, a.den).is_some_and(|d| d.owner == who)
                    && let Some(e) = entry_bytes(reg, &a)
                {
                    put_bytes(&mut out, 1, &e);
                }
            }
            typed(ty, out)
        }
        "GetForMercenaryDenRequest" => {
            let den = read_id(body, 1)
                .and_then(|i| by_id_den(reg, i as i64))
                .filter(|d| d.owner == who);
            let Some(den) = den else {
                return status(404);
            };
            let mut out = Vec::new();
            for a in of_den(reg.deltas(), den.id) {
                if let Some(e) = entry_bytes(reg, &a) {
                    put_bytes(&mut out, 1, &e);
                }
            }
            put_bytes(
                &mut out,
                2,
                &timestamp(next_generation(reg.deltas(), den.id, now)),
            );
            typed(ty, out)
        }
        _ => {
            let mut out = Vec::new();
            put_uint(&mut out, 1, u64::from(capacity_for(reg, who)));
            typed(ty, out)
        }
    }
}

/// The gateway arm (`merc_den::gateway` asks it first): the activity
/// messages of `mercenaryden.activity.api`. Every other type is `None`. On a
/// [`Ground::Local`] there is no world: the lists are empty, a den or an
/// activity is 404 and the capacity 0.
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(PKG)?;
    if !matches!(
        rest,
        "GetAllRequest" | "GetForMercenaryDenRequest" | "GetCapacityRequest" | "StartRequest"
    ) {
        return None;
    }
    let Ground::Server(w) = g else {
        return Some(match rest {
            "GetAllRequest" | "GetCapacityRequest" => typed(ty, Vec::new()),
            _ => status(404),
        });
    };
    if rest == "StartRequest" {
        let uuid = get_bytes(body, 1)
            .and_then(|m| get_bytes(&m, 1))
            .and_then(|b| <[u8; 16]>::try_from(b).ok());
        let Some(uuid) = uuid else {
            return Some(status(404));
        };
        let mut reg = crate::service::world_write_timed(w, "merc_den_activity_start");
        return Some(match start(&mut reg, character, &uuid, now()) {
            Ok(attributes) => {
                let mut out = Vec::new();
                put_bytes(&mut out, 1, &attributes);
                typed(ty, out)
            }
            Err(e) => status(e.status()),
        });
    }
    let reg = crate::service::world_read_timed(w, "merc_den_activity_gateway");
    Some(read_reply(&reg, rest, ty, body, character, now()))
}

// ---------------------------------------------------------------------------
// The GM line (`/mercden generate <den>`, `complete <activity>`, `list <den>`)
// ---------------------------------------------------------------------------

const USAGE: &str = "usage: /mercden generate <den> | complete <activity uuid> | list <den>";

/// Make an activity at `den` now, whatever its clock says; the capacity
/// still holds. The GM line and the client scenario use it.
pub fn generate_now(reg: &mut Registry, den: i64, now: u64) -> Result<Activity, String> {
    let d = by_id_den(reg, den).ok_or_else(|| format!("no mercenary den {den}"))?;
    if of_den(reg.deltas(), den).len() >= capacity_for(reg, d.owner) as usize {
        return Err(format!("mercenary den {den} is at its capacity"));
    }
    let (next, count) = clock_of(reg.deltas(), den).unwrap_or((now + GEN_S, 0));
    let a = make(reg, &d, count, now)
        .ok_or_else(|| "no template can open (a missing dungeon or text)".to_owned())?;
    put_clock(reg.deltas_mut(), den, next, count + 1);
    Ok(a)
}

/// Complete a started activity as if its site had been cleared.
pub fn complete_now(reg: &mut Registry, uuid: &[u8; 16], now: u64) -> Result<(), String> {
    let a = by_id(reg.deltas(), uuid).ok_or("no such activity")?;
    if !a.started {
        return Err("that activity is not started".to_owned());
    }
    let owned = by_id_den(reg, a.den).is_some_and(|d| owns_site(reg, &a, d.owner));
    if owned {
        crate::site::vacate(reg, a.site);
        crate::escalation::completed(reg, a.site);
    }
    if finish(reg, &a, now) == Finish::Completed {
        Ok(())
    } else {
        Err("it could not complete (its den or template is gone)".to_owned())
    }
}

/// The `/mercden` words this module owns; `None` for any other.
pub fn slash(reg: &mut Registry, args: &[&str], now: u64) -> Option<String> {
    match args {
        ["generate", d] => Some(match d.parse::<i64>() {
            Ok(den) => match generate_now(reg, den, now) {
                Ok(a) => format!("activity {} (template {})", hex(&a.id), a.template),
                Err(e) => e,
            },
            Err(_) => USAGE.to_owned(),
        }),
        ["complete", u] => Some(match unhex(u) {
            Some(id) => match complete_now(reg, &id, now) {
                Ok(()) => "completed".to_owned(),
                Err(e) => e,
            },
            None => USAGE.to_owned(),
        }),
        ["list", d] => Some(match d.parse::<i64>() {
            Ok(den) => {
                let rows: Vec<String> = of_den(reg.deltas(), den)
                    .iter()
                    .map(|a| {
                        format!(
                            "{} template {} started={} expiry {} site {}",
                            hex(&a.id),
                            a.template,
                            a.started,
                            a.expiry,
                            a.site
                        )
                    })
                    .collect();
                format!(
                    "{} activities, next at {}\n{}",
                    rows.len(),
                    next_generation(reg.deltas(), den, now),
                    rows.join("\n")
                )
            }
            Err(_) => USAGE.to_owned(),
        }),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests;
