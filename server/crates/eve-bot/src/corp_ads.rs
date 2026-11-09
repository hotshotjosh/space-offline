//! **Corp adverts for bots** (#152): a bot CEO whose corp has room posts one
//! recruitment advert and takes it down when the corp is full; a corpless
//! bot that wants to join searches the corp finder first and applies to the
//! corp of the best advert it would be taken by, and only then falls back to
//! the pick it made without the finder.
//!
//! The wire is the client's own (`corpRecProxy`, the calls
//! `base_corporation.py:619-637` make and `tools/client-ci/scenarios/fci_corp_ads.py`
//! exercises): `GetRecruitmentAdsForCorporation`, `CreateRecruitmentAd(days,
//! typeMask, langMask, description, recruiters, title, hourMask1, minSP,
//! otherMask)`, `DeleteRecruitmentAd(adID)` and `SearchCorpAds(typeMask,
//! langMask, excludeAlliances, excludeFriendlyFire, spRestriction,
//! minMembers, maxMembers, maxISKTaxRate, maxLPTaxRate, searchTimeMask,
//! otherMask)`, which answers `[(grade, advert row), ...]`.
//!
//! The decisions are pure ([`decide`], [`pick`]); [`tend`] and [`find`] are
//! the wire. The lines are `advert posted corp=ID`, `advert removed corp=ID`
//! and `applied corp=ID via=finder`; the corp turn's echo puts `[soc] NAME
//! corp ` in front of them (`driver::corp_echo`).
//!
//! **[A]** An advert carries no filters (all masks 0), so every finder
//! search matches it. Falsified by a soak where `advert posted` lines appear
//! and no `via=finder` line ever follows although corpless bots apply.

use crate::corpmind::{Hall, db_i64, db_rows};
use crate::society::{self, Offer};
use crate::{Conn, Result};
use eve_marshal::PyValue;

/// How long a posted advert runs, in days (the client's maximum).
pub const AD_DAYS: i64 = eve_server::corp_rec::MAX_DAYS;

/// What a CEO does about its corp's adverts this turn.
#[derive(Debug, Clone, PartialEq)]
pub enum AdAct {
    /// Room and no advert: post one.
    Post,
    /// Full with adverts up: take these down.
    Remove(Vec<i64>),
    /// Nothing to do.
    Keep,
}

/// **The advert decision** (pure): a corp of `size` that has room under
/// [`society::CORP_SIZE_CAP`] keeps exactly one advert up; a full one none.
/// `own` are the ids of its live adverts.
pub fn decide(size: usize, own: &[i64]) -> AdAct {
    let recruiting = society::room(size, 0, 0);
    match (recruiting, own.is_empty()) {
        (true, true) => AdAct::Post,
        (false, false) => AdAct::Remove(own.to_vec()),
        _ => AdAct::Keep,
    }
}

/// **The seeker's pick** (pure): of the corps with a live advert (`found`),
/// the one [`society::best_corp`] likes best among `offers`; `None` when no
/// advertising corp would have the bot.
pub fn pick(found: &[i64], offers: &[Offer]) -> Option<i64> {
    let among: Vec<Offer> = offers
        .iter()
        .filter(|o| found.contains(&o.corp))
        .copied()
        .collect();
    society::best_corp(&among)
}

/// The log line for an application that came from an advert.
pub fn applied_line(corp: i64) -> String {
    format!("applied corp={corp} via=finder")
}

/// The advert's title: the corp's name, cut to the client's limit.
pub fn title(name: &str) -> String {
    let t: String = format!("{name} recruiting")
        .chars()
        .take(eve_server::corp_rec::MAX_TITLE)
        .collect();
    t.trim().to_owned()
}

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

