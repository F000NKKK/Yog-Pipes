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

mod endpoints;
mod merge;
mod search;
mod split;

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
    /// False once removed: its slot waits on the free list for reuse.
    live: bool,
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
    eps: Vec<Endpoint>,
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

    /// A new network holding `net`, under a fresh version.
    fn new_net(&mut self, net: Network) -> NetId {
        let index = match self.free_nets.pop() {
            Some(i) => i,
            None => {
                self.nets.push(NetSlot::default());
                self.nets.len() as u32 - 1
            }
        };
        let version = self.next_version();
        let slot = &mut self.nets[index as usize];
        slot.net = Some(Network { version, ..net });
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
}
