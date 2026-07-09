//! Yog Pipes — universal transport framework for Yog mods.
//!
//! # Concepts
//!
//! - **PipeKind** — what the pipe carries: `Item`, `Fluid`, `Signal`, `Energy(YogFlux)`.
//! - **PipeTier** — speed/capacity tier (fully configurable by the modder).
//! - **PipeDef** — registration entry: kind + tier + texture/shape + link groups.
//! - **PipeGraph** — virtual graph of connected pipes + inventories, rebuilt on place/break.
//! - **TransferState** — state machine: Extract → Route → Insert.
//!
//! ## Yog Flux (YF)
//!
//! Custom energy unit: 1 YF = 1 redstone tick equivalent.
//! Energy pipes transfer YF between producers and consumers.
//!
//! ## Example
//!
//! ```ignore
//! use yog_pipes::{PipeKind, PipeTier, PipeDef, PipeRecipe, register_pipe};
//!
//! // Iron item pipe
//! register_pipe(
//!     PipeDef {
//!         block_id: "mymod:item_pipe_iron",
//!         kind: PipeKind::Item,
//!         tier: PipeTier { name: "Iron", speed: 2, tick_interval: 15, signal_range: 16, energy_buffer: 250 },
//!         texture: Some("mymod:textures/block/item_pipe.png"),
//!         link_groups: &["pipe_item", "inventory"],
//!         shape: None,
//!     },
//!     PipeRecipe { material: "minecraft:iron_ingot", ..Default::default() }
//! );
//! ```

mod framework;
mod graph;
mod transfer;

pub use framework::{
    PipeKind, PipeTier, PipeDef, PipeRecipe, YogFluxUnit,
    register_pipe,
};
pub use graph::{
    NodeKey, PipeGraph, PipeNode, PipeEdge,
    rebuild_graph, propagate_signals, find_path, GRAPH,
};
pub use transfer::{TransferState, transfer_tick, schedule, TRANSFER, TICK};

use yog_api::{Mod, Registry};

pub struct YogPipesMod;

impl Mod for YogPipesMod {
    fn register(registry: &mut Registry) {
        yog_api::info!("[yog-pipes] pipe transport framework ready.");
        let _ = registry; // no default registrations — modders call register_pipe()
    }
}

yog_api::export_mod!(YogPipesMod);
