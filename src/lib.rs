//! Yog Pipes — universal transport framework for Yog mods.
//!
//! # Architecture
//!
//! Unlike traditional "pipe blocks" that hardcode item/fluid types, **yog-pipes**
//! is a **framework**: it provides a virtual graph + state-machine transfer engine
//! and lets modders define *what* flows through the pipes.
//!
//! ## Concepts
//!
//! | Concept | Description |
//! |---------|-------------|
//! | `PipeKind` | What the pipe carries: `Item`, `Fluid`, `Signal`, `Energy(YogFlux)` |
//! | `PipeTier` | Speed/capacity tier (Stone → Netherite) |
//! | `PipeDef`  | Registration entry: kind + tier + texture/shape customization + link groups |
//! | `PipeGraph`| Virtual graph of connected pipes + inventories, rebuilt on place/break |
//! | `TransferState` | State machine: Extract → Route → Insert |
//!
//! ## Quick start
//!
//! ```ignore
//! use yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};
//!
//! // Item pipe
//! register_pipe(PipeDef {
//!     block_id: "mymod:item_pipe",
//!     kind: PipeKind::Item,
//!     tier: PipeTier::Iron,
//!     texture: Some("mymod:textures/block/item_pipe.png"),
//!     link_groups: &["pipe_item", "inventory"],
//!     shape: None, // default 4..12 voxel shape
//! });
//!
//! // Energy pipe with Yog Flux
//! register_pipe(PipeDef {
//!     block_id: "mymod:flux_pipe",
//!     kind: PipeKind::Energy(YogFlux),
//!     tier: PipeTier::Gold,
//!     texture: None,
//!     link_groups: &["pipe_energy"],
//!     shape: Some((3.0, 3.0, 3.0, 13.0, 13.0, 13.0)),
//! });
//! ```
//!
//! ## Yog Flux (YF)
//!
//! A custom energy unit for Yog mods. 1 YF = 1 redstone tick equivalent.
//! Energy pipes transfer YF between energy producers and consumers.

mod graph;
mod transfer;

use std::sync::{LazyLock, Mutex};

pub use graph::{NodeKey, PipeGraph, rebuild_graph, propagate_signals, GRAPH};
pub use transfer::{TransferState, transfer_tick, TRANSFER};

// ── Pipe kind ────────────────────────────────────────────────────────────────

/// What a pipe carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipeKind {
    /// Item stacks (like hoppers / BuildCraft item pipes).
    Item,
    /// Fluids (placeholder for future fluid API).
    Fluid,
    /// Redstone signals (strength 0–15, propagated through graph).
    Signal,
    /// Yog Flux energy (custom unit).
    Energy(YogFluxUnit),
}

/// Yog Flux energy unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum YogFluxUnit {
    /// 1 YF = 1 redstone tick equivalent. Stored in pipe network buffers.
    Flux,
}

// ── Pipe tier ────────────────────────────────────────────────────────────────

/// Speed/capacity tier for a pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeTier {
    /// Display name (e.g. "Iron", "Gold").
    pub name: &'static str,
    /// Transfer speed: items/ticks/operations per cycle.
    pub speed: u32,
    /// Game ticks between transfer cycles.
    pub tick_interval: u32,
    /// Max graph distance for signal propagation.
    pub signal_range: u32,
    /// Max energy buffer per pipe node (for Energy kind).
    pub energy_buffer: u64,
}

impl PipeTier {
    pub const STONE:     PipeTier = PipeTier { name: "Stone",     speed: 1,  tick_interval: 20, signal_range: 8,   energy_buffer: 100  };
    pub const IRON:      PipeTier = PipeTier { name: "Iron",      speed: 2,  tick_interval: 15, signal_range: 16,  energy_buffer: 250  };
    pub const GOLD:      PipeTier = PipeTier { name: "Gold",      speed: 4,  tick_interval: 10, signal_range: 32,  energy_buffer: 500  };
    pub const DIAMOND:   PipeTier = PipeTier { name: "Diamond",   speed: 8,  tick_interval: 5,  signal_range: 64,  energy_buffer: 1000 };
    pub const NETHERITE: PipeTier = PipeTier { name: "Netherite", speed: 16, tick_interval: 3,  signal_range: 128, energy_buffer: 2000 };
}

