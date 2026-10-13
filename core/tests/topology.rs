//! Topology, composition, signals, messaging and registration checks.

mod common;

use common::*;
use yog_pipes_core::engine::{EndpointDef, ModelSpec, TargetDef};
use yog_pipes_core::ids::{CapId, Face, FxHashMap, Pos};
use yog_pipes_core::model::Rng;
use yog_pipes_core::registry::{IndicatorSource, Policy, SideMode};

/// Components of the pipe blocks in `blocks` by flood fill (the oracle).
fn components(blocks: &FxHashMap<Pos, String>, pipe: &str) -> FxHashMap<Pos, usize> {
    let mut label: FxHashMap<Pos, usize> = FxHashMap::default();
    let mut next = 0;
    for (&start, b) in blocks {
        if b != pipe || label.contains_key(&start) {
            continue;
        }
        let mut stack = vec![start];
        label.insert(start, next);
        while let Some(c) = stack.pop() {
            for f in Face::ALL {
                let n = c.offset(f);
                if blocks.get(&n).is_some_and(|b| b == pipe) && !label.contains_key(&n) {
                    label.insert(n, next);
                    stack.push(n);
                }
            }
        }
        next += 1;
    }
    label
}

fn check(s: &Sim, c: CapId, pipe: &str) {
    let oracle = components(&s.w.blocks, pipe);
    let layer = s.e.layer(c);
    assert_eq!(layer.node_count(), oracle.len());
    let mut map: FxHashMap<usize, yog_pipes_core::ids::NetId> = FxHashMap::default();
    for (&pos, &comp) in &oracle {
        let net = s.e.net_at(c, pos).expect("node has a network");
        let expected = *map.entry(comp).or_insert(net);
        assert_eq!(net, expected, "one component, one network");
    }
    let distinct: std::collections::HashSet<_> = map.values().collect();
    assert_eq!(
        distinct.len(),
        map.len(),
        "different components, different networks"
    );
    assert_eq!(layer.nets().count(), map.len());
    for (net, n) in layer.nets() {
        assert_eq!(
            n.size as usize,
            oracle.values().filter(|&&v| map[&v] == net).count()
        );
    }
}

#[test]
fn networks_match_components_under_random_churn() {
    for seed in 1..20 {
        let mut s = Sim::new();
        kinds(&mut s.e);
        let c = cap(
            &mut s.e,
            "test:sig",
            "sig",
            &[],
            ModelSpec::Signal {
                channels: 1,
                attenuation: 0,
            },
            Policy::RoundRobin,
            &[],
        );
        attach(&mut s.e, "test:wire", "test:sig", &[], &[]);
        let mut rng = Rng(seed);
        for _ in 0..600 {
            let pos = p(
                (rng.next_u64() % 7) as i32,
                (rng.next_u64() % 3) as i32,
                (rng.next_u64() % 7) as i32,
            );
            if s.w.blocks.contains_key(&pos) {
                s.break_block(pos);
            } else {
                s.place(pos, "test:wire");
            }
            check(&s, c, "test:wire");
        }
    }
}

#[test]
fn link_groups_and_colours_partition() {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let c = cap(
        &mut s.e,
        "test:power",
        "power",
        &[FE],
        ModelSpec::Buffered,
        Policy::RoundRobin,
        &[],
    );
    attach(
        &mut s.e,
        "test:copper",
        "test:power",
        &["copper"],
        &[("capacity", 10.0), ("throughput", 1.0)],
    );
    attach(
        &mut s.e,
        "test:gold",
        "test:power",
        &["gold"],
        &[("capacity", 10.0), ("throughput", 1.0)],
    );
    attach(
        &mut s.e,
        "test:adapter",
        "test:power",
        &["copper", "gold"],
        &[("capacity", 10.0), ("throughput", 1.0)],
    );
    s.place(p(0, 0, 0), "test:copper");
    s.place(p(1, 0, 0), "test:gold");
    assert_eq!(
        s.e.layer(c).nets().count(),
        2,
        "different groups don't link"
    );
    s.place(p(0, 0, 1), "test:adapter");
    s.place(p(1, 0, 1), "test:adapter");
    assert_eq!(
        s.e.layer(c).nets().count(),
        1,
        "an adapter bridges both groups"
    );
    s.with_io(|e, io, probe| e.set_colour(io, probe, c, p(0, 0, 1), 3));
    s.with_io(|e, io, probe| e.set_colour(io, probe, c, p(1, 0, 1), 4));
    assert_eq!(
        s.e.layer(c).nets().count(),
        2,
        "different colours don't link"
    );
}

