//! `in_flight` — discrete units travelling through the pipes (§5.4):
//! BuildCraft transport pipes, Mekanism logistical transporters, Thermal
//! itemducts — for any kind.
//!
//! Every `interval` ticks each source (rotating) is simulated for up to
//! `stack`; a destination is chosen by the policy among sinks that
//! simulate-accept; only then is the source extracted for real and a unit
//! spawned. Units store no path: each hop asks the BFS tree rooted at the
//! destination's pipe (cached per network version), so a topology change
//! costs nothing per unit. Per-tick cost is O(units in flight).
//!
//! A unit that can't be delivered re-targets, else returns to its source,
//! else waits and retries (Mekanism's idle items). It is never duplicated
//! and never dropped while its pipe exists; a unit in a broken pipe is
//! spilled (the capability's event hook decides what that means).

use crate::ids::{Face, FxHashMap, NetId, NodeId, Pos, ResId};
use crate::layer::{HOP_HERE, Layer, Port};
use crate::model::buffered::cx_priority;
use crate::model::{Activity, Cx, Model, NetMap, Record, Want};
use crate::persist::{Dec, Enc};
use crate::policy::order;
use crate::registry::{IndicatorSource, Registry};

const SPEED: usize = 0;
const STACK: usize = 1;
const INTERVAL: usize = 2;
const PARAMS: &[&str] = &["speed", "stack", "interval"];

/// 16.16 fixed point: one block.
const ONE: u32 = 1 << 16;
/// Ticks a unit with nowhere to go waits before retrying.
pub const RETRY: u64 = 20;
/// Default ticks between source pulls.
const DEFAULT_INTERVAL: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub res: ResId,
    pub amount: u64,
    pub node: NodeId,
    /// Progress through `node`, 16.16 blocks.
    pub progress: u32,
    /// Destination pipe node and its face toward the sink.
    pub dest: Option<(NodeId, Face)>,
    /// Where it came from (positions survive saves).
    pub source: (Pos, Face),
    pub color: u8,
    pub returning: bool,
    pub wait_until: u64,
}

/// What [`InFlight::target`] looks for.
struct Seek {
    from: NodeId,
    res: ResId,
    amount: u64,
    color: u8,
    /// Endpoint position not to pick (the unit's source).
    except: Option<Pos>,
    rr: usize,
}

#[derive(Debug, Default, Clone)]
struct Net {
    units: Vec<Unit>,
    next_pull: u64,
    rr: usize,
}

#[derive(Default)]
pub struct InFlight {
    nets: NetMap<Net>,
    /// Units per node (indication).
    counts: FxHashMap<NodeId, u32>,
    ports: Vec<Port>,
    cands: Vec<usize>,
}

impl InFlight {
    pub fn new() -> Self {
        Self::default()
    }

    /// Units in a network (for tests and info).
    pub fn units(&self, net: NetId) -> &[Unit] {
        self.nets.get(net).map_or(&[], |n| &n.units)
    }

    fn count(&mut self, cx: &mut Cx, node: NodeId, delta: i32) {
        let c = self.counts.entry(node).or_insert(0);
        *c = c.saturating_add_signed(delta);
        let v = *c;
        if v == 0 {
            self.counts.remove(&node);
        }
        if let Some(i) = cx.indicator(IndicatorSource::Units) {
            let levels = cx.ind.levels[i as usize] as u64;
            cx.indicate_node(node, i, (v as u64).min(levels), levels.max(1));
        }
    }

    /// Pick a sink (≠ `except` endpoint position) accepting some of the
    /// unit; returns the port and how much it would take.
    fn target(&mut self, cx: &mut Cx, net: NetId, q: Seek) -> Option<(Port, u64)> {
        let Seek {
            from,
            res,
            amount,
            color,
            except,
            rr,
        } = q;
        let n = cx.layer.net(net)?;
        self.ports.clear();
        self.ports.extend_from_slice(&n.ports);
        let dist = cx.layer.port_distances(from).to_vec();
        self.cands.clear();
        for (i, &p) in self.ports.iter().enumerate() {
            if Some(cx.layer.endpoint(p.ep).pos) != except && dist[i] != u32::MAX && cx.sink(p) {
                self.cands.push(i);
            }
        }
        let policy = cx.reg.cap(cx.cap).policy;
        let ports = &self.ports;
        order(
            policy,
            &mut self.cands,
            rr,
            cx.rng,
            |i| cx_priority(cx.layer, cx.reg, ports[i]),
            |i| dist[i],
        );
        for k in 0..self.cands.len() {
            let p = self.ports[self.cands[k]];
            let acc = cx.insert(p, res, amount, color, true);
            if acc > 0 {
                return Some((p, acc));
            }
        }
        None
    }

