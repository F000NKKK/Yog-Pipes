//! Yog Pipes — universal transport framework for Yog mods.
//!
//! Provides pipe types and registration API. Other mods can either:
//!
//! 1. **Direct dependency** — add `yog-pipes` to `Cargo.toml` and call
//!    [`register_pipe`] directly:
//!    ```ignore
//!    use yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};
//!
//!    register_pipe(registry, PipeDef { ... }).unwrap();
//!    ```
//!
//! 2. **Interop** (for mods that can't depend directly) — call the exported
//!    function via `registry.interop().call("register_pipe", &args)`:
//!    ```ignore
//!    use yog_pipes_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, RegisterPipeArgs};
//!
//!    registry.interop().call("register_pipe", &RegisterPipeArgs {
//!        api_ptr: registry.raw_api() as usize,
//!        def: PipeDef { ... },
//!    }).unwrap();
//!    ```

mod framework;
mod graph;
mod transfer;

use yog_api::{Mod, Registry};

// Re-export all public types so mods can `use yog_pipes::*`.
pub use framework::{PipeKind, PipeTier, PipeDef, RegisterPipeArgs, register_pipe, register_pipe_interop};

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