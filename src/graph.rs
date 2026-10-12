//! Virtual pipe graph — an incrementally maintained adjacency network.
//!
//! The graph mirrors the world: a position is a node only when it holds a
//! block registered via `crate::pipe::register_pipe`, and two adjacent
//! nodes are linked only when their link groups share at least one tag —
//! the same rule `yog-registry` uses to grow the blocks' visual arms, so the
//! graph never connects pipes the player sees as disconnected.
//!
//! **Pipes branch.** A node is not assumed to have exactly one neighbor in
//! each direction — any pipe block can have anywhere from zero to six
//! connected neighbors (a T-junction, a cross, …). Edges are stored as a
//! proper adjacency map (`NodeKey -> HashSet<NodeKey>`), so every traversal
//! below walks *all* of a node's neighbors in O(degree), not by scanning the
//! whole edge set or assuming a fixed fan-out.
//!
//! **Updates are local.** Placing a pipe explores only positions the graph
//! doesn't know yet; breaking one removes just that node and its edges.
//! Nothing ever rescans the whole network, and one dimension's changes never
//! touch another dimension's nodes. Positions the graph has never seen (a
//! network built before a restart, or placed by `/setblock`) are discovered
//! lazily the first time a transport call touches them.
//!
//! World access goes through a `probe` closure (position → the pipe's link
//! groups, or `None` if no pipe is there) so the topology logic stays
//! independent of the live server — `lib.rs`/`transport.rs` build the probe
//! from `yog_api::World`, the tests below from a plain map.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex, MutexGuard};

use crate::payload::PipePayload;

/// A position in the world: (dimension, x, y, z). The dimension is always a
/// namespaced id (`minecraft:overworld`) — see [`normalize_dim`].
pub type NodeKey = (String, i32, i32, i32);

/// Upper bound on positions one discovery may add, so a pathological build
/// (or a probe bug) can't stall the server thread on an endless walk.
const MAX_DISCOVERY: usize = 1 << 16;

/// The full pipe network topology: an adjacency map. A key exists for every
/// known node, even an isolated one (empty neighbor set).
#[derive(Debug, Clone, Default)]
pub struct PipeGraph {
    adjacency: HashMap<NodeKey, HashSet<NodeKey>>,
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

    fn contains(&self, pos: &NodeKey) -> bool {
        self.adjacency.contains_key(pos)
    }

    /// Drop `pos` and every edge touching it.
    fn remove(&mut self, pos: &NodeKey) {
        if let Some(neighbors) = self.adjacency.remove(pos) {
            for n in neighbors {
                if let Some(set) = self.adjacency.get_mut(&n) {
                    set.remove(pos);
                }
            }
        }
    }

    /// Add the pipe network around `start`, exploring outward only through
    /// positions the graph doesn't know yet: a known neighbor gets linked to
    /// the new node but isn't re-explored, since its own edges are already
    /// correct. Returns how many nodes were added.
    fn discover(&mut self, start: NodeKey, probe: &dyn Fn(&NodeKey) -> Option<Vec<String>>) -> usize {
        let Some(groups) = probe(&start) else {
            return 0;
        };
        let mut added = 0;
        let mut queue: VecDeque<(NodeKey, Vec<String>)> = VecDeque::new();
        self.adjacency.entry(start.clone()).or_default();
        queue.push_back((start, groups));

        while let Some((current, groups)) = queue.pop_front() {
            added += 1;
            for neighbor in neighbor_positions(&current) {
                let Some(n_groups) = probe(&neighbor) else {
                    continue;
                };
                if !shares_group(&groups, &n_groups) {
                    continue;
                }
                let known = self.contains(&neighbor);
                self.link(current.clone(), neighbor.clone());
                if !known {
                    if added + queue.len() >= MAX_DISCOVERY {
                        yog_api::warn!(
                            "[yog-pipes] network at {current:?} exceeds {MAX_DISCOVERY} nodes; discovery truncated"
                        );
                        return added;
                    }
                    queue.push_back((neighbor, n_groups));
                }
            }
        }
        added
    }
}