/// One block, two capabilities from two "mods": each has its own network
/// view (groups, sides) and its own indicator namespace.
#[test]
fn combined_pipe_composes_independent_capabilities() {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let power = cap(
        &mut s.e,
        "energy_addon:power",
        "power",
        &[FE],
        ModelSpec::Buffered,
        Policy::RoundRobin,
        &[("fill", IndicatorSource::Fill)],
    );
    let sig = cap(
        &mut s.e,
        "signal_addon:bundle",
        "bundle",
        &[],
        ModelSpec::Signal {
            channels: 16,
            attenuation: 0,
        },
        Policy::RoundRobin,
        &[("power", IndicatorSource::Power)],
    );
    const UNI: &str = "mymod:universal_cable";
    attach(
        &mut s.e,
        UNI,
        "energy_addon:power",
        &["cable"],
        &[("capacity", 1000.0), ("throughput", 100.0)],
    );
    attach(&mut s.e, UNI, "signal_addon:bundle", &["bundle"], &[]);
    attach(
        &mut s.e,
        "mymod:plain_cable",
        "energy_addon:power",
        &["cable"],
        &[("capacity", 1000.0), ("throughput", 100.0)],
    );
    endpoint(
        &mut s.e,
        "mymod:gen",
        "energy_addon:power",
        &[FE],
        false,
        true,
    );
    endpoint(
        &mut s.e,
        "mymod:bat",
        "energy_addon:power",
        &[FE],
        true,
        false,
    );
    s.e.register_endpoint(EndpointDef {
        cap: "signal_addon:bundle".into(),
        target: TargetDef::Block("mymod:lamp".into()),
        kinds: vec![],
        can_insert: false,
        can_extract: false,
        listens_signals: true,
        listens_messages: false,
        groups: vec![],
        priority: 0,
        max_packet: 0,
        pressure: 0,
        handler: 0,
    })
    .unwrap();

    // uni uni plain uni : power is one network, signal is two.
    let fe = s.res(FE, FE);
    s.store(p(-1, 0, 0), "mymod:gen", 10_000, &[(fe, 5_000)]);
    s.place(p(0, 0, 0), UNI);
    s.place(p(1, 0, 0), UNI);
    s.place(p(2, 0, 0), "mymod:plain_cable");
    s.place(p(3, 0, 0), UNI);
    s.store(p(4, 0, 0), "mymod:bat", 10_000, &[]);
    s.place(p(0, 1, 0), "mymod:lamp");
    s.place(p(3, 1, 0), "mymod:lamp");
    assert_eq!(s.e.layer(power).nets().count(), 1);
    assert_eq!(s.e.layer(sig).nets().count(), 2);

    // Signal: a side set off in the signal layer leaves power alone.
    s.side(sig, p(0, 0, 0), Face::East, SideMode::Off);
    assert_eq!(s.e.layer(sig).nets().count(), 3);
    assert_eq!(s.e.layer(power).nets().count(), 1);

    s.with_io(|e, io, _| e.emit(io, sig, p(0, 0, 0), 5, 12));
    s.tick(20);
    assert_eq!(s.e.level(sig, p(0, 0, 0), 5), 12);
    assert_eq!(
        s.e.level(sig, p(1, 0, 0), 5),
        0,
        "cut by the signal-only side"
    );
    assert!(s.w.signals.contains(&(p(0, 1, 0), 5, 12)), "lamp told");
    assert!(
        !s.w.signals.iter().any(|x| x.0 == p(3, 1, 0)),
        "other network's lamp not told"
    );
    assert!(
        s.w.stores[&p(4, 0, 0)].total() > 0,
        "power flows past the signal cut"
    );

    // Both indicators on one block, namespaced.
    let shown = s.e.indication(p(0, 0, 0));
    let names: Vec<&str> = shown.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(names, vec!["power_fill", "bundle_power"]);
    assert!(shown.iter().any(|x| x.0 == "bundle_power" && x.1 == 12));
}

#[test]
fn attenuated_signal_fades_with_distance() {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let c = cap(
        &mut s.e,
        "test:dust",
        "dust",
        &[],
        ModelSpec::Signal {
            channels: 1,
            attenuation: 1,
        },
        Policy::RoundRobin,
        &[],
    );
    attach(&mut s.e, "test:dust", "test:dust", &[], &[]);
    for x in 0..20 {
        s.place(p(x, 0, 0), "test:dust");
    }
    s.with_io(|e, io, _| e.emit(io, c, p(0, 0, 0), 0, 15));
    s.tick(1);
    assert_eq!(s.e.level(c, p(0, 0, 0), 0), 15);
    assert_eq!(s.e.level(c, p(5, 0, 0), 0), 10);
    assert_eq!(s.e.level(c, p(14, 0, 0), 0), 1);
    assert_eq!(s.e.level(c, p(15, 0, 0), 0), 0);
    s.break_block(p(3, 0, 0));
    s.tick(1);
    assert_eq!(s.e.level(c, p(5, 0, 0), 0), 0);
}

