//! HN-3 (#203) over a real socket: a bot holding surplus loot and the
//! HyperCores for it offers it; a second bot with spare ISK buys tickets in
//! it, the offer sells out, the seller is paid the sale less the fee, and the
//! winner claims the prize. Nothing is granted by a GM during a turn: the
//! setup (`/createitem`) is the test's own stand-in for drops and a market.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only**: an invented prize type and group, an invented
//! reference price. No `[A]` value is an expected value: the prize is priced
//! and the wallets sized from the module's own constants, and the assertions
//! are about kinds and order.

use super::common::{config, gm, login, pilot, serve};
use eve_bot::Conn;
use eve_bot::builder::SellLine;
use eve_bot::nes_use::hypernet::{self, State};
use eve_server::hypernet as hn;
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

const SELLER: i64 = 90_203_001;
const BUYER: i64 = 90_203_002;
const RICH: i64 = 90_203_003;
const SUPPLY: i64 = 90_203_004;
const POOR: i64 = 90_203_005;
const PRIZE: i64 = 990_203_001;
const DUD: i64 = 990_203_002;
/// An invented group: no fit plan wants it.
const GROUP: i64 = 990_203;

/// The prize's reference price: twice the module's floor.
fn prize_isk() -> f64 {
    hypernet::MIN_ITEM_ISK * 2.0
}

fn content() -> Arc<eve_server::content::Content> {
    let row = |id: i64, group: i64, category: i64, name: &str| {
        format!("type\t{id}\t{group}\t{category}\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t\t{name}\n")
    };
    let rows = format!(
        "{}{}{}{}",
        eve_server::typedata::FIXTURE,
        row(PRIZE, GROUP, 7, "Fixture Prize"),
        row(DUD, GROUP, 7, "Fixture Dud"),
        row(hn::TOKEN_TYPE_ID, GROUP + 1, 17, "Fixture Core"),
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    let mut prices = eve_server::econ_prices::Table::fixture();
    let p = prize_isk();
    let core = hypernet::CORE_MAX_ISK / 2.0;
    prices.prices.extend(
        eve_server::econ_prices::Table::parse(&[
            format!("{PRIZE}\t{p}\tbase\t{p}\t7\t{GROUP}\t\t1"),
            format!("{DUD}\t1000.0\tbase\t1000.0\t7\t{GROUP}\t\t1"),
            format!(
                "{}\t{core}\tbase\t{core}\t17\t{}\t\t1",
                hn::TOKEN_TYPE_ID,
                GROUP + 1
            ),
        ])
        .expect("parses")
        .prices,
    );
    content.insert("econ_prices", prices);
    Arc::new(content)
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        eve_server::model::Character {
            balance: 1_000.0,
            ..pilot(SELLER, "Hnseller", 100_203_001)
        },
        eve_server::model::Character {
            balance: hypernet::RESERVE * 5.0,
            ..pilot(BUYER, "Hnbuyer", 100_203_002)
        },
        eve_server::model::Character {
            balance: hypernet::RESERVE * 2.0,
            ..pilot(RICH, "Hnrich", 100_203_003)
        },
        eve_server::model::Character {
            balance: hypernet::RESERVE,
            ..pilot(SUPPLY, "Hnsupply", 100_203_004)
        },
        eve_server::model::Character {
            balance: hypernet::RESERVE / 2.0,
            ..pilot(POOR, "Hnpoor", 100_203_005)
        },
    ];
    config.content = Some(content());
    serve(config)
}

fn units(c: &mut Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar lists")
        .iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.quantity)
        .sum()
}

fn turn(c: &mut Conn, st: &mut State) -> String {
    hypernet::turn(c, st, &BTreeSet::new()).expect("a turn is a note, not a fault")
}

fn running(c: &mut Conn) -> Vec<hypernet::Seen> {
    let out = c.call("raffleProxy", "Grab", vec![]).expect("grab");
    hypernet::raffles_of(&out.value().expect("a list"))
}

