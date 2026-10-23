//! Searches over a network, cached per network version: hop distances to
//! its ports, next hops towards a node, its nodes.

use std::collections::VecDeque;

use super::{HOP_HERE, Layer};
use crate::ids::{Face, FxHashMap, FxHashSet, NO_NODE, NetId, NodeId};

impl Layer {
    /// BFS hop distance from `from` to every port of its network, in
    /// `ports` order (`u32::MAX` = unreachable), cached per version.
    /// `None` for a node in no network.
    pub fn port_distances(&mut self, from: NodeId) -> Option<&[u32]> {
        let id = self.net_of(from)?;
        let version = self.net(id)?.version;
        let key = (id.index, from);
        let fresh = self
            .dist_cache
            .get(&key)
            .is_some_and(|(v, _)| *v == version);
        if !fresh {
            let want: FxHashSet<NodeId> = self.net(id)?.ports.iter().map(|p| p.node).collect();
            let dist = self.distances(from, &want);
            let out = self
                .net(id)?
                .ports
                .iter()
                .map(|p| dist.get(&p.node).copied().unwrap_or(u32::MAX))
                .collect();
            self.dist_cache.insert(key, (version, out));
        }
        self.dist_cache.get(&key).map(|(_, d)| d.as_slice())
    }

    /// Hop distances from `from` to each of `want` it reaches.
    fn distances(&mut self, from: NodeId, want: &FxHashSet<NodeId>) -> FxHashMap<NodeId, u32> {
        let base = self.stamps(1);
        let mut dist: FxHashMap<NodeId, u32> = FxHashMap::default();
        let mut queue = VecDeque::from([(from, 0u32)]);
        self.nodes[from.index()].mark = base;
        let mut left = want.len();
        while let Some((c, d)) = queue.pop_front() {
            if want.contains(&c) {
                dist.insert(c, d);
                left -= 1;
                if left == 0 {
                    break;
                }
            }
            for l in self.nodes[c.index()].links {
                if l != NO_NODE && self.nodes[l.index()].mark != base {
                    self.nodes[l.index()].mark = base;
                    queue.push_back((l, d + 1));
                }
            }
        }
        dist
    }

    /// The face to leave each node by to reach `dest` (a BFS tree rooted at
    /// `dest`), cached per network version. `None` for a node not in
    /// `dest`'s network.
    pub fn next_hop(&mut self, dest: NodeId, from: NodeId) -> Option<u8> {
        let id = self.net_of(dest)?;
        let version = self.net(id)?.version;
        let fresh = self
            .tree_cache
            .get(&dest)
            .is_some_and(|(n, v, _)| *n == id.index && *v == version);
        if !fresh {
            let mut tree: FxHashMap<NodeId, u8> = FxHashMap::default();
            tree.insert(dest, HOP_HERE);
            let mut queue = VecDeque::from([dest]);
            while let Some(c) = queue.pop_front() {
                for f in Face::ALL {
                    let l = self.nodes[c.index()].links[f as usize];
                    if l != NO_NODE && !tree.contains_key(&l) {
                        // From `l`, go back the way we came.
                        tree.insert(l, f.opposite() as u8);
                        queue.push_back(l);
                    }
                }
            }
            self.tree_cache.insert(dest, (id.index, version, tree));
        }
        self.tree_cache.get(&dest)?.2.get(&from).copied()
    }

    /// Drop cached searches of networks that changed since (housekeeping).
    pub(crate) fn prune_caches(&mut self) {
        let nets = &self.nets;
        let live = |i: u32, v: u32| {
            nets.get(i as usize)
                .and_then(|s| s.net.as_ref())
                .is_some_and(|n| n.version == v)
        };
        self.dist_cache.retain(|k, (v, _)| live(k.0, *v));
        self.tree_cache.retain(|_, (n, v, _)| live(*n, *v));
    }

    /// Nodes of a network, by BFS from its representative (O(size)).
    pub fn net_nodes(&mut self, id: NetId, out: &mut Vec<NodeId>) {
        out.clear();
        let Some(start) = self.net(id).map(|n| n.any_node) else {
            return;
        };
        let base = self.stamps(1);
        self.nodes[start.index()].mark = base;
        out.push(start);
        let mut i = 0;
        while i < out.len() {
            let c = out[i];
            i += 1;
            for l in self.nodes[c.index()].links {
                if l != NO_NODE && self.nodes[l.index()].mark != base {
                    self.nodes[l.index()].mark = base;
                    out.push(l);
                }
            }
        }
    }
}
