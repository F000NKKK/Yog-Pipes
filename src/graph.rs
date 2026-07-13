//! Virtual pipe graph — BFS-based adjacency network.
//!
//! When a pipe is placed or broken, the graph is rebuilt from that position
//! via BFS through real world blocks: a neighbor becomes an edge only when
//! it is itself a block registered via `crate::pipe::register_pipe` (see
//! `crate::pipe::is_pipe_block`) — never guessed from the block id's
//! spelling and never assumed present without checking the world.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex};

use yog_api::BlockPos;

/// A position in the world: (dimension, x, y, z).
pub type NodeKey = (String, i32, i32, i32);

/// A node in the pipe graph.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // energy fields await the Energy pipe kind's transfer step
pub struct PipeNode {
    pub signal_out: u8,
    /// Energy buffer (for Yog Flux pipes).
    pub energy: u64,
    /// Max energy capacity.
    pub energy_cap: u64,
}

/// Undirected edge between two nodes.
pub type PipeEdge = (NodeKey, NodeKey);

/// The full pipe network graph.
#[derive(Debug, Clone, Default)]
pub struct PipeGraph {
    pub nodes: HashMap<NodeKey, PipeNode>,
    pub edges: HashSet<PipeEdge>,
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
/// `yog_api::World`, never assumed).
pub fn rebuild_graph(srv: &dyn yog_api::Server, dim: &str, x: i32, y: i32, z: i32) {
    let world = yog_api::World::new(srv, dim);
    let mut graph = GRAPH.lock().unwrap();

    // Drop stale edges/nodes touching a position that's no longer a pipe —
    // covers both the break case and pipes removed elsewhere in the network.
    graph
        .edges
        .retain(|(a, b)| is_pipe_at(&world, a) && is_pipe_at(&world, b));
    graph.nodes.retain(|pos, _| is_pipe_at(&world, pos));

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
            graph.edges.insert((current.clone(), neighbor.clone()));
            if !visited.contains(&neighbor) {
                queue.push_back(neighbor);
            }
        }
    }
}

/// Propagate a signal through the pipe graph via BFS with attenuation.
/// Each hop reduces signal strength by 1 — call after changing the value a
/// source node emits (event-driven, like vanilla redstone, rather than
/// recomputed every tick).
pub fn propagate_signals(source: NodeKey, strength: u8) {
    let graph = GRAPH.lock().unwrap();
    let mut queue: VecDeque<(NodeKey, u8)> = VecDeque::new();
    let mut visited: HashMap<NodeKey, u8> = HashMap::new();
    queue.push_back((source.clone(), strength));

    while let Some((current, sig)) = queue.pop_front() {
        if sig == 0 {
            continue;
        }
        let entry = visited.entry(current.clone()).or_insert(0);
        if *entry >= sig {
            continue;
        }
        *entry = sig;

        for edge in &graph.edges {
            let (a, b) = edge;
            let next = if a == &current {
                b
            } else if b == &current {
                a
            } else {
                continue;
            };
            let next_sig = sig.saturating_sub(1);
            if next_sig > 0 {
                queue.push_back((next.clone(), next_sig));
            }
        }
    }
    drop(graph);

    let mut graph = GRAPH.lock().unwrap();
    for (pos, sig) in &visited {
        graph.nodes.entry(pos.clone()).or_default().signal_out = *sig;
    }
}

/// Current signal level the graph has computed at `pos` (0 if the position
/// isn't a known node, or no signal has propagated there yet).
pub fn signal_at(pos: &NodeKey) -> u8 {
    GRAPH
        .lock()
        .unwrap()
        .nodes
        .get(pos)
        .map(|n| n.signal_out)
        .unwrap_or(0)
}

/// Find the shortest path between two nodes using BFS.
#[allow(dead_code)]
pub fn find_path(from: &NodeKey, to: &NodeKey) -> Option<Vec<NodeKey>> {
    let graph = GRAPH.lock().unwrap();
    let mut queue: VecDeque<NodeKey> = VecDeque::new();
    let mut came_from: HashMap<NodeKey, NodeKey> = HashMap::new();
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
        for edge in &graph.edges {
            let (a, b) = edge;
            let next = if a == &current {
                b
            } else if b == &current {
                a
            } else {
                continue;
            };
            if came_from.contains_key(next) {
                continue;
            }
            came_from.insert(next.clone(), current.clone());
            queue.push_back(next.clone());
        }
    }
    None
}
