//! Criterion benches of the transport engine. The topology scenarios use
//! the same shapes and sizes as the 0.4 baseline (`line`, `grid` =
//! ⌈√N⌉², `tree` = comb) so before/after compare directly; the model
//! scenarios cover each built-in mechanic.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use yog_pipes_core::engine::{
    AttachDef, CapabilityDef, EndpointDef, Engine, ModelSpec, Probe, TargetDef,
};
use yog_pipes_core::ids::{BlockTypeId, CapId, DimId, Face, FxHashMap, Pos, ResId};
use yog_pipes_core::model::{Call, Event, Io, Want};
use yog_pipes_core::registry::{
    Indicator, IndicatorSource, Kind, KindForm, MergeRule, Policy, Registry, Side, SideMode,
};

const PIPE: &str = "bench:pipe";
const SOURCE: &str = "bench:source";
const SINK: &str = "bench:sink";
const CAP: &str = "bench:cap";
const KIND: &str = "bench:unit";

fn line(n: usize) -> Vec<(i32, i32, i32)> {
    (0..n as i32).map(|x| (x, 0, 0)).collect()
}
fn grid(n: usize) -> Vec<(i32, i32, i32)> {
    let s = (n as f64).sqrt().ceil() as i32;
    let mut v = Vec::new();
    'o: for x in 0..s {
        for z in 0..s {
            if v.len() == n {
                break 'o;
            }
            v.push((x, 0, z));
        }
    }
    v
}
fn tree(n: usize) -> Vec<(i32, i32, i32)> {
    let side = ((n as f64) / 1.5).sqrt().ceil() as i32;
    let mut v: Vec<_> = (0..side * 2).map(|x| (x, 0, 0)).collect();
    'o: for x in (0..side * 2).step_by(2) {
        for z in 1.. {
            if v.len() >= n {
                break 'o;
            }
            if z > side {
                break;
            }
            v.push((x, 0, z));
        }
    }
    v.truncate(n);
    v
}

const SIZES: [usize; 4] = [100, 1_000, 10_000, 64_000];
type Shape = fn(usize) -> Vec<(i32, i32, i32)>;
const SHAPES: [(&str, Shape); 3] = [("line", line), ("grid", grid), ("tree", tree)];

fn pos(c: (i32, i32, i32)) -> Pos {
    Pos::new(DimId(0), c.0, c.1, c.2)
}

// ── Mock world ───────────────────────────────────────────────────────────────

struct World {
    blocks: FxHashMap<Pos, String>,
}

impl Probe for World {
    fn block(&mut self, reg: &Registry, pos: Pos) -> Option<BlockTypeId> {
        reg.block_id(self.blocks.get(&pos)?)
    }
}

/// Endless sources and sinks: the cost measured is the engine's.
struct Endless {
    res: ResId,
    moved: u64,
}

impl Io for Endless {
    fn insert(&mut self, _: &Registry, _: &Call, _: ResId, amount: u64, simulate: bool) -> u64 {
        if !simulate {
            self.moved += amount;
        }
        amount
    }
    fn extract(
        &mut self,
        _: &mut Registry,
        _: &Call,
        _: Want,
        max: u64,
        _: bool,
    ) -> Option<(ResId, u64)> {
        Some((self.res, max))
    }
    fn signal(&mut self, _: &Registry, _: &Call, _: u8, _: u8) {}
    fn message(&mut self, _: &Registry, _: &Call, _: Pos, _: &[u8]) -> bool {
        true
    }
    fn event(&mut self, _: &Registry, _: CapId, _: &Event) {}
    fn indicate(&mut self, _: &Registry, _: CapId, _: Pos, _: u8, _: u8) {}
}

fn engine(model: ModelSpec, params: &[(&str, f64)]) -> (Engine, CapId, ResId) {
    let mut e = Engine::new();
    e.dim("minecraft:overworld");
    let k = e
        .register_kind(Kind {
            name: KIND.into(),
            form: KindForm::Continuous,
            merge: MergeRule::ById,
            unit: "u".into(),
            conversions: vec![],
        })
        .unwrap();
    let c = e
        .register_capability(CapabilityDef {
            name: CAP.into(),
            kinds: vec![KIND.into()],
            exclusive: false,
            state_prefix: "bench".into(),
            indicators: vec![Indicator {
                name: "fill".into(),
                levels: 15,
                source: IndicatorSource::Fill,
            }],
            policy: Policy::RoundRobin,
            events: 0,
            model,
        })
        .unwrap();
    e.attach(AttachDef {
        block: PIPE.into(),
        cap: CAP.into(),
        groups: vec![],
        params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
    })
    .unwrap();
    for (block, ins, ext) in [(SOURCE, false, true), (SINK, true, false)] {
        e.register_endpoint(EndpointDef {
            cap: CAP.into(),
            target: TargetDef::Block(block.into()),
            kinds: vec![KIND.into()],
            can_insert: ins,
            can_extract: ext,
            listens_signals: true,
            listens_messages: true,
            groups: vec![],
            priority: 0,
            max_packet: 0,
            pressure: 0,
            handler: 0,
        })
        .unwrap();
    }
    let res = e.reg.resources.intern(&e.reg.kinds, k, KIND, &[]);
    (e, c, res)
}

