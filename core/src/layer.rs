//! One capability's topology: nodes, links, networks, endpoints' ports,
//! side configuration and chunk tracking.
//!
//! **Pipes branch.** A node has a fixed `[NodeId; 6]` adjacency by face, so
//! anything from 0 to 6 neighbours (T-junctions, crosses, loops) is the
//! normal case, and a BFS walks contiguous memory without hashing.
//!
//! **Networks are maintained incrementally.** Adding nodes unions the
//! networks they touch (the largest keeps its id, the others are relabelled
//! — union by size). Removing a node or cutting a link runs an interleaved
//! BFS from the affected neighbours that stops as soon as at most one piece
//! is still open, so its cost is proportional to the pieces that split off,
//! not to the network (see [`Layer::split_check`]).
//!
//! The layer knows nothing about models, the world or blocks beyond the
//! attach table: the engine (`crate::engine`) drives discovery and tells
//! the models what happened.

use std::collections::VecDeque;

use crate::ids::{
    ChunkKey, EpId, EpTypeId, Face, FxHashMap, FxHashSet, NO_NODE, NetId, NodeId, Pos,
};
use crate::registry::{Attach, Side, SideMode, groups_meet};

/// "No network" in a node (only between insertion and settling).
const NO_NET: u32 = u32::MAX;

/// Next-hop value for the destination itself.
pub const HOP_HERE: u8 = 6;

#[derive(Debug, Clone)]
pub struct Node {
    pub pos: Pos,
    /// Index into the layer's attach table.
    pub att: u16,
    /// Color partition: 0 links to anything, otherwise only to 0 or equal.
    pub color: u8,
    pub links: [NodeId; 6],
    net: u32,
    mark: u32,
}

impl Node {
    #[inline]
    pub fn net_index(&self) -> u32 {
        self.net
    }
}

/// An endpoint's connection to one node through one face — the unit
/// throughput, priority and side mode apply to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Port {
    pub ep: EpId,
    pub node: NodeId,
    /// The node's face that points at the endpoint.
    pub face: Face,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub pos: Pos,
    pub ty: EpTypeId,
    ports: u8,
}

#[derive(Debug, Clone)]
pub struct Network {
    pub size: u32,
    /// `(attach index, node count)` — aggregates (capacity, bottleneck)
    /// come from here, never from walking nodes.
    pub types: Vec<(u16, u32)>,
    pub ports: Vec<Port>,
    /// Bumped (to a layer-unique value) on every topology or port change;
    /// caches keyed by it die with the change.
    pub version: u32,
    /// Nodes of this network in unloaded chunks; > 0 = frozen.
    pub frozen: u32,
    /// Any node of the network (relabelling start, saved representative).
    pub any_node: NodeId,
    pub(crate) active: bool,
    pub(crate) wake_at: u64,
}

impl Default for Network {
    fn default() -> Self {
        Network {
            size: 0,
            types: Vec::new(),
            ports: Vec::new(),
            version: 0,
            frozen: 0,
            any_node: NO_NODE,
            active: false,
            wake_at: 0,
        }
    }
}

impl Network {
    fn add_type(&mut self, att: u16, n: u32) {
        match self.types.iter_mut().find(|(a, _)| *a == att) {
            Some(t) => t.1 += n,
            None => self.types.push((att, n)),
        }
    }