    /// A unit reached its destination node: deliver, or decide what next.
    /// Returns `true` when it is gone.
    fn arrive(&mut self, cx: &mut Cx, net: NetId, u: &mut Unit, rr: usize) -> bool {
        let ports = cx
            .layer
            .net(net)
            .map(|n| n.ports.clone())
            .unwrap_or_default();
        if let Some((node, face)) = u.dest
            && let Some(&port) = ports.iter().find(|p| p.node == node && p.face == face)
        {
            let put = if u.returning {
                cx.give_back(port, u.res, u.amount)
            } else {
                cx.insert(port, u.res, u.amount, u.color, false)
            };
            u.amount -= put;
            if u.amount == 0 {
                return true;
            }
        }
        self.retarget(cx, net, u, rr);
        false
    }

    fn retarget(&mut self, cx: &mut Cx, net: NetId, u: &mut Unit, rr: usize) {
        u.progress = 0;
        if !u.returning
            && let Some((p, _)) = self.target(
                cx,
                net,
                Seek {
                    from: u.node,
                    res: u.res,
                    amount: u.amount,
                    color: u.color,
                    except: None,
                    rr,
                },
            )
        {
            u.dest = Some((p.node, p.face));
            return;
        }
        // Back to the source, if it is still on this network.
        let (spos, sface) = u.source;
        let back = cx
            .layer
            .node_at(spos.offset(sface))
            .filter(|&n| cx.layer.net_of(n) == Some(net));
        match back {
            Some(n) if !u.returning || u.dest != Some((n, sface.opposite())) => {
                u.dest = Some((n, sface.opposite()));
                u.returning = true;
            }
            _ => {
                // Nowhere to go: wait, then try sinks again.
                u.returning = false;
                u.dest = None;
                u.wait_until = cx.tick + RETRY;
            }
        }
    }
}

