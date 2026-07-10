//! Yog Pipes — universal transport framework for Yog mods.
//!
//! Provides pipe types and registration API. Other mods depend on this and
//! call `register_pipe()` via interop.
//!
//! ## For modders
//!
//! ```ignore
//! use yog_pipes_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};
//!
//! fn register(registry: &mut Registry) {
//!     register_pipe(registry.raw_api() as usize, PipeDef {
//!         block_id: "mymod:cool_pipe".into(),
//!         kind: PipeKind::Item,
//!         tier: PipeTier { name: "Iron".into(), speed: 2, tick_interval: 15,
//!                          signal_range: 16, energy_buffer: 250 },
//!         link_groups: vec!["pipe_item".into()],
//!         recipe_material: "minecraft:iron_ingot".into(),
//!         recipe_center: String::new(),
//!     }).unwrap();
//! }
//! ```

mod framework;
mod graph;
mod transfer;

use yog_api::{Mod, Registry, yog_export};

// Re-export types at crate root for convenience
pub use framework::{PipeKind, PipeTier, PipeDef};

// ── Exported function ────────────────────────────────────────────────────────

/// Wrapper struct for the interop call (single-arg requirement).
#[derive(::yog_api::rkyv::Archive, ::yog_api::rkyv::Serialize, ::yog_api::rkyv::Deserialize)]
struct RegisterPipeArgs {
    /// Raw `YogApi` pointer from `Registry::raw_api()`.
    api_ptr: usize,
    /// Pipe definition.
    def: framework::PipeDef,
}

/// Register a pipe block+item+recipe. Called by other mods via interop.
#[yog_export]
fn register_pipe(args: RegisterPipeArgs) -> Result<(), String> {
    let mut registry = unsafe { Registry::from_raw(args.api_ptr as *const yog_api::YogApi) };
    framework::register_pipe_impl(&mut registry, args.def)
}

// ── Mod entry point ──────────────────────────────────────────────────────────

pub struct YogPipesMod;

impl Mod for YogPipesMod {
    fn register(registry: &mut Registry) {
        yog_api::info!("[yog-pipes] pipe transport framework ready.");

        registry.interop().export("register_pipe", __yog_wrap_register_pipe as *const std::ffi::c_void);

        // Infrastructure: rebuild graph when any pipe block is placed or broken
        registry.on_player_place_block(|e, phase, _srv| {
            if phase != yog_api::EventPhase::Post { return true; }
            if !e.block_id.contains(":pipe_") { return true; }
            graph::rebuild_graph("overworld", e.pos.x, e.pos.y, e.pos.z);
            true
        });

        registry.on_block_break(|e, phase, _srv| {
            if phase != yog_api::EventPhase::Post { return true; }
            if !e.block_id.contains(":pipe_") { return true; }
            graph::rebuild_graph("overworld", e.pos.x, e.pos.y, e.pos.z);
            true
        });

        registry.on_tick(|srv| transfer::transfer_tick(srv));
    }
}

yog_api::export_mod!(YogPipesMod);
