//! `signal` — levels on channels, optionally attenuated per block (§5.6):
//! vanilla redstone dust, Project Red bundled cables, EnderIO redstone
//! conduits, AE2-style channels.
//!
//! Emitters set levels with `emit(pos, channel, level)` (event-driven, no
//! polling); the network settles at once, so the level is there the same
//! tick. Without attenuation a channel's level is the max of its
//! emitters over the whole network; with attenuation every node gets
//! `max(emitter level − attenuation · distance)` by a multi-source BFS that
//! stops where the level reaches 0. Listeners (`on_signal` endpoints) are
//! told only when the level at their pipe changes. Levels are not
//! resources: nothing here is conserved, by design.

use crate::ids::{Face, FxHashMap, NO_NODE, NetId, NodeId, Pos, ResId};
use crate::layer::Layer;
use crate::model::{Activity, Cx, Model, NetMap, Record};
use crate::persist::{Dec, Enc};
use crate::registry::{IndicatorSource, Registry, SideMode};

/// Highest signal level.
pub const MAX_LEVEL: u8 = 15;

#[derive(Debug, Default, Clone)]
struct Net {
    dirty: bool,
    /// Per channel, without attenuation.
    levels: Vec<u8>,
    /// Per node per channel, with attenuation (only non-zero nodes).
    nodes: FxHashMap<NodeId, Vec<u8>>,
}

pub struct Signal {
    channels: u16,
    attenuation: u8,
    emitters: FxHashMap<Pos, Vec<u8>>,
    nets: NetMap<Net>,
    /// Last level told to each listener: `(endpoint, channel) → level`.
    told: FxHashMap<(Pos, u8), u8>,
}

impl Signal {
    pub fn new(channels: u16, attenuation: u8) -> Self {
        Signal {
            channels: channels.clamp(1, 256),
            attenuation,
            emitters: FxHashMap::default(),
            nets: NetMap::default(),
            told: FxHashMap::default(),
        }
    }

    fn dirty(&mut self, net: NetId) {
        self.nets.entry(net).dirty = true;
    }

    /// Networks an emitter at `at` feeds: the node there, else the nodes
    /// of the endpoint's adjacent pipes whose side toward it is not off.
    fn feeds(layer: &Layer, at: Pos, out: &mut Vec<(NodeId, NetId)>) {
        out.clear();
        if let Some(n) = layer.node_at(at) {
            if let Some(net) = layer.net_of(n) {
                out.push((n, net));
            }
            return;
        }
        for f in Face::ALL {
            if let Some(n) = layer.node_at(at.offset(f))
                && layer.side(n, f.opposite()).mode != SideMode::Off
                && let Some(net) = layer.net_of(n)
            {
                out.push((n, net));
            }
        }
    }

    fn node_level(&self, layer: &Layer, node: NodeId, channel: u8) -> u8 {
        let Some(net) = layer.net_of(node) else {
            return 0;
        };
        let Some(st) = self.nets.get(net) else {
            return 0;
        };
        if self.attenuation == 0 {
            st.levels.get(channel as usize).copied().unwrap_or(0)
        } else {
            st.nodes
                .get(&node)
                .and_then(|v| v.get(channel as usize))
                .copied()
                .unwrap_or(0)
        }
    }

    fn recompute(&mut self, cx: &mut Cx, net: NetId) {
        let ch = self.channels as usize;
        let mut sources: Vec<(NodeId, Vec<u8>)> = Vec::new();
        let mut feeds = Vec::new();
        for (&pos, levels) in &self.emitters {
            Self::feeds(cx.layer, pos, &mut feeds);
            for &(n, nn) in &feeds {
                if nn == net {
                    sources.push((n, levels.clone()));
                }
            }
        }
        let st = self.nets.entry(net);
        let old_nodes = std::mem::take(&mut st.nodes);
        if self.attenuation == 0 {
            st.levels = vec![0; ch];
            for (_, l) in &sources {
                for (dst, &src) in st.levels.iter_mut().zip(l) {
                    *dst = (*dst).max(src);
                }
            }
        } else {
            let att = self.attenuation;
            let mut nodes: FxHashMap<NodeId, Vec<u8>> = FxHashMap::default();
            for c in 0..ch {
                // Bucketed BFS: highest levels first, each node settles once.
                let mut buckets: Vec<Vec<NodeId>> = vec![Vec::new(); MAX_LEVEL as usize + 1];
                for (n, l) in &sources {
                    if l[c] > 0 {
                        buckets[l[c] as usize].push(*n);
                    }
                }
                for lvl in (1..=MAX_LEVEL as usize).rev() {
                    let list = std::mem::take(&mut buckets[lvl]);
                    for n in list {
                        let e = nodes.entry(n).or_insert_with(|| vec![0; ch]);
                        if e[c] >= lvl as u8 {
                            continue;
                        }
                        e[c] = lvl as u8;
                        let next = (lvl as u8).saturating_sub(att);
                        if next == 0 {
                            continue;
                        }
                        for l in cx.layer.node(n).links {
                            if l != NO_NODE && nodes.get(&l).is_none_or(|v| v[c] < next) {
                                buckets[next as usize].push(l);
                            }
                        }
                    }
                }
            }
            nodes.retain(|_, v| v.iter().any(|&x| x > 0));
            st.nodes = nodes;
        }
        st.dirty = false;

        // Listeners.
        let ports = cx
            .layer
            .net(net)
            .map(|n| n.ports.clone())
            .unwrap_or_default();
        for p in ports {
            let ty = cx.layer.endpoint(p.ep).ty;
            if !cx.reg.endpoint(ty).listens_signals || cx.layer.side(p.node, p.face).disabled {
                continue;
            }
            let at = cx.layer.endpoint(p.ep).pos;
            for c in (0..self.channels).map(|c| c as u8) {
                let lvl = self.node_level(cx.layer, p.node, c);
                let told = self.told.get(&(at, c)).copied().unwrap_or(0);
                if lvl != told {
                    if lvl == 0 {
                        self.told.remove(&(at, c));
                    } else {
                        self.told.insert((at, c), lvl);
                    }
                    let call = cx.call(p);
                    cx.io.signal(cx.reg, &call, c, lvl);
                }
            }
        }

        // Indication: max over channels.
        if let Some(i) = cx.indicator(IndicatorSource::Power) {
            let st = self.nets.get(net).unwrap();
            if self.attenuation == 0 {
                let m = st.levels.iter().copied().max().unwrap_or(0);
                cx.indicate_net(net, i, m as u64, MAX_LEVEL as u64);
            } else {
                for (n, v) in &st.nodes {
                    let m = v.iter().copied().max().unwrap_or(0);
                    cx.indicate_node(*n, i, m as u64, MAX_LEVEL as u64);
                }
                for n in old_nodes.keys() {
                    if !st.nodes.contains_key(n) && cx.layer.net_of(*n).is_some() {
                        cx.indicate_node(*n, i, 0, 1);
                    }
                }
            }
        }
    }
}

