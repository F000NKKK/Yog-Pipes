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

        // TODO: when the loader is rebuilt with ABI 27, uncomment:
        // registry.interop().export(
        //     "register_pipe_json",
        //     register_pipe_from_json_c as *const std::ffi::c_void,
        // );

        // Infrastructure: rebuild graph when any pipe block is placed or broken
        registry.on_player_place_block(|e, phase, _srv| {
            if phase != yog_api::EventPhase::Post { return true; }
            if !e.block_id.contains(":pipe_") { return true; }
            rebuild_graph("overworld", e.pos.x, e.pos.y, e.pos.z);
            true
        });

        registry.on_block_break(|e, phase, _srv| {
            if phase != yog_api::EventPhase::Post { return true; }
            if !e.block_id.contains(":pipe_") { return true; }
            rebuild_graph("overworld", e.pos.x, e.pos.y, e.pos.z);
            true
        });

        registry.on_tick(|srv| transfer_tick(srv));
    }
}

/// C-ABI compatible: register a pipe from a JSON string.
///
/// Once the loader is rebuilt with ABI 27, other mods can import this via:
/// ```ignore
/// let f: unsafe extern "C" fn(*const YogApi, *const c_char) =
///     std::mem::transmute(registry.interop().import("yog-pipes:register_pipe_json").unwrap());
/// f(registry.raw_api(), json_str.as_ptr() as *const c_char);
/// ```
#[no_mangle]
pub unsafe extern "C" fn register_pipe_from_json_c(
    api: *const yog_api::YogApi,
    json_ptr: *const std::os::raw::c_char,
) {
    if api.is_null() || json_ptr.is_null() { return; }
    let json = match std::ffi::CStr::from_ptr(json_ptr).to_str() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut registry = unsafe { yog_api::Registry::from_raw(api) };
    framework::register_pipe_from_json(&mut registry, json);
}

yog_api::export_mod!(YogPipesMod);
