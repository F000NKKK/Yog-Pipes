//! Yog Pipes — universal transport framework for Yog mods.
//!
//! This is a **framework**, not a mod. It provides building blocks for mods
//! to define their own pipes without dictating recipes, tiers, or models.
//!
//! ## Philosophy
//!
//! - **No fixed fields** — `PipeDef` carries an open `properties` map instead
//!   of a rigid `PipeTier` struct. Mods define whatever parameters they need.
//! - **No recipes** — the framework never generates recipes. Mods register
//!   their own via `registry.add_shaped_recipe()` if they want crafting.
//! - **3D models** — `ModelDef` lets mods describe block models with
//!   per-face textures, elements, and rotation — no JSON files needed.
//! - **Link groups** — control which blocks pipes connect to.
//! - **Extensible pipe kinds** — `PipeKind::Custom("mymod:mana")` for custom types.
//!
//! ## Usage
//!
//! ### Direct dependency
//!
//! ```ignore
//! use yog_pipes::{PipeKind, PipeDef, register_pipe};
//!
//! register_pipe(registry, PipeDef {
//!     block_id: "mymod:pipe_iron".into(),
//!     kind: PipeKind::Item,
//!     properties: [("speed", 2.0), ("tick_interval", 15.0)].into(),
//!     model: None,
//!     link_groups: vec!["pipe_item".into(), "inventory".into()],
//! }).unwrap();
//! ```
//!
//! ### Interop
//!
//! ```ignore
//! use yog_exports::yog_pipes::{PipeKind, PipeDef, RegisterPipeArgs};
//!
//! yog_exports::yog_pipes::register_pipe(RegisterPipeArgs {
//!     api_ptr: registry.raw_api() as usize,
//!     def: PipeDef { ... },
//! }).unwrap();
//! ```
//!
//! ### Custom 3D model
//!
//! ```ignore
//! use yog_pipes::{PipeKind, PipeDef, ModelDef, ModelElement, FaceDef, register_pipe};
//!
//! register_pipe(registry, PipeDef {
//!     block_id: "mymod:fancy_pipe".into(),
//!     kind: PipeKind::Fluid,
//!     properties: [("fluid_capacity", 8000.0)].into(),
//!     model: Some(ModelDef {
//!         texture: Some("mymod:block/fancy_pipe".into()),
//!         elements: vec![ModelElement {
//!             from: [4.0, 0.0, 4.0],
//!             to: [12.0, 16.0, 12.0],
//!             faces: [].into(),
//!             rotation: None,
//!         }],
//!     }),
//!     link_groups: vec!["pipe_fluid".into(), "tank".into()],
//! }).unwrap();
//! ```

mod framework;
mod graph;
mod transfer;

use yog_api::{Mod, Registry};

// Re-export all public types
pub use framework::{
    register_pipe, register_pipe_interop, ElementRotation, FaceDef, ModelDef, ModelElement,
    PipeDef, PipeKind, RegisterPipeArgs,
};

// ── Mod entry point ──────────────────────────────────────────────────────────

pub struct YogPipesMod;

impl Mod for YogPipesMod {
    fn register(registry: &mut Registry) {
        yog_api::info!("[yog-pipes] pipe transport framework ready.");

        // Export `register_pipe` for interop consumers
        registry.interop().export(
            "register_pipe",
            register_pipe_interop as *const std::ffi::c_void,
        );

        // Infrastructure: rebuild graph when any pipe block is placed or broken
        registry.on_player_place_block(|e, phase, _srv| {
            if phase != yog_api::EventPhase::Post {
                return true;
            }
            if !e.block_id.contains(":pipe_") {
                return true;
            }
            graph::rebuild_graph("overworld", e.pos.x, e.pos.y, e.pos.z);
            true
        });

        registry.on_block_break(|e, phase, _srv| {
            if phase != yog_api::EventPhase::Post {
                return true;
            }
            if !e.block_id.contains(":pipe_") {
                return true;
            }
            graph::rebuild_graph("overworld", e.pos.x, e.pos.y, e.pos.z);
            true
        });

        registry.on_tick(|srv| transfer::transfer_tick(srv));
    }
}

yog_api::export_mod!(YogPipesMod);
