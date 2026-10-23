//! Conservation: whatever the topology does — partial accepts, full sinks,
//! splits mid-transfer, merges, loops, breaking pipes with units inside,
//! save/load — `stores + pipes + spilled + lost` never changes.

mod common;

use common::*;
use yog_pipes_core::engine::{ModelSpec, NodeFace};
use yog_pipes_core::ids::{CapId, Face};
use yog_pipes_core::model::Rng;
use yog_pipes_core::registry::{
    Filter, FilterEntry, FilterMode, IndicatorSource, Policy, Side, SideMode,
};

const CABLE: &str = "test:cable";
const GEN: &str = "test:generator";
const BATTERY: &str = "test:battery";
const POWER: &str = "test:power";

fn energy(policy: Policy) -> (Sim, CapId) {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let c = cap(
        &mut s.e,
        POWER,
        "power",
        &[FE],
        ModelSpec::Buffered,
        policy,
        &[("fill", IndicatorSource::Fill)],
    );
    attach(
        &mut s.e,
        CABLE,
        POWER,
        &["cable"],
        &[("capacity", 100.0), ("throughput", 37.0)],
    );
    endpoint(&mut s.e, GEN, POWER, &[FE], false, true);
    endpoint(&mut s.e, BATTERY, POWER, &[FE], true, false);
    (s, c)
}

/// Random place/break/tick churn on a small grid, checking the ledger
/// after every step.
fn churn(s: &mut Sim, pipe: &str, seed: u64, steps: u32) {
    let total = s.ledger();
    let mut rng = Rng(seed);
    for _ in 0..steps {
        let x = (rng.next_u64() % 8) as i32;
        let z = (rng.next_u64() % 8) as i32;
        let pos = p(x, 0, z);
        match rng.next_u64() % 4 {
            0 if !s.w.blocks.contains_key(&pos) => s.place(pos, pipe),
            1 if s.w.blocks.get(&pos).is_some_and(|b| b == pipe) => s.break_block(pos),
            _ => s.tick(1 + (rng.next_u64() % 3) as u32),
        }
        assert_eq!(s.ledger(), total, "ledger changed (seed {seed})");
    }
}

#[test]
fn buffered_conserves_under_churn_and_partial_accepts() {
    for policy in [
        Policy::RoundRobin,
        Policy::Priority,
        Policy::EvenSplit,
        Policy::Random,
    ] {
        for seed in 1..6 {
            let (mut s, _) = energy(policy);
            let fe = s.res(FE, FE);
            s.store(p(-1, 0, 0), GEN, 100_000, &[(fe, 10_000)]);
            s.store(p(8, 0, 3), BATTERY, 333, &[]);
            s.store(p(3, 0, 8), BATTERY, 1_000_000, &[]);
            s.w.stores.get_mut(&p(8, 0, 3)).unwrap().accept_per_call = 7;
            churn(&mut s, CABLE, seed, 400);
        }
    }
}

#[test]
fn buffered_moves_and_splits_exactly() {
    let (mut s, c) = energy(Policy::RoundRobin);
    let fe = s.res(FE, FE);
    s.store(p(-1, 0, 0), GEN, 100_000, &[(fe, 1_000)]);
    for x in 0..10 {
        s.place(p(x, 0, 0), CABLE);
    }
    assert_eq!(s.e.layer(c).nets().count(), 1);
    s.tick(5);
    let held = s.in_pipes(c);
    assert!(held > 0 && held <= 1_000);
    // Split in the middle: the buffer is divided, the broken cable's share
    // is spilled; nothing appears or vanishes.
    let before = s.ledger();
    s.break_block(p(5, 0, 0));
    assert_eq!(s.e.layer(c).nets().count(), 2);
    assert_eq!(s.ledger(), before);
    assert!(s.w.spilled_total() > 0);
    // Merge back.
    s.place(p(5, 0, 0), CABLE);
    assert_eq!(s.e.layer(c).nets().count(), 1);
    assert_eq!(s.ledger(), before);
    // A consumer drains everything eventually.
    s.store(p(10, 0, 0), BATTERY, 1_000_000, &[]);
    s.tick(200);
    assert_eq!(s.ledger(), before);
    assert_eq!(s.w.stores[&p(-1, 0, 0)].total(), 0);
    assert_eq!(s.in_pipes(c), 0);
}

