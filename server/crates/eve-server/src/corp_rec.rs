//! **Corporation recruitment adverts** (lane `corp-ads`, #102): a corp's
//! personnel managers post adverts, and the corp finder lists them by the
//! finder's own filters. The wire is `service/corp_rec.rs`; this is the store
//! and the rules, with no registry.
//!
//! One delta kind carries the state:
//!
//! | kind | key | value |
//! | --- | --- | --- |
//! | [`KIND`] | `ad id` | `corp created expires type_mask lang_mask other_mask hour_mask min_sp title description recruiters` |
//! | [`KIND`] | `next` | `n`, the next advert id |
//!
//! `recruiters` is a comma list of character ids. Dates are FILETIMEs.
//!
//! What the client fixes (`appConst`, `recruitmentUtil`,
//! `corpRecruitmentContainer`, all **[V]**; the file:line of each is at the
//! item):
//!
//! * at most [`MAX_ADS`] adverts per corp (`corporationMaxRecruitmentAds`,
//!   `appConst.py:409`; the Create button disables at that count,
//!   `corp_ui_recruitment.py:93`);
//! * an advert runs at most [`MAX_DAYS`] days from now
//!   (`corporationMaxRecruitmentAdDuration`, `appConst.py:410`; the editor's
//!   extension slider is `max - daysRemaining`, `corpRecruitmentContainer.py:287`);
//! * a title is at most [`MAX_TITLE`] and a message at most
//!   [`MAX_DESCRIPTION`] characters (`appConst.py:412-413`); an empty title is
//!   refused by the client before the call (`:587`);
//! * a time zone is a 24-bit hour mask with at most [`MAX_HOURS`] hours set
//!   (`corp_ui_recruitment_const.py` `MAX_HOURS`, `recruitmentUtil.IsTimezoneValid`);
//! * the type mask is one bit per recruitment type id (`pow(2, type_id)`,
//!   `recruitment.py:_recruitment_type`). Which ids are playstyles, areas or
//!   languages is the client's static data, and the client refuses a bad mix
//!   before the call, so the server does not know the groups and checks only
//!   that a mask is not negative.
//!
//! **Who may post.** The client shows the Create/Edit/Remove buttons only to a
//! member of the advert's corp with `corpRolePersonnelManager` in
//! `session.corprole` (`recruitmentEntry.py:37-42` `HasAccess`) **[V]**; the
//! server enforces the same, and lets the CEO through, who holds every role.
//!
//! **Not the client's, all [A]:** the finder's matching below (the client sends
//! the filters and displays what comes back; the server owns the meaning), the
//! grade, and that adverts are free (the client has `AdvertPrice` text for a
//! per-duration price, `corpRecruitmentContainer.py:533`, but no ISK moves
//! here; falsifier: a retail ad charging at creation).

use crate::delta::Deltas;

/// The adverts and their counter.
pub const KIND: &str = "corpad";

/// `appConst.corporationMaxRecruitmentAds` **[V]**.
pub const MAX_ADS: usize = 3;
/// `appConst.corporationMaxRecruitmentAdDuration` (days) **[V]**.
pub const MAX_DAYS: i64 = 28;
/// `appConst.corporationRecMaxTitleLength` **[V]**.
pub const MAX_TITLE: usize = 40;
/// `appConst.corporationRecMaxMessageLength` **[V]**.
pub const MAX_DESCRIPTION: usize = 1000;
/// `corp_ui_recruitment_const.MAX_HOURS` **[V]**.
pub const MAX_HOURS: u32 = 8;
/// Recruiters named on one advert. Not the client's; a bound on one call.
pub const MAX_RECRUITERS: usize = 10;
/// The client's `maxValue` for the minimum-SP box
/// (`corpRecruitmentContainer.py:340`) **[V]**.
pub const MAX_MIN_SP: i64 = 600_000_000;

