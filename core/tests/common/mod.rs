//! A mock world for engine tests: blocks by position, endpoint storages,
//! and a ledger of every unit that left the network as an event.

#![allow(dead_code)]

use yog_pipes_core::engine::{
    AttachDef, CapabilityDef, EndpointDef, Engine, ModelSpec, Probe, TargetDef,
};
use yog_pipes_core::ids::{BlockTypeId, CapId, DimId, Face, FxHashMap, Pos, ResId};
use yog_pipes_core::model::{Call, Event, Io, Want};
use yog_pipes_core::registry::{
    Indicator, IndicatorSource, Kind, KindForm, MergeRule, Policy, Registry, Side, SideMode,
};

pub const DIM: DimId = DimId(0);

pub fn p(x: i32, y: i32, z: i32) -> Pos {
    Pos::new(DIM, x, y, z)
}

/// One storage block: contents by resource, a capacity per resource and
/// optional per-call limits.
#[derive(Debug, Clone, Default)]
pub struct Store {
    pub items: Vec<(ResId, u64)>,
    pub capacity: u64,
    /// Max accepted per insert call (0 = no limit).
    pub accept_per_call: u64,
}

impl Store {
    pub fn total(&self) -> u64 {
        self.items.iter().map(|i| i.1).sum()
    }
    pub fn of(&self, r: ResId) -> u64 {
        self.items.iter().find(|i| i.0 == r).map_or(0, |i| i.1)
    }
}

#[derive(Default)]
pub struct World {
    pub blocks: FxHashMap<Pos, String>,
    pub stores: FxHashMap<Pos, Store>,
    pub spilled: Vec<(Pos, ResId, u64)>,
    pub lost: Vec<(Pos, ResId, u64)>,
    pub overloads: Vec<(Pos, u64, u64)>,
    pub signals: Vec<(Pos, u8, u8)>,
    pub messages: Vec<(Pos, Pos, Vec<u8>)>,
    pub indications: Vec<(CapId, Pos, u8, u8)>,
}

impl World {
    pub fn spilled_total(&self) -> u64 {
        self.spilled.iter().map(|s| s.2).sum()
    }
    pub fn lost_total(&self) -> u64 {
        self.lost.iter().map(|s| s.2).sum()
    }
    pub fn stored_total(&self) -> u64 {
        self.stores.values().map(Store::total).sum()
    }
}

pub struct WorldProbe<'a>(pub &'a FxHashMap<Pos, String>);

impl Probe for WorldProbe<'_> {
    fn block(&mut self, reg: &Registry, pos: Pos) -> Option<BlockTypeId> {
        reg.block_id(self.0.get(&pos)?)
    }
}

/// `Io` over the stores; split from `World` so the engine can borrow the
/// probe (blocks) and the io (everything else) at once.
pub struct WorldIo<'a> {
    pub stores: &'a mut FxHashMap<Pos, Store>,
    pub spilled: &'a mut Vec<(Pos, ResId, u64)>,
    pub lost: &'a mut Vec<(Pos, ResId, u64)>,
    pub overloads: &'a mut Vec<(Pos, u64, u64)>,
    pub signals: &'a mut Vec<(Pos, u8, u8)>,
    pub messages: &'a mut Vec<(Pos, Pos, Vec<u8>)>,
    pub indications: &'a mut Vec<(CapId, Pos, u8, u8)>,
}

impl Io for WorldIo<'_> {
    fn insert(
        &mut self,
        _reg: &Registry,
        call: &Call,
        res: ResId,
        amount: u64,
        simulate: bool,
    ) -> u64 {
        let Some(s) = self.stores.get_mut(&call.at) else {
            return 0;
        };
        let room = s.capacity.saturating_sub(s.total());
        let mut take = amount.min(room);
        if s.accept_per_call > 0 {
            take = take.min(s.accept_per_call);
        }
        if !simulate && take > 0 {
            match s.items.iter_mut().find(|i| i.0 == res) {
                Some(i) => i.1 += take,
                None => s.items.push((res, take)),
            }
        }
        take
    }

    fn extract(
        &mut self,
        reg: &mut Registry,
        call: &Call,
        want: Want,
        max: u64,
        simulate: bool,
    ) -> Option<(ResId, u64)> {
        let s = self.stores.get_mut(&call.at)?;
        let idx = s.items.iter().position(|&(r, n)| {
            n > 0
                && match want {
                    Want::Any => true,
                    Want::Res(w) => w == r,
                    Want::Filter(f) => reg.filters.get(f).passes(&reg.resources, r, 0),
                }
        })?;
        let (r, n) = s.items[idx];
        let take = n.min(max);
        if !simulate {
            s.items[idx].1 -= take;
            if s.items[idx].1 == 0 {
                s.items.remove(idx);
            }
        }
        Some((r, take))
    }

    fn signal(&mut self, _reg: &Registry, call: &Call, channel: u8, level: u8) {
        self.signals.push((call.at, channel, level));
    }

    fn message(&mut self, _reg: &Registry, call: &Call, from: Pos, payload: &[u8]) -> bool {
        self.messages.push((call.at, from, payload.to_vec()));
        true
    }

    fn event(&mut self, _reg: &Registry, _cap: CapId, event: &Event) {
        match *event {
            Event::Spilled { at, res, amount } => self.spilled.push((at, res, amount)),
            Event::Lost { at, res, amount } => self.lost.push((at, res, amount)),
            Event::Overload { at, amount, limit } => self.overloads.push((at, amount, limit)),
        }
    }

    fn indicate(&mut self, _reg: &Registry, cap: CapId, at: Pos, index: u8, value: u8) {
        self.indications.push((cap, at, index, value));
    }
}