impl Model for Signal {
    fn reset(&mut self) {
        *self = Signal::new(self.channels, self.attenuation);
    }

    fn params(&self) -> &'static [&'static str] {
        &[]
    }

    fn node_added(&mut self, cx: &mut Cx, node: NodeId, net: NetId) {
        cx.ind.node_joined(node, net);
        self.dirty(net);
    }

    fn node_removed(&mut self, cx: &mut Cx, node: NodeId, net: NetId) {
        cx.ind.forget(node);
        self.dirty(net);
    }

    fn merged(&mut self, cx: &mut Cx, into: NetId, from: NetId) {
        self.nets.take(from);
        cx.ind.net_changed(into);
        self.dirty(into);
    }

    fn split(&mut self, cx: &mut Cx, from: NetId, parts: &[(NetId, u32)]) {
        self.dirty(from);
        cx.ind.net_changed(from);
        for &(p, _) in parts {
            self.dirty(p);
            cx.ind.net_changed(p);
        }
    }

    fn net_removed(&mut self, _cx: &mut Cx, net: NetId) {
        self.nets.take(net);
    }

    fn tick(&mut self, cx: &mut Cx, net: NetId) -> Activity {
        if self.nets.get(net).is_none_or(|n| n.dirty) {
            self.recompute(cx, net);
        }
        Activity::Idle
    }

    fn emit(&mut self, cx: &mut Cx, at: Pos, channel: u8, level: u8) {
        if channel as u16 >= self.channels {
            return;
        }
        let level = level.min(MAX_LEVEL);
        let e = self
            .emitters
            .entry(at)
            .or_insert_with(|| vec![0; self.channels as usize]);
        if e[channel as usize] == level {
            return;
        }
        e[channel as usize] = level;
        if e.iter().all(|&l| l == 0) {
            self.emitters.remove(&at);
        }
        // Settled now, not next tick: a signal crosses its network the tick
        // it's emitted (as redstone dust does), and what reads the level
        // this tick sees it.
        let mut feeds = Vec::new();
        Self::feeds(cx.layer, at, &mut feeds);
        let mut nets: Vec<NetId> = Vec::with_capacity(feeds.len());
        for (_, net) in feeds {
            if !nets.contains(&net) {
                nets.push(net);
            }
        }
        for net in nets {
            self.recompute(cx, net);
        }
    }

    fn forget(&mut self, cx: &mut Cx, at: Pos) {
        if !self.emitters.contains_key(&at) {
            return;
        }
        for c in (0..self.channels).map(|c| c as u8) {
            self.emit(cx, at, c, 0);
        }
    }

    fn level(&self, layer: &Layer, at: Pos, channel: u8) -> u8 {
        if let Some(n) = layer.node_at(at) {
            return self.node_level(layer, n, channel);
        }
        let mut feeds = Vec::new();
        Self::feeds(layer, at, &mut feeds);
        feeds
            .iter()
            .map(|&(n, _)| self.node_level(layer, n, channel))
            .max()
            .unwrap_or(0)
    }

    /// Emitters are saved at their own positions (they may be machines,
    /// not pipes); `restore` gets them back through the engine's global
    /// state hook.
    fn save(&self, _layer: &Layer, _reg: &Registry, _out: &mut Vec<Record>) {}

    fn restore(&mut self, _cx: &mut Cx, _node: NodeId, _net: NetId, _bytes: &[u8]) -> bool {
        true
    }

    fn save_global(&self, reg: &Registry) -> Vec<u8> {
        let mut e = Enc::default();
        e.u32(self.emitters.len() as u32);
        for (&pos, levels) in &self.emitters {
            e.pos(reg, pos).bytes(levels);
        }
        e.buf
    }

    fn load_global(&mut self, reg: &mut Registry, bytes: &[u8]) {
        let mut d = Dec::new(bytes);
        let Some(n) = d.u32() else {
            return;
        };
        for _ in 0..n {
            let (Some(pos), Some(levels)) = (d.pos(reg), d.bytes()) else {
                return;
            };
            let mut v = vec![0; self.channels as usize];
            for (i, &l) in levels.iter().take(v.len()).enumerate() {
                v[i] = l;
            }
            self.emitters.insert(pos, v);
        }
    }

    fn totals(&self, _layer: &Layer, _net: NetId, _out: &mut Vec<(ResId, u64)>) {}
}