/// `appConst.corpRolePersonnelManager` **[V]**; the wire module's tests pin it
/// to `service::corp::CORP_ROLE_PERSONNEL_MANAGER`.
pub const ROLE_PERSONNEL_MANAGER: i64 = 128;

const DAY: i64 = crate::seed::DAY;

/// Whether a member whose session roles are `roles` (or the CEO) may post,
/// edit and remove adverts.
pub fn may_manage(roles: i64, is_ceo: bool) -> bool {
    is_ceo || roles & ROLE_PERSONNEL_MANAGER != 0
}

/// Why a call is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NoTitle,
    TitleTooLong,
    DescriptionTooLong,
    /// A mask is negative, or the time zone has more than [`MAX_HOURS`] hours.
    BadMask,
    /// Zero or more than [`MAX_DAYS`] days on a new advert, or an extension
    /// that is negative.
    BadDuration,
    /// The corp already has [`MAX_ADS`] live adverts.
    TooManyAds,
    /// A negative or over-large minimum SP.
    BadMinSp,
    /// No such advert in this corp.
    NoSuchAd,
}

/// The fields a create or an edit sets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fields {
    pub type_mask: i64,
    pub lang_mask: i64,
    pub other_mask: i64,
    pub hour_mask: i64,
    pub min_sp: i64,
    pub title: String,
    pub description: String,
    pub recruiters: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ad {
    pub id: i64,
    pub corp: i64,
    pub created: i64,
    pub expires: i64,
    pub f: Fields,
}

impl Ad {
    pub fn live(&self, now: i64) -> bool {
        self.expires > now
    }
}

/// Check and normalise the fields of a create or an edit: the title is
/// trimmed (the message is the editor's text, kept as typed), recruiters are
/// de-duplicated and capped.
pub fn validate(mut f: Fields) -> Result<Fields, Refusal> {
    f.title = f.title.trim().to_owned();
    if f.title.is_empty() {
        return Err(Refusal::NoTitle);
    }
    if f.title.chars().count() > MAX_TITLE {
        return Err(Refusal::TitleTooLong);
    }
    if f.description.chars().count() > MAX_DESCRIPTION {
        return Err(Refusal::DescriptionTooLong);
    }
    if f.type_mask < 0 || f.lang_mask < 0 || f.other_mask < 0 || f.hour_mask < 0 {
        return Err(Refusal::BadMask);
    }
    // 24 hours in a day; a bit above that is not an hour.
    if f.hour_mask >> 24 != 0 || f.hour_mask.count_ones() > MAX_HOURS {
        return Err(Refusal::BadMask);
    }
    if !(0..=MAX_MIN_SP).contains(&f.min_sp) {
        return Err(Refusal::BadMinSp);
    }
    let mut seen = Vec::new();
    for r in f.recruiters.drain(..) {
        if !seen.contains(&r) {
            seen.push(r);
        }
    }
    seen.truncate(MAX_RECRUITERS);
    f.recruiters = seen;
    Ok(f)
}

fn ad_key(id: i64) -> Vec<String> {
    vec!["ad".to_owned(), id.to_string()]
}

impl Ad {
    fn row(&self) -> Vec<String> {
        vec![
            self.corp.to_string(),
            self.created.to_string(),
            self.expires.to_string(),
            self.f.type_mask.to_string(),
            self.f.lang_mask.to_string(),
            self.f.other_mask.to_string(),
            self.f.hour_mask.to_string(),
            self.f.min_sp.to_string(),
            self.f.title.clone(),
            self.f.description.clone(),
            self.f
                .recruiters
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ]
    }

    fn parse(id: i64, v: &[String]) -> Option<Ad> {
        let n = |i: usize| v.get(i)?.parse::<i64>().ok();
        let recruiters = v
            .get(10)?
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<i64>().ok())
            .collect::<Option<Vec<_>>>()?;
        Some(Ad {
            id,
            corp: n(0)?,
            created: n(1)?,
            expires: n(2)?,
            f: Fields {
                type_mask: n(3)?,
                lang_mask: n(4)?,
                other_mask: n(5)?,
                hour_mask: n(6)?,
                min_sp: n(7)?,
                title: v.get(8)?.clone(),
                description: v.get(9)?.clone(),
                recruiters,
            },
        })
    }
}

