//! Tests of the extras' seam: the codec, the address override, the kit rungs.
//! Ids are invented (`990_221_xxx`); no [A] constant appears as an expected
//! value.

use super::gw::*;
use super::kit::*;
use eve_marshal::PyValue;
use eve_server::industry::{Blueprint, Table};
use std::collections::BTreeSet;
use std::net::SocketAddr;

const SKYHOOK_BP: i64 = 990_221_011;
const SKYHOOK: i64 = 990_221_001;
const FRIGATE_BP: i64 = 990_221_012;
const FRIGATE: i64 = 990_221_002;
const GROUP_FRIGATE: i64 = 990_221_900;

fn bp(blueprint: i64, product: i64) -> Blueprint {
    Blueprint {
        blueprint_type_id: blueprint,
        product_type_id: product,
        product_quantity: 1,
        time: 60,
        max_production_limit: 10,
        materials: Vec::new(),
        skills: Vec::new(),
    }
}

fn recipes() -> Table {
    Table {
        blueprints: vec![bp(SKYHOOK_BP, SKYHOOK), bp(FRIGATE_BP, FRIGATE)],
        ..Table::default()
    }
}

fn group_of(t: i64) -> Option<i64> {
    match t {
        SKYHOOK => Some(eve_server::skyhook::GROUP_SKYHOOK),
        FRIGATE => Some(GROUP_FRIGATE),
        _ => None,
    }
}

#[test]
fn codec_round_trips_ids_uints_and_nested_messages() {
    let mut inner = Vec::new();
    put_uint(&mut inner, 1, 990_221_021);
    put_uint(&mut inner, 2, 300);
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, &inner);
    put_bytes(&mut msg, 3, &id(990_221_022));
    put_bytes(&mut msg, 3, &id(990_221_023));
    put_uint(&mut msg, 4, 7);
    let first = bytes(&msg, 1).expect("nested");
    assert_eq!(uint(first, 1), Some(990_221_021));
    assert_eq!(uint(first, 2), Some(300));
    assert_eq!(uint(&msg, 4), Some(7));
    let ids: Vec<_> = fields(&msg, 3).iter().filter_map(|b| uint(b, 1)).collect();
    assert_eq!(ids, vec![990_221_022, 990_221_023]);
    assert_eq!(read_id(&msg, 3), Some(990_221_022));
    // An elided zero id reads None; a missing field too.
    let mut zero = Vec::new();
    put_bytes(&mut zero, 1, &id(0));
    assert_eq!(read_id(&zero, 1), None);
    assert_eq!(read_id(&zero, 2), None);
}

#[test]
fn a_truncated_message_decodes_to_none_and_never_panics() {
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, &id(990_221_031));
    put_uint(&mut msg, 2, 1 << 40);
    put_bytes(&mut msg, 3, b"abcdef");
    for n in 0..msg.len() {
        let p = &msg[..n];
        let _ = (fields(p, 1), uint(p, 2), bytes(p, 3), read_id(p, 1));
    }
    // A cut inside the length-delimited tail and inside the varint.
    assert_eq!(bytes(&msg[..msg.len() - 1], 3), None);
    let mut big = Vec::new();
    put_uint(&mut big, 2, 1 << 40);
    assert_eq!(uint(&big, 2), Some(1 << 40));
    assert_eq!(uint(&big[..big.len() - 1], 2), None);
    // Garbage wire types and an endless varint.
    assert_eq!(uint(&[0x0b], 1), None);
    assert_eq!(
        uint(
            &[
                0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff
            ],
            1
        ),
        None
    );
}

#[test]
fn the_override_is_found_by_the_game_address_and_only_by_it() {
    let a: SocketAddr = "192.0.2.1:1".parse().unwrap();
    let b: SocketAddr = "192.0.2.2:1".parse().unwrap();
    let gateway: SocketAddr = "192.0.2.3:1".parse().unwrap();
    set_gateway_for(a, gateway);
    assert_eq!(gateway_for(a), gateway);
    assert_eq!(
        gateway_for(b),
        SocketAddr::new(b.ip(), eve_server::gateway::PORT)
    );
    clear_gateway_for(a);
}

#[test]
fn the_kit_is_a_rung_only_while_wanted() {
    let none = BTreeSet::new();
    assert!(kit_rungs(&recipes(), &group_of, &|_| true, &none).is_empty());
    let want: BTreeSet<i64> = [SKYHOOK].into();
    assert_eq!(
        kit_rungs(&recipes(), &group_of, &|_| true, &want),
        vec![vec![SKYHOOK_BP]]
    );
}

#[test]
fn a_kit_product_the_store_cannot_create_is_no_rung() {
    let want: BTreeSet<i64> = [SKYHOOK].into();
    assert!(kit_rungs(&recipes(), &group_of, &|_| false, &want).is_empty());
}

#[test]
fn a_non_kit_product_is_never_a_kit_rung() {
    let want: BTreeSet<i64> = [FRIGATE].into();
    assert!(kit_rungs(&recipes(), &group_of, &|_| true, &want).is_empty());
    assert!(!is_kit(GROUP_FRIGATE));
    assert!(is_kit(eve_server::merc_den::GROUP_MERCENARY_DEN));
    assert!(is_kit(eve_server::sov_upgrade::UPGRADE_GROUPS[0]));
}

#[test]
fn the_want_board_forgets_a_want_after_its_ttl() {
    let product = 990_221_003;
    want(product);
    let now = crate::clock::instant();
    assert!(wanted(now).contains(&product));
    assert!(!wanted(now + WANT_TTL + std::time::Duration::from_secs(1)).contains(&product));
}

#[test]
fn a_launch_answer_reads_as_an_id_or_a_refusal_label() {
    let item = 990_221_041;
    let ok = PyValue::Dict(vec![(
        PyValue::List(vec![PyValue::Int(990_221_042)]),
        PyValue::Int(item),
    )]);
    let refused = PyValue::Dict(vec![(
        PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Bytes(b"SomeLabel".to_vec()),
            PyValue::Tuple(vec![]),
        ])]),
        PyValue::Int(item),
    )]);
    // The dict's wire order is (value, key), as `Conn::bound_call_kw` documents.
    assert_eq!(launch_result(&ok, item), Ok(990_221_042));
    assert_eq!(launch_result(&refused, item), Err("SomeLabel".to_owned()));
    assert!(launch_result(&PyValue::None, item).is_err());
}

#[test]
fn the_extras_send_no_slash_line() {
    let slash = ["Slash", "Cmd"].concat();
    let qa = ["public", "QaToolsServer"].concat();
    for (name, src) in [
        ("extras.rs", include_str!("../extras.rs")),
        ("gw.rs", include_str!("gw.rs")),
        ("kit.rs", include_str!("kit.rs")),
    ] {
        assert!(!src.contains(&slash), "{name} names a slash command");
        assert!(!src.contains(&qa), "{name} names the QA tools service");
    }
}