/// Global pipe topology, maintained incrementally (see module docs).
static GRAPH: LazyLock<Mutex<PipeGraph>> = LazyLock::new(|| Mutex::new(PipeGraph::default()));

/// Last payload delivered to each position. Kept apart from the topology so
/// a node being re-linked (a neighbor placed or broken) never loses it.
static PAYLOADS: LazyLock<Mutex<HashMap<NodeKey, PipePayload>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lock `m`, ignoring poisoning: every mutation here leaves the maps
/// consistent between statements, so a panicked holder can't corrupt them.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Namespace a bare dimension id (`"overworld"` → `"minecraft:overworld"`),
/// matching the ids the loader passes in events, so callers can use either.
pub fn normalize_dim(dim: &str) -> String {
    if dim.contains(':') {
        dim.to_string()
    } else {
        format!("minecraft:{dim}")
    }
}

const NEIGHBOR_OFFSETS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

fn neighbor_positions(pos: &NodeKey) -> impl Iterator<Item = NodeKey> + '_ {
    NEIGHBOR_OFFSETS
        .iter()
        .map(move |(dx, dy, dz)| (pos.0.clone(), pos.1 + dx, pos.2 + dy, pos.3 + dz))
}

fn shares_group(a: &[String], b: &[String]) -> bool {
    a.iter().any(|g| b.contains(g))
}

/// A probe backed by the live server: the link groups of the pipe at a
/// position, or `None` if the block there isn't a registered pipe.
pub fn world_probe(srv: &dyn yog_api::Server) -> impl Fn(&NodeKey) -> Option<Vec<String>> + '_ {
    move |pos: &NodeKey| {
        let id = srv.get_block(
            &pos.0,
            yog_api::BlockPos {
                x: pos.1,
                y: pos.2,
                z: pos.3,
            },
        )?;
        crate::pipe::pipe_groups(&id)
    }
}

/// A pipe was placed at `pos`: (re)link it and explore whatever part of its
/// network the graph didn't know yet.
pub fn on_placed(pos: NodeKey, probe: &dyn Fn(&NodeKey) -> Option<Vec<String>>) {
    let mut graph = lock(&GRAPH);
    graph.remove(&pos);
    graph.discover(pos, probe);
}

/// The pipe at `pos` is gone: drop the node, its edges and its payload.
/// A network split in two by the removal needs no extra work — the two
/// halves simply stop being reachable from each other.
pub fn on_removed(pos: &NodeKey) {
    lock(&GRAPH).remove(pos);
    lock(&PAYLOADS).remove(pos);
}

/// Re-check every known node of `dim` inside the axis-aligned box
/// `center ± radius` against the world, dropping the ones no longer a pipe
/// (an explosion, a mod's bulk `set_block`).
pub fn revalidate_area(
    dim: &str,
    center: (i32, i32, i32),
    radius: i32,
    probe: &dyn Fn(&NodeKey) -> Option<Vec<String>>,
) {
    let stale: Vec<NodeKey> = {
        let graph = lock(&GRAPH);
        graph
            .adjacency
            .keys()
            .filter(|k| {
                k.0 == dim
                    && (k.1 - center.0).abs() <= radius
                    && (k.2 - center.1).abs() <= radius
                    && (k.3 - center.2).abs() <= radius
            })
            .filter(|k| probe(k).is_none())
            .cloned()
            .collect()
    };
    for pos in &stale {
        on_removed(pos);
    }
}

/// Make sure the network around `pos` is known, discovering it from the
/// world if not (after a restart, or for pipes placed without a player).
/// Returns whether `pos` is a node afterwards.
pub fn ensure_known(pos: &NodeKey, probe: &dyn Fn(&NodeKey) -> Option<Vec<String>>) -> bool {
    let mut graph = lock(&GRAPH);
    if !graph.contains(pos) {
        graph.discover(pos.clone(), probe);
    }
    graph.contains(pos)
}

