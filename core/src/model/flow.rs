//! `flow` — a volume per pipe segment, moving toward lower head (§5.5):
//! BuildCraft fluid pipes, Create fluid networks with pumps, Thermal
//! fluiducts in per-segment style.
//!
//! Each tick, for the **active** segments only, `min(rate, ⌊Δh/2⌋)` moves
//! to each linked neighbour with a lower head and a compatible resource,
//! where `h = amount + head · potential`. Potentials come from pumps
//! (endpoints with `pressure > 0`: `potential = pressure − distance`,
//! multi-source BFS cached per network version), so flow runs away from a
//! pump for `pressure` blocks like Create; without pumps it is plain
//! equalisation (BuildCraft). A segment with no transfer leaves the active
//! set, so a settled network costs nothing.
//!
//! One resource per segment (several per network, design choice D5).
//! Conservation: every transfer is an integer move between two segments
//! or one segment and one endpoint call.

use crate::ids::{EpId, FxHashMap, NO_NODE, NetId, NodeId, ResId};
use crate::layer::{Layer, Port};
use crate::model::{Activity, Cx, Model, NetMap, Record, Want};
use crate::persist::{Dec, Enc};
use crate::registry::{IndicatorSource, Registry};

const CAPACITY: usize = 0;
const RATE: usize = 1;
const HEAD: usize = 2;
const PARAMS: &[&str] = &["capacity", "rate", "head"];

const POLL: u32 = 10;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub res: Option<ResId>,
    pub amount: u64,
}

#[derive(Debug, Default, Clone)]
struct Net {
    active: Vec<NodeId>,
    /// Pump potentials, valid for `version`.
    potential: FxHashMap<NodeId, u32>,
    version: u32,
}

#[derive(Default)]
pub struct Flow {
    segs: Vec<Segment>,
    queued: Vec<bool>,
    nets: NetMap<Net>,
    ports: Vec<Port>,
    pulled: Vec<EpId>,
}

impl Flow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn segment(&self, node: NodeId) -> Segment {
        self.segs.get(node.index()).copied().unwrap_or_default()
    }

    fn grow(&mut self, node: NodeId) {
        if self.segs.len() <= node.index() {
            self.segs.resize(node.index() + 1, Segment::default());
            self.queued.resize(node.index() + 1, false);
        }
    }

    fn wake(&mut self, net: NetId, node: NodeId) {
        self.grow(node);
        if !self.queued[node.index()] {
            self.queued[node.index()] = true;
            self.nets.entry(net).active.push(node);
        }
    }

    fn show(&self, cx: &mut Cx, node: NodeId) {
        if let Some(i) = cx.indicator(IndicatorSource::Fill) {
            let cap = cx.layer.param(node, CAPACITY) as u64;
            cx.indicate_node(node, i, self.segs[node.index()].amount, cap.max(1));
        }
    }

    fn potentials(&mut self, cx: &mut Cx, net: NetId) {
        let Some(n) = cx.layer.net(net) else {
            return;
        };
        let version = n.version;
        let st = self.nets.entry(net);
        if st.version == version && version != 0 {
            return;
        }
        st.version = version;
        st.potential.clear();
        let mut queue: std::collections::VecDeque<(NodeId, u32)> = Default::default();
        for p in &n.ports {
            let pressure = cx.reg.endpoint(cx.layer.endpoint(p.ep).ty).pressure;
            if pressure > 0 {
                let e = st.potential.entry(p.node).or_insert(0);
                if pressure > *e {
                    *e = pressure;
                    queue.push_back((p.node, pressure));
                }
            }
        }
        while let Some((c, v)) = queue.pop_front() {
            if v <= 1 || st.potential.get(&c).copied() != Some(v) {
                continue;
            }
            for l in cx.layer.node(c).links {
                if l == NO_NODE {
                    continue;
                }
                let e = st.potential.entry(l).or_insert(0);
                if v - 1 > *e {
                    *e = v - 1;
                    queue.push_back((l, v - 1));
                }
            }
        }
    }
}