#[test]
fn messaging_posts_broadcasts_and_bounds_inboxes() {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let c = cap(
        &mut s.e,
        "test:data",
        "data",
        &[],
        ModelSpec::Signal {
            channels: 1,
            attenuation: 0,
        },
        Policy::RoundRobin,
        &[],
    );
    attach(&mut s.e, "test:bus", "test:data", &[], &[]);
    for x in 0..4 {
        s.place(p(x, 0, 0), "test:bus");
    }
    s.place(p(10, 0, 0), "test:bus");
    let (a, b, far) = (p(0, 0, 0), p(3, 0, 0), p(10, 0, 0));
    assert!(s.with_io(|e, io, _| e.post(io, c, a, b, b"hi")));
    assert!(
        !s.with_io(|e, io, _| e.post(io, c, a, far, b"no")),
        "not connected"
    );
    assert_eq!(s.e.receive(c, b).as_deref(), Some(&b"hi"[..]));
    assert_eq!(s.e.receive(c, b), None);
    for _ in 0..yog_pipes_core::engine::INBOX {
        assert!(s.with_io(|e, io, _| e.post(io, c, a, b, b"x")));
    }
    assert!(
        !s.with_io(|e, io, _| e.post(io, c, a, b, b"x")),
        "full inbox refuses"
    );
    s.with_io(|e, io, _| e.broadcast(io, c, a, b"level=3"));
    assert_eq!(s.e.latest(c, b), Some(&b"level=3"[..]));
    assert_eq!(s.e.latest(c, far), None);
}

#[test]
fn registration_is_checked() {
    let mut s = Sim::new();
    kinds(&mut s.e);
    cap(
        &mut s.e,
        "test:power",
        "power",
        &[FE],
        ModelSpec::Buffered,
        Policy::RoundRobin,
        &[],
    );
    // Unknown parameter.
    assert!(
        s.e.attach(yog_pipes_core::engine::AttachDef {
            block: "x".into(),
            cap: "test:power".into(),
            groups: vec![],
            params: vec![("speed".into(), 1.0)],
        })
        .is_err()
    );
    // Endpoint of a kind the capability doesn't carry.
    let bad = EndpointDef {
        cap: "test:power".into(),
        target: TargetDef::Block("test:tank".into()),
        kinds: vec![FLUID.into()],
        can_insert: true,
        can_extract: false,
        listens_signals: false,
        listens_messages: false,
        groups: vec![],
        priority: 0,
        max_packet: 0,
        pressure: 0,
        handler: 0,
    };
    assert!(s.e.register_endpoint(bad).is_err());
    // Un-namespaced kind, duplicate state prefix.
    assert!(
        s.e.register_kind(yog_pipes_core::registry::Kind {
            name: "fe".into(),
            form: yog_pipes_core::registry::KindForm::Continuous,
            merge: yog_pipes_core::registry::MergeRule::ById,
            unit: "FE".into(),
            conversions: vec![],
        })
        .is_err()
    );
    assert!(
        s.e.register_capability(yog_pipes_core::engine::CapabilityDef {
            name: "other:thing".into(),
            kinds: vec![],
            exclusive: false,
            state_prefix: "power".into(),
            indicators: vec![],
            policy: Policy::RoundRobin,
            events: 0,
            model: ModelSpec::Routed,
        })
        .is_err()
    );
}

#[test]
fn kind_conversion_is_exact() {
    let mut s = Sim::new();
    kinds(&mut s.e);
    let fe_kind = s.e.reg.kinds.id(FE).unwrap();
    // 4 FE = 1 EU.
    s.e.register_kind(yog_pipes_core::registry::Kind {
        name: "test:rf".into(),
        form: yog_pipes_core::registry::KindForm::Continuous,
        merge: yog_pipes_core::registry::MergeRule::ById,
        unit: "RF".into(),
        conversions: vec![],
    })
    .unwrap();
    let eu =
        s.e.register_kind(yog_pipes_core::registry::Kind {
            name: "test:eu".into(),
            form: yog_pipes_core::registry::KindForm::Continuous,
            merge: yog_pipes_core::registry::MergeRule::ById,
            unit: "EU".into(),
            conversions: vec![],
        })
        .unwrap();
    s.e.register_kind(yog_pipes_core::registry::Kind {
        name: FE.into(),
        form: yog_pipes_core::registry::KindForm::Continuous,
        merge: yog_pipes_core::registry::MergeRule::ById,
        unit: "FE".into(),
        conversions: vec![yog_pipes_core::registry::Conversion {
            to: eu,
            num: 1,
            den: 4,
        }],
    })
    .unwrap();
    let _ = fe_kind;
    let c = cap(
        &mut s.e,
        "test:power",
        "power",
        &[FE],
        ModelSpec::Routed,
        Policy::RoundRobin,
        &[],
    );
    attach(
        &mut s.e,
        "test:cable",
        "test:power",
        &[],
        &[("throughput", 50.0)],
    );
    endpoint(&mut s.e, "test:gen", "test:power", &[FE], false, true);
    endpoint(
        &mut s.e,
        "test:eu_machine",
        "test:power",
        &["test:eu"],
        true,
        false,
    );
    let fe = s.res(FE, FE);
    s.store(p(-1, 0, 0), "test:gen", 10_000, &[(fe, 1_001)]);
    s.place(p(0, 0, 0), "test:cable");
    s.store(p(1, 0, 0), "test:eu_machine", 10_000, &[]);
    s.tick(100);
    let eu_res = s.res("test:eu", "test:eu");
    let got = s.w.stores[&p(1, 0, 0)].of(eu_res);
    let left = s.w.stores[&p(-1, 0, 0)].of(fe);
    assert_eq!(
        got * 4 + left,
        1_001,
        "every FE either converted or still there"
    );
    assert_eq!(left, 1);
    let _ = c;
}