/// Rebuild, from the world, every network touching `pos`: the one it
/// belongs to and those of the positions next to it (so a removed pipe's
/// former halves are re-checked too). Payloads of positions that are still
/// pipes are kept.
pub fn rebuild_around(pos: &NodeKey, probe: &dyn Fn(&NodeKey) -> Option<Vec<String>>) {
    let mut graph = lock(&GRAPH);
    let mut forgotten: HashSet<NodeKey> = HashSet::new();
    let mut queue: VecDeque<NodeKey> = std::iter::once(pos.clone())
        .chain(neighbor_positions(pos))
        .filter(|p| graph.contains(p))
        .collect();
    while let Some(current) = queue.pop_front() {
        if !forgotten.insert(current.clone()) {
            continue;
        }
        queue.extend(graph.neighbors(&current).cloned());
    }
    for p in &forgotten {
        graph.remove(p);
    }

    for p in forgotten.iter().chain(std::iter::once(pos)) {
        if !graph.contains(p) {
            graph.discover(p.clone(), probe);
        }
    }
    drop(graph);

    let graph = lock(&GRAPH);
    lock(&PAYLOADS).retain(|p, _| !forgotten.contains(p) || graph.contains(p));
}

/// Forget everything — topology and payloads. Called when the server
/// stops, so switching singleplayer worlds never routes over the previous
/// world's network.
pub fn clear() {
    *lock(&GRAPH) = PipeGraph::default();
    lock(&PAYLOADS).clear();
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
    let reached = {
        let graph = lock(&GRAPH);
        if !graph.contains(&source) {
            return Vec::new();
        }
        let mut visited: HashSet<NodeKey> = HashSet::new();
        let mut queue: VecDeque<NodeKey> = VecDeque::new();
        visited.insert(source.clone());
        queue.push_back(source);
        while let Some(current) = queue.pop_front() {
            for next in graph.neighbors(&current) {
                if visited.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
        visited
    };

    let mut payloads = lock(&PAYLOADS);
    for pos in &reached {
        payloads.insert(pos.clone(), payload.clone());
    }
    reached.into_iter().collect()
}

/// Store `payload` at `pos` directly (used by point-to-point `send`,
/// after `find_path` confirms a route exists).
pub fn store_payload(pos: &NodeKey, payload: PipePayload) {
    lock(&PAYLOADS).insert(pos.clone(), payload);
}

/// The payload last delivered to `pos`, if any.
pub fn payload_at(pos: &NodeKey) -> Option<PipePayload> {
    lock(&PAYLOADS).get(pos).cloned()
}

/// Find the shortest path between two nodes using BFS (any branching
/// factor per node — this is a general graph search, not a linear-chain
/// assumption). Marks `from` visited up front: a pipe network can contain
/// loops (e.g. a ring of branches reconnecting), and without that a cycle
/// back to the start would keep re-queuing it forever instead of just
/// being ignored as an already-explored node.
pub fn find_path(from: &NodeKey, to: &NodeKey) -> Option<Vec<NodeKey>> {
    let graph = lock(&GRAPH);
    if !graph.contains(from) || !graph.contains(to) {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const OW: &str = "minecraft:overworld";

    fn key(x: i32, y: i32, z: i32) -> NodeKey {
        (OW.to_string(), x, y, z)
    }

    /// A fake world: position → link groups of the pipe placed there.
    struct Fake(HashMap<NodeKey, Vec<String>>);

    impl Fake {
        fn new() -> Self {
            Fake(HashMap::new())
        }
        fn put(&mut self, x: i32, y: i32, z: i32, groups: &[&str]) {
            self.0
                .insert(key(x, y, z), groups.iter().map(|s| s.to_string()).collect());
        }
        fn probe(&self) -> impl Fn(&NodeKey) -> Option<Vec<String>> + '_ {
            |k| self.0.get(k).cloned()
        }
    }

    fn reachable(g: &PipeGraph, from: &NodeKey) -> HashSet<NodeKey> {
        let mut seen = HashSet::from([from.clone()]);
        let mut queue = VecDeque::from([from.clone()]);
        while let Some(c) = queue.pop_front() {
            for n in g.neighbors(&c) {
                if seen.insert(n.clone()) {
                    queue.push_back(n.clone());
                }
            }
        }
        seen
    }

    #[test]
    fn discovers_branches() {
        let mut w = Fake::new();
        // A T-junction: a line along x with a branch going up at x = 1.
        for x in 0..3 {
            w.put(x, 0, 0, &["item"]);
        }
        w.put(1, 1, 0, &["item"]);
        let mut g = PipeGraph::default();
        assert_eq!(g.discover(key(0, 0, 0), &w.probe()), 4);
        assert_eq!(g.neighbors(&key(1, 0, 0)).count(), 3);
        assert_eq!(reachable(&g, &key(0, 0, 0)).len(), 4);
    }

    #[test]
    fn links_only_shared_groups() {
        let mut w = Fake::new();
        w.put(0, 0, 0, &["item"]);
        w.put(1, 0, 0, &["fluid"]);
        w.put(2, 0, 0, &["fluid", "item"]);
        let mut g = PipeGraph::default();
        g.discover(key(0, 0, 0), &w.probe());
        assert!(!g.contains(&key(1, 0, 0)));
        assert_eq!(g.neighbors(&key(0, 0, 0)).count(), 0);
    }

    #[test]
    fn removal_splits_network() {
        let mut w = Fake::new();
        for x in 0..5 {
            w.put(x, 0, 0, &["item"]);
        }
        let mut g = PipeGraph::default();
        g.discover(key(0, 0, 0), &w.probe());
        g.remove(&key(2, 0, 0));
        assert_eq!(
            reachable(&g, &key(0, 0, 0)),
            HashSet::from([key(0, 0, 0), key(1, 0, 0)])
        );
        assert!(g.neighbors(&key(3, 0, 0)).all(|n| n != &key(2, 0, 0)));
    }

    #[test]
    fn placing_joins_known_networks_without_rescanning() {
        let mut w = Fake::new();
        for x in [0, 1, 3, 4] {
            w.put(x, 0, 0, &["item"]);
        }
        let mut g = PipeGraph::default();
        g.discover(key(0, 0, 0), &w.probe());
        g.discover(key(4, 0, 0), &w.probe());
        assert!(!reachable(&g, &key(0, 0, 0)).contains(&key(4, 0, 0)));

        w.put(2, 0, 0, &["item"]);
        // Only the new node is unknown — the known halves get linked to it
        // but are not walked again.
        assert_eq!(g.discover(key(2, 0, 0), &w.probe()), 1);
        assert_eq!(reachable(&g, &key(0, 0, 0)).len(), 5);
    }

    #[test]
    fn rebuild_drops_stale_nodes() {
        let mut w = Fake::new();
        for x in 0..4 {
            w.put(x, 0, 0, &["item"]);
        }
        on_placed(key(0, 0, 0), &w.probe());
        // Swapped out by something the events don't see (`/setblock`).
        w.0.remove(&key(3, 0, 0));
        w.put(2, 0, 0, &["fluid"]);
        rebuild_around(&key(1, 0, 0), &w.probe());
        assert!(find_path(&key(0, 0, 0), &key(1, 0, 0)).is_some());
        assert!(find_path(&key(0, 0, 0), &key(2, 0, 0)).is_none());
        assert!(!lock(&GRAPH).contains(&key(3, 0, 0)));
    }

    #[test]
    fn dimensions_are_independent() {
        let mut w = Fake::new();
        w.put(0, 0, 0, &["item"]);
        w.0.insert(("minecraft:the_nether".into(), 1, 0, 0), vec!["item".into()]);
        let mut g = PipeGraph::default();
        g.discover(key(0, 0, 0), &w.probe());
        assert_eq!(g.neighbors(&key(0, 0, 0)).count(), 0);
    }

    #[test]
    fn normalizes_bare_dimension() {
        assert_eq!(normalize_dim("overworld"), OW);
        assert_eq!(normalize_dim("mymod:void"), "mymod:void");
    }
}
