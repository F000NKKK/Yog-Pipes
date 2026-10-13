//! `buffered` — one shared buffer per network (§5.2): Mekanism / Thermal /
//! Flux energy, Mekanism fluids, AE2 power, any mana or heat pool.
//!
//! Capacity is the sum of the pipes' `capacity`; every endpoint connection
//! moves at most the network's lowest `throughput` per tick (the weakest
//! tier is the bottleneck). One resource per network: an empty buffer
//! adopts the first resource pulled.
//!
//! Conservation: per tick `Σ extracted = Δbuffer + Σ inserted + Σ spilled`,
//! exactly — the buffer is credited/debited only by the amounts the
//! endpoint calls returned.

use crate::ids::{EpId, NetId, NodeId, ResId};
use crate::layer::{Layer, Port};
use crate::model::{Activity, Cx, Model, NetMap, Record, Want};
use crate::persist::{Dec, Enc};
use crate::policy::{order, water_fill};
use crate::registry::{IndicatorSource, Policy, Registry};

const CAPACITY: usize = 0;
const THROUGHPUT: usize = 1;
const PARAMS: &[&str] = &["capacity", "throughput"];

/// Ticks between polls of an idle network's sources.
pub const POLL: u32 = 10;

#[derive(Debug, Default, Clone)]
struct Buf {
    res: Option<ResId>,
    amount: u64,
    rr: usize,
    flowing: bool,
}

#[derive(Default)]
pub struct Buffered {
    nets: NetMap<Buf>,
    ports: Vec<Port>,
    cands: Vec<usize>,
    caps: Vec<u64>,
    shares: Vec<u64>,
    pulled: Vec<EpId>,
}

impl Buffered {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `amount` of `res` to a network's buffer at `at`, by the merge
    /// rule: different resources keep the larger portion and spill the
    /// smaller (design choice D3).
    fn absorb(&mut self, cx: &mut Cx, net: NetId, res: ResId, amount: u64, at: NodeId) {
        if amount == 0 {
            return;
        }
        let pos = cx.layer.node(at).pos;
        let b = self.nets.entry(net);
        match b.res {
            None => {
                b.res = Some(res);
                b.amount = amount;
            }
            Some(r) if r == res => b.amount += amount,
            Some(r) => {
                if amount > b.amount {
                    let (old, n) = (r, b.amount);
                    b.res = Some(res);
                    b.amount = amount;
                    cx.spill(pos, old, n);
                } else {
                    cx.spill(pos, res, amount);
                }
            }
        }
        self.clamp(cx, net);
    }

    /// Spill whatever exceeds the network's capacity.
    fn clamp(&mut self, cx: &mut Cx, net: NetId) {
        let cap = cx.layer.param_sum(net, CAPACITY) as u64;
        let Some(any) = cx.layer.net(net).map(|n| n.any_node) else {
            return;
        };
        let pos = cx.layer.node(any).pos;
        let b = self.nets.entry(net);
        if b.amount > cap {
            let extra = b.amount - cap;
            b.amount = cap;
            let res = b.res.unwrap();
            cx.spill(pos, res, extra);
        }
        if b.amount == 0 {
            b.res = None;
        }
    }

    fn show(&self, cx: &mut Cx, net: NetId) {
        let b = self.nets.get(net).cloned().unwrap_or_default();
        if let Some(i) = cx.indicator(IndicatorSource::Fill) {
            let cap = cx.layer.param_sum(net, CAPACITY) as u64;
            cx.indicate_net(net, i, b.amount, cap.max(1));
        }
        if let Some(i) = cx.indicator(IndicatorSource::Flow) {
            cx.indicate_net(net, i, b.flowing as u64, 1);
        }
    }
}