/// Take the next advert id (1, 2, ...).
fn alloc_id(d: &mut Deltas) -> i64 {
    let key = vec!["next".to_owned()];
    let n = d
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1);
    d.put(KIND, key, vec![(n + 1).to_string()]);
    n
}

pub fn get(d: &Deltas, id: i64) -> Option<Ad> {
    Ad::parse(id, d.of(KIND).get(&ad_key(id))?)
}

fn all(d: &Deltas) -> impl Iterator<Item = Ad> + '_ {
    d.of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == "ad")
        .filter_map(|(k, row)| Ad::parse(k[1].parse().ok()?, row))
}

/// One corp's adverts that have not run out, oldest first.
pub fn of_corp(d: &Deltas, corp: i64, now: i64) -> Vec<Ad> {
    let mut v: Vec<Ad> = all(d).filter(|a| a.corp == corp && a.live(now)).collect();
    v.sort_by_key(|a| a.id);
    v
}

/// Every live advert, oldest first.
pub fn live(d: &Deltas, now: i64) -> Vec<Ad> {
    let mut v: Vec<Ad> = all(d).filter(|a| a.live(now)).collect();
    v.sort_by_key(|a| a.id);
    v
}

/// Drop the adverts that have run out. Returns the corps that lost one.
pub fn expire(d: &mut Deltas, now: i64) -> Vec<i64> {
    let gone: Vec<Ad> = all(d).filter(|a| !a.live(now)).collect();
    let mut corps = Vec::new();
    for a in &gone {
        d.del(KIND, &ad_key(a.id));
        if !corps.contains(&a.corp) {
            corps.push(a.corp);
        }
    }
    corps
}

/// Post an advert for `corp` running `days` days from `now`. The caller has
/// checked the role.
pub fn create(
    d: &mut Deltas,
    corp: i64,
    now: i64,
    days: i64,
    fields: Fields,
) -> Result<Ad, Refusal> {
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(Refusal::BadDuration);
    }
    let f = validate(fields)?;
    if of_corp(d, corp, now).len() >= MAX_ADS {
        return Err(Refusal::TooManyAds);
    }
    let ad = Ad {
        id: alloc_id(d),
        corp,
        created: now,
        expires: now + days * DAY,
        f,
    };
    d.put(KIND, ad_key(ad.id), ad.row());
    Ok(ad)
}

/// Edit advert `id` of `corp`, and run it `added_days` longer, never past
/// [`MAX_DAYS`] from now. The caller has checked the role.
pub fn update(
    d: &mut Deltas,
    corp: i64,
    now: i64,
    id: i64,
    added_days: i64,
    fields: Fields,
) -> Result<Ad, Refusal> {
    let mut ad = get(d, id)
        .filter(|a| a.corp == corp && a.live(now))
        .ok_or(Refusal::NoSuchAd)?;
    if added_days < 0 {
        return Err(Refusal::BadDuration);
    }
    ad.f = validate(fields)?;
    ad.expires = (ad.expires + added_days * DAY).min(now + MAX_DAYS * DAY);
    d.put(KIND, ad_key(ad.id), ad.row());
    Ok(ad)
}

/// Remove advert `id` of `corp`.
pub fn delete(d: &mut Deltas, corp: i64, id: i64) -> Result<(), Refusal> {
    get(d, id)
        .filter(|a| a.corp == corp)
        .ok_or(Refusal::NoSuchAd)?;
    d.del(KIND, &ad_key(id));
    Ok(())
}