impl Model for InFlight {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn params(&self) -> &'static [&'static str] {
        PARAMS
    }

    fn node_removed(&mut self, cx: &mut Cx, node: NodeId, net: NetId) {
        let pos = cx.layer.node(node).pos;
        let Some(n) = self.nets.get(net) else {
            return;
        };
        let mut units = n.units.clone();
        units.retain(|u| {
            if u.node == node {
                cx.spill(pos, u.res, u.amount);
                false
            } else {
                true
            }
        });
        for u in &mut units {
            if u.dest.is_some_and(|d| d.0 == node) {
                u.dest = None;
            }
        }
        self.nets.entry(net).units = units;
        self.counts.remove(&node);
        cx.ind.forget(node);
    }

    fn merged(&mut self, _cx: &mut Cx, into: NetId, from: NetId) {
        if let Some(f) = self.nets.take(from) {
            self.nets.entry(into).units.extend(f.units);
        }
    }

    fn split(&mut self, cx: &mut Cx, from: NetId, parts: &[(NetId, u32)]) {
        let units = std::mem::take(&mut self.nets.entry(from).units);
        for u in units {
            let net = cx.layer.net_of(u.node).unwrap_or(from);
            let target = if parts.iter().any(|p| p.0 == net) {
                net
            } else {
                from
            };
            self.nets.entry(target).units.push(u);
        }
    }

    fn net_removed(&mut self, _cx: &mut Cx, net: NetId) {
        self.nets.take(net);
    }

    fn tick(&mut self, cx: &mut Cx, net: NetId) -> Activity {
        let Some(n) = cx.layer.net(net) else {
            return Activity::Idle;
        };
        let mut st = std::mem::take(self.nets.entry(net));
        let interval = match cx.layer.param_min(net, INTERVAL) as u64 {
            0 => DEFAULT_INTERVAL,
            i => i,
        };
        let stack = cx.layer.param_min(net, STACK) as u64;

        // Pull from sources.
        if cx.tick >= st.next_pull {
            st.next_pull = cx.tick + interval;
            let ports: Vec<Port> = n.ports.clone();
            let count = ports.len();
            for k in 0..count {
                let src = ports[(st.rr + k) % count];
                if !cx.source(src) {
                    continue;
                }
                let Some((res, n)) = cx.extract(src, Want::Any, stack, true) else {
                    continue;
                };
                let color = cx.layer.side(src.node, src.face).color;
                let src_pos = cx.layer.endpoint(src.ep).pos;
                let Some((dest, acc)) = self.target(
                    cx,
                    net,
                    Seek {
                        from: src.node,
                        res,
                        amount: n,
                        color,
                        except: Some(src_pos),
                        rr: st.rr,
                    },
                ) else {
                    continue;
                };
                let Some((_, got)) = cx.extract(src, Want::Res(res), n.min(acc), false) else {
                    continue;
                };
                st.units.push(Unit {
                    res,
                    amount: got,
                    node: src.node,
                    progress: 0,
                    dest: Some((dest.node, dest.face)),
                    source: (src_pos, src.face.opposite()),
                    color,
                    returning: false,
                    wait_until: 0,
                });
                self.count(cx, src.node, 1);
            }
            st.rr = st.rr.wrapping_add(1);
        }

        // Move.
        let mut i = 0;
        while i < st.units.len() {
            let mut u = st.units[i].clone();
            let mut gone = false;
            if u.wait_until <= cx.tick {
                let speed = (cx.layer.param(u.node, SPEED) * ONE as f64) as u32;
                u.progress = u.progress.saturating_add(speed.max(1));
                while u.progress >= ONE {
                    let Some((dest, _)) = u.dest else {
                        self.retarget(cx, net, &mut u, st.rr);
                        break;
                    };
                    match cx.layer.next_hop(dest, u.node) {
                        Some(HOP_HERE) => {
                            gone = self.arrive(cx, net, &mut u, st.rr);
                            break;
                        }
                        Some(face) => {
                            let next = cx.layer.node(u.node).links[face as usize];
                            self.count(cx, u.node, -1);
                            self.count(cx, next, 1);
                            u.node = next;
                            u.progress -= ONE;
                        }
                        None => {
                            self.retarget(cx, net, &mut u, st.rr);
                            break;
                        }
                    }
                }
            }
            if gone {
                self.count(cx, u.node, -1);
                st.units.swap_remove(i);
            } else {
                st.units[i] = u;
                i += 1;
            }
        }
        let busy = !st.units.is_empty();
        *self.nets.entry(net) = st;
        if busy {
            Activity::Active
        } else {
            Activity::Sleep(interval as u32)
        }
    }

    fn save(&self, layer: &Layer, reg: &Registry, out: &mut Vec<Record>) {
        for (net, n) in self.nets.iter() {
            if layer.net(net).is_none() {
                continue;
            }
            for u in &n.units {
                let mut e = Enc::default();
                e.res(reg, u.res)
                    .u64(u.amount)
                    .u32(u.progress)
                    .u8(u.color)
                    .u8(u.returning as u8);
                e.pos(reg, u.source.0).face(u.source.1);
                match u.dest {
                    Some((d, f)) => e.u8(1).pos(reg, layer.node(d).pos).face(f),
                    None => e.u8(0),
                };
                out.push(Record {
                    pos: layer.node(u.node).pos,
                    bytes: e.buf,
                });
            }
        }
    }

    fn restore(&mut self, cx: &mut Cx, node: NodeId, net: NetId, bytes: &[u8]) -> bool {
        let mut d = Dec::new(bytes);
        let mut read = || -> Option<Unit> {
            let res = d.res(cx.reg)?;
            let (amount, progress, color, returning) = (d.u64()?, d.u32()?, d.u8()?, d.u8()? != 0);
            let source = (d.pos(cx.reg)?, d.face()?);
            let dest = match d.u8()? {
                1 => {
                    let p = d.pos(cx.reg)?;
                    let f = d.face()?;
                    cx.layer.node_at(p).map(|n| (n, f))
                }
                _ => None,
            };
            Some(Unit {
                res,
                amount,
                node,
                progress,
                dest,
                source,
                color,
                returning,
                wait_until: 0,
            })
        };
        let Some(u) = read() else {
            return false;
        };
        self.nets.entry(net).units.push(u);
        self.count(cx, node, 1);
        true
    }

    fn totals(&self, layer: &Layer, net: NetId, out: &mut Vec<(ResId, u64)>) {
        if layer.net(net).is_none() {
            return;
        }
        for u in self.units(net) {
            match out.iter_mut().find(|t| t.0 == u.res) {
                Some(t) => t.1 += u.amount,
                None => out.push((u.res, u.amount)),
            }
        }
    }
}
