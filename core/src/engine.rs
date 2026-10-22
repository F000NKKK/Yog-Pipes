//! The engine: registrations, one [`Layer`] + [`Model`] per capability,
//! world discovery through a [`Probe`], the scheduler, messaging and
//! saving. The host (the Yog-Pipes mod, or a test) drives it with world
//! events and ticks and implements [`Io`] for endpoint calls.

use std::collections::VecDeque;

use crate::ids::{
    BlockTypeId, CapId, ChunkKey, EpTypeId, Face, FxHashMap, FxHashSet, KindId, NetId, NodeId, Pos,
    ResId,
};
use crate::indicate::{self, Indication, MAX_INDICATORS};
use crate::layer::{Layer, Settled, Split};
use crate::model::buffered::Buffered;
use crate::model::external::{Driver, External};
use crate::model::flow::Flow;
use crate::model::in_flight::InFlight;
use crate::model::routed::Routed;
use crate::model::signal::Signal;
use crate::model::{Activity, Call, Cx, Io, Model, Record, Rng};
use crate::persist::{Dec, Enc};
use crate::registry::{
    Attach, Capability, EndpointTarget, EndpointType, Filter, FilterEntry, FilterMode, Indicator,
    Kind, Policy, Registry, Side, SideMode, groups_meet,
};

/// Most nodes one discovery adds (a runaway guard, not a limit on network
/// size: a bigger network is completed by the next discovery that touches
/// it). Counted in nodes, not probed positions — every node also probes
/// the air around it.
pub const MAX_DISCOVERY: usize = 262_144;

/// Bounded per-endpoint inbox for `post`.
pub const INBOX: usize = 64;

/// Save format version.
pub const FORMAT: u32 = 1;
const MAGIC: &[u8; 4] = b"YPT1";

/// Ticks between cache housekeeping passes.
const PRUNE_EVERY: u64 = 1200;

/// The world, as discovery sees it.
pub trait Probe {
    /// The engine's type of the block at `pos` (`reg.block_id(...)` of its
    /// id), or `None` for a block the engine doesn't know.
    fn block(&mut self, reg: &Registry, pos: Pos) -> Option<BlockTypeId>;
}

/// Which built-in mechanic a capability uses, or an external one.
pub enum ModelSpec {
    Buffered,
    Routed,
    InFlight,
    Flow,
    Signal {
        channels: u16,
        attenuation: u8,
    },
    External {
        driver: Box<dyn Driver>,
        params: Vec<String>,
    },
}

impl ModelSpec {
    fn build(self) -> Box<dyn Model> {
        match self {
            ModelSpec::Buffered => Box::new(Buffered::new()),
            ModelSpec::Routed => Box::new(Routed::new()),
            ModelSpec::InFlight => Box::new(InFlight::new()),
            ModelSpec::Flow => Box::new(Flow::new()),
            ModelSpec::Signal {
                channels,
                attenuation,
            } => Box::new(Signal::new(channels, attenuation)),
            ModelSpec::External { driver, params } => Box::new(External::new(driver, params)),
        }
    }

    fn tag(&self) -> u8 {
        match self {
            ModelSpec::Buffered => 0,
            ModelSpec::Routed => 1,
            ModelSpec::InFlight => 2,
            ModelSpec::Flow => 3,
            ModelSpec::Signal { .. } => 4,
            ModelSpec::External { .. } => 5,
        }
    }
}

pub struct CapabilityDef {
    pub name: String,
    pub kinds: Vec<String>,
    pub exclusive: bool,
    pub state_prefix: String,
    pub indicators: Vec<Indicator>,
    pub policy: Policy,
    pub events: u32,
    pub model: ModelSpec,
}

pub struct AttachDef {
    pub block: String,
    pub cap: String,
    pub groups: Vec<String>,
    pub params: Vec<(String, f64)>,
}

#[derive(Debug, Clone)]
pub enum TargetDef {
    Block(String),
    Position(Pos),
}

#[derive(Debug, Clone)]
pub struct EndpointDef {
    pub cap: String,
    pub target: TargetDef,
    pub kinds: Vec<String>,
    pub can_insert: bool,
    pub can_extract: bool,
    pub listens_signals: bool,
    pub listens_messages: bool,
    pub groups: Vec<String>,
    pub priority: i32,
    pub max_packet: u64,
    pub pressure: u32,
    pub handler: u32,
}

/// A snapshot of one network, for tooltips, debugging and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetInfo {
    pub net: NetId,
    pub size: u32,
    pub endpoints: u32,
    pub frozen: bool,
    pub totals: Vec<(ResId, u64)>,
}

#[derive(Default)]
struct Parked {
    seeds: Vec<Pos>,
    sides: FxHashMap<Pos, Vec<(Face, Side)>>,
    colors: FxHashMap<Pos, u8>,
    records: FxHashMap<Pos, Vec<Vec<u8>>>,
    global: Option<Vec<u8>>,
    inbox: FxHashMap<Pos, VecDeque<Vec<u8>>>,
}

struct Slot {
    layer: Layer,
    model: Box<dyn Model>,
    tag: u8,
    ind: Indication,
    inbox: FxHashMap<Pos, VecDeque<Vec<u8>>>,
    latest: FxHashMap<u32, (u32, Vec<u8>)>,
}

pub struct Engine {
    pub reg: Registry,
    slots: Vec<Slot>,
    rng: Rng,
    tick: u64,
    /// Saved state waiting for its capability or its nodes, by capability
    /// name.
    parked: FxHashMap<String, Parked>,
    /// Ticks between indication flushes.
    pub indication_interval: u64,
    region: Vec<Vec<NodeId>>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine {
            reg: Registry::default(),
            slots: Vec::new(),
            rng: Rng(0x9e37_79b9_7f4a_7c15),
            tick: 0,
            parked: FxHashMap::default(),
            indication_interval: indicate::DEFAULT_INTERVAL,
            region: Vec::new(),
        }
    }
}