/// An engine plus a world, with helpers that split the borrows.
pub struct Sim {
    pub e: Engine,
    pub w: World,
}

macro_rules! io {
    ($w:expr) => {
        WorldIo {
            stores: &mut $w.stores,
            spilled: &mut $w.spilled,
            lost: &mut $w.lost,
            overloads: &mut $w.overloads,
            signals: &mut $w.signals,
            messages: &mut $w.messages,
            indications: &mut $w.indications,
        }
    };
}

impl Sim {
    pub fn new() -> Self {
        let mut e = Engine::new();
        assert_eq!(e.dim("minecraft:overworld"), DIM);
        Sim {
            e,
            w: World::default(),
        }
    }

    pub fn place(&mut self, pos: Pos, block: &str) {
        self.w.blocks.insert(pos, block.to_string());
        let mut io = io!(self.w);
        self.e.place(&mut io, &mut WorldProbe(&self.w.blocks), pos);
    }

    /// Place a storage block (an endpoint) with `capacity`.
    pub fn store(&mut self, pos: Pos, block: &str, capacity: u64, items: &[(ResId, u64)]) {
        self.w.stores.insert(
            pos,
            Store {
                items: items.to_vec(),
                capacity,
                accept_per_call: 0,
            },
        );
        self.place(pos, block);
    }

    pub fn break_block(&mut self, pos: Pos) {
        self.w.blocks.remove(&pos);
        let mut io = io!(self.w);
        self.e.remove(&mut io, pos);
    }

    pub fn tick(&mut self, n: u32) {
        for _ in 0..n {
            let mut io = io!(self.w);
            self.e.tick(&mut io);
        }
    }

    pub fn with_io<R>(
        &mut self,
        f: impl FnOnce(&mut Engine, &mut WorldIo, &mut WorldProbe) -> R,
    ) -> R {
        let mut io = io!(self.w);
        let mut probe = WorldProbe(&self.w.blocks);
        f(&mut self.e, &mut io, &mut probe)
    }

    /// Set the mode of the `cap` node at `pos` on `face`.
    pub fn side(&mut self, cap: CapId, pos: Pos, face: Face, mode: SideMode) {
        self.with_io(|e, io, probe| {
            let side = Side {
                mode,
                ..Default::default()
            };
            e.set_side(io, probe, cap, pos, face, side)
        });
    }

    pub fn res(&mut self, kind: &str, id: &str) -> ResId {
        let k = self.e.reg.kinds.id(kind).unwrap();
        let reg = &mut self.e.reg;
        reg.resources.intern(&reg.kinds, k, id, &[])
    }

    /// Units held by the network models of `cap`.
    pub fn in_pipes(&self, cap: CapId) -> u64 {
        self.e.totals(cap).iter().map(|t| t.1).sum()
    }

    /// Everything that exists: stores + pipes (all caps) + spilled + lost.
    pub fn ledger(&self) -> u64 {
        let pipes: u64 = self
            .e
            .reg
            .caps
            .iter()
            .enumerate()
            .map(|(i, _)| self.in_pipes(CapId(i as u16)))
            .sum();
        self.w.stored_total() + pipes + self.w.spilled_total() + self.w.lost_total()
    }
}

// ── Common registrations ─────────────────────────────────────────────────────

pub const FE: &str = "test:fe";
pub const ITEM: &str = "test:item";
pub const FLUID: &str = "test:fluid";

pub fn kinds(e: &mut Engine) {
    e.register_kind(Kind {
        name: FE.into(),
        form: KindForm::Continuous,
        merge: MergeRule::ById,
        unit: "FE".into(),
        conversions: vec![],
    })
    .unwrap();
    e.register_kind(Kind {
        name: ITEM.into(),
        form: KindForm::Discrete,
        merge: MergeRule::ByIdAndData,
        unit: "items".into(),
        conversions: vec![],
    })
    .unwrap();
    e.register_kind(Kind {
        name: FLUID.into(),
        form: KindForm::Continuous,
        merge: MergeRule::ById,
        unit: "mB".into(),
        conversions: vec![],
    })
    .unwrap();
}

pub fn cap(
    e: &mut Engine,
    name: &str,
    prefix: &str,
    kinds: &[&str],
    model: ModelSpec,
    policy: Policy,
    ind: &[(&str, IndicatorSource)],
) -> CapId {
    e.register_capability(CapabilityDef {
        name: name.into(),
        kinds: kinds.iter().map(|s| s.to_string()).collect(),
        exclusive: false,
        state_prefix: prefix.into(),
        indicators: ind
            .iter()
            .map(|(n, s)| Indicator {
                name: n.to_string(),
                levels: 15,
                source: *s,
            })
            .collect(),
        policy,
        events: 0,
        model,
    })
    .unwrap()
}

pub fn attach(e: &mut Engine, block: &str, cap: &str, groups: &[&str], params: &[(&str, f64)]) {
    e.attach(AttachDef {
        block: block.into(),
        cap: cap.into(),
        groups: groups.iter().map(|s| s.to_string()).collect(),
        params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
    })
    .unwrap();
}

pub fn endpoint(
    e: &mut Engine,
    block: &str,
    cap: &str,
    kinds: &[&str],
    insert: bool,
    extract: bool,
) {
    e.register_endpoint(EndpointDef {
        cap: cap.into(),
        target: TargetDef::Block(block.into()),
        kinds: kinds.iter().map(|s| s.to_string()).collect(),
        can_insert: insert,
        can_extract: extract,
        listens_signals: false,
        listens_messages: false,
        groups: vec![],
        priority: 0,
        max_packet: 0,
        pressure: 0,
        handler: 0,
    })
    .unwrap();
}
