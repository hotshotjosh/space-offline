//! The Proving Grounds results tests. **Every id here is invented**: pilots
//! 990,1xx and 990,3xx, match type 990,901, schedules 990,801 to 990,803,
//! reward items 990,951, the skill point token 990,961 and its points. The
//! client's `typeKredits` appears by name only ([`TYPE_KREDITS`]); no `[A]`
//! value is asserted by its number.

use super::*;
use crate::content::Content;
use crate::model::{Character, SandboxConfig};
use crate::proving::MatchType;
use crate::proving::tests::{ft, header_for_queue};
use crate::pyutil::dict_get;
use crate::rollover::Day;
use crate::service::corp_wallet::Via;
use std::sync::Arc;
use std::time::Instant;

const A: i64 = 990_101;
const B: i64 = 990_102;
const C: i64 = 990_103;
const GONE: i64 = 990_199;
const MT: i64 = 990_901;
const S1: i64 = 990_801;
const S2: i64 = 990_802;
const S_QA: i64 = 990_803;
/// A type the world can make (the redeem store's invented fixture item).
const ITEM: i64 = crate::nes::fixture::ITEM;
/// A type nothing can make.
const ITEM_UNKNOWN: i64 = 990_951;
const ITEM_QTY: i64 = 3;
const SP_TOKEN: i64 = 990_961;
const SP_PER_TOKEN: i64 = 7_777;
const SP_TOKENS_GIVEN: i64 = 2;
const ISK_AMOUNT: i64 = 5_000;
/// The mission dial of the awards' world: not 1, so a grant that ignores the
/// dial shows.
const DIAL: f64 = 2.0;

fn finished(schedule: i64, win: &[i64], lose: &[i64]) -> MatchFinished {
    MatchFinished {
        match_type_id: MT,
        schedule_id: schedule,
        results: vec![
            (win.to_vec(), MatchResult::Win),
            (lose.to_vec(), MatchResult::Loss),
        ],
    }
}

fn drew(schedule: i64, a: &[i64], b: &[i64]) -> MatchFinished {
    MatchFinished {
        match_type_id: MT,
        schedule_id: schedule,
        results: vec![
            (a.to_vec(), MatchResult::Draw),
            (b.to_vec(), MatchResult::Draw),
        ],
    }
}

fn st(wins: i64, losses: i64, reached_at: i64) -> Stat {
    Stat {
        wins,
        losses,
        reached_at,
        ..Stat::default()
    }
}

fn catalogue() -> Catalogue {
    let row = |s: i64, start: &str, end: &str, qa: u8| {
        format!("{s}\t2x1\t\t{start}\t{end}\t{qa}\ttest\n")
    };
    let text = format!(
        "{}{}{}{}",
        header_for_queue(),
        row(S1, "2000-01-01", "2000-01-02", 0),
        row(S2, "2999-01-01", "2999-12-31", 0),
        row(S_QA, "", "", 1),
    );
    let mt = MatchType {
        id: MT,
        number_of_fleets: 2,
        fleet_size: 1,
        rewards: vec![
            (ITEM, ITEM_QTY),
            (TYPE_KREDITS, ISK_AMOUNT),
            (SP_TOKEN, SP_TOKENS_GIVEN),
            (SP_TOKEN_TYPES[0], 1),
            (ITEM_UNKNOWN, 1),
        ],
        ..MatchType::default()
    };
    Catalogue::parse(&text, vec![mt]).expect("the rows parse")
}

fn pilots() -> Vec<Character> {
    [(A, 11), (B, 12), (C, 13)]
        .into_iter()
        .map(|(id, user)| Character {
            character_id: id,
            ship_id: id + 1_000_000,
            owner_user_id: Some(user),
            ..Character::default()
        })
        .collect()
}