#[test]
fn loops_split_only_when_cut_twice() {
    let (mut s, c) = energy(Policy::RoundRobin);
    // A 4×4 ring.
    for i in 0..4 {
        s.place(p(i, 0, 0), CABLE);
        s.place(p(i, 0, 3), CABLE);
        s.place(p(0, 0, i), CABLE);
        s.place(p(3, 0, i), CABLE);
    }
    assert_eq!(s.e.layer(c).nets().count(), 1);
    s.break_block(p(1, 0, 0));
    assert_eq!(s.e.layer(c).nets().count(), 1);
    s.break_block(p(2, 0, 3));
    assert_eq!(s.e.layer(c).nets().count(), 2);
}

#[test]
fn side_off_cuts_links_and_ports() {
    let (mut s, c) = energy(Policy::RoundRobin);
    let fe = s.res(FE, FE);
    s.store(p(-1, 0, 0), GEN, 1000, &[(fe, 500)]);
    s.place(p(0, 0, 0), CABLE);
    s.place(p(1, 0, 0), CABLE);
    s.store(p(2, 0, 0), BATTERY, 1000, &[]);
    s.with_io(|e, io, probe| {
        let off = Side {
            mode: SideMode::Off,
            ..Default::default()
        };
        e.set_side(
            io,
            probe,
            NodeFace {
                cap: c,
                pos: p(0, 0, 0),
                face: Face::East,
            },
            off,
        );
    });
    assert_eq!(s.e.layer(c).nets().count(), 2);
    s.tick(50);
    assert_eq!(s.w.stores[&p(2, 0, 0)].total(), 0);
    s.with_io(|e, io, probe| {
        e.set_side(
            io,
            probe,
            NodeFace {
                cap: c,
                pos: p(0, 0, 0),
                face: Face::East,
            },
            Side::default(),
        )
    });
    assert_eq!(s.e.layer(c).nets().count(), 1);
    s.tick(50);
    assert_eq!(s.w.stores[&p(2, 0, 0)].total(), 500);
}

// ── routed ───────────────────────────────────────────────────────────────────

#[test]
fn routed_with_loss_conserves_and_reports_overload() {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let c = cap(
        &mut s.e,
        "test:eu",
        "eu",
        &[FE],
        ModelSpec::Routed,
        Policy::Nearest,
        &[],
    );
    attach(
        &mut s.e,
        CABLE,
        "test:eu",
        &[],
        &[("throughput", 64.0), ("loss", 200.0), ("max_packet", 32.0)],
    );
    endpoint(&mut s.e, GEN, "test:eu", &[FE], false, true);
    endpoint(&mut s.e, BATTERY, "test:eu", &[FE], true, false);
    let fe = s.res(FE, FE);
    s.store(p(-1, 0, 0), GEN, 100_000, &[(fe, 5_000)]);
    for x in 0..12 {
        s.place(p(x, 0, 0), CABLE);
    }
    s.store(p(12, 0, 0), BATTERY, 100_000, &[]);
    s.store(p(5, 0, 1), BATTERY, 50, &[]);
    let total = s.ledger();
    s.tick(200);
    assert_eq!(s.ledger(), total);
    assert!(s.w.lost_total() > 0, "distance loss is reported");
    assert!(!s.w.overloads.is_empty(), "packets above 32 overload");
    assert_eq!(s.in_pipes(c), 0, "routed stores nothing");
    assert_eq!(
        s.w.stores[&p(5, 0, 1)].total(),
        50,
        "nearest sink filled first"
    );
    churn(&mut s, CABLE, 7, 300);
}

// ── in_flight ────────────────────────────────────────────────────────────────

