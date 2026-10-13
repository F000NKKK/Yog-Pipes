//! `routed` — instant routing from sources to sinks, nothing stored in the
//! pipes (§5.3): Pipez, EnderIO conduits, AE2/RS-style access, IC2 EU
//! packets with distance loss, Immersive Engineering wires.
//!
//! Each tick every source (rotating start) is simulated for up to the
//! network's `throughput`; sinks are ordered by the policy and simulated;
//! only the planned total is extracted for real; then sinks are filled for
//! real and whatever they refuse after all goes to the other sinks, back
//! into the source, or is spilled — never lost silently.
//!
//! `loss` is in thousandths of a unit per block of distance (BFS hops,
//! cached per network version): delivering over `d` blocks costs
//! `⌈d · loss / 1000⌉` units per transfer, reported as `Lost`.
//! `max_packet` (smallest non-zero over the network's pipes, and per
//! endpoint) raises `Overload` for a bigger transfer; it still arrives
//! (design choice D6).
//!
//! Conservation: `Σ extracted = Σ inserted + Σ lost + Σ spilled`.

use crate::ids::{NetId, NodeId, ResId};
use crate::layer::{Layer, Port};
use crate::model::buffered::cx_priority;
use crate::model::{Activity, Cx, Event, Model, NetMap, Record, Want};
use crate::policy::{order, water_fill};
use crate::registry::{IndicatorSource, Policy, Registry};

const THROUGHPUT: usize = 0;
const LOSS: usize = 1;
const MAX_PACKET: usize = 2;
const PARAMS: &[&str] = &["throughput", "loss", "max_packet"];

const POLL: u32 = 10;

#[derive(Debug, Default, Clone)]
struct State {
    rr: usize,
}

#[derive(Default)]
pub struct Routed {
    nets: NetMap<State>,
    ports: Vec<Port>,
    cands: Vec<usize>,
    dist: Vec<u32>,
    caps: Vec<u64>,
    shares: Vec<u64>,
    plan: Vec<(usize, u64, u64)>,
}

impl Routed {
    pub fn new() -> Self {
        Self::default()
    }
}

fn loss_for(hops: u32, milli: u64) -> u64 {
    if hops == u32::MAX {
        return u64::MAX;
    }
    (hops as u64 * milli).div_ceil(1000)
}

impl Model for Routed {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn params(&self) -> &'static [&'static str] {
        PARAMS
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
        let rate = cx.layer.param_min(net, THROUGHPUT) as u64;
        let milli = cx.layer.param_max(net, LOSS) as u64;
        let limit = cx.layer.param_min_nonzero(net, MAX_PACKET) as u64;
        let policy = cx.reg.cap(cx.cap).policy;
        let rr = self.nets.entry(net).rr;
        let count = self.ports.len();
        let mut moved = 0u64;

        for k in 0..count {
            let src = self.ports[(rr + k) % count];
            if !cx.source(src) {
                continue;
            }
            let Some((res, offered)) = cx.extract(src, Want::Any, rate, true) else {
                continue;
            };
            let colour = cx.layer.side(src.node, src.face).colour;
            self.dist.clear();
            self.dist
                .extend_from_slice(cx.layer.port_distances(src.node));
            self.cands.clear();
            for (i, &p) in self.ports.iter().enumerate() {
                if p.ep != src.ep && cx.sink(p) {
                    self.cands.push(i);
                }
            }
            let (ports, dist) = (&self.ports, &self.dist);
            order(
                policy,
                &mut self.cands,
                rr,
                cx.rng,
                |i| cx_priority(cx.layer, cx.reg, ports[i]),
                |i| dist[i],
            );

            // Plan: (port index, units it accepts, loss on the way).
            self.plan.clear();
            let mut left = offered;
            if policy == Policy::EvenSplit {
                self.caps.clear();
                for &i in &self.cands {
                    let loss = loss_for(self.dist[i], milli);
                    let cap = if offered > loss {
                        cx.insert(self.ports[i], res, offered - loss, colour, true)
                            .saturating_add(loss)
                    } else {
                        0
                    };
                    self.caps.push(if cap > loss { cap } else { 0 });
                }
                water_fill(offered, &self.caps, &mut self.shares);
                for (k, &i) in self.cands.iter().enumerate() {
                    let loss = loss_for(self.dist[i], milli);
                    if self.shares[k] > loss {
                        self.plan.push((i, self.shares[k] - loss, loss));
                        left -= self.shares[k];
                    }
                }
            } else {
                for &i in &self.cands {
                    let loss = loss_for(self.dist[i], milli);
                    if left <= loss {
                        continue;
                    }
                    let acc = cx.insert(self.ports[i], res, left - loss, colour, true);
                    if acc > 0 {
                        self.plan.push((i, acc, loss));
                        left -= acc + loss;
                    }
                }
            }
            let total = offered - left;
            if total == 0 {
                continue;
            }

            // Execute: the real extraction may give less than simulated.
            let Some((_, got)) = cx.extract(src, Want::Res(res), total, false) else {
                continue;
            };
            moved += got;
            let mut left = got;
            for k in 0..self.plan.len() {
                let (i, acc, loss) = self.plan[k];
                if left <= loss {
                    break;
                }
                let port = self.ports[i];
                let give = acc.min(left - loss);
                let put = cx.insert(port, res, give, colour, false);
                if put == 0 {
                    continue;
                }
                left -= put + loss;
                let at = cx.layer.endpoint(port.ep).pos;
                if loss > 0 {
                    cx.event(Event::Lost {
                        at,
                        res,
                        amount: loss,
                    });
                }
                let ep_limit = cx.reg.endpoint(cx.layer.endpoint(port.ep).ty).max_packet;
                for lim in [limit, ep_limit] {
                    if lim > 0 && put > lim {
                        cx.event(Event::Overload {
                            at,
                            amount: put,
                            limit: lim,
                        });
                        break;
                    }
                }
            }
            // A sink refused after all: any other sink, then the source,
            // then spill.
            if left > 0 {
                for k in 0..self.cands.len() {
                    let i = self.cands[k];
                    let loss = loss_for(self.dist[i], milli);
                    if left <= loss {
                        continue;
                    }
                    let put = cx.insert(self.ports[i], res, left - loss, colour, false);
                    if put > 0 {
                        left -= put + loss;
                        if loss > 0 {
                            let at = cx.layer.endpoint(self.ports[i].ep).pos;
                            cx.event(Event::Lost {
                                at,
                                res,
                                amount: loss,
                            });
                        }
                    }
                }
                left -= cx.give_back(src, res, left);
                let at = cx.layer.endpoint(src.ep).pos;
                cx.spill(at, res, left);
            }
        }
        self.nets.entry(net).rr = rr.wrapping_add(1);
        if let Some(i) = cx.indicator(IndicatorSource::Flow) {
            cx.indicate_net(net, i, (moved > 0) as u64, 1);
        }
        if moved > 0 {
            Activity::Active
        } else {
            Activity::Sleep(POLL)
        }
    }

    fn node_added(&mut self, cx: &mut Cx, node: NodeId, net: NetId) {
        cx.ind.node_joined(node, net);
    }

    fn save(&self, _layer: &Layer, _reg: &Registry, _out: &mut Vec<Record>) {}

    fn restore(&mut self, _cx: &mut Cx, _node: NodeId, _net: NetId, _bytes: &[u8]) -> bool {
        true
    }

    fn totals(&self, _layer: &Layer, _net: NetId, _out: &mut Vec<(ResId, u64)>) {}
}