/// Build a `Cx` for slot `c` out of disjoint engine fields.
macro_rules! cx {
    ($e:ident, $c:expr, $io:expr) => {{
        let slot = &mut $e.slots[$c];
        (
            &mut slot.model,
            Cx {
                cap: CapId($c as u16),
                layer: &mut slot.layer,
                reg: &mut $e.reg,
                io: $io,
                ind: &mut slot.ind,
                rng: &mut $e.rng,
                tick: $e.tick,
            },
        )
    }};
}

fn valid_prefix(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    pub fn layer(&self, cap: CapId) -> &Layer {
        &self.slots[cap.index()].layer
    }

    /// The indicator levels the world shows for `node` of `cap`, in the
    /// capability's indicator order.
    pub fn shown(&self, cap: CapId, node: NodeId) -> &[u8] {
        self.slots[cap.index()].ind.shown(node)
    }

    pub fn model(&self, cap: CapId) -> &dyn Model {
        &*self.slots[cap.index()].model
    }

    pub fn model_mut(&mut self, cap: CapId) -> &mut dyn Model {
        &mut *self.slots[cap.index()].model
    }

    pub fn dim(&mut self, name: &str) -> crate::ids::DimId {
        self.reg.dim(name)
    }

    // ── Registration ─────────────────────────────────────────────────────

    pub fn register_kind(&mut self, kind: Kind) -> Result<KindId, String> {
        self.reg.kinds.register(kind)
    }

    /// Register (or update) a capability. Re-registering a name keeps its
    /// network state and model unless the model mechanic changed.
    pub fn register_capability(&mut self, def: CapabilityDef) -> Result<CapId, String> {
        if !def.name.contains(':') {
            return Err(format!(
                "capability `{}` must be namespaced (`mymod:name`)",
                def.name
            ));
        }
        if !valid_prefix(&def.state_prefix) {
            return Err(format!(
                "state prefix `{}` must be `[a-z0-9_]+`",
                def.state_prefix
            ));
        }
        if let Some(other) = self
            .reg
            .caps
            .iter()
            .find(|c| c.state_prefix == def.state_prefix && c.name != def.name)
        {
            return Err(format!(
                "state prefix `{}` is already used by `{}`",
                def.state_prefix, other.name
            ));
        }
        if def.indicators.len() > MAX_INDICATORS {
            return Err(format!(
                "at most {MAX_INDICATORS} indicators per capability"
            ));
        }
        if let Some(i) = def
            .indicators
            .iter()
            .find(|i| i.levels == 0 || !valid_prefix(&i.name))
        {
            return Err(format!(
                "indicator `{}`: name must be `[a-z0-9_]+` and levels ≥ 1",
                i.name
            ));
        }
        let mut kinds = Vec::new();
        for k in &def.kinds {
            kinds.push(
                self.reg
                    .kinds
                    .id(k)
                    .ok_or_else(|| format!("unknown kind `{k}`"))?,
            );
        }
        let cap = Capability {
            name: def.name.clone(),
            kinds,
            exclusive: def.exclusive,
            state_prefix: def.state_prefix,
            indicators: def.indicators,
            policy: def.policy,
            events: def.events,
        };
        let levels: Vec<u8> = cap.indicators.iter().map(|i| i.levels).collect();
        let tag = def.model.tag();
        let id = match self.reg.cap_id(&def.name) {
            Some(id) => {
                self.reg.caps[id.index()] = cap;
                let slot = &mut self.slots[id.index()];
                slot.ind.levels = levels;
                if slot.tag != tag {
                    slot.model = def.model.build();
                    slot.tag = tag;
                }
                id
            }
            None => {
                self.reg.caps.push(cap);
                self.slots.push(Slot {
                    layer: Layer::default(),
                    model: def.model.build(),
                    tag,
                    ind: Indication::new(levels),
                    inbox: FxHashMap::default(),
                    latest: FxHashMap::default(),
                });
                CapId(self.reg.caps.len() as u16 - 1)
            }
        };
        if let Some(p) = self.parked.get_mut(&def.name) {
            if let Some(g) = p.global.take() {
                self.slots[id.index()].model.load_global(&mut self.reg, &g);
            }
            for (pos, q) in p.inbox.drain() {
                self.slots[id.index()].inbox.insert(pos, q);
            }
        }
        Ok(id)
    }

    /// Give `block` capability `cap`. Unknown parameter names are refused.
    pub fn attach(&mut self, def: AttachDef) -> Result<(), String> {
        let cap = self
            .reg
            .cap_id(&def.cap)
            .ok_or_else(|| format!("unknown capability `{}`", def.cap))?;
        let names = self.slots[cap.index()].model.params();
        let mut params = vec![0.0; names.len()];
        for (k, v) in &def.params {
            let i = names.iter().position(|n| n == k).ok_or_else(|| {
                format!("`{}` has no parameter `{k}` (known: {names:?})", def.cap)
            })?;
            params[i] = *v;
        }
        let bt = self.reg.block_entry(&def.block);
        if self.reg.cap(cap).exclusive
            && let Some(&(other, _)) = self.reg.blocks[bt.index()]
                .attaches
                .iter()
                .find(|&&(c, _)| c != cap && self.reg.cap(c).exclusive)
        {
            return Err(format!(
                "`{}` already carries exclusive capability `{}`",
                def.block,
                self.reg.cap(other).name
            ));
        }
        let mut groups: Vec<u32> = def
            .groups
            .iter()
            .map(|g| self.reg.groups.intern(g.clone()))
            .collect();
        groups.sort_unstable();
        groups.dedup();
        let att = Attach {
            cap,
            groups,
            params,
        };
        let existing = self.reg.blocks[bt.index()]
            .attaches
            .iter()
            .find(|a| a.0 == cap)
            .map(|a| a.1);
        let index = self.slots[cap.index()].layer.set_attach(existing, att);
        if existing.is_none() {
            self.reg.blocks[bt.index()].attaches.push((cap, index));
        }
        Ok(())
    }

    /// Register (or replace) an endpoint type. Kind mismatches are refused
    /// here, never discovered at transfer time.
    pub fn register_endpoint(&mut self, def: EndpointDef) -> Result<EpTypeId, String> {
        let cap = self
            .reg
            .cap_id(&def.cap)
            .ok_or_else(|| format!("unknown capability `{}`", def.cap))?;
        let mut kinds = Vec::new();
        for k in &def.kinds {
            kinds.push(
                self.reg
                    .kinds
                    .id(k)
                    .ok_or_else(|| format!("unknown kind `{k}`"))?,
            );
        }
        let c = self.reg.cap(cap);
        if !kinds.is_empty()
            && !c.kinds.is_empty()
            && !kinds.iter().any(|&k| {
                c.carries(&self.reg.kinds, k)
                    || c.kinds
                        .iter()
                        .any(|&ck| self.reg.kinds.convertible(ck, k).is_some())
            })
        {
            return Err(format!(
                "endpoint kinds {:?} don't meet the kinds of `{}` (directly or by a conversion)",
                def.kinds, def.cap
            ));
        }
        let target = match def.target {
            TargetDef::Block(ref b) => EndpointTarget::Block(self.reg.block_entry(b)),
            TargetDef::Position(p) => EndpointTarget::Position(p),
        };
        let mut groups: Vec<u32> = def
            .groups
            .iter()
            .map(|g| self.reg.groups.intern(g.clone()))
            .collect();
        groups.sort_unstable();
        groups.dedup();
        let ty = EndpointType {
            cap,
            target,
            kinds,
            can_insert: def.can_insert,
            can_extract: def.can_extract,
            listens_signals: def.listens_signals,
            listens_messages: def.listens_messages,
            groups,
            priority: def.priority,
            max_packet: def.max_packet,
            pressure: def.pressure,
            handler: def.handler,
        };
        if let Some(i) = self
            .reg
            .endpoints
            .iter()
            .position(|e| e.cap == cap && e.target == target)
        {
            self.reg.endpoints[i] = ty;
            return Ok(EpTypeId(i as u32));
        }
        let id = EpTypeId(self.reg.endpoints.len() as u32);
        self.reg.endpoints.push(ty);
        match target {
            EndpointTarget::Block(b) => self.reg.blocks[b.index()].endpoints.push(id),
            EndpointTarget::Position(p) => self.reg.positional.entry(p).or_default().push(id),
        }
        Ok(id)
    }

    /// Forget a positional endpoint (a binding the player removed) and
    /// disconnect it.
    pub fn unregister_endpoint(&mut self, io: &mut dyn Io, cap: &str, pos: Pos) {
        let Some(cap) = self.reg.cap_id(cap) else {
            return;
        };
        if let Some(list) = self.reg.positional.get_mut(&pos) {
            list.retain(|&t| self.reg.endpoints[t.index()].cap != cap);
        }
        // The type entry stays (ids are indices); it just targets nothing.
        for e in &mut self.reg.endpoints {
            if e.cap == cap && e.target == EndpointTarget::Position(pos) {
                e.can_insert = false;
                e.can_extract = false;
                e.listens_messages = false;
                e.listens_signals = false;
            }
        }
        let _ = io;
        for n in self.slots[cap.index()].layer.remove_endpoint(pos) {
            self.slots[cap.index()].ind.net_changed(n);
        }
    }

    pub fn add_filter(&mut self, f: Filter) -> crate::ids::FilterId {
        self.reg.filters.add(f)
    }

    // ── World events ─────────────────────────────────────────────────────

    /// A block appeared (or changed) at `pos`: discover what it connects.
    pub fn place(&mut self, io: &mut dyn Io, probe: &mut dyn Probe, pos: Pos) {
        self.discover(io, probe, pos);
    }

    /// The block at `pos` is gone.
    pub fn remove(&mut self, io: &mut dyn Io, pos: Pos) {
        for c in 0..self.slots.len() {
            let (model, mut cx) = cx!(self, c, io);
            model.forget(&mut cx, pos);
            if let Some(node) = cx.layer.node_at(pos) {
                let net = cx.layer.net_of(node).unwrap();
                model.node_removed(&mut cx, node, net);
                let split = cx.layer.remove_node(node);
                cx.ind.forget(node);
                Self::after_split(model, &mut cx, split);
            }
            if cx.layer.endpoint_at(pos).is_some() {
                for n in cx.layer.remove_endpoint(pos) {
                    cx.ind.net_changed(n);
                }
            }
        }
    }

    /// Re-read the block at `pos` (changed by `/setblock` or a mod).
    pub fn refresh(&mut self, io: &mut dyn Io, probe: &mut dyn Probe, pos: Pos) {
        let now = probe.block(&self.reg, pos);
        let known = self
            .slots
            .iter()
            .any(|s| s.layer.node_at(pos).is_some() || s.layer.endpoint_at(pos).is_some());
        if known {
            self.remove(io, pos);
        }
        if now.is_some() {
            self.discover(io, probe, pos);
        }
    }

    /// Re-check every known node and endpoint within `radius` of `center`
    /// (explosions remove blocks without per-block events).
    pub fn revalidate_area(
        &mut self,
        io: &mut dyn Io,
        probe: &mut dyn Probe,
        center: Pos,
        radius: i32,
    ) {
        let mut gone = Vec::new();
        for s in &self.slots {
            for (_, n) in s.layer.nodes() {
                let p = n.pos;
                if p.dim == center.dim
                    && (p.x - center.x).abs() <= radius
                    && (p.y - center.y).abs() <= radius
                    && (p.z - center.z).abs() <= radius
                {
                    gone.push(p);
                }
            }
        }
        gone.sort();
        gone.dedup();
        for p in gone {
            let bt = probe.block(&self.reg, p);
            let still = bt.is_some_and(|b| {
                self.slots.iter().enumerate().all(|(c, s)| {
                    s.layer.node_at(p).is_none()
                        || self.reg.blocks[b.index()]
                            .attaches
                            .iter()
                            .any(|a| a.0.index() == c)
                })
            });
            if !still {
                self.remove(io, p);
                if bt.is_some() {
                    self.discover(io, probe, p);
                }
            }
        }
    }

    pub fn set_chunk_loaded(&mut self, dim: crate::ids::DimId, cx_: i32, cz: i32, loaded: bool) {
        let key = ChunkKey { dim, x: cx_, z: cz };
        for s in &mut self.slots {
            s.layer.set_chunk_loaded(key, loaded);
        }
    }

    fn after_split(model: &mut Box<dyn Model>, cx: &mut Cx, split: Split) {
        let Some(net) = split.net else {
            return;
        };
        if split.emptied {
            model.net_removed(cx, net);
            return;
        }
        if !split.parts.is_empty() {
            model.split(cx, net, &split.parts);
            cx.ind.net_changed(net);
            for &(p, _) in &split.parts {
                cx.ind.net_changed(p);
            }
        }
    }

    fn after_settle(
        model: &mut Box<dyn Model>,
        cx: &mut Cx,
        s: &Settled,
        nodes: &[NodeId],
        parked: Option<&mut Parked>,
    ) {
        let Some(net) = s.net else {
            return;
        };
        for &(from, _) in &s.absorbed {
            model.merged(cx, net, from);
        }
        if !s.absorbed.is_empty() {
            cx.ind.net_changed(net);
        }
        for &n in nodes {
            model.node_added(cx, n, net);
        }
        if let Some(p) = parked
            && !p.records.is_empty()
        {
            for &n in nodes {
                let pos = cx.layer.node(n).pos;
                if let Some(list) = p.records.remove(&pos) {
                    let mut left = Vec::new();
                    for bytes in list {
                        // The network may have changed id through merges
                        // caused by earlier restores: look it up again.
                        let net = cx.layer.net_of(n).unwrap();
                        if !model.restore(cx, n, net, &bytes) {
                            left.push(bytes);
                        }
                    }
                    if !left.is_empty() {
                        p.records.insert(pos, left);
                    }
                }
            }
        }
    }

    /// BFS over unknown positions from `start`, adding nodes to every
    /// layer their block belongs to, then linking, settling networks and
    /// connecting endpoints.
    fn discover(&mut self, io: &mut dyn Io, probe: &mut dyn Probe, start: Pos) {
        let n_caps = self.slots.len();
        let mut region = std::mem::take(&mut self.region);
        region.resize_with(n_caps, Vec::new);
        for r in &mut region {
            r.clear();
        }
        let mut probed: FxHashMap<Pos, Option<BlockTypeId>> = FxHashMap::default();
        let mut queue: VecDeque<Pos> = VecDeque::from([start]);
        let mut endpoints: Vec<Pos> = Vec::new();
        probed.insert(start, probe.block(&self.reg, start));
        let mut found = 0usize;

        while let Some(pos) = queue.pop_front() {
            let bt = probed[&pos];
            let mut grew = false;
            if let Some(b) = bt {
                for k in 0..self.reg.blocks[b.index()].attaches.len() {
                    let (cap, att) = self.reg.blocks[b.index()].attaches[k];
                    let c = cap.index();
                    let layer = &mut self.slots[c].layer;
                    if layer.node_at(pos).is_some() {
                        continue;
                    }
                    let id = layer.insert_node(pos, att);
                    if let Some(p) = self.parked.get_mut(&self.reg.caps[c].name) {
                        if let Some(sides) = p.sides.remove(&pos) {
                            for (f, s) in sides {
                                layer.set_side_raw(id, f, s);
                            }
                        }
                        if let Some(col) = p.colors.remove(&pos) {
                            layer.set_color_raw(id, col);
                        }
                    }
                    region[c].push(id);
                    if !grew {
                        found += 1;
                    }
                    grew = true;
                }
            }
            if bt.is_some() || self.reg.positional.contains_key(&pos) {
                endpoints.push(pos);
            }
            if grew {
                for f in Face::ALL {
                    let np = pos.offset(f);
                    if probed.contains_key(&np) {
                        continue;
                    }
                    if found >= MAX_DISCOVERY {
                        break;
                    }
                    let nb = probe.block(&self.reg, np);
                    probed.insert(np, nb);
                    // Expand into blocks with a capability not yet known
                    // there; endpoints are noted without expanding.
                    match nb {
                        Some(b)
                            if self.reg.blocks[b.index()].attaches.iter().any(|&(c, _)| {
                                self.slots[c.index()].layer.node_at(np).is_none()
                            }) =>
                        {
                            queue.push_back(np);
                        }
                        _ => {
                            if nb.is_some() || self.reg.positional.contains_key(&np) {
                                endpoints.push(np);
                            }
                        }
                    }
                }
            }
        }

        for (c, nodes) in region.iter().enumerate() {
            if nodes.is_empty() {
                continue;
            }
            // Link the new nodes to every linkable neighbour.
            {
                let layer = &mut self.slots[c].layer;
                for &id in nodes {
                    let pos = layer.node(id).pos;
                    for f in Face::ALL {
                        if let Some(nb) = layer.node_at(pos.offset(f))
                            && layer.linkable(id, f, nb)
                        {
                            layer.link_raw(id, f, nb);
                        }
                    }
                }
            }
            // Settle each connected piece of new nodes.
            let name = self.reg.caps[c].name.clone();
            let mut parked = self.parked.remove(&name);
            let mut comp = Vec::new();
            for &id in nodes {
                if self.slots[c].layer.net_of(id).is_some() {
                    continue;
                }
                comp.clear();
                comp.push(id);
                let mut seen: FxHashSet<NodeId> = FxHashSet::from_iter([id]);
                let mut i = 0;
                while i < comp.len() {
                    let cur = comp[i];
                    i += 1;
                    for l in self.slots[c].layer.node(cur).links {
                        if l != crate::ids::NO_NODE
                            && self.slots[c].layer.net_of(l).is_none()
                            && seen.insert(l)
                        {
                            comp.push(l);
                        }
                    }
                }
                let (model, mut cx) = cx!(self, c, io);
                let settled = cx.layer.settle(&comp);
                Self::after_settle(model, &mut cx, &settled, &comp, parked.as_mut());
            }
            if let Some(p) = parked {
                self.parked.insert(name, p);
            }
        }

        // Connect endpoints: every endpoint position next to a node.
        endpoints.sort();
        endpoints.dedup();
        for &ep in &endpoints {
            let bt = probed.get(&ep).copied().flatten();
            self.connect_endpoint(ep, bt);
        }
        // New nodes next to endpoints that were not probed this time.
        for (c, nodes) in region.iter().enumerate() {
            for &id in nodes {
                let pos = self.slots[c].layer.node(id).pos;
                for f in Face::ALL {
                    let np = pos.offset(f);
                    if probed.contains_key(&np) {
                        continue;
                    }
                    let bt = probe.block(&self.reg, np);
                    probed.insert(np, bt);
                    self.connect_endpoint(np, bt);
                }
            }
        }
        self.region = region;
    }

    /// Connect the block at `pos` (type `bt`) as an endpoint of every
    /// capability it is one for, to every adjacent node that takes it.
    fn connect_endpoint(&mut self, pos: Pos, bt: Option<BlockTypeId>) {
        for c in 0..self.slots.len() {
            let cap = CapId(c as u16);
            let Some(ty) = self.reg.endpoint_type(cap, pos, bt) else {
                continue;
            };
            let ept = &self.reg.endpoints[ty.index()];
            let layer = &mut self.slots[c].layer;
            if layer.node_at(pos).is_some() {
                // A pipe of this capability is not its own endpoint.
                continue;
            }
            for f in Face::ALL {
                let Some(node) = layer.node_at(pos.offset(f)) else {
                    continue;
                };
                let face = f.opposite();
                if layer.side(node, face).mode == SideMode::Off
                    || !groups_meet(
                        &layer.atts[layer.node(node).att as usize].groups,
                        &ept.groups,
                    )
                {
                    continue;
                }
                let ep = layer.ensure_endpoint(pos, ty);
                layer.add_port(ep, node, face);
            }
        }
    }

    // ── Configuration ────────────────────────────────────────────────────

    /// Configure the face `face` of the `cap` node at `pos`.
    pub fn set_side(
        &mut self,
        io: &mut dyn Io,
        probe: &mut dyn Probe,
        cap: CapId,
        pos: Pos,
        face: Face,
        side: Side,
    ) {
        let c = cap.index();
        let Some(node) = self.slots[c].layer.node_at(pos) else {
            return;
        };
        let old = self.slots[c].layer.side(node, face);
        self.slots[c].layer.set_side_raw(node, face, side);
        if (old.mode == SideMode::Off) != (side.mode == SideMode::Off) {
            self.relink(io, probe, c, node, face);
        }
    }

    /// Set the color of the `cap` node at `pos` (0 = uncolored) and
    /// re-evaluate its links.
    pub fn set_color(
        &mut self,
        io: &mut dyn Io,
        probe: &mut dyn Probe,
        cap: CapId,
        pos: Pos,
        color: u8,
    ) {
        let c = cap.index();
        let Some(node) = self.slots[c].layer.node_at(pos) else {
            return;
        };
        self.slots[c].layer.set_color_raw(node, color);
        for f in Face::ALL {
            self.relink(io, probe, c, node, f);
        }
    }

    /// Make the link (or port) on `node`'s `face` match the rules again.
    fn relink(
        &mut self,
        io: &mut dyn Io,
        probe: &mut dyn Probe,
        c: usize,
        node: NodeId,
        face: Face,
    ) {
        let pos = self.slots[c].layer.node(node).pos;
        let np = pos.offset(face);
        let linked = self.slots[c].layer.node(node).links[face as usize];
        match self.slots[c].layer.node_at(np) {
            Some(nb) => {
                let ok = self.slots[c].layer.linkable(node, face, nb);
                if linked != crate::ids::NO_NODE && !ok {
                    let (model, mut cx) = cx!(self, c, io);
                    let split = cx.layer.unlink(node, face);
                    Self::after_split(model, &mut cx, split);
                } else if linked == crate::ids::NO_NODE && ok {
                    let (model, mut cx) = cx!(self, c, io);
                    if let Some((into, from, _)) = cx.layer.link_settled(node, face, nb) {
                        model.merged(&mut cx, into, from);
                        cx.ind.net_changed(into);
                    }
                }
            }
            None => {
                let bt = probe.block(&self.reg, np);
                self.connect_endpoint(np, bt);
            }
        }
    }

    // ── Scheduling ───────────────────────────────────────────────────────

    /// A machine at (or a pipe at) `pos` has something to give or room to
    /// take: tick its networks now.
    pub fn wake(&mut self, pos: Pos) {
        let tick = self.tick;
        for s in &mut self.slots {
            let l = &mut s.layer;
            let mut nets: Vec<NetId> = Vec::new();
            if let Some(n) = l.node_at(pos) {
                nets.extend(l.net_of(n));
            }
            for f in Face::ALL {
                if let Some(n) = l.node_at(pos.offset(f)) {
                    nets.extend(l.net_of(n));
                }
            }
            for n in nets {
                l.activate(n, tick);
            }
        }
    }

    pub fn tick(&mut self, io: &mut dyn Io) {
        self.tick += 1;
        let tick = self.tick;
        for c in 0..self.slots.len() {
            let mut list = std::mem::take(&mut self.slots[c].layer.active);
            list.sort_unstable();
            list.dedup();
            let mut keep = Vec::with_capacity(list.len());
            for idx in list {
                let layer = &mut self.slots[c].layer;
                let id = layer.net_id(idx);
                let Some(n) = layer.net_mut(id) else {
                    continue;
                };
                if !n.active {
                    continue;
                }
                if n.frozen > 0 {
                    n.active = false;
                    continue;
                }
                if n.wake_at > tick {
                    keep.push(idx);
                    continue;
                }
                let (model, mut cx) = cx!(self, c, io);
                let act = model.tick(&mut cx, id);
                let Some(n) = cx.layer.net_mut(id) else {
                    continue;
                };
                match act {
                    Activity::Active => {
                        n.wake_at = tick + 1;
                        keep.push(idx);
                    }
                    Activity::Sleep(t) => {
                        n.wake_at = tick + t.max(1) as u64;
                        keep.push(idx);
                    }
                    Activity::Idle => n.active = false,
                }
            }
            let slot = &mut self.slots[c];
            keep.append(&mut slot.layer.active);
            slot.layer.active = keep;
            if tick.is_multiple_of(self.indication_interval.max(1)) && !slot.ind.is_empty() {
                let reg = &self.reg;
                let cap = CapId(c as u16);
                slot.ind.flush(&mut slot.layer, |pos, i, v| {
                    io.indicate(reg, cap, pos, i, v)
                });
            }
            if tick.is_multiple_of(PRUNE_EVERY) {
                slot.layer.prune_caches();
            }
        }
    }

    // ── Signals ──────────────────────────────────────────────────────────

    pub fn emit(&mut self, io: &mut dyn Io, cap: CapId, pos: Pos, channel: u8, level: u8) {
        let c = cap.index();
        let (model, mut cx) = cx!(self, c, io);
        model.emit(&mut cx, pos, channel, level);
        self.wake_cap(c, pos);
    }

    fn wake_cap(&mut self, c: usize, pos: Pos) {
        let tick = self.tick;
        let l = &mut self.slots[c].layer;
        let mut nets = Vec::new();
        if let Some(n) = l.node_at(pos) {
            nets.extend(l.net_of(n));
        }
        for f in Face::ALL {
            if let Some(n) = l.node_at(pos.offset(f)) {
                nets.extend(l.net_of(n));
            }
        }
        for n in nets {
            l.activate(n, tick);
        }
    }

    pub fn level(&self, cap: CapId, pos: Pos, channel: u8) -> u8 {
        let s = &self.slots[cap.index()];
        s.model.level(&s.layer, pos, channel)
    }

    // ── Messaging ────────────────────────────────────────────────────────

    /// The network a position belongs to: its node, else the first
    /// network its endpoint is connected to.
    pub fn net_at(&self, cap: CapId, pos: Pos) -> Option<NetId> {
        let l = &self.slots[cap.index()].layer;
        if let Some(n) = l.node_at(pos) {
            return l.net_of(n);
        }
        let ep = l.endpoint_at(pos)?;
        Face::ALL.into_iter().find_map(|f| {
            let n = l.node_at(pos.offset(f))?;
            let net = l.net_of(n)?;
            l.net(net)?
                .ports
                .iter()
                .any(|p| p.ep == ep && p.node == n)
                .then_some(net)
        })
    }

    /// Deliver `payload` from `from` to `to` if both are on one network:
    /// to `to`'s `on_message` handler, else its inbox. `false` when not
    /// connected or the inbox is full (nothing is overwritten).
    pub fn post(
        &mut self,
        io: &mut dyn Io,
        cap: CapId,
        from: Pos,
        to: Pos,
        payload: &[u8],
    ) -> bool {
        let (Some(a), Some(b)) = (self.net_at(cap, from), self.net_at(cap, to)) else {
            return false;
        };
        if a != b {
            return false;
        }
        let c = cap.index();
        if let Some(call) = self.message_call(c, to, b)
            && io.message(&self.reg, &call, from, payload)
        {
            return true;
        }
        let q = self.slots[c].inbox.entry(to).or_default();
        if q.len() >= INBOX {
            return false;
        }
        q.push_back(payload.to_vec());
        true
    }

    fn message_call(&self, c: usize, to: Pos, net: NetId) -> Option<Call> {
        let l = &self.slots[c].layer;
        let ep = l.endpoint_at(to)?;
        let ty = self.reg.endpoint(l.endpoint(ep).ty);
        if !ty.listens_messages {
            return None;
        }
        let p = l.net(net)?.ports.iter().find(|p| p.ep == ep)?;
        Some(Call {
            cap: CapId(c as u16),
            ty: l.endpoint(ep).ty,
            handler: ty.handler,
            at: to,
            side: p.face.opposite(),
            pipe: l.node(p.node).pos,
        })
    }

    /// Send `payload` to every `on_message` endpoint of `from`'s network
    /// (except `from`) and keep it as the network's latest value. Returns
    /// how many endpoints received it.
    pub fn broadcast(&mut self, io: &mut dyn Io, cap: CapId, from: Pos, payload: &[u8]) -> u32 {
        let Some(net) = self.net_at(cap, from) else {
            return 0;
        };
        let c = cap.index();
        self.slots[c]
            .latest
            .insert(net.index, (net.generation, payload.to_vec()));
        let l = &self.slots[c].layer;
        let mut targets: Vec<Pos> = l
            .net(net)
            .map(|n| n.ports.iter().map(|p| l.endpoint(p.ep).pos).collect())
            .unwrap_or_default();
        targets.sort();
        targets.dedup();
        let mut sent = 0;
        for to in targets {
            if to == from {
                continue;
            }
            if let Some(call) = self.message_call(c, to, net)
                && io.message(&self.reg, &call, from, payload)
            {
                sent += 1;
            }
        }
        sent
    }

    /// Take the oldest message posted to `pos`.
    pub fn receive(&mut self, cap: CapId, pos: Pos) -> Option<Vec<u8>> {
        let inbox = &mut self.slots[cap.index()].inbox;
        let q = inbox.get_mut(&pos)?;
        let m = q.pop_front();
        if q.is_empty() {
            inbox.remove(&pos);
        }
        m
    }

    /// The last broadcast on `pos`'s network.
    pub fn latest(&self, cap: CapId, pos: Pos) -> Option<&[u8]> {
        let net = self.net_at(cap, pos)?;
        let (g, v) = self.slots[cap.index()].latest.get(&net.index)?;
        (*g == net.generation).then_some(v.as_slice())
    }

    // ── Queries ──────────────────────────────────────────────────────────

    pub fn net_info(&self, cap: CapId, pos: Pos) -> Option<NetInfo> {
        let net = self.net_at(cap, pos)?;
        let s = &self.slots[cap.index()];
        let n = s.layer.net(net)?;
        let mut totals = Vec::new();
        s.model.totals(&s.layer, net, &mut totals);
        Some(NetInfo {
            net,
            size: n.size,
            endpoints: n.ports.len() as u32,
            frozen: n.frozen > 0,
            totals,
        })
    }

    /// Every network of a capability with its totals (conservation tests).
    pub fn totals(&self, cap: CapId) -> Vec<(ResId, u64)> {
        let s = &self.slots[cap.index()];
        let mut out = Vec::new();
        for (net, _) in s.layer.nets() {
            s.model.totals(&s.layer, net, &mut out);
        }
        let mut merged: Vec<(ResId, u64)> = Vec::new();
        for (r, a) in out {
            match merged.iter_mut().find(|m| m.0 == r) {
                Some(m) => m.1 += a,
                None => merged.push((r, a)),
            }
        }
        merged
    }

    /// Indicator values shown at `pos`: `(property name, value)`.
    pub fn indication(&self, pos: Pos) -> Vec<(String, u8)> {
        let mut out = Vec::new();
        for (c, s) in self.slots.iter().enumerate() {
            let Some(n) = s.layer.node_at(pos) else {
                continue;
            };
            let cap = &self.reg.caps[c];
            for (i, &v) in s.ind.shown(n).iter().enumerate() {
                out.push((
                    format!("{}_{}", cap.state_prefix, cap.indicators[i].name),
                    v,
                ));
            }
        }
        out
    }

    // ── Persistence ──────────────────────────────────────────────────────

    pub fn save(&self) -> Vec<u8> {
        let reg = &self.reg;
        let mut e = Enc::default();
        e.buf.extend_from_slice(MAGIC);
        e.u32(FORMAT);
        let mut names: Vec<&str> = reg.caps.iter().map(|c| c.name.as_str()).collect();
        for k in self.parked.keys() {
            if !names.contains(&k.as_str()) {
                names.push(k);
            }
        }
        e.u32(names.len() as u32);
        for name in names {
            e.str(name);
            let slot = reg.cap_id(name).map(|c| &self.slots[c.index()]);
            let parked = self.parked.get(name);
            // Seeds: one node per network, plus seeds not yet rediscovered.
            let mut seeds: Vec<Pos> = parked.map(|p| p.seeds.clone()).unwrap_or_default();
            if let Some(s) = slot {
                seeds.extend(s.layer.nets().map(|(_, n)| s.layer.node(n.any_node).pos));
            }
            e.u32(seeds.len() as u32);
            for p in seeds {
                e.pos(reg, p);
            }
            let mut sides: Vec<(Pos, Face, Side)> = Vec::new();
            let mut colors: Vec<(Pos, u8)> = Vec::new();
            let mut records: Vec<Record> = Vec::new();
            if let Some(s) = slot {
                sides.extend(s.layer.configured_sides());
                colors.extend(
                    s.layer
                        .nodes()
                        .filter(|(_, n)| n.color != 0)
                        .map(|(_, n)| (n.pos, n.color)),
                );
                s.model.save(&s.layer, reg, &mut records);
            }
            if let Some(p) = parked {
                for (pos, list) in &p.sides {
                    sides.extend(list.iter().map(|&(f, s)| (*pos, f, s)));
                }
                colors.extend(p.colors.iter().map(|(&p, &c)| (p, c)));
                for (pos, list) in &p.records {
                    records.extend(list.iter().map(|b| Record {
                        pos: *pos,
                        bytes: b.clone(),
                    }));
                }
            }
            e.u32(sides.len() as u32);
            for (p, f, s) in sides {
                e.pos(reg, p)
                    .face(f)
                    .u8(s.mode as u8)
                    .u8(s.disabled as u8)
                    .i32(s.priority)
                    .u8(s.color);
                match s.filter {
                    Some(id) => {
                        e.u8(1);
                        self.save_filter(&mut e, reg.filters.get(id));
                    }
                    None => {
                        e.u8(0);
                    }
                }
            }
            e.u32(colors.len() as u32);
            for (p, c) in colors {
                e.pos(reg, p).u8(c);
            }
            e.u32(records.len() as u32);
            for r in records {
                e.pos(reg, r.pos).bytes(&r.bytes);
            }
            let global = match (slot, parked.and_then(|p| p.global.as_ref())) {
                (Some(s), _) => s.model.save_global(reg),
                (None, Some(g)) => g.clone(),
                _ => Vec::new(),
            };
            e.bytes(&global);
            let mut inbox: Vec<(Pos, &VecDeque<Vec<u8>>)> = Vec::new();
            if let Some(s) = slot {
                inbox.extend(s.inbox.iter().map(|(p, q)| (*p, q)));
            }
            if let Some(p) = parked {
                inbox.extend(p.inbox.iter().map(|(p, q)| (*p, q)));
            }
            e.u32(inbox.len() as u32);
            for (p, q) in inbox {
                e.pos(reg, p).u32(q.len() as u32);
                for m in q {
                    e.bytes(m);
                }
            }
        }
        e.buf
    }

    fn save_filter(&self, e: &mut Enc, f: &Filter) {
        e.u8(f.mode as u8).u32(f.entries.len() as u32);
        for entry in &f.entries {
            match entry {
                FilterEntry::Resource(r) => e.u8(0).res(&self.reg, *r),
                FilterEntry::Id(id) => e.u8(1).str(id),
                FilterEntry::Kind(k) => e.u8(2).str(&self.reg.kinds.get(*k).name),
                FilterEntry::Color(c) => e.u8(3).u8(*c),
            };
        }
    }

    fn load_filter(reg: &mut Registry, d: &mut Dec) -> Option<Filter> {
        let mode = match d.u8()? {
            0 => FilterMode::Allow,
            _ => FilterMode::Deny,
        };
        let n = d.u32()?;
        let mut entries = Vec::new();
        for _ in 0..n {
            match d.u8()? {
                0 => {
                    if let Some(r) = d.res(reg) {
                        entries.push(FilterEntry::Resource(r));
                    }
                }
                1 => entries.push(FilterEntry::Id(d.str()?.into())),
                2 => {
                    if let Some(k) = reg.kinds.id(d.str()?) {
                        entries.push(FilterEntry::Kind(k));
                    }
                }
                _ => entries.push(FilterEntry::Color(d.u8()?)),
            }
        }
        Some(Filter { mode, entries })
    }

    /// Park a save: state is handed to capabilities as they register and
    /// to nodes as they are rediscovered (call [`Engine::rediscover`]).
    pub fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() < 8 || &bytes[..4] != MAGIC {
            return Err("not a yog-pipes transport save".into());
        }
        let mut d = Dec::new(&bytes[4..]);
        let bad = || "truncated yog-pipes save".to_string();
        let version = d.u32().ok_or_else(bad)?;
        if version != FORMAT {
            return Err(format!("unsupported save format {version}"));
        }
        let caps = d.u32().ok_or_else(bad)?;
        for _ in 0..caps {
            let name = d.str().ok_or_else(bad)?.to_string();
            let mut p = Parked::default();
            for _ in 0..d.u32().ok_or_else(bad)? {
                p.seeds.push(d.pos(&mut self.reg).ok_or_else(bad)?);
            }
            for _ in 0..d.u32().ok_or_else(bad)? {
                let pos = d.pos(&mut self.reg).ok_or_else(bad)?;
                let face = d.face().ok_or_else(bad)?;
                let mode = SideMode::from_index(d.u8().ok_or_else(bad)?);
                let disabled = d.u8().ok_or_else(bad)? != 0;
                let priority = d.i32().ok_or_else(bad)?;
                let color = d.u8().ok_or_else(bad)?;
                let filter = match d.u8().ok_or_else(bad)? {
                    1 => {
                        let f = Self::load_filter(&mut self.reg, &mut d).ok_or_else(bad)?;
                        Some(self.reg.filters.add(f))
                    }
                    _ => None,
                };
                p.sides.entry(pos).or_default().push((
                    face,
                    Side {
                        mode,
                        disabled,
                        priority,
                        filter,
                        color,
                    },
                ));
            }
            for _ in 0..d.u32().ok_or_else(bad)? {
                let pos = d.pos(&mut self.reg).ok_or_else(bad)?;
                p.colors.insert(pos, d.u8().ok_or_else(bad)?);
            }
            for _ in 0..d.u32().ok_or_else(bad)? {
                let pos = d.pos(&mut self.reg).ok_or_else(bad)?;
                p.records
                    .entry(pos)
                    .or_default()
                    .push(d.bytes().ok_or_else(bad)?.to_vec());
            }
            let global = d.bytes().ok_or_else(bad)?;
            if !global.is_empty() {
                p.global = Some(global.to_vec());
            }
            for _ in 0..d.u32().ok_or_else(bad)? {
                let pos = d.pos(&mut self.reg).ok_or_else(bad)?;
                let mut q = VecDeque::new();
                for _ in 0..d.u32().ok_or_else(bad)? {
                    q.push_back(d.bytes().ok_or_else(bad)?.to_vec());
                }
                p.inbox.insert(pos, q);
            }
            if let Some(id) = self.reg.cap_id(&name) {
                if let Some(g) = p.global.take() {
                    self.slots[id.index()].model.load_global(&mut self.reg, &g);
                }
                for (pos, q) in p.inbox.drain() {
                    self.slots[id.index()].inbox.insert(pos, q);
                }
            }
            self.parked.insert(name, p);
        }
        Ok(())
    }

    /// Rediscover saved networks of registered capabilities from their
    /// seeds. Call after capabilities are registered (e.g. on the first
    /// server tick); cheap when there is nothing left to do.
    pub fn rediscover(&mut self, io: &mut dyn Io, probe: &mut dyn Probe) {
        let mut seeds = Vec::new();
        for c in &self.reg.caps {
            if let Some(p) = self.parked.get_mut(&c.name) {
                seeds.append(&mut p.seeds);
            }
        }
        for s in seeds {
            if !self.slots.iter().any(|sl| sl.layer.node_at(s).is_some()) {
                self.discover(io, probe, s);
            }
        }
    }

    /// Saved records still waiting for their node (e.g. a pipe removed
    /// while the server was down). Kept and re-saved, never invented.
    pub fn parked_records(&self) -> usize {
        self.parked
            .values()
            .map(|p| p.records.values().map(Vec::len).sum::<usize>())
            .sum()
    }

    /// Forget everything about the world (server stopping). Registrations
    /// stay.
    pub fn clear_world(&mut self) {
        for s in &mut self.slots {
            s.layer = {
                let mut l = Layer::default();
                for a in &s.layer.atts {
                    l.set_attach(None, a.clone());
                }
                l
            };
            s.ind = Indication::new(s.ind.levels.clone());
            s.inbox.clear();
            s.latest.clear();
            s.model.reset();
        }
        self.parked.clear();
        self.tick = 0;
    }
}