    fn sub_type(&mut self, att: u16, n: u32) {
        if let Some(i) = self.types.iter().position(|(a, _)| *a == att) {
            self.types[i].1 -= n;
            if self.types[i].1 == 0 {
                self.types.swap_remove(i);
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct NetSlot {
    generation: u32,
    net: Option<Network>,
}

/// What [`Layer::settle`] did with a freshly added region.
#[derive(Debug, Clone, Default)]
pub struct Settled {
    pub net: Option<NetId>,
    pub created: bool,
    /// Networks absorbed into `net`, with their sizes before the merge.
    pub absorbed: Vec<(NetId, u32)>,
}

/// What a removal or a cut did to the network it touched.
#[derive(Debug, Clone, Default)]
pub struct Split {
    /// The network that lost the node/link (still alive unless `emptied`).
    pub net: Option<NetId>,
    pub emptied: bool,
    /// New networks split off `net`, with their sizes.
    pub parts: Vec<(NetId, u32)>,
}

#[derive(Debug, Default)]
pub struct Layer {
    pub atts: Vec<Attach>,
    compat: Vec<Vec<bool>>,
    nodes: Vec<Node>,
    free_nodes: Vec<u32>,
    index: FxHashMap<Pos, NodeId>,
    nets: Vec<NetSlot>,
    free_nets: Vec<u32>,
    eps: Vec<Option<Endpoint>>,
    free_eps: Vec<u32>,
    ep_index: FxHashMap<Pos, EpId>,
    sides: FxHashMap<(NodeId, Face), Side>,
    chunks: FxHashMap<ChunkKey, Vec<NodeId>>,
    unloaded: FxHashSet<ChunkKey>,
    epoch: u32,
    versions: u32,
    pub(crate) active: Vec<u32>,
    dist_cache: FxHashMap<(u32, NodeId), (u32, Vec<u32>)>,
    tree_cache: FxHashMap<NodeId, (u32, u32, FxHashMap<NodeId, u8>)>,
    scratch: Vec<NodeId>,
}

impl Layer {
    // ── Attach table ─────────────────────────────────────────────────────

    /// Add (or replace, for the same block type) an attach entry and
    /// recompute which entries link to which.
    pub fn set_attach(&mut self, slot: Option<u16>, att: Attach) -> u16 {
        let i = match slot {
            Some(i) => {
                self.atts[i as usize] = att;
                i
            }
            None => {
                self.atts.push(att);
                (self.atts.len() - 1) as u16
            }
        };
        let n = self.atts.len();
        self.compat = (0..n)
            .map(|a| {
                (0..n)
                    .map(|b| groups_meet(&self.atts[a].groups, &self.atts[b].groups))
                    .collect()
            })
            .collect();
        i
    }

    // ── Nodes ────────────────────────────────────────────────────────────

    #[inline]
    pub fn node_at(&self, pos: Pos) -> Option<NodeId> {
        self.index.get(&pos).copied()
    }

    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn node_count(&self) -> usize {
        self.index.len()
    }

    /// Every live node (for saving and debugging; O(slab)).
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (NodeId(i as u32), n))
            .filter(|(id, n)| self.index.get(&n.pos) == Some(id))
    }

    /// The highest node index + 1 (for dense per-node model storage).
    pub fn node_capacity(&self) -> usize {
        self.nodes.len()
    }

    #[inline]
    pub fn param(&self, node: NodeId, p: usize) -> f64 {
        self.atts[self.node(node).att as usize]
            .params
            .get(p)
            .copied()
            .unwrap_or(0.0)
    }

    pub fn side(&self, node: NodeId, face: Face) -> Side {
        if self.sides.is_empty() {
            return Side::default();
        }
        self.sides.get(&(node, face)).copied().unwrap_or_default()
    }

    /// Whether `a` (through `face`) and `b` may link.
    pub fn linkable(&self, a: NodeId, face: Face, b: NodeId) -> bool {
        let (na, nb) = (self.node(a), self.node(b));
        self.compat[na.att as usize][nb.att as usize]
            && (na.color == 0 || nb.color == 0 || na.color == nb.color)
            && self.side(a, face).mode != SideMode::Off
            && self.side(b, face.opposite()).mode != SideMode::Off
    }

    /// Add an unlinked, unsettled node.
    pub fn insert_node(&mut self, pos: Pos, att: u16) -> NodeId {
        let node = Node {
            pos,
            att,
            color: 0,
            links: [NO_NODE; 6],
            net: NO_NET,
            mark: 0,
        };
        let id = match self.free_nodes.pop() {
            Some(i) => {
                self.nodes[i as usize] = node;
                NodeId(i)
            }
            None => {
                self.nodes.push(node);
                NodeId(self.nodes.len() as u32 - 1)
            }
        };
        self.index.insert(pos, id);
        self.chunks.entry(pos.chunk()).or_default().push(id);
        id
    }

    /// Link `a` to `b` through `a`'s `face` (no network bookkeeping).
    pub fn link_raw(&mut self, a: NodeId, face: Face, b: NodeId) {
        self.nodes[a.index()].links[face as usize] = b;
        self.nodes[b.index()].links[face.opposite() as usize] = a;
    }

    // ── Networks ─────────────────────────────────────────────────────────

    #[inline]
    pub fn net_id(&self, index: u32) -> NetId {
        NetId {
            index,
            generation: self.nets[index as usize].generation,
        }
    }

    /// The network a node is in.
    #[inline]
    pub fn net_of(&self, node: NodeId) -> Option<NetId> {
        let i = self.node(node).net;
        (i != NO_NET).then(|| self.net_id(i))
    }

    pub fn net(&self, id: NetId) -> Option<&Network> {
        let slot = self.nets.get(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.net.as_ref()
    }

    pub(crate) fn net_mut(&mut self, id: NetId) -> Option<&mut Network> {
        let slot = self.nets.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.net.as_mut()
    }

    /// Every live network.
    pub fn nets(&self) -> impl Iterator<Item = (NetId, &Network)> {
        self.nets.iter().enumerate().filter_map(|(i, s)| {
            s.net.as_ref().map(|n| {
                (
                    NetId {
                        index: i as u32,
                        generation: s.generation,
                    },
                    n,
                )
            })
        })
    }

    fn new_net(&mut self) -> NetId {
        let index = match self.free_nets.pop() {
            Some(i) => i,
            None => {
                self.nets.push(NetSlot::default());
                self.nets.len() as u32 - 1
            }
        };
        let version = self.next_version();
        let slot = &mut self.nets[index as usize];
        slot.net = Some(Network {
            version,
            any_node: NO_NODE,
            ..Default::default()
        });
        NetId {
            index,
            generation: slot.generation,
        }
    }

    fn free_net(&mut self, id: NetId) {
        let slot = &mut self.nets[id.index as usize];
        slot.net = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.free_nets.push(id.index);
    }

    fn next_version(&mut self) -> u32 {
        self.versions = self.versions.wrapping_add(1);
        self.versions
    }

    /// Mark a network changed: new version, and wake it.
    pub(crate) fn touch(&mut self, id: NetId) {
        let v = self.next_version();
        if let Some(n) = self.net_mut(id) {
            n.version = v;
        }
        self.activate(id, 0);
    }

    /// Put a network on the active list (no-op if already there); it is
    /// ticked from `at` on.
    pub(crate) fn activate(&mut self, id: NetId, at: u64) {
        let Some(n) = self.net_mut(id) else {
            return;
        };
        n.wake_at = if n.active { n.wake_at.min(at) } else { at };
        if !n.active {
            n.active = true;
            self.active.push(id.index);
        }
    }

    pub fn param_sum(&self, id: NetId, p: usize) -> f64 {
        self.net(id).map_or(0.0, |n| {
            n.types
                .iter()
                .map(|&(a, c)| {
                    self.atts[a as usize].params.get(p).copied().unwrap_or(0.0) * c as f64
                })
                .sum()
        })
    }

    pub fn param_min(&self, id: NetId, p: usize) -> f64 {
        self.net(id).map_or(0.0, |n| {
            n.types
                .iter()
                .map(|&(a, _)| self.atts[a as usize].params.get(p).copied().unwrap_or(0.0))
                .fold(f64::INFINITY, f64::min)
        })
    }

    pub fn param_max(&self, id: NetId, p: usize) -> f64 {
        self.net(id).map_or(0.0, |n| {
            n.types
                .iter()
                .map(|&(a, _)| self.atts[a as usize].params.get(p).copied().unwrap_or(0.0))
                .fold(0.0, f64::max)
        })
    }

    /// Smallest non-zero value of a parameter (0 = none set: no limit).
    pub fn param_min_nonzero(&self, id: NetId, p: usize) -> f64 {
        self.net(id).map_or(0.0, |n| {
            let m = n
                .types
                .iter()
                .map(|&(a, _)| self.atts[a as usize].params.get(p).copied().unwrap_or(0.0))
                .filter(|&v| v > 0.0)
                .fold(f64::INFINITY, f64::min);
            if m.is_finite() { m } else { 0.0 }
        })
    }

    /// Bump the BFS epoch by `k` and return the first stamp of the new
    /// range. Marks are reset before the counter could wrap.
    fn stamps(&mut self, k: u32) -> u32 {
        if self.epoch > u32::MAX - 64 {
            for n in &mut self.nodes {
                n.mark = 0;
            }
            self.epoch = 0;
        }
        let base = self.epoch + 1;
        self.epoch += k;
        base
    }

    /// Give a freshly inserted, internally linked region its network: a
    /// new one, or the largest network it touches, absorbing the others.
    pub fn settle(&mut self, region: &[NodeId]) -> Settled {
        if region.is_empty() {
            return Settled::default();
        }
        // Distinct networks touched, with one node of each.
        let mut touched: Vec<(u32, NodeId)> = Vec::new();
        for &n in region {
            for l in self.nodes[n.index()].links {
                if l == NO_NODE {
                    continue;
                }
                let ni = self.nodes[l.index()].net;
                if ni != NO_NET && !touched.iter().any(|t| t.0 == ni) {
                    touched.push((ni, l));
                }
            }
        }
        let mut out = Settled::default();
        let target = match touched
            .iter()
            .max_by_key(|(i, _)| self.nets[*i as usize].net.as_ref().map_or(0, |n| n.size))
        {
            Some(&(i, _)) => self.net_id(i),
            None => {
                out.created = true;
                self.new_net()
            }
        };
        for &(i, start) in &touched {
            if i == target.index {
                continue;
            }
            let from = self.net_id(i);
            let size = self.net(from).map_or(0, |n| n.size);
            self.absorb(target, from, start);
            out.absorbed.push((from, size));
        }
        for &n in region {
            self.nodes[n.index()].net = target.index;
            let (att, chunk) = (self.nodes[n.index()].att, self.nodes[n.index()].pos.chunk());
            let frozen = self.unloaded.contains(&chunk) as u32;
            let net = self.net_mut(target).unwrap();
            net.size += 1;
            net.frozen += frozen;
            net.add_type(att, 1);
            if net.any_node == NO_NODE {
                net.any_node = n;
            }
        }
        self.touch(target);
        out.net = Some(target);
        out
    }

    /// Relabel every node of `from` (reached from `start`) into `into` and
    /// move its ports, counts and flags.
    fn absorb(&mut self, into: NetId, from: NetId, start: NodeId) {
        let mut queue = std::mem::take(&mut self.scratch);
        queue.clear();
        queue.push(start);
        self.nodes[start.index()].net = into.index;
        while let Some(c) = queue.pop() {
            for l in self.nodes[c.index()].links {
                if l != NO_NODE && self.nodes[l.index()].net == from.index {
                    self.nodes[l.index()].net = into.index;
                    queue.push(l);
                }
            }
        }
        self.scratch = queue;
        let old = self.nets[from.index as usize].net.take().unwrap();
        self.free_net_slot(from);
        let net = self.net_mut(into).unwrap();
        net.size += old.size;
        net.frozen += old.frozen;
        for (a, c) in old.types {
            net.add_type(a, c);
        }
        net.ports.extend(old.ports);
    }

    fn free_net_slot(&mut self, id: NetId) {
        let slot = &mut self.nets[id.index as usize];
        slot.generation = slot.generation.wrapping_add(1);
        self.free_nets.push(id.index);
    }

    /// Link two settled nodes; merges their networks if they differ.
    /// Returns `(into, from, from's size)` when a merge happened.
    pub fn link_settled(
        &mut self,
        a: NodeId,
        face: Face,
        b: NodeId,
    ) -> Option<(NetId, NetId, u32)> {
        self.link_raw(a, face, b);
        let (na, nb) = (self.net_of(a)?, self.net_of(b)?);
        if na == nb {
            self.touch(na);
            return None;
        }
        let (sa, sb) = (self.net(na)?.size, self.net(nb)?.size);
        let (into, from, start) = if sa >= sb { (na, nb, b) } else { (nb, na, a) };
        let size = sa.min(sb);
        self.absorb(into, from, start);
        self.touch(into);
        Some((into, from, size))
    }

    /// Remove a node: drop its ports, unlink it, and split its network if
    /// that disconnected it.
    pub fn remove_node(&mut self, node: NodeId) -> Split {
        let net = self.net_of(node);
        // Ports of this node.
        if let Some(id) = net {
            let n = self.net_mut(id).unwrap();
            let mut gone = Vec::new();
            n.ports.retain(|p| {
                let keep = p.node != node;
                if !keep {
                    gone.push(p.ep);
                }
                keep
            });
            for ep in gone {
                self.drop_port_count(ep);
            }
        }
        let mut seeds: Vec<NodeId> = Vec::with_capacity(6);
        for f in Face::ALL {
            let l = self.nodes[node.index()].links[f as usize];
            if l != NO_NODE {
                self.nodes[l.index()].links[f.opposite() as usize] = NO_NODE;
                self.nodes[node.index()].links[f as usize] = NO_NODE;
                if !seeds.contains(&l) {
                    seeds.push(l);
                }
            }
        }
        for f in Face::ALL {
            self.sides.remove(&(node, f));
        }
        let pos = self.nodes[node.index()].pos;
        self.index.remove(&pos);
        if let Some(v) = self.chunks.get_mut(&pos.chunk()) {
            v.retain(|&n| n != node);
            if v.is_empty() {
                self.chunks.remove(&pos.chunk());
            }
        }
        let att = self.nodes[node.index()].att;
        self.nodes[node.index()].net = NO_NET;
        self.free_nodes.push(node.0);
        let Some(id) = net else {
            return Split::default();
        };
        let frozen = self.unloaded.contains(&pos.chunk()) as u32;
        let n = self.net_mut(id).unwrap();
        n.size -= 1;
        n.frozen -= frozen;
        n.sub_type(att, 1);
        if n.size == 0 {
            self.free_net(id);
            return Split {
                net: Some(id),
                emptied: true,
                parts: Vec::new(),
            };
        }
        if n.any_node == node {
            n.any_node = seeds[0];
        }
        let parts = self.split_check(id, &seeds);
        self.touch(id);
        Split {
            net: Some(id),
            emptied: false,
            parts,
        }
    }

    /// Cut the link on `a`'s `face` (side set to off, color change).
    pub fn unlink(&mut self, a: NodeId, face: Face) -> Split {
        let b = self.nodes[a.index()].links[face as usize];
        if b == NO_NODE {
            return Split::default();
        }
        self.nodes[a.index()].links[face as usize] = NO_NODE;
        self.nodes[b.index()].links[face.opposite() as usize] = NO_NODE;
        let Some(id) = self.net_of(a) else {
            return Split::default();
        };
        let parts = self.split_check(id, &[a, b]);
        self.touch(id);
        Split {
            net: Some(id),
            emptied: false,
            parts,
        }
    }

    /// After `net` lost a node or link, find out whether `seeds` (the
    /// former neighbours, all still in `net`) are still connected.
    ///
    /// One BFS per seed, interleaved one node per search per round, with a
    /// union-find over the searches (two that meet are the same piece). A
    /// piece whose frontier runs dry is a complete component. As soon as at
    /// most one piece is still open, every closed piece becomes its own
    /// network and the open one keeps `net` without being walked to the
    /// end — so the cost is proportional to the pieces that split off.
    pub fn split_check(&mut self, net: NetId, seeds: &[NodeId]) -> Vec<(NetId, u32)> {
        let k = seeds.len();
        if k <= 1 {
            return Vec::new();
        }
        let base = self.stamps(k as u32);
        let mut parent: Vec<usize> = (0..k).collect();
        fn find(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        let mut queues: Vec<VecDeque<NodeId>> = vec![VecDeque::new(); k];
        let mut seen: Vec<Vec<NodeId>> = vec![Vec::new(); k];
        for (i, &s) in seeds.iter().enumerate() {
            let m = self.nodes[s.index()].mark;
            if m >= base && m < base + k as u32 {
                // Duplicate seed: already some other search's start.
                let j = (m - base) as usize;
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                parent[ri] = rj;
                continue;
            }
            self.nodes[s.index()].mark = base + i as u32;
            queues[i].push_back(s);
            seen[i].push(s);
        }
        loop {
            // Which pieces are still open?
            let mut open_roots: Vec<usize> = Vec::with_capacity(k);
            for (i, q) in queues.iter().enumerate() {
                if !q.is_empty() {
                    let r = find(&mut parent, i);
                    if !open_roots.contains(&r) {
                        open_roots.push(r);
                    }
                }
            }
            if open_roots.len() <= 1 {
                break;
            }
            for i in 0..k {
                let Some(c) = queues[i].pop_front() else {
                    continue;
                };
                for l in self.nodes[c.index()].links {
                    if l == NO_NODE {
                        continue;
                    }
                    let m = self.nodes[l.index()].mark;
                    if m >= base && m < base + k as u32 {
                        let j = (m - base) as usize;
                        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                        if ri != rj {
                            parent[ri] = rj;
                        }
                        continue;
                    }
                    self.nodes[l.index()].mark = base + i as u32;
                    queues[i].push_back(l);
                    seen[i].push(l);
                }
            }
        }
        // Group searches by piece.
        let mut pieces: Vec<(usize, bool, u32)> = Vec::new(); // (root, open, size)
        for i in 0..k {
            let r = find(&mut parent, i);
            let open = !queues[i].is_empty();
            match pieces.iter_mut().find(|p| p.0 == r) {
                Some(p) => {
                    p.1 |= open;
                    p.2 += seen[i].len() as u32;
                }
                None => pieces.push((r, open, seen[i].len() as u32)),
            }
        }
        if pieces.len() <= 1 {
            return Vec::new();
        }
        // The piece that keeps `net`: the open one, else the largest.
        let keep = pieces
            .iter()
            .find(|p| p.1)
            .or_else(|| pieces.iter().max_by_key(|p| p.2))
            .map(|p| p.0)
            .unwrap();
        let mut out = Vec::new();
        for &(root, _, size) in &pieces {
            if root == keep {
                continue;
            }
            let id = self.new_net();
            let mut types: Vec<(u16, u32)> = Vec::new();
            let mut frozen = 0;
            let mut first = NO_NODE;
            for (i, part) in seen.iter().enumerate() {
                if find(&mut parent, i) != root {
                    continue;
                }
                for &n in part {
                    let node = &mut self.nodes[n.index()];
                    node.net = id.index;
                    if first == NO_NODE {
                        first = n;
                    }
                    match types.iter_mut().find(|t| t.0 == node.att) {
                        Some(t) => t.1 += 1,
                        None => types.push((node.att, 1)),
                    }
                    if !self.unloaded.is_empty() && self.unloaded.contains(&node.pos.chunk()) {
                        frozen += 1;
                    }
                }
            }
            let old = self.net_mut(net).unwrap();
            old.size -= size;
            old.frozen -= frozen;
            for &(a, c) in &types {
                old.sub_type(a, c);
            }
            let ports: Vec<Port>;
            {
                let all = std::mem::take(&mut old.ports);
                let (moved, kept): (Vec<Port>, Vec<Port>) = all
                    .into_iter()
                    .partition(|p| self.nodes[p.node.index()].net == id.index);
                ports = moved;
                self.net_mut(net).unwrap().ports = kept;
            }
            if self
                .net(net)
                .is_some_and(|n| self.nodes[n.any_node.index()].net != net.index)
            {
                // The representative moved out; any kept seed will do.
                let kept_seed = (0..k)
                    .find(|&i| find(&mut parent, i) == keep)
                    .map(|i| seeds[i])
                    .unwrap();
                self.net_mut(net).unwrap().any_node = kept_seed;
            }
            let new = self.net_mut(id).unwrap();
            new.size = size;
            new.types = types;
            new.frozen = frozen;
            new.ports = ports;
            new.any_node = first;
            self.activate(id, 0);
            out.push((id, size));
        }
        out
    }

    // ── Endpoints and ports ──────────────────────────────────────────────

    pub fn endpoint_at(&self, pos: Pos) -> Option<EpId> {
        self.ep_index.get(&pos).copied()
    }

    pub fn endpoint(&self, id: EpId) -> &Endpoint {
        self.eps[id.index()].as_ref().expect("live endpoint")
    }

    /// Get or create the endpoint instance at `pos`.
    pub fn ensure_endpoint(&mut self, pos: Pos, ty: EpTypeId) -> EpId {
        if let Some(&e) = self.ep_index.get(&pos) {
            self.eps[e.index()].as_mut().unwrap().ty = ty;
            return e;
        }
        let ep = Endpoint { pos, ty, ports: 0 };
        let id = match self.free_eps.pop() {
            Some(i) => {
                self.eps[i as usize] = Some(ep);
                EpId(i)
            }
            None => {
                self.eps.push(Some(ep));
                EpId(self.eps.len() as u32 - 1)
            }
        };
        self.ep_index.insert(pos, id);
        id
    }

    /// Connect `ep` to `node` through the node's `face` (idempotent).
    pub fn add_port(&mut self, ep: EpId, node: NodeId, face: Face) {
        let Some(id) = self.net_of(node) else {
            return;
        };
        let port = Port { ep, node, face };
        let net = self.net_mut(id).unwrap();
        if net.ports.contains(&port) {
            return;
        }
        net.ports.push(port);
        self.eps[ep.index()].as_mut().unwrap().ports += 1;
        self.touch(id);
    }

    fn drop_port_count(&mut self, ep: EpId) {
        let e = self.eps[ep.index()].as_mut().unwrap();
        e.ports -= 1;
        if e.ports == 0 {
            let pos = e.pos;
            self.eps[ep.index()] = None;
            self.free_eps.push(ep.0);
            self.ep_index.remove(&pos);
        }
    }

    /// Disconnect and forget the endpoint at `pos`. Returns the networks
    /// it was connected to.
    pub fn remove_endpoint(&mut self, pos: Pos) -> Vec<NetId> {
        let Some(ep) = self.ep_index.get(&pos).copied() else {
            return Vec::new();
        };
        let mut nets = Vec::new();
        for f in Face::ALL {
            let Some(node) = self.node_at(pos.offset(f)) else {
                continue;
            };
            let Some(id) = self.net_of(node) else {
                continue;
            };
            let net = self.net_mut(id).unwrap();
            let before = net.ports.len();
            net.ports.retain(|p| !(p.ep == ep && p.node == node));
            if net.ports.len() != before && !nets.contains(&id) {
                nets.push(id);
            }
        }
        if self.eps[ep.index()].is_some() {
            self.eps[ep.index()] = None;
            self.free_eps.push(ep.0);
        }
        self.ep_index.remove(&pos);
        for &n in &nets {
            self.touch(n);
        }
        nets
    }

    // ── Side configuration and color ────────────────────────────────────

    pub fn set_side_raw(&mut self, node: NodeId, face: Face, side: Side) {
        if side == Side::default() {
            self.sides.remove(&(node, face));
        } else {
            self.sides.insert((node, face), side);
        }
        if let Some(id) = self.net_of(node) {
            self.touch(id);
        }
    }

    pub fn set_color_raw(&mut self, node: NodeId, color: u8) {
        self.nodes[node.index()].color = color;
    }

    /// Every side configured away from the default (for saving).
    pub fn configured_sides(&self) -> impl Iterator<Item = (Pos, Face, Side)> + '_ {
        self.sides
            .iter()
            .map(|(&(n, f), &s)| (self.node(n).pos, f, s))
    }

    // ── Chunk loading ────────────────────────────────────────────────────

    /// Track a chunk's load state; networks with a node in an unloaded
    /// chunk are frozen (not ticked). O(nodes in the chunk).
    pub fn set_chunk_loaded(&mut self, chunk: ChunkKey, loaded: bool) {
        let changed = if loaded {
            self.unloaded.remove(&chunk)
        } else {
            self.unloaded.insert(chunk)
        };
        if !changed {
            return;
        }
        let nodes = self.chunks.get(&chunk).cloned().unwrap_or_default();
        for n in nodes {
            let Some(id) = self.net_of(n) else {
                continue;
            };
            let net = self.net_mut(id).unwrap();
            if loaded {
                net.frozen -= 1;
            } else {
                net.frozen += 1;
            }
            self.activate(id, 0);
        }
    }

    // ── Cached searches ──────────────────────────────────────────────────

    /// BFS hop distance from `from` to every port of its network, in
    /// `ports` order (`u32::MAX` = unreachable), cached per version.
    pub fn port_distances(&mut self, from: NodeId) -> &[u32] {
        let id = self.net_of(from).expect("settled node");
        let version = self.net(id).unwrap().version;
        let key = (id.index, from);
        let fresh = self
            .dist_cache
            .get(&key)
            .is_some_and(|(v, _)| *v == version);
        if !fresh {
            let base = self.stamps(1);
            let mut dist: FxHashMap<NodeId, u32> = FxHashMap::default();
            let mut queue = VecDeque::from([(from, 0u32)]);
            self.nodes[from.index()].mark = base;
            let want: FxHashSet<NodeId> =
                self.net(id).unwrap().ports.iter().map(|p| p.node).collect();
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
            let out = self
                .net(id)
                .unwrap()
                .ports
                .iter()
                .map(|p| dist.get(&p.node).copied().unwrap_or(u32::MAX))
                .collect();
            self.dist_cache.insert(key, (version, out));
        }
        &self.dist_cache[&key].1
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
        self.tree_cache[&dest].2.get(&from).copied()
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
