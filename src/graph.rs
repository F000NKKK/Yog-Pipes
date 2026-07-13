//! Virtual pipe graph — BFS-based adjacency network.
//!
//! When a pipe is placed or broken, the graph is rebuilt from that position
//! via BFS through `connect_groups`-linked neighbors. Nodes represent pipe
//! blocks and adjacent inventories. Edges represent connections.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex};

/// A position in the world: (dimension, x, y, z).
pub type NodeKey = (String, i32, i32, i32);

/// A node in the pipe graph.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PipeNode {
    pub pos: NodeKey,
    pub is_source: bool,
    pub is_sink: bool,
    pub signal_in: u8,
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
#[allow(dead_code)]
pub struct PipeGraph {
    pub nodes: HashMap<NodeKey, PipeNode>,
    pub edges: HashSet<PipeEdge>,
}

/// Global pipe graph, rebuilt on placement/break.
pub static GRAPH: LazyLock<Mutex<PipeGraph>> = LazyLock::new(|| Mutex::new(PipeGraph::default()));

/// Rebuild the graph around `pos` using BFS through connected pipes.
pub fn rebuild_graph(dim: &str, x: i32, y: i32, z: i32) {
    let mut graph = GRAPH.lock().unwrap();
    let start: NodeKey = (dim.to_string(), x, y, z);
    let mut visited: HashSet<NodeKey> = HashSet::new();
    let mut queue: VecDeque<NodeKey> = VecDeque::new();
    queue.push_back(start.clone());

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());

        // Scan 6 neighbor directions
        for (dx, dy, dz) in &[
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let neighbor: NodeKey = (
                current.0.clone(),
                current.1 + dx,
                current.2 + dy,
                current.3 + dz,
            );
            if visited.contains(&neighbor) {
                continue;
            }
            // TODO: query world for pipe/inventory at neighbor via Server API
            // For now, mark all 6 neighbors as connected
            graph.edges.insert((current.clone(), neighbor.clone()));
            queue.push_back(neighbor);
        }
    }
}

/// Propagate redstone signal through the pipe graph via BFS with attenuation.
/// Each hop reduces signal strength by 1.
#[allow(dead_code)]
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
        if let Some(node) = graph.nodes.get_mut(pos) {
            node.signal_out = *sig;
        }
    }
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
            // Reconstruct path
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
