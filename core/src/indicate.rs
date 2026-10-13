//! Declarative indication (§9): models publish raw values, this quantises
//! them to each indicator's levels, remembers what the world already shows
//! and hands the host only real changes, at most once per flush interval.
//!
//! Per-network indicators (buffer fill) are stored once per network; a
//! change walks the network's nodes at the next flush, so a 10k-node
//! network costs one sweep per *level* change, never one per tick.

use crate::ids::{FxHashMap, FxHashSet, NetId, NodeId, Pos};
use crate::layer::Layer;

/// Indicators per capability.
pub const MAX_INDICATORS: usize = 4;

/// Default ticks between flushes.
pub const DEFAULT_INTERVAL: u64 = 4;

type Values = [u8; MAX_INDICATORS];

#[derive(Debug, Default)]
pub struct Indication {
    /// `levels` of each declared indicator.
    pub levels: Vec<u8>,
    /// What the world shows, per node index.
    shown: Vec<Values>,
    /// Pending per-node values, with a mask of the indices set.
    node_vals: FxHashMap<NodeId, (u8, Values)>,
    /// Current per-network values (generation-checked).
    net_vals: FxHashMap<u32, (u32, Values)>,
    dirty_nets: FxHashSet<u32>,
    /// Indices published per network (the rest are per node).
    net_level: u8,
    scratch: Vec<NodeId>,
}

/// `⌈num / den · levels⌉`, so anything non-zero shows at least level 1.
pub fn quantise(num: u64, den: u64, levels: u8) -> u8 {
    if num == 0 || den == 0 {
        return 0;
    }
    let q = (num as u128 * levels as u128).div_ceil(den as u128);
    q.min(levels as u128) as u8
}

impl Indication {
    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    pub fn node(&mut self, node: NodeId, index: u8, num: u64, den: u64) {
        let Some(&levels) = self.levels.get(index as usize) else {
            return;
        };
        let v = quantise(num, den, levels);
        let shown = self.shown.get(node.index()).map_or(0, |s| s[index as usize]);
        let bit = 1u8 << index;
        match self.node_vals.get_mut(&node) {
            Some((mask, vals)) => {
                *mask |= bit;
                vals[index as usize] = v;
            }
            None if shown != v => {
                let mut vals = [0; MAX_INDICATORS];
                vals[index as usize] = v;
                self.node_vals.insert(node, (bit, vals));
            }
            None => {}
        }
    }

    pub fn net(&mut self, net: NetId, index: u8, num: u64, den: u64) {
        let Some(&levels) = self.levels.get(index as usize) else {
            return;
        };
        let v = quantise(num, den, levels);
        self.net_level |= 1 << index;
        let e = self.net_vals.entry(net.index).or_insert((net.generation, [0; MAX_INDICATORS]));
        if e.0 != net.generation {
            *e = (net.generation, [0; MAX_INDICATORS]);
        }
        if e.1[index as usize] != v {
            e.1[index as usize] = v;
            self.dirty_nets.insert(net.index);
        }
    }

    /// A network changed shape: re-show its values at the next flush.
    pub fn net_changed(&mut self, net: NetId) {
        if self.net_vals.contains_key(&net.index) {
            self.dirty_nets.insert(net.index);
        }
    }

    /// A node joined `net` without changing its shape otherwise: show the
    /// network's values on it alone (no sweep).
    pub fn node_joined(&mut self, node: NodeId, net: NetId) {
        let Some(&(generation, vals)) = self.net_vals.get(&net.index) else {
            return;
        };
        if generation != net.generation {
            return;
        }
        let set = self.net_level;
        let e = self.node_vals.entry(node).or_insert((0, [0; MAX_INDICATORS]));
        for i in 0..MAX_INDICATORS {
            if set & (1 << i) != 0 && e.0 & (1 << i) == 0 {
                e.0 |= 1 << i;
                e.1[i] = vals[i];
            }
        }
    }

    /// A node is gone: the next block placed there starts at 0.
    pub fn forget(&mut self, node: NodeId) {
        if let Some(s) = self.shown.get_mut(node.index()) {
            *s = [0; MAX_INDICATORS];
        }
        self.node_vals.remove(&node);
    }

    /// Values currently shown at a node.
    pub fn shown(&self, node: NodeId) -> &[u8] {
        let n = self.levels.len();
        self.shown.get(node.index()).map_or(&[][..], |s| &s[..n])
    }

    /// Emit every change since the last flush through `emit(pos, index,
    /// value)`.
    pub fn flush(&mut self, layer: &mut Layer, mut emit: impl FnMut(Pos, u8, u8)) {
        let n = self.levels.len();
        if n == 0 {
            return;
        }
        if self.shown.len() < layer.node_capacity() {
            self.shown.resize(layer.node_capacity(), [0; MAX_INDICATORS]);
        }
        let mut nodes = std::mem::take(&mut self.scratch);
        for index in std::mem::take(&mut self.dirty_nets) {
            let Some(&(generation, vals)) = self.net_vals.get(&index) else {
                continue;
            };
            let id = NetId { index, generation };
            if layer.net(id).is_none() {
                self.net_vals.remove(&index);
                continue;
            }
            layer.net_nodes(id, &mut nodes);
            for &node in &nodes {
                let shown = &mut self.shown[node.index()];
                for i in 0..n {
                    if self.net_level & (1 << i) != 0 && shown[i] != vals[i] {
                        shown[i] = vals[i];
                        emit(layer.node(node).pos, i as u8, vals[i]);
                    }
                }
            }
        }
        self.scratch = nodes;
        for (node, (mask, vals)) in self.node_vals.drain() {
            if layer.node_capacity() <= node.index()
                || layer.node_at(layer.node(node).pos) != Some(node)
            {
                continue;
            }
            let shown = &mut self.shown[node.index()];
            for i in 0..n {
                if mask & (1 << i) != 0 && shown[i] != vals[i] {
                    shown[i] = vals[i];
                    emit(layer.node(node).pos, i as u8, vals[i]);
                }
            }
        }
        self.net_vals
            .retain(|&i, (g, _)| layer.net(NetId { index: i, generation: *g }).is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::quantise;

    #[test]
    fn quantise_rounds_up_and_clamps() {
        assert_eq!(quantise(0, 100, 15), 0);
        assert_eq!(quantise(1, 100, 15), 1);
        assert_eq!(quantise(100, 100, 15), 15);
        assert_eq!(quantise(200, 100, 15), 15);
        assert_eq!(quantise(50, 100, 4), 2);
    }
}
