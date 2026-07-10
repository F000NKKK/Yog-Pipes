//! Yog Pipes — universal bridge framework for Yog mods.
//!
//! Provides pipe types, registration API, and an extensible energy system.
//! Other mods can either:
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
//!    use yog_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, RegisterPipeArgs};
//!
//!    registry.interop().call("register_pipe", &RegisterPipeArgs {
//!        api_ptr: registry.raw_api() as usize,
//!        def: PipeDef { ... },
//!    }).unwrap();
//!    ```
//!
//! 3. **Custom energy types** — register new energy types for the pipe network:
//!    ```ignore
//!    use yog_pipes::{EnergyType, register_energy_type};
//!
//!    register_energy_type(registry, EnergyType {
//!        id: "mymod:mana".into(),
//!        display_name: "Mana".into(),
//!        yf_per_unit: 2.0,
//!        units_per_yf: 0.5,
//!    }).unwrap();
//!    ```
//!
//! ## What Yog Pipes can do
//!
//! - **Item transport** — move item stacks between inventories through a pipe network.
//! - **Fluid transport** — transfer fluids between tanks and machines.
//! - **Signal propagation** — blocks can emit and listen on signal channels through pipes,
//!   enabling wireless-like communication without redstone wiring.
//! - **Energy transfer** — Yog Flux (YF), Redstone Flux (RF), Forge Energy (FE), EU,
//!   or any custom energy type registered by other mods.
//! - **Extensible energy system** — any mod can register a new `EnergyType` and other
//!   mods can build pipes, generators, and consumers for it.

mod framework;
mod graph;
mod transfer;

use yog_api::{Mod, Registry};

// Re-export all public types so mods can `use yog_pipes::*`.
pub use framework::{
    PipeKind, PipeTier, PipeDef, RegisterPipeArgs,
    EnergyType, EnergyTypeId, register_energy_type,
    register_pipe, register_pipe_interop,
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

        // Register built-in energy types
        let _ = register_energy_type(registry, EnergyType {
            id: "yog:flux".into(),
            display_name: "Yog Flux".into(),
            yf_per_unit: 1.0,
            units_per_yf: 1.0,
        });
        let _ = register_energy_type(registry, EnergyType {
            id: "forge:energy".into(),
            display_name: "Forge Energy".into(),
            yf_per_unit: 0.1,
            units_per_yf: 10.0,
        });
        let _ = register_energy_type(registry, EnergyType {
            id: "redstone:flux".into(),
            display_name: "Redstone Flux".into(),
            yf_per_unit: 0.1,
            units_per_yf: 10.0,
        });
        let _ = register_energy_type(registry, EnergyType {
            id: "ic2:eu".into(),
            display_name: "Energy Unit (EU)".into(),
            yf_per_unit: 4.0,
            units_per_yf: 0.25,
        });

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