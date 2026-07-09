//! Pipe network — virtual graph + state machine for item/signal transfer.
//!
//! ## Architecture
//!
//! Instead of block-to-block iteration, pipes form a **virtual graph**:
//! - **Nodes**: pipe blocks, source/destination inventories (chests, furnaces, ...)
//! - **Edges**: adjacency between nodes (connected via `YogConnectingBlock`)
//!
//! On placement/break, the graph is rebuilt via BFS from the changed position.
//! A **state machine** processes transfers each tick:
//! 1. **Extract** — pull items from source inventories
//! 2. **Route**   — find path through pipes to nearest accepting destination
//! 3. **Insert**  — push items into the destination
//!
//! Signals propagate through the pipe graph with distance-based attenuation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex};

use yog_api::{BlockDef, ItemDef, Registry};

// ── Tier definitions ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeTier {
    pub name: &'static str,
    pub item_id: &'static str,
    pub speed: u32,
    pub tick_interval: u32,
    pub signal_range: u32, // max graph distance for signal propagation
}

const TIERS: &[PipeTier] = &[
    PipeTier { name: "Stone",     item_id: "yog-pipes:item_pipe_stone",     speed: 1,  tick_interval: 20, signal_range: 8  },
    PipeTier { name: "Iron",      item_id: "yog-pipes:item_pipe_iron",      speed: 2,  tick_interval: 15, signal_range: 16 },
    PipeTier { name: "Gold",      item_id: "yog-pipes:item_pipe_gold",      speed: 4,  tick_interval: 10, signal_range: 32 },
    PipeTier { name: "Diamond",   item_id: "yog-pipes:item_pipe_diamond",   speed: 8,  tick_interval: 5,  signal_range: 64 },
    PipeTier { name: "Netherite", item_id: "yog-pipes:item_pipe_netherite", speed: 16, tick_interval: 3,  signal_range: 128},
];

// ── Graph types ──────────────────────────────────────────────────────────────

/// A position in the world: (dimension, x, y, z).
pub type NodeKey = (String, i32, i32, i32);

/// A node in the pipe graph.
#[derive(Debug, Clone)]
pub struct PipeNode {
    pub pos: NodeKey,
    pub tier: PipeTier,
    pub is_source: bool,  // adjacent to an inventory that can provide items
    pub is_sink: bool,    // adjacent to an inventory that can accept items
    pub signal_in: u8,    // redstone signal strength (0-15) arriving at this node
    pub signal_out: u8,   // signal to emit to adjacent redstone consumers
}

/// Edge between two pipe nodes (undirected).
pub type PipeEdge = (NodeKey, NodeKey);

/// The full pipe network graph.
#[derive(Debug, Clone, Default)]
pub struct PipeGraph {
    pub nodes: HashMap<NodeKey, PipeNode>,
    pub edges: HashSet<PipeEdge>,
}

/// Global pipe graph, rebuilt on placement/break.
pub static GRAPH: LazyLock<Mutex<PipeGraph>> = LazyLock::new(|| Mutex::new(PipeGraph::default()));

// ── State machine ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Idle,
    Extracting { from: NodeKey, slot: usize },
    Routing   { from: NodeKey, to: NodeKey, path: Vec<NodeKey> },
    Inserting { to: NodeKey, slot: usize },
}

/// Per-pipe-network transfer state.
pub static TRANSFER: LazyLock<Mutex<TransferState>> = LazyLock::new(|| Mutex::new(TransferState::Idle));

/// Tick counter for interval-based transfers.
static TICK: LazyLock<Mutex<u64>> = LazyLock::new(|| Mutex::new(0));

// ── Graph operations ─────────────────────────────────────────────────────────

/// Rebuild the graph around `pos` using BFS through connected pipes.
pub fn rebuild_graph(dim: &str, x: i32, y: i32, z: i32) {
    let mut graph = GRAPH.lock().unwrap();
    // BFS from this position
    let start = (dim.to_string(), x, y, z);
    let mut visited: HashSet<NodeKey> = HashSet::new();
    let mut queue: VecDeque<NodeKey> = VecDeque::new();
    queue.push_back(start.clone());

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) { continue; }
        visited.insert(current.clone());

        // For each neighbor direction, check if there's a pipe or inventory
        for (dx, dy, dz) in &[(1,0,0), (-1,0,0), (0,1,0), (0,-1,0), (0,0,1), (0,0,-1)] {
            let neighbor: NodeKey = (current.0.clone(), current.1 + dx, current.2 + dy, current.3 + dz);
            if visited.contains(&neighbor) { continue; }
            // TODO: query world for pipe block or inventory at neighbor
            // For now, just mark as connected
            graph.edges.insert((current.clone(), neighbor.clone()));
            queue.push_back(neighbor);
        }
    }
}

