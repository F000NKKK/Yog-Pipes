//! Virtual pipe graph — BFS-based adjacency network.
//!
//! When a pipe is placed or broken, the graph is rebuilt from that position
//! via BFS through real world blocks: a neighbor becomes an edge only when
//! it is itself a block registered via `crate::pipe::register_pipe` (see
//! `crate::pipe::is_pipe_block`) — never guessed from the block id's
//! spelling and never assumed present without checking the world.
//!
//! **Pipes branch.** A node is not assumed to have exactly one neighbor in
//! each direction — any pipe block can have anywhere from zero to six
//! connected neighbors (a T-junction, a cross, …). Edges are stored as a
//! proper adjacency map (`NodeKey -> HashSet<NodeKey>`), so every traversal
//! below (`rebuild_graph`, `deliver_broadcast`, `find_path`) walks *all* of
//! a node's neighbors in O(degree), not by scanning the whole edge set or
//! assuming a fixed fan-out.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex};

use yog_api::BlockPos;

use crate::payload::PipePayload;

/// A position in the world: (dimension, x, y, z).
pub type NodeKey = (String, i32, i32, i32);

/// A node in the pipe graph.
#[derive(Debug, Clone, Default)]
pub struct PipeNode {
    /// The last payload delivered to this node, if any — set by
    /// `deliver_broadcast` or a direct `store_payload` call.
    pub payload: Option<PipePayload>,
}

/// The full pipe network graph: an adjacency map (a node can have any
/// number of neighbors — branching is the normal case, not an edge case).
#[derive(Debug, Clone, Default)]
pub struct PipeGraph {
    pub nodes: HashMap<NodeKey, PipeNode>,
    pub adjacency: HashMap<NodeKey, HashSet<NodeKey>>,
}

impl PipeGraph {
    fn link(&mut self, a: NodeKey, b: NodeKey) {
        self.adjacency
            .entry(a.clone())
            .or_default()
            .insert(b.clone());
        self.adjacency.entry(b).or_default().insert(a);
    }

    fn neighbors(&self, pos: &NodeKey) -> impl Iterator<Item = &NodeKey> {
        self.adjacency.get(pos).into_iter().flatten()
    }
}

/// Global pipe graph, rebuilt on placement/break.
pub static GRAPH: LazyLock<Mutex<PipeGraph>> = LazyLock::new(|| Mutex::new(PipeGraph::default()));

const NEIGHBOR_OFFSETS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

fn is_pipe_at(world: &yog_api::World, pos: &NodeKey) -> bool {
    world
        .get_block(BlockPos {
            x: pos.1,
            y: pos.2,
            z: pos.3,
        })
        .map(|id| crate::pipe::is_pipe_block(&id))
        .unwrap_or(false)
}

/// Rebuild the graph around `pos` using BFS through pipe blocks actually
/// present in the world (`srv`/`dim` resolve real block ids via
/// `yog_api::World`, never assumed). Handles any branching factor per node.
pub fn rebuild_graph(srv: &dyn yog_api::Server, dim: &str, x: i32, y: i32, z: i32) {
    let world = yog_api::World::new(srv, dim);
    let mut graph = GRAPH.lock().unwrap();

    // Drop stale nodes/adjacency touching a position that's no longer a
    // pipe — covers both the break case and pipes removed elsewhere.
    graph.nodes.retain(|pos, _| is_pipe_at(&world, pos));
    graph.adjacency.retain(|pos, _| is_pipe_at(&world, pos));
    for neighbors in graph.adjacency.values_mut() {
        neighbors.retain(|pos| is_pipe_at(&world, pos));
    }

    let start: NodeKey = (dim.to_string(), x, y, z);
    if !is_pipe_at(&world, &start) {
        return;
    }

    let mut visited: HashSet<NodeKey> = HashSet::new();
    let mut queue: VecDeque<NodeKey> = VecDeque::new();
    queue.push_back(start);

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());
        graph.nodes.entry(current.clone()).or_default();

        for (dx, dy, dz) in NEIGHBOR_OFFSETS {
            let neighbor: NodeKey = (
                current.0.clone(),
                current.1 + dx,
                current.2 + dy,
                current.3 + dz,
            );
            if !is_pipe_at(&world, &neighbor) {
                continue;
            }
            graph.link(current.clone(), neighbor.clone());
            if !visited.contains(&neighbor) {
                queue.push_back(neighbor);
            }
        }
    }
}

/// Broadcast `payload` from `source` to every node reachable through the
/// network (BFS over *all* neighbors at each branch, not just two) —
/// storing a clone at each reached node — and return the reached nodes so
/// the caller can dispatch any handlers bound there. Yog-Pipes does not
/// attenuate or otherwise interpret the payload; a mod that wants
/// distance-based falloff (like redstone) encodes that itself via
/// `PipePayload::metadata` and re-broadcasts, or calls `send` for
/// point-to-point delivery instead.
pub fn deliver_broadcast(source: NodeKey, payload: PipePayload) -> Vec<NodeKey> {
    let mut graph = GRAPH.lock().unwrap();
    if !graph.nodes.contains_key(&source) {
        return Vec::new();
    }

    let mut visited: HashSet<NodeKey> = HashSet::new();
    let mut queue: VecDeque<NodeKey> = VecDeque::new();
    queue.push_back(source);

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());
        for next in graph.neighbors(&current).cloned().collect::<Vec<_>>() {
            if !visited.contains(&next) {
                queue.push_back(next);
            }
        }
    }

    for pos in &visited {
        graph.nodes.entry(pos.clone()).or_default().payload = Some(payload.clone());
    }

    visited.into_iter().collect()
}

/// Store `payload` at `pos` directly (used by point-to-point `send`,
/// after `find_path` confirms a route exists).
pub fn store_payload(pos: &NodeKey, payload: PipePayload) {
    GRAPH
        .lock()
        .unwrap()
        .nodes
        .entry(pos.clone())
        .or_default()
        .payload = Some(payload);
}

/// The payload last delivered to `pos`, if any.
pub fn payload_at(pos: &NodeKey) -> Option<PipePayload> {
    GRAPH
        .lock()
        .unwrap()
        .nodes
        .get(pos)
        .and_then(|n| n.payload.clone())
}

/// Find the shortest path between two nodes using BFS (any branching
/// factor per node — this is a general graph search, not a linear-chain
/// assumption). Marks `from` visited up front: a pipe network can contain
/// loops (e.g. a ring of branches reconnecting), and without that a cycle
/// back to the start would keep re-queuing it forever instead of just
/// being ignored as an already-explored node.
pub fn find_path(from: &NodeKey, to: &NodeKey) -> Option<Vec<NodeKey>> {
    let graph = GRAPH.lock().unwrap();
    let mut visited: HashSet<NodeKey> = HashSet::new();
    let mut queue: VecDeque<NodeKey> = VecDeque::new();
    let mut came_from: HashMap<NodeKey, NodeKey> = HashMap::new();
    visited.insert(from.clone());
    queue.push_back(from.clone());

    while let Some(current) = queue.pop_front() {
        if &current == to {
            let mut path = vec![current.clone()];
            let mut cur = current;
            while let Some(prev) = came_from.get(&cur) {
                path.push(prev.clone());
                cur = prev.clone();
            }
            path.reverse();
            return Some(path);
        }
        for next in graph.neighbors(&current) {
            if visited.contains(next) {
                continue;
            }
            visited.insert(next.clone());
            came_from.insert(next.clone(), current.clone());
            queue.push_back(next.clone());
        }
    }
    None
}