/// **The CEO's advert pass** (`Plan::Adverts`): read the corp's live
/// adverts, decide, post or remove.
pub fn tend(conn: &mut Conn, hall: &Hall, me: i64, corp: i64) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let o = conn.call("corpRecProxy", "GetRecruitmentAdsForCorporation", vec![])?;
    if o.refusal().is_some() {
        return Ok(out);
    }
    let own: Vec<i64> = o
        .value()
        .iter()
        .flat_map(db_rows)
        .filter(|r| db_i64(r, "corporationID") == Some(corp))
        .filter_map(|r| db_i64(r, "adID"))
        .collect();
    let (size, name) = {
        let st = hall.lock();
        (
            st.size(corp),
            st.corps
                .get(&corp)
                .map(|c| c.name.clone())
                .unwrap_or_default(),
        )
    };
    match decide(size, &own) {
        AdAct::Keep => {}
        AdAct::Post => {
            let o = conn.call(
                "corpRecProxy",
                "CreateRecruitmentAd",
                vec![
                    PyValue::Int(AD_DAYS),
                    PyValue::Int(0),
                    PyValue::Int(0),
                    uni("Looking for pilots to fly with."),
                    PyValue::List(vec![PyValue::Int(me)]),
                    uni(&title(&name)),
                    PyValue::Int(0),
                    PyValue::Int(0),
                    PyValue::Int(0),
                ],
            )?;
            match o.refusal() {
                Some(key) => out.push(format!("advert post: refused {key}")),
                None => out.push(format!("advert posted corp={corp}")),
            }
        }
        AdAct::Remove(ids) => {
            for id in ids {
                let o = conn.call(
                    "corpRecProxy",
                    "DeleteRecruitmentAd",
                    vec![PyValue::Int(id)],
                )?;
                match o.refusal() {
                    Some(key) => out.push(format!("advert remove {id}: refused {key}")),
                    None => out.push(format!("advert removed corp={corp}")),
                }
            }
        }
    }
    Ok(out)
}

/// **The seeker's finder search** (`Plan::Find`): the finder with no filter,
/// then [`pick`]. `None` when the call fails or no advertising corp fits.
pub fn find(conn: &mut Conn, offers: &[Offer]) -> Result<Option<i64>> {
    let o = conn.call(
        "corpRecProxy",
        "SearchCorpAds",
        vec![
            PyValue::Int(0),
            PyValue::Int(0),
            PyValue::Bool(false),
            PyValue::Bool(false),
            PyValue::None,
            PyValue::Int(0),
            PyValue::Int(1000),
            PyValue::Real(1.0),
            PyValue::Real(1.0),
            PyValue::Int(0),
            PyValue::Int(0),
        ],
    )?;
    if o.refusal().is_some() {
        return Ok(None);
    }
    let found: Vec<i64> = o
        .value()
        .iter()
        .flat_map(db_rows)
        .filter_map(|r| db_i64(r, "corporationID"))
        .collect();
    Ok(pick(&found, offers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(corp: i64, fit: f64, size: usize) -> Offer {
        Offer {
            corp,
            fit,
            grudge: 0.0,
            size,
            enemy: false,
        }
    }

    #[test]
    fn a_corp_with_room_and_no_advert_posts_one() {
        assert_eq!(decide(3, &[]), AdAct::Post);
    }

    #[test]
    fn a_corp_with_room_and_an_advert_keeps_it() {
        assert_eq!(decide(3, &[41]), AdAct::Keep);
    }

    #[test]
    fn a_full_corp_takes_its_adverts_down() {
        assert_eq!(
            decide(society::CORP_SIZE_CAP, &[41, 42]),
            AdAct::Remove(vec![41, 42])
        );
        assert_eq!(decide(society::CORP_SIZE_CAP, &[]), AdAct::Keep);
        assert_eq!(decide(society::CORP_SIZE_CAP - 1, &[]), AdAct::Post);
    }

    #[test]
    fn the_seeker_applies_to_the_best_corp_that_advertises() {
        let offers = [offer(10, 0.9, 4), offer(20, 0.8, 4), offer(30, 0.7, 4)];
        // The best corp overall (10) has no advert: the best advertiser wins.
        assert_eq!(pick(&[20, 30], &offers), Some(20));
        assert_eq!(pick(&[10, 20, 30], &offers), Some(10));
    }

    #[test]
    fn the_seeker_finds_nothing_when_no_advertiser_would_have_it() {
        let offers = [offer(10, 0.9, 4), offer(20, 0.9, society::CORP_SIZE_CAP)];
        assert_eq!(pick(&[20], &offers), None, "a full corp is no offer");
        assert_eq!(pick(&[99], &offers), None, "not a bot corp");
        assert_eq!(pick(&[], &offers), None);
    }

    #[test]
    fn the_lines_name_the_corp() {
        assert_eq!(applied_line(7), "applied corp=7 via=finder");
    }

    #[test]
    fn a_long_corp_name_still_makes_a_title_the_server_takes() {
        let t = title(&"x".repeat(100));
        assert!(t.chars().count() <= eve_server::corp_rec::MAX_TITLE);
        assert!(!t.is_empty());
        assert_eq!(title("Invented"), "Invented recruiting");
    }
}
