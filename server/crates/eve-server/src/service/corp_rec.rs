//! **`corpRecProxy`: the corporation recruitment adverts and the corp finder**
//! (lane `corp-ads`, #102), answered from `crate::corp_rec`. State and rules
//! live there; this is the wire.
//!
//! | call (client file:line **[V]**) | answer |
//! | --- | --- |
//! | `GetRecruitmentAdsForCorporation()` (`bco_recruitment.py:48`) | list of advert rows of the caller's corp |
//! | `GetRecruitmentAdsByCorpID(corpID)` (`base_corporation.py:631`) | list of advert rows of that corp |
//! | `GetRecruitmentAd(corpID, adID)` (`:640`) | one advert row, or `None` |
//! | `GetRecruiters(adID)` (`:634`) | list of character ids |
//! | `SearchCorpAds(typeMask, langMask, excludeAlliances, excludeFriendlyFire, spRestriction, minMembers, maxMembers, maxISKTaxRate, maxLPTaxRate, searchTimeMask, otherMask)` (`:637`) | list of `(grade, advert row)` |
//! | `CreateRecruitmentAd(days, typeMask, langMask, description, recruiters, title, hourMask1, minSP, otherMask)` (`:619`) | `None`; `OnCorporationRecruitmentAdChanged` to the corp |
//! | `UpdateRecruitmentAd(adID, typeMask, langMask, description, recruiters, title, addedDays, hourMask1, minSP=, otherMask=)` (`:622`) | `None`; the same push |
//! | `DeleteRecruitmentAd(adID)` (`:625`) | `None`; the same push |
//!
//! **The advert row.** `bco_recruitment.GetRecruitmentAdsForCorporation`
//! reads `recruitment.corporationID` and `.adID` and builds a
//! `Rowset(recruitment.__columns__)` when `type(recruitment) == blue.DBRow`,
//! else `Rowset(recruitment.header)`, so a `DbRow` is the answer that cannot
//! trip (a `KeyVal` has no `header`). The columns are the ones the client
//! reads: `adID corporationID allianceID channelID createDateTime
//! expiryDateTime typeMask langMask otherMask hourMask1 minSP title
//! description memberCount` (`recruitmentEntry.py`,
//! `corpRecruitmentContainer.py:200-230`, `base_corporation.py:1081-1101`).
//! `channelID` is always null: no advert chat channel exists here, and the
//! client only copies it into the entry data. The masks are `I8`, since the
//! type mask has a bit per recruitment type id up to the 40s.
//!
//! Errors: `CrpAccessDenied` for a caller without the Personnel Manager role,
//! `CustomNotify` text for a rule refusal (the client has no `UserError` of
//! its own for a server refusal here).

use super::args::Args;
use super::*;
use crate::corp_rec::{self as cr, Ad, CorpFacts, Fields, Filter, Refusal};

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["corpRecProxy"];

/// `OnCorporationRecruitmentAdChanged`: the advert list moved.
#[derive(Debug, Clone, PartialEq)]
pub struct AdPush {
    pub to: Vec<i64>,
}

impl crate::notify::Push for AdPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnCorporationRecruitmentAdChanged", vec![])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

fn tell_corp(reg: &mut Registry, corp: i64) {
    let to = reg.corporations().members_of(corp);
    if !to.is_empty() {
        reg.schedule(std::time::Duration::ZERO, AdPush { to });
    }
}