// ── Signal propagation ──────────────────────────────────────────────────────

/// Propagate redstone signal through the pipe graph via BFS with attenuation.
/// Each pipe node reduces signal strength by 1 per hop (up to its tier's range).
pub fn propagate_signals(source: NodeKey, strength: u8) {
    let graph = GRAPH.lock().unwrap();
    // BFS with distance tracking
    let mut queue: VecDeque<(NodeKey, u8)> = VecDeque::new();
    let mut visited: HashMap<NodeKey, u8> = HashMap::new();
    queue.push_back((source.clone(), strength));

    while let Some((current, sig)) = queue.pop_front() {
        if sig == 0 { continue; }
        let entry = visited.entry(current.clone()).or_insert(0);
        if *entry >= sig { continue; }
        *entry = sig;

        // Propagate to neighbors
        for edge in &graph.edges {
            let (a, b) = edge;
            let next = if a == &current { b } else if b == &current { a } else { continue };
            let next_sig = sig.saturating_sub(1);
            if next_sig > 0 {
                queue.push_back((next.clone(), next_sig));
            }
        }
    }
    drop(graph);

    // Update node signal values
    let mut graph = GRAPH.lock().unwrap();
    for (pos, sig) in &visited {
        if let Some(node) = graph.nodes.get_mut(pos) {
            node.signal_out = *sig;
        }
    }
}

// ── Transfer tick ────────────────────────────────────────────────────────────

/// Run one step of the state machine. Called every game tick.
pub fn transfer_tick(_srv: &dyn yog_api::Server) {
    let mut tick = TICK.lock().unwrap();
    *tick += 1;

    let mut state = TRANSFER.lock().unwrap();
    match *state {
        TransferState::Idle => {
            // Find a source inventory that has items
            // TODO: scan graph for source nodes with non-empty inventories
            // For now, no-op
        }
        TransferState::Extracting { from: _, slot: _ } => {
            // TODO: extract item from source inventory
            *state = TransferState::Idle;
        }
        TransferState::Routing { from: _, to: _, path: _ } => {
            // Path already found; move to inserting
            *state = TransferState::Idle;
        }
        TransferState::Inserting { to: _, slot: _ } => {
            // TODO: insert item into destination
            *state = TransferState::Idle;
        }
    }
}

// ── Registration ─────────────────────────────────────────────────────────────

pub fn register(registry: &mut Registry) {
    for tier in TIERS {
        registry.register_block(
            BlockDef::new(tier.item_id)
                .strength(1.5, 3.0)
                .sound("stone")
                .shape(4.0, 4.0, 4.0, 12.0, 12.0, 12.0)
                .connects_to_neighbors()
                .connect_groups(&["pipe_item", "pipe_signal"])
        );

        registry.register_item(
            ItemDef::new(tier.item_id)
                .tooltip(format!(
                    "§7Tier: §e{}§7 | Speed: §b{} items/op§7 | Interval: §a{} ticks§7 | Signal range: §c{}",
                    tier.name, tier.speed, tier.tick_interval, tier.signal_range
                ))
        );

        let material = match tier.name {
            "Stone"     => "minecraft:cobblestone",
            "Iron"      => "minecraft:iron_ingot",
            "Gold"      => "minecraft:gold_ingot",
            "Diamond"   => "minecraft:diamond",
            "Netherite" => "minecraft:netherite_ingot",
            _ => "minecraft:cobblestone",
        };

        registry.add_shaped_recipe(
            yog_api::ShapedRecipe::new(
                &format!("yog-pipes:{}_craft", tier.item_id.replace(':', "_")),
                tier.item_id, 4,
            )
            .row(" M ")
            .row("MGM")
            .row(" M ")
            .key('M', material)
            .key('G', "minecraft:glass_pane")
        );
    }

    // Rebuild graph on block place/break
    registry.on_place_block(|e, phase, _srv| -> bool {
        if phase != yog_api::EventPhase::Post { return true; }
        if !e.block_id.starts_with("yog-pipes:") { return true; }
        rebuild_graph(&e.dimension, e.pos.x, e.pos.y, e.pos.z);
        true
    });

    registry.on_block_break(|e, phase, _srv| -> bool {
        if phase != yog_api::EventPhase::Post { return true; }
        if !e.block_id.starts_with("yog-pipes:") { return true; }
        rebuild_graph(&e.dimension, e.pos.x, e.pos.y, e.pos.z);
        true
    });

    // Transfer tick
    registry.on_tick(|srv| transfer_tick(srv));
}