const TUBE: &str = "test:tube";
const CHEST: &str = "test:chest";
const ITEMS: &str = "test:items";

fn items() -> (Sim, CapId) {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let c = cap(
        &mut s.e,
        ITEMS,
        "items",
        &[ITEM],
        ModelSpec::InFlight,
        Policy::RoundRobin,
        &[("units", IndicatorSource::Units)],
    );
    attach(
        &mut s.e,
        TUBE,
        ITEMS,
        &[],
        &[("speed", 0.5), ("stack", 8.0), ("interval", 2.0)],
    );
    endpoint(&mut s.e, CHEST, ITEMS, &[ITEM], true, true);
    (s, c)
}

#[test]
fn in_flight_delivers_and_spills_units_in_broken_pipes() {
    let (mut s, c) = items();
    let stone = s.res(ITEM, "minecraft:stone");
    s.store(p(-1, 0, 0), CHEST, 1000, &[(stone, 200)]);
    for x in 0..20 {
        s.place(p(x, 0, 0), TUBE);
    }
    s.side(c, p(0, 0, 0), Face::West, SideMode::Out);
    s.store(p(20, 0, 0), CHEST, 1000, &[]);
    let total = s.ledger();
    s.tick(30);
    assert!(s.in_pipes(c) > 0, "units are in flight");
    assert_eq!(s.ledger(), total);
    // Break a tube with units inside (the `units` indicator shows them).
    let target = (0..20)
        .map(|x| p(x, 0, 0))
        .find(|&q| s.e.indication(q).iter().any(|i| i.1 > 0))
        .expect("a tube shows units");
    let spilled = s.w.spilled_total();
    s.break_block(target);
    assert_eq!(s.ledger(), total);
    assert!(
        s.w.spilled_total() > spilled,
        "units in the broken tube are spilled"
    );
    s.place(target, TUBE);
    s.tick(400);
    assert_eq!(s.ledger(), total);
    assert_eq!(s.in_pipes(c), 0);
    let delivered = s.w.stores[&p(20, 0, 0)].total();
    assert_eq!(
        delivered + s.w.stores[&p(-1, 0, 0)].total() + s.w.spilled_total(),
        200
    );
}

#[test]
fn in_flight_full_sinks_return_or_wait() {
    let (mut s, c) = items();
    let stone = s.res(ITEM, "minecraft:stone");
    s.store(p(-1, 0, 0), CHEST, 1000, &[(stone, 64)]);
    for x in 0..6 {
        s.place(p(x, 0, 0), TUBE);
    }
    s.side(c, p(0, 0, 0), Face::West, SideMode::Out);
    s.store(p(6, 0, 0), CHEST, 10, &[]);
    let total = s.ledger();
    for _ in 0..50 {
        s.tick(5);
        assert_eq!(s.ledger(), total);
    }
    assert_eq!(s.w.stores[&p(6, 0, 0)].total(), 10);
    churn(&mut s, TUBE, 11, 300);
    let _ = c;
}

#[test]
fn in_flight_filters_route_by_resource() {
    let (mut s, c) = items();
    let stone = s.res(ITEM, "minecraft:stone");
    let dirt = s.res(ITEM, "minecraft:dirt");
    s.store(p(-1, 0, 0), CHEST, 1000, &[(stone, 30), (dirt, 30)]);
    for x in 0..5 {
        s.place(p(x, 0, 0), TUBE);
    }
    s.side(c, p(0, 0, 0), Face::West, SideMode::Out);
    s.store(p(2, 0, 1), CHEST, 1000, &[]);
    s.store(p(5, 0, 0), CHEST, 1000, &[]);
    // The side chest only takes stone.
    let f = s.e.add_filter(Filter {
        mode: FilterMode::Allow,
        entries: vec![FilterEntry::Id("minecraft:stone".into())],
    });
    s.with_io(|e, io, probe| {
        let side = Side {
            filter: Some(f),
            ..Default::default()
        };
        e.set_side(
            io,
            probe,
            NodeFace {
                cap: c,
                pos: p(2, 0, 0),
                face: Face::South,
            },
            side,
        );
    });
    s.tick(400);
    assert_eq!(s.w.stores[&p(2, 0, 1)].of(dirt), 0);
    assert!(s.w.stores[&p(2, 0, 1)].of(stone) > 0);
    assert_eq!(s.ledger(), 60);
}

