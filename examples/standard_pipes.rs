//! Example: Standard pipes (5 tiers × 4 kinds = 20 pipes).
//!
//! This example shows how to register pipes using the `yog-pipes` framework.
//! Copy this into your own mod to customise textures, recipes, and tiers.
//!
//! ## Direct dependency (recommended)
//!
//! Add `yog-pipes` to your `[dependencies]` in `yog.toml`:
//! ```toml
//! [dependencies]
//! yog-pipes = "0.1"
//! ```
//!
//! Then call `register_pipe` directly:
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
//! ## Interop — dynamic mods without compile-time linking
//!
//! If your mod can't link `yog-pipes` at compile time (loaded dynamically by the runtime),
//! add `yog-pipes` to `[dependencies]` in `yog.toml`:
//! ```toml
//! [dependencies]
//! yog-pipes = "0.1"
//! ```
//!
//! The `yog build` tool automatically maps this to the exports crate:
//! ```toml
//! yog-exports = { package = "yog-pipes-exports", version = "0.1" }
//! ```
//!
//! The generated exports crate exposes the interop function. It accepts a single
//! serialisable `RegisterPipeArgs` struct (not the raw `Registry`) — the C-ABI
//! layer serialises everything via rkyv:
//! ```ignore
//! use yog_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, RegisterPipeArgs};
//!
//! yog_exports::yog_pipes::register_pipe(RegisterPipeArgs {
//!     api_ptr: registry.raw_api() as usize,
//!     def: PipeDef {
//!         block_id: "mymod:fluid_pipe_iron".into(),
//!         kind: PipeKind::Fluid,
//!         tier: PipeTier { name: "Iron".into(), speed: 2, tick_interval: 15,
//!                          signal_range: 16, energy_buffer: 250 },
//!         link_groups: vec!["pipe_fluid".into(), "tank".into()],
//!         recipe_material: "minecraft:iron_ingot".into(),
//!         recipe_center: "minecraft:water_bucket".into(),
//!         energy_type: None,
//!     },
//! }).unwrap();
//! ```
//!
//! The `register_pipe_interop` function (annotated with `#[yog_export]`) generates
//! a C-ABI wrapper. The imports crate uses `import!` to bind to it and expose
//! a normal Rust function that handles rkyv serialisation transparently.

use yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};

use yog_api::{Mod, Registry};

pub struct ExamplePipes;

impl Mod for ExamplePipes {
    fn register(registry: &mut Registry) {
        let tiers = [
            PipeTier { name: "Stone".into(),     speed: 1,  tick_interval: 20, signal_range: 8,   energy_buffer: 100  },
            PipeTier { name: "Iron".into(),      speed: 2,  tick_interval: 15, signal_range: 16,  energy_buffer: 250  },
            PipeTier { name: "Gold".into(),      speed: 4,  tick_interval: 10, signal_range: 32,  energy_buffer: 500  },
            PipeTier { name: "Diamond".into(),   speed: 8,  tick_interval: 5,  signal_range: 64,  energy_buffer: 1000 },
            PipeTier { name: "Netherite".into(), speed: 16, tick_interval: 3,  signal_range: 128, energy_buffer: 2000 },
        ];

        for tier in &tiers {
            let mat = match tier.name.as_str() {
                "Stone"     => "minecraft:cobblestone",
                "Iron"      => "minecraft:iron_ingot",
                "Gold"      => "minecraft:gold_ingot",
                "Diamond"   => "minecraft:diamond",
                "Netherite" => "minecraft:netherite_ingot",
                _           => "minecraft:cobblestone",
            };

            // ── Item pipe ─────────────────────────────────────────────────────
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

            // ── Fluid pipe ────────────────────────────────────────────────────
            register_pipe(registry, PipeDef {
                block_id: format!("example:fluid_pipe_{}", tier.name.to_lowercase()),
                kind: PipeKind::Fluid,
                tier: tier.clone(),
                texture: None,
                shape: None,
                link_groups: vec!["pipe_fluid".into(), "tank".into()],
                recipe_material: mat.into(),
                recipe_center: "minecraft:water_bucket".into(),
                energy_type: None,
            }).unwrap();

            // ── Signal pipe ───────────────────────────────────────────────────
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

            // ── Energy pipe (all types: YF, FE, RF, EU, custom) ──────────────
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