fn raise(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

fn custom(text: &str) -> ServiceError {
    raise("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

fn refusal(r: Refusal) -> ServiceError {
    custom(match r {
        Refusal::NoTitle => "An advert needs a title.",
        Refusal::TitleTooLong => "That advert title is too long.",
        Refusal::DescriptionTooLong => "That advert message is too long.",
        Refusal::BadMask => "That advert's play styles or hours are not valid.",
        Refusal::BadDuration => "That is not a duration an advert can run.",
        Refusal::TooManyAds => "Your corporation already has the most adverts it can have.",
        Refusal::BadMinSp => "That minimum skill point requirement is not valid.",
        Refusal::NoSuchAd => "That advert does not exist in your corporation.",
    })
}

/// The caller's corp when they may manage adverts.
fn manager(reg: &Registry, me: i64) -> Result<i64, ServiceError> {
    const NEED: &str = "You need the Personnel Manager role to do that.";
    let denied = || {
        raise(
            "CrpAccessDenied",
            vec![("reason", PyValue::Unicode(NEED.to_owned()))],
        )
    };
    let m = reg.corporations().membership(me).ok_or_else(denied)?;
    let is_ceo = reg
        .corporations()
        .get(m.corporation_id)
        .is_some_and(|c| c.ceo_id == me);
    // The client's `session.corprole`: all-locations roles plus "other".
    if cr::may_manage(m.roles | m.roles_at_other, is_ceo) {
        Ok(m.corporation_id)
    } else {
        Err(denied())
    }
}

const AD_COLUMNS: &[(&str, DbType)] = &[
    ("adID", DbType::I4),
    ("corporationID", DbType::I4),
    ("allianceID", DbType::I4),
    ("channelID", DbType::I4),
    ("createDateTime", DbType::FileTime),
    ("expiryDateTime", DbType::FileTime),
    ("typeMask", DbType::I8),
    ("langMask", DbType::I8),
    ("otherMask", DbType::I8),
    ("hourMask1", DbType::I8),
    ("minSP", DbType::I8),
    ("title", DbType::WStr),
    ("description", DbType::WStr),
    ("memberCount", DbType::I4),
];

fn alliance_of(reg: &Registry, corp: i64) -> Option<i64> {
    crate::alliance::Alliances::load(reg.deltas())
        .member(corp)
        .map(|m| m.alliance_id)
}

fn text(s: &str) -> DbValue {
    DbValue::Object(PyValue::Unicode(s.to_owned()))
}

fn ad_row(reg: &Registry, ad: &Ad) -> PyValue {
    let values = vec![
        DbValue::Int(ad.id),
        DbValue::Int(ad.corp),
        alliance_of(reg, ad.corp).map_or(DbValue::Null, DbValue::Int),
        DbValue::Null,
        DbValue::Int(ad.created),
        DbValue::Int(ad.expires),
        DbValue::Int(ad.f.type_mask),
        DbValue::Int(ad.f.lang_mask),
        DbValue::Int(ad.f.other_mask),
        DbValue::Int(ad.f.hour_mask),
        DbValue::Int(ad.f.min_sp),
        text(&ad.f.title),
        text(&ad.f.description),
        DbValue::Int(reg.corporations().members_of(ad.corp).len() as i64),
    ];
    let d = DbRowDescriptor::from_pairs(AD_COLUMNS).expect("the advert columns are a valid layout");
    match DbRow::new(d, values) {
        Ok(r) => PyValue::DbRow(Box::new(r)),
        Err(_) => PyValue::None,
    }
}

fn facts(reg: &Registry, corp: i64, now: i64) -> Option<CorpFacts> {
    let c = reg.corporations().get(corp).filter(|c| !c.deleted)?;
    Some(CorpFacts {
        members: reg.corporations().members_of(corp).len() as i64,
        isk_tax: c.tax_rate,
        lp_tax: c.lp_tax_rate,
        in_alliance: alliance_of(reg, corp).is_some(),
        friendly_fire: c.aggression.legal_at(now),
        recruiting: c.recruiting,
    })
}

/// The shared middle of create and update: arguments `first ..` are
/// `typeMask, langMask, description, recruiters, title`.
fn fields(a: &Args, first: usize) -> Fields {
    let recruiters = a
        .get_or_kw(first + 3, "recruiters")
        .and_then(|v| match v {
            PyValue::List(x) | PyValue::Tuple(x) => Some(x.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
        .iter()
        .filter_map(|v| super::args::as_int(Some(v)))
        .collect();
    Fields {
        type_mask: a.int_lax_or_kw(first, "typeMask").unwrap_or(0),
        lang_mask: a.int_lax_or_kw(first + 1, "langMask").unwrap_or(0),
        description: a.text_or_kw(first + 2, "description").unwrap_or_default(),
        recruiters,
        title: a.text_or_kw(first + 4, "title").unwrap_or_default(),
        ..Fields::default()
    }
}

/// Recruiters must be members of the corp: the client offers only members.
fn in_corp(reg: &Registry, corp: i64, ids: Vec<i64>) -> Vec<i64> {
    let members = reg.corporations().members_of(corp);
    ids.into_iter().filter(|c| members.contains(c)).collect()
}

fn create(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let corp = manager(reg, me)?;
    let a = Args::new(req);
    // (days, typeMask, langMask, description, recruiters, title, hourMask1,
    //  minSP, otherMask)
    let days = a.int_lax_or_kw(0, "days").unwrap_or(0);
    let mut f = fields(&a, 1);
    f.hour_mask = a.int_lax_or_kw(6, "hourMask1").unwrap_or(0);
    f.min_sp = a.int_lax_or_kw(7, "minSP").unwrap_or(0);
    f.other_mask = a.int_lax_or_kw(8, "otherMask").unwrap_or(0);
    f.recruiters = in_corp(reg, corp, f.recruiters);
    let now = crate::pyutil::filetime_now();
    cr::expire(reg.deltas_mut(), now);
    cr::create(reg.deltas_mut(), corp, now, days, f).map_err(refusal)?;
    tell_corp(reg, corp);
    Ok(PyValue::None)
}

fn update(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let corp = manager(reg, me)?;
    let a = Args::new(req);
    // (adID, typeMask, langMask, description, recruiters, title, addedDays,
    //  hourMask1, minSP=, otherMask=)
    let id = a.int_lax_or_kw(0, "adID").unwrap_or(0);
    let mut f = fields(&a, 1);
    let added = a.int_lax_or_kw(6, "addedDays").unwrap_or(0);
    f.hour_mask = a.int_lax_or_kw(7, "hourMask1").unwrap_or(0);
    f.min_sp = a.int_lax_or_kw(8, "minSP").unwrap_or(0);
    f.other_mask = a.int_lax_or_kw(9, "otherMask").unwrap_or(0);
    f.recruiters = in_corp(reg, corp, f.recruiters);
    let now = crate::pyutil::filetime_now();
    cr::update(reg.deltas_mut(), corp, now, id, added, f).map_err(refusal)?;
    tell_corp(reg, corp);
    Ok(PyValue::None)
}

fn delete(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let corp = manager(reg, me)?;
    let id = Args::new(req).int_lax_or_kw(0, "adID").unwrap_or(0);
    cr::delete(reg.deltas_mut(), corp, id).map_err(refusal)?;
    tell_corp(reg, corp);
    Ok(PyValue::None)
}

fn search(reg: &Registry, req: &Request<'_>) -> PyValue {
    let a = Args::new(req);
    // (typeMask, langMask, excludeAlliances, excludeFriendlyFire,
    //  spRestriction, minMembers, maxMembers, maxISKTaxRate, maxLPTaxRate,
    //  searchTimeMask, otherMask)
    let int = |i: usize, kw: &str, d: i64| a.int_lax_or_kw(i, kw).unwrap_or(d);
    let q = Filter {
        type_mask: int(0, "typeMask", 0),
        lang_mask: int(1, "langMask", 0),
        exclude_alliances: a.flag_lax_or_kw(2, "excludeAlliances"),
        exclude_friendly_fire: a.flag_lax_or_kw(3, "excludeFriendlyFire"),
        sp_restriction: a.int_lax_or_kw(4, "spRestriction"),
        min_members: int(5, "minMembers", 0),
        max_members: int(6, "maxMembers", i64::MAX),
        max_isk_tax: a.float_or_kw(7, "maxISKTaxRate").unwrap_or(1.0),
        max_lp_tax: a.float_or_kw(8, "maxLPTaxRate").unwrap_or(1.0),
        time_mask: int(9, "searchTimeMask", 0),
        other_mask: int(10, "otherMask", 0),
    };
    let now = crate::pyutil::filetime_now();
    let hits: Vec<(i64, Ad)> = cr::live(reg.deltas(), now)
        .into_iter()
        .filter_map(|ad| {
            let f = facts(reg, ad.corp, now)?;
            Some((cr::grade(&ad, &f, &q)?, ad))
        })
        .collect();
    PyValue::List(
        cr::rank(hits)
            .iter()
            .map(|(g, ad)| PyValue::Tuple(vec![PyValue::Int(*g), ad_row(reg, ad)]))
            .collect(),
    )
}

fn rows_of(reg: &Registry, corp: i64, now: i64) -> PyValue {
    PyValue::List(
        cr::of_corp(reg.deltas(), corp, now)
            .iter()
            .map(|ad| ad_row(reg, ad))
            .collect(),
    )
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let a = Args::new(req);
    let now = crate::pyutil::filetime_now();
    match req.method {
        "CreateRecruitmentAd" | "UpdateRecruitmentAd" | "DeleteRecruitmentAd" => {
            let Some(me) = req.actor else {
                return Err(reg.no_method(req));
            };
            match req.method {
                "CreateRecruitmentAd" => create(reg, req, me),
                "UpdateRecruitmentAd" => update(reg, req, me),
                _ => delete(reg, req, me),
            }
        }
        "GetRecruitmentAdsForCorporation" => {
            let corp = req
                .actor
                .and_then(|me| reg.corporations().membership(me))
                .map(|m| m.corporation_id);
            Ok(corp.map_or(PyValue::List(vec![]), |c| rows_of(reg, c, now)))
        }
        "GetRecruitmentAdsByCorpID" => {
            Ok(rows_of(reg, a.int_lax_or_kw(0, "corpID").unwrap_or(0), now))
        }
        "GetRecruitmentAd" => {
            let corp = a.int_lax_or_kw(0, "corpID").unwrap_or(0);
            let id = a.int_lax_or_kw(1, "adID").unwrap_or(0);
            Ok(cr::get(reg.deltas(), id)
                .filter(|ad| ad.corp == corp && ad.live(now))
                .map_or(PyValue::None, |ad| ad_row(reg, &ad)))
        }
        "GetRecruiters" => {
            let id = a.int_lax_or_kw(0, "adID").unwrap_or(0);
            Ok(PyValue::List(
                cr::get(reg.deltas(), id)
                    .filter(|ad| ad.live(now))
                    .map(|ad| ad.f.recruiters)
                    .unwrap_or_default()
                    .into_iter()
                    .map(PyValue::Int)
                    .collect(),
            ))
        }
        "SearchCorpAds" => Ok(search(reg, req)),
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: nothing here is a client row.
    const CEO: i64 = 90_000_811;
    const HR: i64 = 90_000_812;
    const PLAIN: i64 = 90_000_813;
    const SEEKER: i64 = 90_000_814;
    const RIVAL: i64 = 90_000_815;
    const CORP: i64 = 98_000_811;
    const RIVAL_CORP: i64 = 98_000_812;
    const PM: i64 = cr::ROLE_PERSONNEL_MANAGER;

    fn pilot(id: i64, name: &str) -> crate::model::Character {
        crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 100_000_000,
            ..crate::model::Character::default()
        }
    }

    fn record(reg: &mut Registry, fields: &[&str]) {
        reg.corporations
            .parse_record(fields)
            .expect("a world record");
    }

    /// `corp` record: id ceo creator created tax lptax recruit ff shapes(3)
    /// colors(3) name ticker description url.
    fn corp_record(reg: &mut Registry, corp: i64, ceo: i64, recruiting: bool, name: &str) {
        let (c, k) = (corp.to_string(), ceo.to_string());
        let r = if recruiting { "1" } else { "0" };
        record(
            reg,
            &[
                "corp", &c, &k, &k, "1", "0.1", "0.0", r, "0", "-1", "-1", "-1", "-1", "-1", "-1",
                name, name, "d", "u",
            ],
        );
    }

    /// CORP: CEO, HR (Personnel Manager) and PLAIN; RIVAL_CORP: RIVAL as CEO.
    /// SEEKER is in no player corp.
    fn world() -> Registry {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(CEO, "Ceo"),
                pilot(HR, "Hr"),
                pilot(PLAIN, "Plain"),
                pilot(SEEKER, "Seeker"),
                pilot(RIVAL, "Rival"),
            ];
        });
        corp_record(&mut reg, CORP, CEO, true, "Alpha");
        corp_record(&mut reg, RIVAL_CORP, RIVAL, true, "Beta");
        for (who, corp, roles) in [
            (CEO, CORP, 0),
            (HR, CORP, PM),
            (PLAIN, CORP, 0),
            (RIVAL, RIVAL_CORP, 0),
        ] {
            let (who, corp, roles) = (who.to_string(), corp.to_string(), roles.to_string());
            record(&mut reg, &["corpmember", &who, &corp, &roles, "1"]);
        }
        reg
    }

    fn call(
        reg: &mut Registry,
        who: Option<i64>,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "corpRecProxy",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: who,
        })
    }

    fn i(n: i64) -> PyValue {
        PyValue::Int(n)
    }

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raised exception: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("header")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("args")
        };
        match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the key is a str: {other:?}"),
        }
    }

    /// `CreateRecruitmentAd(days, typeMask, langMask, description,
    /// recruiters, title, hourMask1, minSP, otherMask)`.
    fn post(
        reg: &mut Registry,
        who: i64,
        title: &str,
        type_mask: i64,
    ) -> Result<PyValue, ServiceError> {
        call(
            reg,
            Some(who),
            "CreateRecruitmentAd",
            &[
                i(7),
                i(type_mask),
                i(1),
                u("We mine rocks."),
                PyValue::List(vec![i(HR), i(SEEKER)]),
                u(title),
                i(0),
                i(0),
                i(0),
            ],
        )
    }

    fn col(row: &PyValue, name: &str) -> DbValue {
        let PyValue::DbRow(r) = row else {
            panic!("not a DbRow: {row:?}")
        };
        let at = AD_COLUMNS
            .iter()
            .position(|(n, _)| *n == name)
            .expect("a column");
        r.values()[at].clone()
    }

    fn search_all(reg: &mut Registry, who: i64, type_mask: i64) -> Vec<(i64, PyValue)> {
        // typeMask, langMask, exclAlliances, exclFF, spRestriction, minMembers,
        // maxMembers, maxISK, maxLP, searchTimeMask, otherMask
        let v = call(
            reg,
            Some(who),
            "SearchCorpAds",
            &[
                i(type_mask),
                i(0),
                PyValue::Bool(false),
                PyValue::Bool(false),
                PyValue::None,
                i(0),
                i(1000),
                PyValue::Real(1.0),
                PyValue::Real(1.0),
                i(0),
                i(0),
            ],
        )
        .expect("search answers");
        let PyValue::List(rows) = v else {
            panic!("a list")
        };
        rows.into_iter()
            .map(|p| {
                let PyValue::Tuple(t) = p else {
                    panic!("a (grade, ad) pair")
                };
                let PyValue::Int(g) = t[0] else {
                    panic!("a grade")
                };
                (g, t[1].clone())
            })
            .collect()
    }

    #[test]
    fn the_personnel_manager_role_matches_the_corp_modules_bit() {
        assert_eq!(
            cr::ROLE_PERSONNEL_MANAGER,
            super::super::corp::CORP_ROLE_PERSONNEL_MANAGER
        );
    }

    #[test]
    fn nothing_posted_answers_empty_lists_for_every_read() {
        let mut reg = world();
        for method in ["GetRecruitmentAdsForCorporation", "SearchCorpAds"] {
            assert_eq!(
                call(&mut reg, Some(PLAIN), method, &[]).unwrap(),
                PyValue::List(vec![]),
                "{method}"
            );
        }
        assert_eq!(
            call(
                &mut reg,
                Some(PLAIN),
                "GetRecruitmentAdsByCorpID",
                &[i(CORP)]
            )
            .unwrap(),
            PyValue::List(vec![])
        );
        assert_eq!(
            call(&mut reg, Some(PLAIN), "GetRecruitmentAd", &[i(CORP), i(1)]).unwrap(),
            PyValue::None
        );
        assert_eq!(
            call(&mut reg, Some(PLAIN), "GetRecruiters", &[i(1)]).unwrap(),
            PyValue::List(vec![])
        );
        // No character: still a list, never None.
        assert_eq!(
            call(&mut reg, None, "GetRecruitmentAdsForCorporation", &[]).unwrap(),
            PyValue::List(vec![])
        );
    }

    #[test]
    fn a_personnel_manager_and_the_ceo_post_and_a_plain_member_cannot() {
        let mut reg = world();
        let e = post(&mut reg, PLAIN, "Nope", 2).expect_err("refused");
        assert_eq!(key_of(&e), "CrpAccessDenied");
        let e = post(&mut reg, SEEKER, "Nope", 2).expect_err("refused");
        assert_eq!(key_of(&e), "CrpAccessDenied");
        assert_eq!(
            post(&mut reg, HR, "Miners wanted", 2).unwrap(),
            PyValue::None
        );
        assert_eq!(
            post(&mut reg, CEO, "Haulers wanted", 4).unwrap(),
            PyValue::None
        );
        let mine = call(
            &mut reg,
            Some(PLAIN),
            "GetRecruitmentAdsForCorporation",
            &[],
        )
        .unwrap();
        let PyValue::List(rows) = mine else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(col(&rows[0], "adID"), DbValue::Int(1));
        assert_eq!(col(&rows[0], "corporationID"), DbValue::Int(CORP));
        assert_eq!(col(&rows[0], "memberCount"), DbValue::Int(3));
        assert_eq!(col(&rows[0], "allianceID"), DbValue::Null);
        assert_eq!(col(&rows[0], "channelID"), DbValue::Null);
        assert_eq!(col(&rows[0], "typeMask"), DbValue::Int(2));
        assert_eq!(col(&rows[0], "title"), text("Miners wanted"));
    }

    #[test]
    fn the_post_tells_the_corp_to_reload() {
        let mut reg = world();
        assert_eq!(reg.scheduled_len(), 0);
        post(&mut reg, HR, "Miners wanted", 2).unwrap();
        assert_eq!(reg.scheduled_len(), 1);
    }

    #[test]
    fn a_fourth_advert_and_a_blank_title_are_refused() {
        let mut reg = world();
        for n in 0..cr::MAX_ADS {
            post(&mut reg, HR, &format!("ad {n}"), 2).unwrap();
        }
        let e = post(&mut reg, HR, "ad 3", 2).expect_err("refused");
        assert_eq!(key_of(&e), "CustomNotify");
        let e = post(&mut reg, RIVAL, "  ", 2).expect_err("refused");
        assert_eq!(key_of(&e), "CustomNotify");
    }

    #[test]
    fn a_recruiter_outside_the_corp_is_dropped() {
        let mut reg = world();
        post(&mut reg, HR, "Miners wanted", 2).unwrap();
        // HR is a member, SEEKER is not.
        assert_eq!(
            call(&mut reg, Some(SEEKER), "GetRecruiters", &[i(1)]).unwrap(),
            PyValue::List(vec![i(HR)])
        );
    }

    #[test]
    fn the_finder_lists_it_for_a_stranger_and_grades_it() {
        let mut reg = world();
        post(&mut reg, HR, "Miners wanted", 0b0110).unwrap();
        post(&mut reg, RIVAL, "Pirates wanted", 0b1000).unwrap();
        let all = search_all(&mut reg, SEEKER, 0);
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|(g, _)| *g == 100));
        let only = search_all(&mut reg, SEEKER, 0b0010);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].0, 100);
        assert_eq!(col(&only[0].1, "corporationID"), DbValue::Int(CORP));
        let part = search_all(&mut reg, SEEKER, 0b1010);
        assert_eq!(part.len(), 2);
        assert_eq!(part[0].0, 50);
        assert_eq!(part[1].0, 50);
        assert!(search_all(&mut reg, SEEKER, 0b10_0000).is_empty());
    }

    #[test]
    fn the_by_corp_and_single_reads_find_it() {
        let mut reg = world();
        post(&mut reg, HR, "Miners wanted", 2).unwrap();
        let by = call(
            &mut reg,
            Some(SEEKER),
            "GetRecruitmentAdsByCorpID",
            &[i(CORP)],
        )
        .unwrap();
        let PyValue::List(rows) = by else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1);
        let one = call(&mut reg, Some(SEEKER), "GetRecruitmentAd", &[i(CORP), i(1)]).unwrap();
        assert_eq!(col(&one, "adID"), DbValue::Int(1));
        // Wrong corp for that id.
        assert_eq!(
            call(
                &mut reg,
                Some(SEEKER),
                "GetRecruitmentAd",
                &[i(RIVAL_CORP), i(1)]
            )
            .unwrap(),
            PyValue::None
        );
    }

    #[test]
    fn a_corp_that_stops_recruiting_leaves_the_finder_but_not_its_own_list() {
        let mut reg = world();
        post(&mut reg, HR, "Miners wanted", 2).unwrap();
        assert_eq!(search_all(&mut reg, SEEKER, 0).len(), 1);
        corp_record(&mut reg, CORP, CEO, false, "Alpha");
        assert!(search_all(&mut reg, SEEKER, 0).is_empty());
        let mine = call(&mut reg, Some(HR), "GetRecruitmentAdsForCorporation", &[]).unwrap();
        assert!(matches!(mine, PyValue::List(ref v) if v.len() == 1));
    }

    #[test]
    fn edit_and_delete_belong_to_the_adverts_own_corp() {
        let mut reg = world();
        post(&mut reg, HR, "Miners wanted", 2).unwrap();
        // UpdateRecruitmentAd(adID, typeMask, langMask, description,
        // recruiters, title, addedDays, hourMask1, minSP=, otherMask=)
        let upd = |reg: &mut Registry, who: i64, title: &str| {
            call(
                reg,
                Some(who),
                "UpdateRecruitmentAd",
                &[
                    i(1),
                    i(4),
                    i(1),
                    u("Now we haul."),
                    PyValue::List(vec![]),
                    u(title),
                    i(2),
                    i(0),
                    i(0),
                    i(0),
                ],
            )
        };
        let e = upd(&mut reg, RIVAL, "Hijack").expect_err("a rival corp's CEO finds nothing");
        assert_eq!(key_of(&e), "CustomNotify");
        let e = upd(&mut reg, PLAIN, "Hijack").expect_err("a plain member cannot");
        assert_eq!(key_of(&e), "CrpAccessDenied");
        assert_eq!(upd(&mut reg, CEO, "Haulers wanted").unwrap(), PyValue::None);
        let one = call(&mut reg, Some(SEEKER), "GetRecruitmentAd", &[i(CORP), i(1)]).unwrap();
        assert_eq!(col(&one, "title"), text("Haulers wanted"));
        assert_eq!(col(&one, "typeMask"), DbValue::Int(4));
        let e = call(&mut reg, Some(RIVAL), "DeleteRecruitmentAd", &[i(1)]).expect_err("no");
        assert_eq!(key_of(&e), "CustomNotify");
        let e = call(&mut reg, Some(PLAIN), "DeleteRecruitmentAd", &[i(1)]).expect_err("no");
        assert_eq!(key_of(&e), "CrpAccessDenied");
        assert_eq!(
            call(&mut reg, Some(HR), "DeleteRecruitmentAd", &[i(1)]).unwrap(),
            PyValue::None
        );
        assert!(search_all(&mut reg, SEEKER, 0).is_empty());
    }

    #[test]
    fn adverts_survive_the_world_file_through_the_delta_store() {
        let mut reg = world();
        post(&mut reg, HR, "Miners wanted", 2).unwrap();
        let rows = reg.deltas().of(cr::KIND).clone();
        assert!(!rows.is_empty());
        let mut again = world();
        for (k, v) in rows {
            again.deltas_mut().put(cr::KIND, k, v);
        }
        assert_eq!(search_all(&mut again, SEEKER, 0).len(), 1);
    }

    #[test]
    fn an_unknown_method_is_not_swallowed() {
        let mut reg = world();
        assert!(call(&mut reg, Some(HR), "NoSuchCall", &[]).is_err());
    }
}