impl Model for Buffered {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn params(&self) -> &'static [&'static str] {
        PARAMS
    }

    fn node_added(&mut self, cx: &mut Cx, node: NodeId, net: NetId) {
        cx.ind.node_joined(node, net);
    }

    fn node_removed(&mut self, cx: &mut Cx, node: NodeId, net: NetId) {
        let size = cx.layer.net(net).map_or(1, |n| n.size.max(1)) as u64;
        let pos = cx.layer.node(node).pos;
        let b = self.nets.entry(net);
        let share = b.amount / size;
        b.amount -= share;
        if let Some(res) = b.res {
            if b.amount == 0 {
                b.res = None;
            }
            cx.spill(pos, res, share);
        }
    }

    fn merged(&mut self, cx: &mut Cx, into: NetId, from: NetId) {
        if let Some(f) = self.nets.take(from)
            && let Some(res) = f.res
        {
            let at = cx.layer.net(into).unwrap().any_node;
            self.absorb(cx, into, res, f.amount, at);
        }
        cx.ind.net_changed(into);
        self.show(cx, into);
    }

    fn split(&mut self, cx: &mut Cx, from: NetId, parts: &[(NetId, u32)]) {
        let b = self.nets.entry(from).clone();
        let kept = cx.layer.net(from).map_or(0, |n| n.size) as u64;
        let total = kept + parts.iter().map(|p| p.1 as u64).sum::<u64>();
        if let Some(res) = b.res {
            let mut left = b.amount;
            for &(p, size) in parts {
                let share = (b.amount as u128 * size as u128 / total.max(1) as u128) as u64;
                left -= share;
                let e = self.nets.entry(p);
                e.res = (share > 0).then_some(res);
                e.amount = share;
                self.clamp(cx, p);
                self.show(cx, p);
            }
            let e = self.nets.entry(from);
            e.amount = left;
            self.clamp(cx, from);
        }
        cx.ind.net_changed(from);
        self.show(cx, from);
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
        let capacity = cx.layer.param_sum(net, CAPACITY) as u64;
        let rate = cx.layer.param_min(net, THROUGHPUT) as u64;
        let policy = cx.reg.cap(cx.cap).policy;
        let mut b = self.nets.entry(net).clone();
        let count = self.ports.len();
        let mut moved = 0u64;

        // Pull from sources until full.
        self.pulled.clear();
        for k in 0..count {
            let port = self.ports[(b.rr + k) % count];
            let room = capacity.saturating_sub(b.amount);
            if room == 0 {
                break;
            }
            if !cx.source(port) {
                continue;
            }
            let want = b.res.map_or(Want::Any, Want::Res);
            if let Some((res, got)) = cx.extract(port, want, rate.min(room), false) {
                b.res = Some(res);
                b.amount += got;
                moved += got;
                self.pulled.push(port.ep);
            }
        }

        // Push to sinks by the policy.
        if let Some(res) = b.res {
            self.cands.clear();
            for (i, &p) in self.ports.iter().enumerate() {
                if !self.pulled.contains(&p.ep) && cx.sink(p) {
                    self.cands.push(i);
                }
            }
            let ports = &self.ports;
            order(
                policy,
                &mut self.cands,
                b.rr,
                cx.rng,
                |i| cx_priority(cx.layer, cx.reg, ports[i]),
                |_| 0,
            );
            if policy == Policy::EvenSplit {
                self.caps.clear();
                for &i in &self.cands {
                    let want = rate.min(b.amount);
                    self.caps.push(cx.insert(self.ports[i], res, want, 0, true));
                }
                water_fill(b.amount, &self.caps, &mut self.shares);
                for (k, &i) in self.cands.iter().enumerate() {
                    let got = cx.insert(self.ports[i], res, self.shares[k], 0, false);
                    b.amount -= got;
                    moved += got;
                }
            } else {
                for &i in &self.cands {
                    if b.amount == 0 {
                        break;
                    }
                    let got = cx.insert(self.ports[i], res, rate.min(b.amount), 0, false);
                    b.amount -= got;
                    moved += got;
                }
            }
            if b.amount == 0 {
                b.res = None;
            }
        }
        b.rr = b.rr.wrapping_add(1);
        b.flowing = moved > 0;
        *self.nets.entry(net) = b;
        self.show(cx, net);
        if moved > 0 {
            Activity::Active
        } else {
            Activity::Sleep(POLL)
        }
    }

    fn save(&self, layer: &Layer, reg: &Registry, out: &mut Vec<Record>) {
        for (net, b) in self.nets.iter() {
            let (Some(n), Some(res)) = (layer.net(net), b.res) else {
                continue;
            };
            let mut e = Enc::default();
            e.res(reg, res).u64(b.amount);
            out.push(Record {
                pos: layer.node(n.any_node).pos,
                bytes: e.buf,
            });
        }
    }

    fn restore(&mut self, cx: &mut Cx, node: NodeId, net: NetId, bytes: &[u8]) -> bool {
        let mut d = Dec::new(bytes);
        let (Some(res), Some(amount)) = (d.res(cx.reg), d.u64()) else {
            return false;
        };
        self.absorb(cx, net, res, amount, node);
        self.show(cx, net);
        true
    }

    fn totals(&self, layer: &Layer, net: NetId, out: &mut Vec<(ResId, u64)>) {
        if layer.net(net).is_some()
            && let Some(b) = self.nets.get(net)
            && let Some(res) = b.res
        {
            out.push((res, b.amount));
        }
    }
}

/// Endpoint + side priority without borrowing a whole `Cx`.
pub(crate) fn cx_priority(layer: &Layer, reg: &Registry, port: Port) -> i32 {
    reg.endpoint(layer.endpoint(port.ep).ty).priority + layer.side(port.node, port.face).priority
}