// ── flow ─────────────────────────────────────────────────────────────────────

#[test]
fn flow_equalises_and_conserves() {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let c = cap(
        &mut s.e,
        "test:fluid_pipe",
        "fluid",
        &[FLUID],
        ModelSpec::Flow,
        Policy::RoundRobin,
        &[("fill", IndicatorSource::Fill)],
    );
    attach(
        &mut s.e,
        "test:pipe",
        "test:fluid_pipe",
        &[],
        &[("capacity", 1000.0), ("rate", 80.0)],
    );
    endpoint(
        &mut s.e,
        "test:tank",
        "test:fluid_pipe",
        &[FLUID],
        true,
        true,
    );
    endpoint(
        &mut s.e,
        "test:drain",
        "test:fluid_pipe",
        &[FLUID],
        true,
        false,
    );
    let water = s.res(FLUID, "minecraft:water");
    s.store(p(-1, 0, 0), "test:tank", 100_000, &[(water, 20_000)]);
    s.store(p(10, 0, 0), "test:drain", 100_000, &[]);
    for x in 0..10 {
        s.place(p(x, 0, 0), "test:pipe");
    }
    s.side(c, p(0, 0, 0), Face::West, SideMode::Out);
    let total = s.ledger();
    s.tick(300);
    assert_eq!(s.ledger(), total);
    assert!(
        s.w.stores[&p(10, 0, 0)].total() > 0,
        "fluid reached the drain"
    );
    s.break_block(p(4, 0, 0));
    assert_eq!(s.ledger(), total);
    churn(&mut s, "test:pipe", 5, 300);
}

// ── persistence ──────────────────────────────────────────────────────────────

#[test]
fn save_load_round_trip_keeps_every_unit() {
    let (mut s, c) = items();
    let stone = s.res(ITEM, "minecraft:stone");
    s.store(p(-1, 0, 0), CHEST, 1000, &[(stone, 100)]);
    for x in 0..15 {
        s.place(p(x, 0, 0), TUBE);
    }
    s.side(c, p(0, 0, 0), Face::West, SideMode::Out);
    s.store(p(15, 0, 0), CHEST, 1000, &[]);
    s.tick(25);
    let in_pipes = s.in_pipes(c);
    assert!(in_pipes > 0);
    let bytes = s.e.save();

    // A fresh engine (server restart): same registrations, same world.
    let (mut t, c2) = items();
    t.w.blocks = s.w.blocks.clone();
    t.w.stores = s.w.stores.clone();
    t.e.load(&bytes).unwrap();
    t.with_io(|e, io, probe| e.rediscover(io, probe));
    assert_eq!(t.in_pipes(c2), in_pipes);
    assert_eq!(t.e.parked_records(), 0);
    let total = t.ledger();
    t.tick(300);
    assert_eq!(t.ledger(), total);
    assert_eq!(t.w.stores[&p(15, 0, 0)].total(), 100);
}

#[test]
fn buffered_save_load_round_trip() {
    let (mut s, c) = energy(Policy::RoundRobin);
    let fe = s.res(FE, FE);
    s.store(p(-1, 0, 0), GEN, 1000, &[(fe, 900)]);
    for x in 0..5 {
        s.place(p(x, 0, 0), CABLE);
    }
    s.tick(3);
    let held = s.in_pipes(c);
    assert!(held > 0);
    let bytes = s.e.save();
    let (mut t, c2) = energy(Policy::RoundRobin);
    t.w.blocks = s.w.blocks.clone();
    t.e.load(&bytes).unwrap();
    t.with_io(|e, io, probe| e.rediscover(io, probe));
    assert_eq!(t.in_pipes(c2), held);
}