fn signal_engine() -> (Engine, CapId, ResId) {
    engine(
        ModelSpec::Signal {
            channels: 16,
            attenuation: 0,
        },
        &[],
    )
}

fn world(cells: &[(i32, i32, i32)]) -> World {
    World {
        blocks: cells.iter().map(|&c| (pos(c), PIPE.to_string())).collect(),
    }
}

/// `m` sources and `m` sinks on top of grid cells, picked as in the 0.4
/// baseline.
fn endpoints(w: &mut World, cells: &[(i32, i32, i32)], m: usize) {
    for i in 0..m {
        let s = cells[(i * 7919) % cells.len()];
        let t = cells[(i * 104_729 + 5_003) % cells.len()];
        w.blocks.insert(pos((s.0, 1, s.2)), SOURCE.into());
        w.blocks.entry(pos((t.0, 1, t.2))).or_insert(SINK.into());
    }
}

fn build(e: &mut Engine, w: &mut World, res: ResId, start: (i32, i32, i32)) {
    let mut io = Endless { res, moved: 0 };
    e.place(&mut io, w, pos(start));
}

// ── Topology ─────────────────────────────────────────────────────────────────

fn discovery(c: &mut Criterion) {
    let mut g = c.benchmark_group("discover");
    for (name, shape) in SHAPES {
        for n in SIZES {
            let cells = shape(n);
            let mut w = world(&cells);
            let (mut e, cap, res) = signal_engine();
            build(&mut e, &mut w, res, cells[0]);
            assert_eq!(
                e.net_info(cap, pos(cells[0])).map(|i| i.size),
                Some(cells.len() as u32)
            );
            g.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
                b.iter_batched(
                    signal_engine,
                    |(mut e, _, res)| {
                        build(&mut e, &mut w, res, cells[0]);
                        e
                    },
                    BatchSize::LargeInput,
                )
            });
        }
    }
}

fn place_break(c: &mut Criterion) {
    let mut g = c.benchmark_group("place_break");
    for n in SIZES {
        // Splitting: the middle of a line (cost ∝ the smaller half).
        let cells = line(n);
        let mut w = world(&cells);
        let (mut e, _, res) = signal_engine();
        build(&mut e, &mut w, res, cells[0]);
        let mid = pos((n as i32 / 2, 0, 0));
        let mut io = Endless { res, moved: 0 };
        g.bench_with_input(BenchmarkId::new("line_mid_split", n), &n, |b, _| {
            b.iter(|| {
                w.blocks.remove(&mid);
                e.remove(&mut io, mid);
                w.blocks.insert(mid, PIPE.into());
                e.place(&mut io, &mut w, mid);
            })
        });
        // Non-splitting: a pipe inside a grid (the searches meet at once).
        let cells = grid(n);
        let mut w = world(&cells);
        let (mut e, _, res) = signal_engine();
        build(&mut e, &mut w, res, cells[0]);
        let s = (n as f64).sqrt().ceil() as i32;
        let mid = pos((s / 2, 0, s / 2));
        g.bench_with_input(BenchmarkId::new("grid_mid_nosplit", n), &n, |b, _| {
            b.iter(|| {
                w.blocks.remove(&mid);
                e.remove(&mut io, mid);
                w.blocks.insert(mid, PIPE.into());
                e.place(&mut io, &mut w, mid);
            })
        });
        // Leaf: the end of a line.
        let cells = line(n);
        let mut w = world(&cells);
        let (mut e, _, res) = signal_engine();
        build(&mut e, &mut w, res, cells[0]);
        let end = pos((n as i32 - 1, 0, 0));
        g.bench_with_input(BenchmarkId::new("line_end", n), &n, |b, _| {
            b.iter(|| {
                w.blocks.remove(&end);
                e.remove(&mut io, end);
                w.blocks.insert(end, PIPE.into());
                e.place(&mut io, &mut w, end);
            })
        });
    }
}