// ── Pipe definition ──────────────────────────────────────────────────────────

/// Registration entry for one pipe block+item.
pub struct PipeDef<'a> {
    /// Block/item id (e.g. `"mymod:item_pipe"`).
    pub block_id: &'a str,
    /// What this pipe carries.
    pub kind: PipeKind,
    /// Speed/capacity tier.
    pub tier: PipeTier,
    /// Optional custom texture path (vanilla resource location).
    /// `None` = default pipe texture.
    pub texture: Option<&'a str>,
    /// Connect groups for automatic neighbor linking.
    pub link_groups: &'a [&'a str],
    /// Optional custom voxel shape: (x0, y0, z0, x1, y1, z1) in 0..16.
    /// `None` = default thin pipe shape (4, 4, 4, 12, 12, 12).
    pub shape: Option<(f64, f64, f64, f64, f64, f64)>,
}

/// Crafter helper for block → item recipes.
pub struct PipeRecipe<'a> {
    /// Material item id for the pipe body (e.g. `"minecraft:iron_ingot"`).
    pub material: &'a str,
    /// Center item (default: `"minecraft:glass_pane"`).
    pub center: &'a str,
}

impl Default for PipeRecipe<'_> {
    fn default() -> Self {
        PipeRecipe { material: "minecraft:cobblestone", center: "minecraft:glass_pane" }
    }
}

// ── Registration ─────────────────────────────────────────────────────────────

/// Counter for unique crafting recipe IDs.
static RECIPE_COUNTER: LazyLock<Mutex<u64>> = LazyLock::new(|| Mutex::new(0));

/// Register a single pipe block + item with the given definition.
pub fn register_pipe(registry: &mut yog_api::Registry, def: PipeDef, recipe: PipeRecipe) {
    let (x0, y0, z0, x1, y1, z1) = def.shape.unwrap_or((4.0, 4.0, 4.0, 12.0, 12.0, 12.0));

    let mut block = yog_api::BlockDef::new(def.block_id)
        .strength(1.5, 3.0)
        .sound("stone")
        .shape(x0, y0, z0, x1, y1, z1)
        .connects_to_neighbors()
        .connect_groups(def.link_groups);

    // Custom texture if provided
    if let Some(tex) = def.texture {
        // Texture path stored in block properties — rendered by the loader
        // (future: loader-side custom block rendering)
        let _ = tex; // TODO: wire texture to block model
    }

    registry.register_block(block);

    let kind_str = match def.kind {
        PipeKind::Item => "Item",
        PipeKind::Fluid => "Fluid",
        PipeKind::Signal => "Signal",
        PipeKind::Energy(_) => "Energy (Yog Flux)",
    };

    registry.register_item(
        yog_api::ItemDef::new(def.block_id)
            .tooltip(format!(
                "§7{} §e{} Pipe\n§7Speed: §b{}§7 | Interval: §a{} ticks\n§7Signal range: §c{}§7 | Buffer: §d{}",
                kind_str, def.tier.name, def.tier.speed, def.tier.tick_interval,
                def.tier.signal_range, def.tier.energy_buffer
            ))
    );

    // Crafting recipe
    let mut counter = RECIPE_COUNTER.lock().unwrap();
    *counter += 1;
    let recipe_id = format!("yog-pipes:craft_{}", *counter);

    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new(&recipe_id, def.block_id, 4)
            .row(" M ")
            .row("MGM")
            .row(" M ")
            .key('M', recipe.material)
            .key('G', recipe.center)
    );
}
