//! Example: Registering pipes with the Yog Pipes framework.
//!
//! This example demonstrates how mods define their own pipes using the
//! framework's open-ended API — no fixed tiers, no forced recipes.
//!
//! ## Direct dependency
//!
//! ```toml
//! [dependencies]
//! yog-pipes = "0.1"
//! ```
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
//! ## Interop
//!
//! ```toml
//! [dependencies]
//! yog-pipes = "0.1"
//! ```
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
//! ---
//!
//! Below are examples showing what different mods might register.
//! Each mod defines its own parameters — the framework does not dictate them.

use yog_pipes::{PipeKind, PipeDef, register_pipe};

use yog_api::{Mod, Registry};

pub struct ExamplePipes;

impl Mod for ExamplePipes {
    fn register(registry: &mut Registry) {
        // ── Example 1: Simple item pipe with custom speed ────────────────────
        register_pipe(registry, PipeDef {
            block_id: "example:item_pipe_stone".into(),
            kind: PipeKind::Item,
            properties: [
                ("speed", 1.0),
                ("tick_interval", 20.0),
            ].into(),
            model: None,
            link_groups: vec!["pipe_item".into(), "inventory".into()],
        }).unwrap();

        // ── Example 2: High-speed item pipe ──────────────────────────────────
        register_pipe(registry, PipeDef {
            block_id: "example:item_pipe_netherite".into(),
            kind: PipeKind::Item,
            properties: [
                ("speed", 16.0),
                ("tick_interval", 2.0),
                ("stack_size", 64.0),
            ].into(),
            model: None,
            link_groups: vec!["pipe_item".into(), "inventory".into()],
        }).unwrap();

        // ── Example 3: Fluid pipe with temperature limit ─────────────────────
        register_pipe(registry, PipeDef {
            block_id: "example:fluid_pipe_iron".into(),
            kind: PipeKind::Fluid,
            properties: [
                ("speed", 2.0),
                ("fluid_capacity", 4000.0),
                ("temperature_max", 1000.0),
            ].into(),
            model: None,
            link_groups: vec!["pipe_fluid".into(), "tank".into()],
        }).unwrap();

        // ── Example 4: Lava-proof pipe with high pressure ───────────────────
        register_pipe(registry, PipeDef {
            block_id: "example:fluid_pipe_obsidian".into(),
            kind: PipeKind::Fluid,
            properties: [
                ("speed", 1.0),
                ("fluid_capacity", 8000.0),
                ("temperature_max", 3000.0),
                ("pressure_max", 50.0),
            ].into(),
            model: None,
            link_groups: vec!["pipe_fluid".into(), "tank".into()],
        }).unwrap();

        // ── Example 5: Signal pipe with range ────────────────────────────────
        register_pipe(registry, PipeDef {
            block_id: "example:signal_pipe_redstone".into(),
            kind: PipeKind::Signal,
            properties: [
                ("signal_range", 15.0),
            ].into(),
            model: None,
            link_groups: vec!["pipe_signal".into(), "redstone".into()],
        }).unwrap();

        // ── Example 6: Energy pipe for Yog Flux only ─────────────────────────
        register_pipe(registry, PipeDef {
            block_id: "example:yf_pipe_basic".into(),
            kind: PipeKind::Energy,
            properties: [
                ("speed", 1.0),
                ("energy_buffer", 10000.0),
                ("yf_per_tick", 100.0),
            ].into(),
            model: None,
            link_groups: vec!["pipe_energy".into(), "energy_producer".into(), "energy_consumer".into()],
        }).unwrap();

        // ── Example 7: Custom pipe kind ("mana" from a magic mod) ────────────
        register_pipe(registry, PipeDef {
            block_id: "example:mana_pipe_crystal".into(),
            kind: PipeKind::Custom("mymod:mana".into()),
            properties: [
                ("mana_per_tick", 50.0),
                ("range", 32.0),
                ("purity_required", 0.8),
            ].into(),
            model: None,
            link_groups: vec!["pipe_mana".into(), "mana_source".into(), "mana_consumer".into()],
        }).unwrap();

        // ── Example 8: No recipe — mod adds its own ─────────────────────────
        // The framework never generates recipes. If a mod wants one:
        // registry.add_shaped_recipe(
        //     yog_api::ShapedRecipe::new("example:pipe_iron", "example:item_pipe_stone", 1)
        //         .row("III")
        //         .row("IGI")
        //         .row("III")
        //         .key('I', "minecraft:iron_ingot")
        //         .key('G', "minecraft:glass_pane")
        // );
    }
}

yog_api::export_mod!(ExamplePipes);