/// Three pilots of three accounts, the catalogue above, one skill point token
/// and a mission dial of [`DIAL`].
fn world_with(sp: bool) -> Registry {
    let mut content = Content::fixture();
    content.insert("proving_events", catalogue());
    if sp {
        content.insert(
            "proving_sp_tokens",
            SpTokens([(SP_TOKEN, SP_PER_TOKEN)].into_iter().collect()),
        );
    }
    let mut reg = Registry::new(SandboxConfig {
        characters: pilots(),
        ..SandboxConfig::default()
    })
    .with_content(Arc::new(content));
    crate::econ_scale::set(
        &mut reg,
        crate::econ_scale::EconScale {
            bounty: 1.0,
            mission: DIAL,
            npc_buy: 1.0,
            insurance: 1.0,
            isk: 1.0,
        },
    );
    reg
}

fn world() -> Registry {
    world_with(true)
}

/// A time at which S1 is over and S2 is not.
fn later() -> i64 {
    ft(2500, 6, 1, 12)
}

fn event(reg: &Registry, s: i64) -> Event {
    reg.content()
        .get::<Catalogue>()
        .all(later())
        .into_iter()
        .find(|e| e.schedule.id == s)
        .expect("the event")
}

type Due = (&'static str, Option<Vec<i64>>, Vec<PyValue>);

fn due(reg: &mut Registry) -> Vec<Due> {
    reg.take_due(Instant::now() + std::time::Duration::from_secs(60))
        .into_iter()
        .map(|p| {
            let (name, args) = p.to_push();
            (name, p.audience(), args)
        })
        .collect()
}

fn text(r: Result<PyValue, ServiceError>) -> String {
    match r.expect("an answer") {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("text, got {other:?}"),
    }
}

fn user_tokens(reg: &Registry, who: i64) -> Vec<crate::nes::Token> {
    crate::nes::tokens(reg.deltas(), crate::nes_vault::key_of(reg, who))
}

// ---- the statistics ----------------------------------------------------------

#[test]
fn a_one_v_one_win_counts_a_win_and_a_loss() {
    let mut d = Deltas::default();
    record(&mut d, &finished(S1, &[A], &[B]), 100);
    let (a, b) = (stat(&d, MT, S1, A), stat(&d, MT, S1, B));
    assert_eq!((a.wins, a.losses, a.draws), (1, 0, 0));
    assert_eq!((b.wins, b.losses, b.draws), (0, 1, 0));
    assert_eq!(a.reached_at, 100);
    assert_eq!(b.reached_at, 0, "a loss does not move it");
}

#[test]
fn a_draw_counts_a_draw_for_both() {
    let mut d = Deltas::default();
    record(&mut d, &drew(S1, &[A], &[B]), 100);
    for who in [A, B] {
        let s = stat(&d, MT, S1, who);
        assert_eq!((s.wins, s.losses, s.draws), (0, 0, 1));
    }
}

#[test]
fn schedules_never_mix() {
    let mut d = Deltas::default();
    record(&mut d, &finished(S1, &[A], &[B]), 100);
    assert_eq!(stat(&d, MT, S2, A), Stat::default());
    assert!(ranked(&d, MT, S2).is_empty());
    assert_eq!(standing(&d, MT, S2, A), (0, Stat::default()));
}

#[test]
fn a_second_win_adds_up_and_keeps_the_award_flag() {
    let mut d = Deltas::default();
    record(&mut d, &finished(S1, &[A], &[B]), 100);
    let mut s = stat(&d, MT, S1, A);
    s.awarded = true;
    put_stat(&mut d, MT, S1, A, s);
    record(&mut d, &finished(S1, &[A], &[B]), 200);
    let s = stat(&d, MT, S1, A);
    assert_eq!((s.wins, s.reached_at, s.awarded), (2, 200, true));
}

// ---- the rank ---------------------------------------------------------------

#[test]
fn rank_orders_by_wins_then_losses_then_first_to_reach_then_id() {
    let mut d = Deltas::default();
    // (character, tally), in a scrambled order so the key order is not the
    // answer; the expected order is the named rule applied by hand.
    let rows = [
        (990_215, st(1, 0, 70)),
        (990_212, st(2, 0, 60)),
        (990_217, st(0, 2, 0)),
        (990_210, st(3, 5, 90)),
        (990_213, st(2, 1, 10)),
        (990_216, st(0, 0, 0)),
        (990_211, st(2, 0, 50)),
        (990_214, st(1, 0, 70)),
    ];
    for (c, s) in rows {
        put_stat(&mut d, MT, S1, c, s);
    }
    let mut expect = rows;
    expect.sort_by_key(|(c, s)| (std::cmp::Reverse(s.wins), s.losses, s.reached_at, *c));
    let got: Vec<(i64, i64)> = ranked(&d, MT, S1)
        .iter()
        .map(|(c, r, _)| (*c, *r))
        .collect();
    let want: Vec<(i64, i64)> = expect
        .iter()
        .enumerate()
        .map(|(i, (c, _))| (*c, i as i64 + 1))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn the_board_lists_every_pilot_who_played_and_a_stranger_is_rank_zero() {
    let mut d = Deltas::default();
    record(&mut d, &finished(S1, &[A], &[B]), 100);
    let (rank, s) = standing(&d, MT, S1, B);
    assert_eq!(ranked(&d, MT, S1).len(), stats_of(&d, MT, S1).len());
    assert!(rank >= 1, "a pilot with a row has a rank");
    assert_eq!(s.losses, 1);
    assert_eq!(standing(&d, MT, S1, C), (0, Stat::default()));
}

// ---- the board --------------------------------------------------------------

/// 120 pilots, the pilot `990_300 + i` having `1000 - i` wins: rank i + 1.
fn crowd() -> Deltas {
    let mut d = Deltas::default();
    for i in 0..120 {
        put_stat(&mut d, MT, S1, 990_300 + i, st(1000 - i, 0, 1));
    }
    d
}

#[test]
fn the_board_is_the_top_hundred_and_a_caller_below_them_last() {
    let d = crowd();
    let caller = 990_300 + 109;
    let b = board(&d, MT, S1, Some(caller));
    assert_eq!(b.len(), BOARD_TOP + 1);
    assert_eq!(b[0].rank, 1);
    assert_eq!(b[BOARD_TOP - 1].rank, BOARD_TOP as i64);
    let last = b.last().unwrap();
    assert_eq!(last.character_id, caller);
    assert_eq!(last.rank, 110);
    assert_eq!(last.wins, 1000 - 109);
}

#[test]
fn a_caller_in_the_top_hundred_is_not_listed_twice() {
    let d = crowd();
    let caller = 990_300 + 49;
    let b = board(&d, MT, S1, Some(caller));
    assert_eq!(b.len(), BOARD_TOP);
    assert_eq!(b.iter().filter(|e| e.character_id == caller).count(), 1);
    // A stranger and no caller get the top hundred alone.
    assert_eq!(board(&d, MT, S1, Some(C)).len(), BOARD_TOP);
    assert_eq!(board(&d, MT, S1, None).len(), BOARD_TOP);
}

#[test]
fn a_board_entry_has_exactly_the_three_keys_the_client_reads() {
    let e = BoardEntry {
        character_id: A,
        rank: 4,
        wins: 9,
    }
    .to_value();
    let PyValue::Dict(pairs) = &e else {
        panic!("a dict, got {e:?}");
    };
    assert_eq!(pairs.len(), 3);
    assert_eq!(dict_get(&e, b"character_id"), Some(&PyValue::Int(A)));
    assert_eq!(dict_get(&e, b"rank"), Some(&PyValue::Int(4)));
    assert_eq!(dict_get(&e, b"wins"), Some(&PyValue::Int(9)));
}

#[test]
fn the_pushes_carry_the_real_numbers_and_name_their_audience() {
    use crate::notify::Push;
    let mut d = Deltas::default();
    record(&mut d, &finished(S1, &[A], &[B]), 100);
    let p = statistics_push(&d, MT, S1, Some(B), Some(B));
    assert_eq!(p.audience(), Some(vec![B]));
    let (name, args) = p.to_push();
    assert_eq!(name, "OnPVPFilamentsCharacterStatistics");
    assert_eq!(dict_get(&args[0], b"matchTypeID"), Some(&PyValue::Int(MT)));
    assert_eq!(dict_get(&args[0], b"scheduleID"), Some(&PyValue::Int(S1)));
    let s = dict_get(&args[0], b"statistics").expect("a statistics dict");
    for (k, v) in [("rank", 2), ("wins", 0), ("losses", 1), ("draws", 0)] {
        assert_eq!(dict_get(s, k.as_bytes()), Some(&PyValue::Int(v)), "{k}");
    }
    // Nobody played: never none, always the zero dict, for the acting session.
    let p = statistics_push(&Deltas::default(), MT, S1, Some(A), None);
    assert_eq!(p.audience(), None);
    let (_, args) = p.to_push();
    let s = dict_get(&args[0], b"statistics").expect("a statistics dict");
    for k in ["rank", "wins", "losses", "draws"] {
        assert_eq!(dict_get(s, k.as_bytes()), Some(&PyValue::Int(0)), "{k}");
    }
    // The board push lists the entries.
    let lb = leaderboard_push(&d, MT, S1, Some(A), Some(A));
    let (name, args) = lb.to_push();
    assert_eq!(name, "OnPVPFilamentsLeaderboard");
    let Some(PyValue::List(entries)) = dict_get(&args[0], b"entries") else {
        panic!("a list of entries");
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(
        dict_get(&entries[0], b"character_id"),
        Some(&PyValue::Int(A))
    );
}

#[test]
fn the_rpc_push_goes_to_the_acting_session_with_the_callers_numbers() {
    let mut reg = world();
    record(reg.deltas_mut(), &finished(S1, &[A], &[B]), 100);
    let now = ft(2000, 1, 1, 12);
    assert!(crate::proving::push_statistics(
        &mut reg,
        now,
        MT,
        S1,
        Some(A)
    ));
    assert!(crate::proving::push_leaderboard(
        &mut reg,
        now,
        MT,
        S1,
        Some(A)
    ));
    let sent: Vec<_> = reg
        .take_pending_raw()
        .into_iter()
        .map(|(_, p)| (p.to_push(), p.audience()))
        .collect();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|(_, aud)| aud.is_none()));
    let stats = dict_get(&sent[0].0.1[0], b"statistics").unwrap();
    assert_eq!(dict_get(stats, b"wins"), Some(&PyValue::Int(1)));
    assert_eq!(dict_get(stats, b"rank"), Some(&PyValue::Int(1)));
}

// ---- a finished match ----------------------------------------------------------

#[test]
fn a_finished_match_pushes_each_participant_their_statistics_then_the_board() {
    let mut reg = world();
    on_finished(&mut reg, &finished(S1, &[A], &[B]));
    assert_eq!(stat(reg.deltas(), MT, S1, A).wins, 1);
    assert_eq!(stat(reg.deltas(), MT, S1, B).losses, 1);
    let sent = due(&mut reg);
    assert_eq!(sent.len(), 4, "two pilots, two pushes each");
    for who in [A, B] {
        let mine: Vec<&str> = sent
            .iter()
            .filter(|(_, aud, _)| aud.as_deref() == Some(&[who][..]))
            .map(|(name, _, _)| *name)
            .collect();
        assert_eq!(
            mine,
            [
                "OnPVPFilamentsCharacterStatistics",
                "OnPVPFilamentsLeaderboard"
            ],
            "pilot {who}"
        );
    }
    let winner = sent
        .iter()
        .find(|(n, aud, _)| *n == "OnPVPFilamentsCharacterStatistics" && aud == &Some(vec![A]))
        .unwrap();
    let s = dict_get(&winner.2[0], b"statistics").unwrap();
    assert_eq!(dict_get(s, b"wins"), Some(&PyValue::Int(1)));
}

#[test]
fn a_character_that_is_gone_is_neither_counted_nor_a_panic() {
    let mut reg = world();
    on_finished(&mut reg, &finished(S1, &[A], &[GONE]));
    assert_eq!(stat(reg.deltas(), MT, S1, A).wins, 1);
    assert_eq!(stat(reg.deltas(), MT, S1, GONE), Stat::default());
    let sent = due(&mut reg);
    assert!(sent.iter().all(|(_, aud, _)| aud == &Some(vec![A])));
    // A match of nobody is nothing.
    on_finished(&mut reg, &finished(S1, &[GONE], &[GONE + 1]));
    assert!(due(&mut reg).is_empty());
}

// ---- the rows ----------------------------------------------------------------

#[test]
fn the_statistics_survive_a_save_and_a_load() {
    let mut reg = world();
    on_finished(&mut reg, &finished(S1, &[A], &[B]));
    let rows: Vec<_> = reg
        .deltas()
        .kinds()
        .map(|(k, r)| (k.to_owned(), r.clone()))
        .collect();
    let mut back = Deltas::default();
    for (k, r) in rows {
        for (key, v) in r {
            back.put(&k, key, v);
        }
    }
    assert_eq!(ranked(&back, MT, S1), ranked(reg.deltas(), MT, S1));
    assert_eq!(history(&back, A, 5), history(reg.deltas(), A, 5));
    assert_eq!(stat(&back, MT, S1, A).wins, 1);
}

#[test]
fn the_history_is_newest_first_and_capped() {
    let mut d = Deltas::default();
    for at in 1..=5 {
        record(&mut d, &finished(S1, &[A], &[B]), at);
    }
    let h = history(&d, A, 10);
    assert_eq!(h.iter().map(|p| p.at).collect::<Vec<_>>(), [5, 4, 3, 2, 1]);
    assert!(h.iter().all(|p| p.result == MatchResult::Win as i64));
    assert_eq!(history(&d, B, 1)[0].result, MatchResult::Loss as i64);
    assert!(history(&d, C, 10).is_empty());
    assert_eq!(prune_history(&mut d, 3), 2);
    let h = history(&d, A, 10);
    assert_eq!(h.iter().map(|p| p.at).collect::<Vec<_>>(), [5, 4, 3]);
    // Pruning never touches the tallies.
    assert_eq!(stat(&d, MT, S1, A).wins, 5);
    assert_eq!(prune_history(&mut d, 3), 0);
}

// ---- the rewards -------------------------------------------------------------

/// A wins against B in S1.
fn played_in(mut reg: Registry) -> Registry {
    record(reg.deltas_mut(), &finished(S1, &[A], &[B]), 100);
    reg
}

fn played() -> Registry {
    played_in(world())
}

#[test]
fn an_award_pays_the_winner_every_kind_of_reward_once() {
    let mut reg = played();
    let ev = event(&reg, S1);
    let base = reg.character(A).unwrap().balance;
    assert_eq!(award(&mut reg, &ev, 500, Via::Clock), 1);

    // An item: a redeem token of the amount, in the winner's account.
    let tokens = user_tokens(&reg, A);
    assert_eq!(
        tokens.len(),
        1,
        "the SP type and the unknown type are skipped"
    );
    assert_eq!(
        (tokens[0].type_id, tokens[0].qty, tokens[0].to),
        (ITEM, ITEM_QTY, A)
    );
    // ISK at the configured dial, untaxed, with the redeemed-ISK-token ref.
    let want = crate::econ_scale::scaled_isk(ISK_AMOUNT as f64, DIAL);
    assert_eq!(reg.character(A).unwrap().balance, base + want);
    let row = reg.journal().entries().last().expect("a journal row");
    assert_eq!(row.entry_type_id, REF_REDEEMED_ISK_TOKEN);
    assert_eq!(row.owner_id_2, A);
    assert_eq!(row.amount, want);
    assert_eq!(row.reference_id, S1);
    // Skill points: the token's points times the amount, in the free pool.
    assert_eq!(crate::free_sp::of(&reg, A), SP_PER_TOKEN * SP_TOKENS_GIVEN);
    // The redeem window is told, and the pilot is marked paid.
    let sent = due(&mut reg);
    let queue = sent
        .iter()
        .find(|(n, _, _)| *n == "OnRedeemingQueueUpdated")
        .expect("the redeem queue push");
    assert_eq!(queue.1, Some(vec![A]));
    assert!(stat(reg.deltas(), MT, S1, A).awarded);
    assert!(awarded(reg.deltas(), MT, S1));
}

#[test]
fn a_pilot_with_no_win_gets_nothing_and_a_second_award_is_a_no_op() {
    let mut reg = played();
    let ev = event(&reg, S1);
    let loser_before = reg.character(B).unwrap().balance;
    assert_eq!(award(&mut reg, &ev, 500, Via::Clock), 1);
    assert!(user_tokens(&reg, B).is_empty());
    assert_eq!(reg.character(B).unwrap().balance, loser_before);
    assert_eq!(crate::free_sp::of(&reg, B), 0);
    assert!(!stat(reg.deltas(), MT, S1, B).awarded);

    let balance = reg.character(A).unwrap().balance;
    assert_eq!(award(&mut reg, &ev, 600, Via::Clock), 0);
    assert_eq!(user_tokens(&reg, A).len(), 1);
    assert_eq!(reg.character(A).unwrap().balance, balance);
    assert_eq!(crate::free_sp::of(&reg, A), SP_PER_TOKEN * SP_TOKENS_GIVEN);
}

#[test]
fn a_pilot_who_wins_after_the_first_award_is_paid_at_the_next() {
    let mut reg = played();
    let ev = event(&reg, S1);
    assert_eq!(award(&mut reg, &ev, 500, Via::Clock), 1);
    record(reg.deltas_mut(), &finished(S1, &[B], &[A]), 700);
    assert_eq!(award(&mut reg, &ev, 800, Via::Clock), 1);
    assert_eq!(user_tokens(&reg, B).len(), 1);
    assert_eq!(user_tokens(&reg, A).len(), 1, "A is not paid twice");
}

#[test]
fn an_sp_token_without_points_and_an_unknown_type_are_skipped_not_queued() {
    // PG-3b is not built: a world without the table has no points for the
    // bundle's SP token types; they are skipped, never made items.
    let mut reg = played_in(world_with(false));
    let ev = event(&reg, S1);
    assert!(
        ev.match_type
            .rewards
            .iter()
            .any(|r| r.0 == SP_TOKEN_TYPES[0])
    );
    assert_eq!(award(&mut reg, &ev, 500, Via::Clock), 1);
    assert_eq!(crate::free_sp::of(&reg, A), 0);
    let types: Vec<i64> = user_tokens(&reg, A).iter().map(|t| t.type_id).collect();
    assert_eq!(types, [ITEM], "only the item the world can make");
}

#[test]
fn a_late_winner_is_paid_by_the_next_rollover_after_an_award() {
    let mut reg = world();
    record(reg.deltas_mut(), &finished(S1, &[A], &[B]), 100);
    let day = Day {
        day: 1,
        days: 1,
        now: later(),
    };
    assert_eq!(on_rollover(&mut reg, &day), 1);
    assert!(awarded(reg.deltas(), MT, S1));
    record(reg.deltas_mut(), &finished(S1, &[B], &[A]), 200);
    assert_eq!(on_rollover(&mut reg, &day), 1, "the marker is not a gate");
    assert_eq!(user_tokens(&reg, B).len(), 1);
    assert_eq!(user_tokens(&reg, A).len(), 1, "A is not paid twice");
}

#[test]
fn pg_award_refuses_an_event_that_is_neither_qa_nor_over() {
    let mut reg = world();
    record(reg.deltas_mut(), &finished(S2, &[A], &[B]), 100);
    let s = text(slash(&mut reg, Some(A), &["award", &S2.to_string()]));
    assert!(s.contains("not over"), "{s}");
    assert!(user_tokens(&reg, A).is_empty());
    assert!(!stat(reg.deltas(), MT, S2, A).awarded);
}

#[test]
fn the_rollover_pays_each_event_that_is_over_once_and_never_a_qa_event() {
    let mut reg = world();
    for s in [S1, S2, S_QA] {
        record(reg.deltas_mut(), &finished(s, &[A], &[B]), 100);
    }
    let day = Day {
        day: 1,
        days: 1,
        now: later(),
    };
    assert_eq!(on_rollover(&mut reg, &day), 1, "only S1 is over");
    assert!(stat(reg.deltas(), MT, S1, A).awarded);
    assert!(!stat(reg.deltas(), MT, S2, A).awarded, "not over yet");
    assert!(!stat(reg.deltas(), MT, S_QA, A).awarded, "QA never ends");
    assert!(awarded(reg.deltas(), MT, S1));
    assert!(!awarded(reg.deltas(), MT, S2));
    assert_eq!(user_tokens(&reg, A).len(), 1);
    // A second rollover pays nothing more.
    assert_eq!(on_rollover(&mut reg, &day), 0);
    assert_eq!(user_tokens(&reg, A).len(), 1);
}

#[test]
fn the_rollover_prunes_the_history() {
    let mut reg = world();
    for at in 0..(HISTORY_KEEP as i64 + 3) {
        record(reg.deltas_mut(), &finished(S2, &[A], &[B]), at + 1);
    }
    let day = Day {
        day: 1,
        days: 1,
        now: later(),
    };
    on_rollover(&mut reg, &day);
    assert_eq!(history(reg.deltas(), A, usize::MAX).len(), HISTORY_KEEP);
}

// ---- /pg ----------------------------------------------------------------------

#[test]
fn pg_stats_board_history_and_award_answer() {
    let mut reg = played();
    let s = text(slash(&mut reg, Some(A), &["stats"]));
    assert!(s.contains("rank 1") && s.contains("1 win"), "{s}");
    let s = text(slash(&mut reg, Some(A), &["stats", &B.to_string()]));
    assert!(s.contains("rank 2") && s.contains("1 loss"), "{s}");
    let s = text(slash(&mut reg, Some(A), &["board"]));
    assert!(s.contains(&format!("1. {A} wins 1")), "{s}");
    let s = text(slash(&mut reg, Some(A), &["history"]));
    assert!(s.contains(&format!("{MT}_{S1} win")), "{s}");
    let s = text(slash(&mut reg, Some(C), &["stats"]));
    assert!(s.contains("no match"), "{s}");

    // `award` with no id uses the last match event; a given id pays that one.
    let s = text(slash(&mut reg, Some(A), &["award"]));
    assert!(s.contains("awarded 1 pilot"), "{s}");
    assert_eq!(user_tokens(&reg, A).len(), 1);
    let s = text(slash(&mut reg, Some(A), &["award", &S1.to_string()]));
    assert!(s.contains("awarded 0 pilot"), "{s}");
    record(reg.deltas_mut(), &finished(S_QA, &[B], &[A]), 900);
    let s = text(slash(&mut reg, Some(B), &["award", &S_QA.to_string()]));
    assert!(
        s.contains("awarded 1 pilot"),
        "a QA event is paid by hand: {s}"
    );
    let s = text(slash(&mut reg, Some(A), &["award", "990999"]));
    assert!(s.contains("no such event"), "{s}");
}

#[test]
fn pg_usage_on_bad_arguments() {
    let mut reg = played();
    for args in [
        &["stats", "abc"][..],
        &["award", "abc"],
        &["board", "extra"],
        &["bogus"],
        &[],
    ] {
        let s = text(slash(&mut reg, Some(A), args));
        assert!(s.starts_with("usage"), "{args:?}: {s}");
    }
    assert!(text(slash(&mut reg, None, &["stats"])).contains("no character"));
}

#[test]
fn the_arena_door_forwards_the_results_commands_outside_an_arena() {
    let mut reg = played();
    let s = text(crate::proving::arena::slash(&mut reg, Some(A), &["stats"]));
    assert!(s.contains("rank 1"), "{s}");
    // Its own commands still need an arena.
    let s = text(crate::proving::arena::slash(&mut reg, Some(A), &["status"]));
    assert!(s.contains("not in an arena"), "{s}");
}

// ---- the registration rows ---------------------------------------------------------

#[test]
fn every_registration_row_exists() {
    assert!(crate::delta_kinds::KINDS.contains(&("pgstat", "proving::results")));
    assert!(crate::delta_kinds::is_registered(KIND));
    assert!(
        crate::rollover_steps::STEPS
            .iter()
            .any(|s| s.name == "proving_awards")
    );
    assert!(
        crate::proving::arena::ON_FINISHED
            .iter()
            .any(|f| *f as *const () == on_finished as *const ())
    );
    assert_eq!(
        crate::econ_ledger::flow(REF_REDEEMED_ISK_TOKEN),
        Some(crate::econ_ledger::Flow::System)
    );
    let pg = crate::slash::COMMANDS
        .iter()
        .find(|c| c.name == "pg")
        .expect("the /pg command");
    for w in ["stats", "board", "history", "award"] {
        assert!(pg.usage.contains(w), "{w}");
    }
}