#[test]
fn a_bot_offers_surplus_loot_another_buys_every_ticket_and_the_winner_claims() {
    let addr = start();
    let mut seller = login(addr, "Hnseller");
    // The setup stands for what drops and the market supply: loot, a dud
    // too cheap to offer, and the HyperCores.
    gm(&mut seller, &format!("/createitem {PRIZE} 1"));
    gm(&mut seller, &format!("/createitem {DUD} 5"));
    gm(
        &mut seller,
        &format!("/createitem {} 500", hn::TOKEN_TYPE_ID),
    );
    let cores_before = units(&mut seller, hn::TOKEN_TYPE_ID);
    let mut sst = State::default();
    let note = turn(&mut seller, &mut sst);
    assert!(note.contains("offered"), "{note}");
    assert_eq!(sst.offered, 1);
    assert_eq!(units(&mut seller, PRIZE), 0, "the prize is in escrow");
    assert_eq!(units(&mut seller, DUD), 5, "the cheap stack stays");
    assert!(
        units(&mut seller, hn::TOKEN_TYPE_ID) < cores_before,
        "the offer burned cores"
    );

    // A second pass makes no second offer of the same stack: nothing left.
    let again = turn(&mut seller, &mut sst);
    assert!(!again.contains("offered"), "{again}");

    // The buyer sees it and, over turns, takes every ticket.
    let mut buyer = login(addr, "Hnbuyer");
    let offer = running(&mut buyer)
        .into_iter()
        .find(|r| r.owner_id == SELLER)
        .expect("the offer is in the browse list");
    let total = offer.total_isk();
    let mut bst = State::default();
    let note = turn(&mut buyer, &mut bst);
    assert!(note.contains("tickets"), "{note}");
    assert!(bst.tickets > 0);
    let seller_wallet = seller.cash_balance().expect("a wallet");
    // Turns until the offer leaves the browse list, at most one per ticket.
    let mut gone = false;
    for _ in 0..offer.count {
        if running(&mut buyer)
            .iter()
            .all(|r| r.raffle_id != offer.raffle_id)
        {
            gone = true;
            break;
        }
        turn(&mut buyer, &mut bst);
    }
    assert!(
        gone || running(&mut buyer)
            .iter()
            .all(|r| r.raffle_id != offer.raffle_id),
        "a sold-out offer leaves the browse list"
    );
    assert_eq!(bst.tickets as i64, offer.count);
    // The seller was paid the total less the fee.
    let paid = seller.cash_balance().expect("a wallet") - seller_wallet;
    assert!(
        (paid - total * (1.0 - hn::TAX)).abs() < 1.0,
        "paid {paid} for {total}"
    );

    // The buyer's next turn claims the prize it won (the only holder).
    let note = turn(&mut buyer, &mut bst);
    assert!(note.contains("claimed"), "{note}");
    assert_eq!(bst.claimed, 1);
    assert_eq!(units(&mut buyer, PRIZE), 1, "the prize is in its hangar");
    // It holds no cores and does not conjure any: the won prize is surplus,
    // but the offer is refused for want of cores, not made.
    assert_eq!(bst.offered, 0);
    assert_eq!(units(&mut buyer, hn::TOKEN_TYPE_ID), 0);
}

#[test]
fn a_bot_below_the_reserve_buys_no_ticket_with_an_offer_in_view() {
    let addr = start();
    let mut seller = login(addr, "Hnseller");
    gm(&mut seller, &format!("/createitem {PRIZE} 1"));
    gm(
        &mut seller,
        &format!("/createitem {} 500", hn::TOKEN_TYPE_ID),
    );
    let mut sst = State::default();
    assert!(turn(&mut seller, &mut sst).contains("offered"));

    // A pilot under the reserve sees the offer and does not buy.
    let mut poor = login(addr, "Hnpoor");
    assert!(
        running(&mut poor).iter().any(|r| r.owner_id == SELLER),
        "the offer is in view"
    );
    let before = poor.cash_balance().expect("a wallet");
    let mut pst = State::default();
    let note = turn(&mut poor, &mut pst);
    assert_eq!(pst.tickets, 0, "{note}");
    assert_eq!(poor.cash_balance().expect("a wallet"), before);
}

#[test]
fn a_bot_with_loot_and_no_cores_says_so_and_offers_nothing() {
    let addr = start();
    let mut buyer = login(addr, "Hnbuyer");
    gm(&mut buyer, &format!("/createitem {PRIZE} 1"));
    let mut bst = State::default();
    let note = turn(&mut buyer, &mut bst);
    assert!(note.contains("HyperCores"), "{note}");
    assert_eq!(bst.offered, 0);
    assert_eq!(units(&mut buyer, PRIZE), 1, "the loot stays");
}

#[test]
fn a_bot_buys_the_cores_it_lacks_from_another_pilots_ask_and_then_offers() {
    let addr = start();
    // A third pilot posts the cores (the setup stands for a drop).
    let mut supply = login(addr, "Hnsupply");
    gm(
        &mut supply,
        &format!("/createitem {} 500", hn::TOKEN_TYPE_ID),
    );
    let station = supply.session().station_id().expect("docked");
    let stack = supply
        .hangar_items()
        .expect("the hangar")
        .into_iter()
        .find(|r| r.type_id == hn::TOKEN_TYPE_ID)
        .expect("the cores are there");
    supply
        .sell(
            &[SellLine {
                station_id: station,
                type_id: hn::TOKEN_TYPE_ID,
                item_id: stack.item_id,
                price: hypernet::CORE_MAX_ISK / 2.0,
                quantity: 500,
            }],
            eve_bot::builder::ORDER_DAYS,
            eve_bot::builder::BROKER_FEE,
        )
        .expect("the ask is placed");
    let supply_before = supply.cash_balance().expect("a wallet");

    // A pilot with loot and ISK above the reserve, and no cores.
    let mut rich = login(addr, "Hnrich");
    gm(&mut rich, &format!("/createitem {PRIZE} 1"));
    assert_eq!(units(&mut rich, hn::TOKEN_TYPE_ID), 0);
    let mut rst = State::default();
    let note = turn(&mut rich, &mut rst);
    assert!(note.contains("bought"), "{note}");
    assert!(note.contains("offered"), "{note}");
    assert_eq!(rst.offered, 1);
    assert_eq!(units(&mut rich, PRIZE), 0, "the prize is in escrow");
    assert!(
        supply.cash_balance().expect("a wallet") > supply_before,
        "the seller of the cores was paid"
    );
}