/// What the finder knows about an advert's corp, which the store does not
/// hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorpFacts {
    pub members: i64,
    pub isk_tax: f64,
    pub lp_tax: f64,
    pub in_alliance: bool,
    pub friendly_fire: bool,
    /// The corp takes applications (`isRecruiting`).
    pub recruiting: bool,
}

/// The finder's filters, in `SearchCorpAds`' argument order
/// (`base_corporation.py:636-637`) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub type_mask: i64,
    pub lang_mask: i64,
    pub exclude_alliances: bool,
    pub exclude_friendly_fire: bool,
    /// `None` when the box is not ticked.
    pub sp_restriction: Option<i64>,
    pub min_members: i64,
    pub max_members: i64,
    pub max_isk_tax: f64,
    pub max_lp_tax: f64,
    pub time_mask: i64,
    pub other_mask: i64,
}

/// The grade (a percentage) when `ad` passes the filter, else `None`.
///
/// **[A]** the whole meaning, the client sending only the filters and showing
/// the pair `(grade, advert)` (`GetRecruitementEntryDataList` reads
/// `for grade, advert in ads`, `node.grade` is printed as a percentage,
/// `recruitmentEntry.py:166-167`). Rules, falsified by a retail search that
/// lists an advert these rules drop or drops one they list:
///
/// * a corp that is not recruiting, or whose members are outside
///   `min_members..=max_members`, or whose taxes are above the maxima, is out;
/// * `exclude_alliances` drops corps in an alliance, `exclude_friendly_fire`
///   drops corps with friendly fire on;
/// * `sp_restriction` drops an advert whose minimum SP is above it;
/// * a language filter needs one language in common; every ticked "other"
///   box (new pilot friendly, focused, roleplay) must be set on the advert;
/// * the search hours must overlap the advert's hours (an advert with no
///   hours is any time);
/// * with a type filter an advert must share at least one type, and the grade
///   is the share of wanted types it has; with none the grade is 100.
pub fn grade(ad: &Ad, facts: &CorpFacts, q: &Filter) -> Option<i64> {
    if !facts.recruiting
        || facts.members < q.min_members
        || facts.members > q.max_members
        || facts.isk_tax > q.max_isk_tax + 1e-9
        || facts.lp_tax > q.max_lp_tax + 1e-9
        || (q.exclude_alliances && facts.in_alliance)
        || (q.exclude_friendly_fire && facts.friendly_fire)
        || q.sp_restriction.is_some_and(|sp| ad.f.min_sp > sp)
    {
        return None;
    }
    if q.lang_mask != 0 && ad.f.lang_mask & q.lang_mask == 0 {
        return None;
    }
    if ad.f.other_mask & q.other_mask != q.other_mask {
        return None;
    }
    if ad.f.hour_mask != 0 && q.time_mask != 0 && ad.f.hour_mask & q.time_mask == 0 {
        return None;
    }
    if q.type_mask == 0 {
        return Some(100);
    }
    let wanted = i64::from(q.type_mask.count_ones());
    let have = i64::from((ad.f.type_mask & q.type_mask).count_ones());
    if have == 0 {
        return None;
    }
    Some(have * 100 / wanted)
}

