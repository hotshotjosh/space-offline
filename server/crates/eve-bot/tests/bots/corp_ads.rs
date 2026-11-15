//! Corp adverts for bots over a real socket (#152): a bot CEO's `tend` posts
//! one recruitment advert while the corp has room, a corpless bot's `find`
//! sees it in the corp finder, and `tend` takes it down once the hall says
//! the corp is full. Names and ids are invented. The same limitation as every
//! test here: both ends marshal with `eve-marshal`.

use super::common::{config, gm, login, pilot};
use eve_bot::Conn;
use eve_bot::career::Band;
use eve_bot::corp_ads;
use eve_bot::corpmind::{CorpMind, Hall, Member};
use eve_bot::mind::{TRAIT_COUNT, Traits};
use eve_bot::society::{self, Offer, Profile, Satisfaction};
use eve_marshal::PyValue;
use eve_server::server::{Config, Server};
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

const ANN: i64 = 90_000_901;
const BEN: i64 = 90_000_902;
const FILLER: i64 = 90_000_910;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn serve_world(config: Config) -> (SocketAddr, Arc<RwLock<Registry>>) {
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

fn found_corp(c: &mut Conn, name: &str, ticker: &str) -> i64 {
    gm(c, "/giveisk 2000000000");
    let corp = c
        .session()
        .get_i64("corpid")
        .expect("a corp in the session");
    let a = c
        .moniker_call(
            "corpRegistry",
            PyValue::Int(corp),
            "AddCorporation",
            vec![uni(name), uni(ticker), uni(""), uni("")],
            vec![],
        )
        .expect("AddCorporation");
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    a.outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"))
        .expect("the founding moves the session to the new corp")
}

fn member(name: &str, corp: i64) -> Member {
    Member {
        name: name.to_owned(),
        traits: Traits([0.5; TRAIT_COUNT]),
        career: "miner".to_owned(),
        band: Band::High,
        corp,
        alliance: None,
        last_turn: Some(0),
        satisfaction: Satisfaction::default(),
        applied: None,
        salt: 0,
    }
}

fn adverts(c: &mut Conn) -> usize {
    let o = c
        .call("corpRecProxy", "GetRecruitmentAdsForCorporation", vec![])
        .expect("GetRecruitmentAdsForCorporation");
    assert_eq!(o.refusal(), None);
    o.value().iter().map(count_rows).sum()
}

/// The `DbRow`s anywhere in `v`.
fn count_rows(v: &PyValue) -> usize {
    match v {
        PyValue::DbRow(_) => 1,
        PyValue::ObjectEx { dict, list, .. } => dict
            .iter()
            .flat_map(|(a, b)| [a, b])
            .chain(list.iter())
            .map(count_rows)
            .sum(),
        PyValue::Dict(entries) => entries
            .iter()
            .flat_map(|(a, b)| [a, b])
            .map(count_rows)
            .sum(),
        PyValue::List(items) | PyValue::Tuple(items) => items.iter().map(count_rows).sum(),
        _ => 0,
    }
}

#[test]
fn a_bot_ceo_posts_an_advert_a_seeker_finds_it_and_a_full_corp_takes_it_down() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ANN, "Ann", 100_000_901),
        pilot(BEN, "Ben", 100_000_902),
    ];
    let (addr, _world) = serve_world(config);
    let mut ann = login(addr, "Ann");
    let mut ben = login(addr, "Ben");
    let corp = found_corp(&mut ann, "Ads Corp Invented", "ADSC");

    let hall = Hall::new(2).with_every(0);
    {
        let mut st = hall.lock();
        st.corps.insert(
            corp,
            CorpMind {
                id: corp,
                ceo: Some(ANN),
                profile: Profile::of("miner", Band::High),
                founded: 0,
                alliance: None,
                name: "Ads Corp Invented".to_owned(),
                applied_alliance: None,
                salt: 0,
            },
        );
        st.members.insert(ANN, member("Ann", corp));
    }
    let hall = Arc::new(hall);

    // 1. Room and no advert: one is posted, and the server lists it.
    let lines = corp_ads::tend(&mut ann, &hall, ANN, corp).expect("tend");
    assert_eq!(lines, vec![format!("advert posted corp={corp}")]);
    assert_eq!(adverts(&mut ann), 1);
    // A second pass keeps the one advert.
    let lines = corp_ads::tend(&mut ann, &hall, ANN, corp).expect("tend");
    assert!(lines.is_empty(), "{lines:?}");

    // 2. The corpless seeker finds the corp through the finder.
    let offers = [Offer {
        corp,
        fit: 1.0,
        grudge: 0.0,
        size: 1,
        enemy: false,
    }];
    assert_eq!(corp_ads::find(&mut ben, &offers).expect("find"), Some(corp));

    // 3. The hall fills the corp: the advert comes down.
    {
        let mut st = hall.lock();
        for i in 0..society::CORP_SIZE_CAP as i64 {
            st.members
                .insert(FILLER + i, member(&format!("Filler {i}"), corp));
        }
        assert!(st.size(corp) >= society::CORP_SIZE_CAP);
    }
    let lines = corp_ads::tend(&mut ann, &hall, ANN, corp).expect("tend");
    assert_eq!(lines, vec![format!("advert removed corp={corp}")]);
    assert_eq!(adverts(&mut ann), 0);
}
