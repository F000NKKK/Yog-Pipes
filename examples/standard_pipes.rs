//! Example: Standard pipes (5 tiers × 4 kinds = 20 pipes).
//!
//! This example shows how to register pipes using the `yog-pipes` framework.
//! Copy this into your own mod to customise textures, recipes, and tiers.
//!
//! ## Direct dependency (recommended)
//!
//! Add `yog-pipes` to your `[dependencies]` in `yog.toml`:
//! ```toml
//! yog-pipes = "0.1"
//! ```
//!
//! Then call `register_pipe` directly in your mod:
//! ```ignore
//! use yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};
//!
//! register_pipe(registry, PipeDef {
//!     block_id: "mymod:item_pipe_iron".into(),
//!     kind: PipeKind::Item,
//!     tier: PipeTier { name: "Iron".into(), speed: 2, tick_interval: 15,
//!                      signal_range: 16, energy_buffer: 250 },
//!     link_groups: vec!["pipe_item".into(), "inventory".into()],
//!     recipe_material: "minecraft:iron_ingot".into(),
//!     recipe_center: String::new(),
//!     energy_type: None,
//! }).unwrap();
//! ```
//!
//! ## Interop (no direct Cargo dependency — mods loaded dynamically)
//!
//! If your mod can't link `yog-pipes` at compile time (e.g. it's a separate Yog mod
//! loaded by the runtime), you still add `yog-pipes` to `[dependencies]` in `yog.toml`.
//! The `yog build` tool automatically maps `yog-pipes` → `yog_exports = { package = "yog_pipes_exports", ... }`,
//! so all export crates share the single `yog_exports` namespace at development time.
//!
//! Your `yog.toml`:
//! ```toml
//! [dependencies]
//! yog-pipes = "0.1"
//! ```
//!
//! Your code uses the `yog_exports` crate to access types and call exported functions:
//! ```ignore
//! use yog_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, RegisterPipeArgs};
//!
//! let args = RegisterPipeArgs {
//!     api_ptr: registry.raw_api() as usize,
//!     def: PipeDef {
//!         block_id: "mymod:fluid_pipe_iron".into(),
//!         kind: PipeKind::Fluid,
//!         tier: PipeTier { name: "Iron".into(), speed: 2, tick_interval: 15,
//!                          signal_range: 16, energy_buffer: 250 },
//!         link_groups: vec!["pipe_fluid".into(), "tank".into()],
//!         recipe_material: "minecraft:iron_ingot".into(),
//!         recipe_center: "minecraft:glass_pane".into(),
                "Stone"     => "minecraft:cobblestone",
                "Iron"      => "minecraft:iron_ingot",
                "Gold"      => "minecraft:gold_ingot",
                "Diamond"   => "minecraft:diamond",
                "Netherite" => "minecraft:netherite_ingot",
                _           => "minecraft:cobblestone",
            };

            // Item pipe
            register_pipe(registry, PipeDef {
                block_id: format!("example:item_pipe_{}", tier.name.to_lowercase()),
                kind: PipeKind::Item,
                tier: tier.clone(),
                texture: None,
                shape: None,
                link_groups: vec!["pipe_item".into(), "inventory".into()],
                recipe_material: mat.into(),
                recipe_center: String::new(),
                energy_type: None,
            }).unwrap();

            // Signal pipe
            register_pipe(registry, PipeDef {
                block_id: format!("example:signal_pipe_{}", tier.name.to_lowercase()),
                kind: PipeKind::Signal,
                tier: tier.clone(),
                texture: None,
                shape: None,
                link_groups: vec!["pipe_signal".into(), "redstone".into()],
                recipe_material: mat.into(),
                recipe_center: "minecraft:redstone".into(),
                energy_type: None,
            }).unwrap();

            // Energy (Yog Flux) pipe — accepts all energy types by default
            register_pipe(registry, PipeDef {
                block_id: format!("example:flux_pipe_{}", tier.name.to_lowercase()),
                kind: PipeKind::Energy,
                tier: tier.clone(),
                texture: None,
                shape: None,
                link_groups: vec!["pipe_energy".into(), "energy_producer".into(), "energy_consumer".into()],
                recipe_material: mat.into(),
                recipe_center: "minecraft:glowstone_dust".into(),
                energy_type: None,
            }).unwrap();
        }
    }
}

yog_api::export_mod!(ExamplePipes);