/// Rank matches: best grade first, then oldest advert.
pub fn rank(mut hits: Vec<(i64, Ad)>) -> Vec<(i64, Ad)> {
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: nothing here is a client row.
    const CORP: i64 = 98_000_801;
    const OTHER: i64 = 98_000_802;
    const NOW: i64 = 134_116_992_000_000_000;

    fn fields(title: &str) -> Fields {
        Fields {
            type_mask: 0b1010,
            lang_mask: 0b1,
            title: title.to_owned(),
            description: "We mine rocks.\nTabs\tare fine.".to_owned(),
            recruiters: vec![7, 8],
            ..Fields::default()
        }
    }

    fn facts() -> CorpFacts {
        CorpFacts {
            members: 20,
            isk_tax: 0.1,
            lp_tax: 0.1,
            in_alliance: false,
            friendly_fire: false,
            recruiting: true,
        }
    }

    fn any() -> Filter {
        Filter {
            type_mask: 0,
            lang_mask: 0,
            exclude_alliances: false,
            exclude_friendly_fire: false,
            sp_restriction: None,
            min_members: 0,
            max_members: 1000,
            max_isk_tax: 1.0,
            max_lp_tax: 1.0,
            time_mask: 0,
            other_mask: 0,
        }
    }

    #[test]
    fn an_advert_round_trips_text_and_recruiters() {
        let mut d = Deltas::default();
        let ad = create(&mut d, CORP, NOW, 7, fields("Miners wanted")).unwrap();
        assert_eq!(ad.id, 1);
        assert_eq!(ad.expires, NOW + 7 * DAY);
        assert_eq!(get(&d, 1), Some(ad));
        assert_eq!(get(&d, 1).unwrap().f.recruiters, vec![7, 8]);
    }

    #[test]
    fn ids_count_up_and_a_corp_holds_three() {
        let mut d = Deltas::default();
        for n in 0..MAX_ADS {
            let ad = create(&mut d, CORP, NOW, 3, fields(&format!("ad {n}"))).unwrap();
            assert_eq!(ad.id, n as i64 + 1);
        }
        assert_eq!(
            create(&mut d, CORP, NOW, 3, fields("one too many")),
            Err(Refusal::TooManyAds)
        );
        // Another corp has its own three.
        assert!(create(&mut d, OTHER, NOW, 3, fields("mine")).is_ok());
        assert_eq!(of_corp(&d, CORP, NOW).len(), 3);
        assert_eq!(of_corp(&d, OTHER, NOW).len(), 1);
    }

    #[test]
    fn a_title_and_a_duration_are_checked() {
        let mut d = Deltas::default();
        assert_eq!(
            create(&mut d, CORP, NOW, 3, fields("   ")),
            Err(Refusal::NoTitle)
        );
        assert_eq!(
            create(&mut d, CORP, NOW, 3, fields(&"x".repeat(MAX_TITLE + 1))),
            Err(Refusal::TitleTooLong)
        );
        assert!(create(&mut d, CORP, NOW, 3, fields(&"x".repeat(MAX_TITLE))).is_ok());
        for days in [0, -1, MAX_DAYS + 1] {
            assert_eq!(
                create(&mut d, CORP, NOW, days, fields("t")),
                Err(Refusal::BadDuration),
                "{days}"
            );
        }
        let mut long = fields("t");
        long.description = "y".repeat(MAX_DESCRIPTION + 1);
        assert_eq!(
            create(&mut d, CORP, NOW, 3, long),
            Err(Refusal::DescriptionTooLong)
        );
    }

    #[test]
    fn masks_and_minimum_sp_are_bounded() {
        let mut d = Deltas::default();
        let mut f = fields("t");
        f.type_mask = -1;
        assert_eq!(create(&mut d, CORP, NOW, 3, f), Err(Refusal::BadMask));
        // Nine hours is more than the eight the client allows.
        let mut f = fields("t");
        f.hour_mask = 0b1_1111_1111;
        assert_eq!(create(&mut d, CORP, NOW, 3, f), Err(Refusal::BadMask));
        let mut f = fields("t");
        f.hour_mask = 0b1111_1111;
        assert!(create(&mut d, CORP, NOW, 3, f).is_ok());
        let mut f = fields("t");
        f.hour_mask = 1 << 24;
        assert_eq!(create(&mut d, CORP, NOW, 3, f), Err(Refusal::BadMask));
        let mut f = fields("t");
        f.min_sp = MAX_MIN_SP + 1;
        assert_eq!(create(&mut d, CORP, NOW, 3, f), Err(Refusal::BadMinSp));
    }

    #[test]
    fn recruiters_are_unique_and_capped() {
        let mut d = Deltas::default();
        let mut f = fields("t");
        f.recruiters = (1..=30).chain([1, 2]).collect();
        let ad = create(&mut d, CORP, NOW, 3, f).unwrap();
        assert_eq!(ad.f.recruiters, (1..=10).collect::<Vec<i64>>());
    }

    #[test]
    fn an_edit_replaces_the_fields_and_extends_to_the_cap() {
        let mut d = Deltas::default();
        let ad = create(&mut d, CORP, NOW, 20, fields("old")).unwrap();
        let later = NOW + DAY;
        let e = update(&mut d, CORP, later, ad.id, 5, fields("new")).unwrap();
        assert_eq!(e.f.title, "new");
        // 20 + 5 days from the start is 24 days; the cap is 28 from `later`.
        assert_eq!(e.expires, NOW + 25 * DAY);
        assert_eq!(get(&d, ad.id).unwrap().f.title, "new");
        // An extension past the cap stops at it.
        let e = update(&mut d, CORP, later, ad.id, 20, fields("new")).unwrap();
        assert_eq!(e.expires, later + MAX_DAYS * DAY);
        assert_eq!(
            update(&mut d, CORP, later, ad.id, -1, fields("x")),
            Err(Refusal::BadDuration)
        );
    }

    #[test]
    fn an_edit_or_delete_of_another_corps_advert_finds_nothing() {
        let mut d = Deltas::default();
        let ad = create(&mut d, CORP, NOW, 3, fields("mine")).unwrap();
        assert_eq!(
            update(&mut d, OTHER, NOW, ad.id, 0, fields("stolen")),
            Err(Refusal::NoSuchAd)
        );
        assert_eq!(delete(&mut d, OTHER, ad.id), Err(Refusal::NoSuchAd));
        assert_eq!(delete(&mut d, CORP, 999), Err(Refusal::NoSuchAd));
        assert_eq!(get(&d, ad.id).unwrap().f.title, "mine");
        assert_eq!(delete(&mut d, CORP, ad.id), Ok(()));
        assert_eq!(get(&d, ad.id), None);
        // The id is not reused.
        assert_eq!(create(&mut d, CORP, NOW, 3, fields("n")).unwrap().id, 2);
    }

    #[test]
    fn a_run_out_advert_is_hidden_then_dropped_and_frees_a_slot() {
        let mut d = Deltas::default();
        for _ in 0..MAX_ADS {
            create(&mut d, CORP, NOW, 3, fields("t")).unwrap();
        }
        let after = NOW + 3 * DAY;
        assert!(of_corp(&d, CORP, after).is_empty());
        assert!(live(&d, after).is_empty());
        // An expired one is not editable.
        assert_eq!(
            update(&mut d, CORP, after, 1, 0, fields("t")),
            Err(Refusal::NoSuchAd)
        );
        // And a new one fits at once.
        assert!(create(&mut d, CORP, after, 3, fields("fresh")).is_ok());
        assert_eq!(expire(&mut d, after), vec![CORP]);
        assert!(get(&d, 1).is_none());
        assert_eq!(live(&d, after).len(), 1);
        assert!(expire(&mut d, after).is_empty());
    }

    #[test]
    fn the_role_gate_is_personnel_manager_or_ceo() {
        assert!(may_manage(ROLE_PERSONNEL_MANAGER, false));
        assert!(may_manage(ROLE_PERSONNEL_MANAGER | 1, false));
        assert!(may_manage(0, true));
        // Director alone is not the client's gate.
        assert!(!may_manage(1, false));
        assert!(!may_manage(0, false));
    }

    fn ad_with(f: impl FnOnce(&mut Fields)) -> Ad {
        let mut fl = fields("t");
        f(&mut fl);
        Ad {
            id: 1,
            corp: CORP,
            created: NOW,
            expires: NOW + DAY,
            f: fl,
        }
    }

    #[test]
    fn no_filter_lists_every_recruiting_advert() {
        let ad = ad_with(|_| {});
        assert_eq!(grade(&ad, &facts(), &any()), Some(100));
        let mut f = facts();
        f.recruiting = false;
        assert_eq!(grade(&ad, &f, &any()), None);
    }

    #[test]
    fn the_type_filter_grades_by_share_of_wanted_types() {
        let ad = ad_with(|f| f.type_mask = 0b0110);
        let mut q = any();
        q.type_mask = 0b0110;
        assert_eq!(grade(&ad, &facts(), &q), Some(100));
        q.type_mask = 0b0011;
        assert_eq!(grade(&ad, &facts(), &q), Some(50));
        q.type_mask = 0b1000;
        assert_eq!(grade(&ad, &facts(), &q), None);
    }

    #[test]
    fn language_other_and_hours_filters() {
        let ad = ad_with(|f| {
            f.lang_mask = 0b100;
            f.other_mask = 0b11;
            f.hour_mask = 0b1111;
        });
        let mut q = any();
        q.lang_mask = 0b100;
        assert!(grade(&ad, &facts(), &q).is_some());
        q.lang_mask = 0b010;
        assert!(grade(&ad, &facts(), &q).is_none());
        let mut q = any();
        q.other_mask = 0b01;
        assert!(grade(&ad, &facts(), &q).is_some());
        q.other_mask = 0b100;
        assert!(grade(&ad, &facts(), &q).is_none());
        let mut q = any();
        q.time_mask = 0b1_0000;
        assert!(grade(&ad, &facts(), &q).is_none());
        q.time_mask = 0b1_1000;
        assert!(grade(&ad, &facts(), &q).is_some());
        // An advert with no hours is any time.
        let open = ad_with(|f| f.hour_mask = 0);
        q.time_mask = 0b1_0000;
        assert!(grade(&open, &facts(), &q).is_some());
    }

    #[test]
    fn corp_filters_members_tax_alliance_friendly_fire_and_sp() {
        let ad = ad_with(|f| f.min_sp = 5_000_000);
        let mut q = any();
        q.min_members = 21;
        assert!(grade(&ad, &facts(), &q).is_none());
        q.min_members = 20;
        q.max_members = 19;
        assert!(grade(&ad, &facts(), &q).is_none());
        let mut q = any();
        q.max_isk_tax = 0.05;
        assert!(grade(&ad, &facts(), &q).is_none());
        q.max_isk_tax = 0.1;
        assert!(grade(&ad, &facts(), &q).is_some());
        q.max_lp_tax = 0.0;
        assert!(grade(&ad, &facts(), &q).is_none());
        let mut f = facts();
        f.in_alliance = true;
        let mut q = any();
        assert!(grade(&ad, &f, &q).is_some());
        q.exclude_alliances = true;
        assert!(grade(&ad, &f, &q).is_none());
        let mut f = facts();
        f.friendly_fire = true;
        let mut q = any();
        q.exclude_friendly_fire = true;
        assert!(grade(&ad, &f, &q).is_none());
        let mut q = any();
        q.sp_restriction = Some(4_999_999);
        assert!(grade(&ad, &facts(), &q).is_none());
        q.sp_restriction = Some(5_000_000);
        assert!(grade(&ad, &facts(), &q).is_some());
    }

    #[test]
    fn ranking_is_grade_then_age() {
        let a = |id| Ad {
            id,
            ..ad_with(|_| {})
        };
        let r = rank(vec![(50, a(1)), (100, a(3)), (100, a(2))]);
        assert_eq!(
            r.iter().map(|(_, a)| a.id).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
    }

    #[test]
    fn an_unparseable_row_is_skipped_not_fatal() {
        let mut d = Deltas::default();
        d.put(KIND, ad_key(5), vec!["not a number".to_owned()]);
        create(&mut d, CORP, NOW, 3, fields("t")).unwrap();
        assert_eq!(live(&d, NOW).len(), 1);
    }
}