fn messaging(c: &mut Criterion) {
    let mut g = c.benchmark_group("messaging");
    for n in SIZES {
        // 0.4 broadcast stored the payload on every node; 0.5 delivers to
        // listening endpoints only — 100 of them here.
        let cells = grid(n);
        let mut w = world(&cells);
        endpoints(&mut w, &cells, 100);
        let (mut e, cap, res) = signal_engine();
        build(&mut e, &mut w, res, cells[0]);
        let mut io = Endless { res, moved: 0 };
        let (from, to) = (pos(cells[0]), pos(cells[cells.len() - 1]));
        assert_eq!(e.net_info(cap, from).map(|i| i.size), Some(n as u32));
        g.bench_with_input(BenchmarkId::new("broadcast_grid_100_listeners", n), &n, |b, _| {
            b.iter(|| black_box(e.broadcast(&mut io, cap, from, &[15])))
        });
        g.bench_with_input(BenchmarkId::new("post_corner_to_corner", n), &n, |b, _| {
            b.iter(|| {
                black_box(e.post(&mut io, cap, from, to, &[1; 8]));
                e.receive(cap, to)
            })
        });
    }
}

// ── Models ───────────────────────────────────────────────────────────────────

fn tick_bench(
    g: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    id: BenchmarkId,
    model: ModelSpec,
    params: &[(&str, f64)],
    m: usize,
    warm: u32,
) {
    let cells = grid(10_000);
    let mut w = world(&cells);
    endpoints(&mut w, &cells, m);
    let (mut e, _, res) = engine(model, params);
    build(&mut e, &mut w, res, cells[0]);
    let mut io = Endless { res, moved: 0 };
    for _ in 0..warm {
        e.tick(&mut io);
    }
    g.bench_function(id, |b| b.iter(|| e.tick(&mut io)));
}

fn models(c: &mut Criterion) {
    let mut g = c.benchmark_group("tick_10k_grid");
    for m in [10usize, 100, 500] {
        tick_bench(
            &mut g,
            BenchmarkId::new("buffered", m),
            ModelSpec::Buffered,
            &[("capacity", 1000.0), ("throughput", 100.0)],
            m,
            5,
        );
        tick_bench(
            &mut g,
            BenchmarkId::new("routed", m),
            ModelSpec::Routed,
            &[("throughput", 100.0)],
            m,
            5,
        );
    }
    // ~10k units in flight: 100 sources, one unit each per tick, slow pipes.
    tick_bench(
        &mut g,
        BenchmarkId::new("in_flight_units", 10_000),
        ModelSpec::InFlight,
        &[("speed", 0.05), ("stack", 1.0), ("interval", 1.0)],
        100,
        100,
    );
    g.finish();

    let mut g = c.benchmark_group("flow");
    // Settled: no endpoints, nothing moves → the network sleeps.
    let cells = grid(10_000);
    let mut w = world(&cells);
    let (mut e, _, res) = engine(ModelSpec::Flow, &[("capacity", 1000.0), ("rate", 100.0)]);
    build(&mut e, &mut w, res, cells[0]);
    let mut io = Endless { res, moved: 0 };
    for _ in 0..50 {
        e.tick(&mut io);
    }
    g.bench_function("settled_10k", |b| b.iter(|| e.tick(&mut io)));
    // Active: a 1k line fed at one end and drained at the other.
    let cells = line(1_000);
    let mut w = world(&cells);
    w.blocks.insert(pos((-1, 0, 0)), SOURCE.into());
    w.blocks.insert(pos((1_000, 0, 0)), SINK.into());
    let (mut e, _, res) = engine(ModelSpec::Flow, &[("capacity", 1000.0), ("rate", 100.0)]);
    build(&mut e, &mut w, res, cells[0]);
    e.place(&mut io, &mut w, pos((1_000, 0, 0)));
    e.place(&mut io, &mut w, pos((-1, 0, 0)));
    for _ in 0..2_000 {
        e.tick(&mut io);
    }
    g.bench_function("flowing_1k_line", |b| b.iter(|| e.tick(&mut io)));
    g.finish();

    let mut g = c.benchmark_group("signal");
    let cells = grid(10_000);
    let mut w = world(&cells);
    endpoints(&mut w, &cells, 10);
    let (mut e, cap, res) = signal_engine();
    build(&mut e, &mut w, res, cells[0]);
    let at = pos(cells[0]);
    let mut level = 0u8;
    g.bench_function("change_10k_grid", |b| {
        b.iter(|| {
            level = (level + 1) % 16;
            e.emit(&mut io, cap, at, 3, level);
            e.tick(&mut io);
        })
    });
    let _ = (Face::Up, Side::default(), SideMode::Auto);
    g.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20);
    targets = discovery, place_break, messaging, models
}
criterion_main!(benches);