impl Model for Flow {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn params(&self) -> &'static [&'static str] {
        PARAMS
    }

    fn node_added(&mut self, _cx: &mut Cx, node: NodeId, net: NetId) {
        self.grow(node);
        self.segs[node.index()] = Segment::default();
        self.queued[node.index()] = false;
        self.wake(net, node);
    }

    fn node_removed(&mut self, cx: &mut Cx, node: NodeId, net: NetId) {
        self.grow(node);
        let s = std::mem::take(&mut self.segs[node.index()]);
        if let Some(res) = s.res {
            cx.spill(cx.layer.node(node).pos, res, s.amount);
        }
        // Neighbours may flow into the gap's former partners differently.
        for l in cx.layer.node(node).links {
            if l != NO_NODE {
                self.wake(net, l);
            }
        }
        self.queued[node.index()] = false;
        if let Some(st) = self.nets.get(net) {
            let active: Vec<NodeId> = st.active.iter().copied().filter(|&n| n != node).collect();
            self.nets.entry(net).active = active;
        }
        cx.ind.forget(node);
    }

    fn merged(&mut self, _cx: &mut Cx, into: NetId, from: NetId) {
        if let Some(f) = self.nets.take(from) {
            self.nets.entry(into).active.extend(f.active);
        }
    }

    fn split(&mut self, cx: &mut Cx, from: NetId, _parts: &[(NetId, u32)]) {
        let active = std::mem::take(&mut self.nets.entry(from).active);
        for n in active {
            let net = cx.layer.net_of(n).unwrap_or(from);
            self.nets.entry(net).active.push(n);
        }
    }

    fn net_removed(&mut self, _cx: &mut Cx, net: NetId) {
        self.nets.take(net);
    }

    fn tick(&mut self, cx: &mut Cx, net: NetId) -> Activity {
        let Some(n) = cx.layer.net(net) else {
            return Activity::Idle;
        };
        self.ports.clear();
        self.ports.extend_from_slice(&n.ports);
        self.potentials(cx, net);
        let mut moved = false;

        // Endpoints: sources fill their segment, sinks drain theirs.
        self.pulled.clear();
        for k in 0..self.ports.len() {
            let p = self.ports[k];
            self.grow(p.node);
            let cap = cx.layer.param(p.node, CAPACITY) as u64;
            let rate = cx.layer.param(p.node, RATE) as u64;
            let s = self.segs[p.node.index()];
            if cx.source(p) && s.amount < cap {
                let want = s.res.map_or(Want::Any, Want::Res);
                if let Some((res, got)) = cx.extract(p, want, rate.min(cap - s.amount), false) {
                    let s = &mut self.segs[p.node.index()];
                    s.res = Some(res);
                    s.amount += got;
                    self.pulled.push(p.ep);
                    self.wake(net, p.node);
                    self.show(cx, p.node);
                    moved = true;
                }
            }
            let s = self.segs[p.node.index()];
            if let Some(res) = s.res
                && s.amount > 0
                && !self.pulled.contains(&p.ep)
                && cx.sink(p)
            {
                let put = cx.insert(p, res, rate.min(s.amount), 0, false);
                if put > 0 {
                    let s = &mut self.segs[p.node.index()];
                    s.amount -= put;
                    if s.amount == 0 {
                        s.res = None;
                    }
                    self.wake(net, p.node);
                    for l in cx.layer.node(p.node).links {
                        if l != NO_NODE {
                            self.wake(net, l);
                        }
                    }
                    self.show(cx, p.node);
                    moved = true;
                }
            }
        }

        // Segments: move toward lower head.
        let st = self.nets.entry(net);
        let active = std::mem::take(&mut st.active);
        let pot = std::mem::take(&mut st.potential);
        let mut next: Vec<NodeId> = Vec::with_capacity(active.len());
        for &a in &active {
            self.queued[a.index()] = false;
        }
        for &a in &active {
            if cx.layer.net_of(a) != Some(net) {
                continue;
            }
            let mut any = false;
            let links = cx.layer.node(a).links;
            for b in links {
                if b == NO_NODE {
                    continue;
                }
                self.grow(b);
                let (sa, sb) = (self.segs[a.index()], self.segs[b.index()]);
                let Some(res) = sa.res else {
                    break;
                };
                if sb.res.is_some_and(|r| r != res) {
                    continue;
                }
                let head = |n: NodeId, amount: u64| {
                    amount
                        + cx.layer.param(n, HEAD) as u64 * pot.get(&n).copied().unwrap_or(0) as u64
                };
                let (ha, hb) = (head(a, sa.amount), head(b, sb.amount));
                if ha <= hb + 1 {
                    continue;
                }
                let rate = cx.layer.param(a, RATE).min(cx.layer.param(b, RATE)) as u64;
                let room = (cx.layer.param(b, CAPACITY) as u64).saturating_sub(sb.amount);
                let m = rate.min((ha - hb) / 2).min(sa.amount).min(room);
                if m == 0 {
                    continue;
                }
                let s = &mut self.segs[a.index()];
                s.amount -= m;
                if s.amount == 0 {
                    s.res = None;
                }
                let t = &mut self.segs[b.index()];
                t.res = Some(res);
                t.amount += m;
                any = true;
                // `a` went down: whoever is above it may flow in now.
                for n in links.into_iter().chain([a]) {
                    if n == NO_NODE {
                        continue;
                    }
                    self.grow(n);
                    if !self.queued[n.index()] {
                        self.queued[n.index()] = true;
                        next.push(n);
                    }
                }
                self.show(cx, a);
                self.show(cx, b);
            }
            moved |= any;
        }
        let st = self.nets.entry(net);
        st.potential = pot;
        // Nodes woken by endpoint transfers during this tick are kept too.
        let woken = std::mem::take(&mut st.active);
        st.active = next;
        st.active.extend(woken);
        if !st.active.is_empty() || moved {
            Activity::Active
        } else {
            Activity::Sleep(POLL)
        }
    }

    fn save(&self, layer: &Layer, reg: &Registry, out: &mut Vec<Record>) {
        for (node, n) in layer.nodes() {
            let s = self.segment(node);
            if let (Some(res), true) = (s.res, s.amount > 0) {
                let mut e = Enc::default();
                e.res(reg, res).u64(s.amount);
                out.push(Record {
                    pos: n.pos,
                    bytes: e.buf,
                });
            }
        }
    }

    fn restore(&mut self, cx: &mut Cx, node: NodeId, net: NetId, bytes: &[u8]) -> bool {
        let mut d = Dec::new(bytes);
        let (Some(res), Some(amount)) = (d.res(cx.reg), d.u64()) else {
            return false;
        };
        self.grow(node);
        let s = &mut self.segs[node.index()];
        match s.res {
            Some(r) if r != res => cx.spill(cx.layer.node(node).pos, res, amount),
            _ => {
                s.res = Some(res);
                s.amount += amount;
            }
        }
        self.wake(net, node);
        self.show(cx, node);
        true
    }

    fn totals(&self, layer: &Layer, net: NetId, out: &mut Vec<(ResId, u64)>) {
        for (node, _) in layer.nodes() {
            if layer.net_of(node) != Some(net) {
                continue;
            }
            let s = self.segment(node);
            if let Some(res) = s.res {
                match out.iter_mut().find(|t| t.0 == res) {
                    Some(t) => t.1 += s.amount,
                    None => out.push((res, s.amount)),
                }
            }
        }
    }
}
