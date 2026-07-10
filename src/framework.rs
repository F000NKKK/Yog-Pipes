//! Pipe framework API — universal transport types for Yog mods.
//!
//! All types are `#[yog_export]`-ed so other mods can import them via:
//! ```ignore
//! use yog_pipes_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};
//! ```
//!
//! ## Quick start
//!
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
//! }).unwrap();
//! ```

use yog_api::yog_export;

// ── Pipe kind ────────────────────────────────────────────────────────────────

/// What a pipe carries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[yog_export]
pub enum PipeKind {
    Item,
    Fluid,
    Signal,
    Energy,
}

// ── Pipe tier ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub struct PipeTier {
    /// Display name (e.g. "Iron", "Gold"). Owned for serialization.
    pub name: String,
    /// Transfer speed: items/operations per cycle.
    pub speed: u32,
    /// Game ticks between transfer cycles.
    pub tick_interval: u32,
    /// Max graph distance for signal propagation.
    pub signal_range: u32,
    /// Max energy buffer per pipe node (for Energy kind).
    pub energy_buffer: u64,
}

// ── Pipe definition ──────────────────────────────────────────────────────────

/// Registration entry for one pipe block+item.
#[derive(Debug, Clone)]
#[yog_export]
pub struct PipeDef {
    /// Block/item id (e.g. `"mymod:item_pipe_iron"`).
    pub block_id: String,
    /// What this pipe carries.
    pub kind: PipeKind,
    /// Speed/capacity tier.
    pub tier: PipeTier,
    /// Custom block texture (optional — framework will use a default).
    pub texture: Option<String>,
    /// Custom collision/selection shape `(min_x, min_y, min_z, max_x, max_y, max_z)`.
    /// `None` = default pipe shape (4..12 on all axes).
    pub shape: Option<(f32, f32, f32, f32, f32, f32)>,
    /// Connect groups for automatic neighbor linking.
    pub link_groups: Vec<String>,
    /// Recipe material item (e.g. `"minecraft:iron_ingot"`). Empty = creative-only.
    pub recipe_material: String,
    /// Recipe center item (default: `"minecraft:glass_pane"`).
    pub recipe_center: String,
}

// ── Registration helper ──────────────────────────────────────────────────────

/// Register one pipe block + item + recipe.
///
/// Called by mods that depend on `yog-pipes` directly:
/// ```ignore
/// register_pipe(registry, PipeDef { ... }).unwrap();
/// register_pipe(registry, PipeDef { ... }).unwrap();
/// ```
pub fn register_pipe(registry: &mut yog_api::Registry, def: PipeDef) -> Result<(), String> {
    let shape = def.shape.unwrap_or((4.0, 4.0, 4.0, 12.0, 12.0, 12.0));

    let mut block = yog_api::BlockDef::new(&def.block_id)
        .strength(1.5, 3.0)
        .sound("stone")
        .shape(shape.0, shape.1, shape.2, shape.3, shape.4, shape.5)
        .connects_to_neighbors();

    if let Some(tex) = &def.texture {
        block = block.texture("default", tex);
    }

    let link_groups: Vec<&str> = def.link_groups.iter().map(|s| s.as_str()).collect();
    block = block.connect_groups(&link_groups);

    registry.register_block(block);

    let kind_str = match def.kind {
        PipeKind::Item => "Item",
        PipeKind::Fluid => "Fluid",
        PipeKind::Signal => "Signal",
        PipeKind::Energy => "Energy (Yog Flux)",
    };

    registry.register_item(
        yog_api::ItemDef::new(&def.block_id)
            .tooltip(format!(
                "§7{} §e{} Pipe\n§7Speed: §b{}§7 | Interval: §a{} ticks\n§7Signal range: §c{}§7 | Buffer: §d{}",
                kind_str, def.tier.name, def.tier.speed, def.tier.tick_interval,
                def.tier.signal_range, def.tier.energy_buffer
            ))
    );

    if !def.recipe_material.is_empty() {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let center = if def.recipe_center.is_empty() { "minecraft:glass_pane" } else { &def.recipe_center };
        registry.add_shaped_recipe(
            yog_api::ShapedRecipe::new(&format!("yog-pipes:pipe_{n}"), &def.block_id, 4)
                .row(" M ")
                .row("MGM")
                .row(" M ")
                .key('M', &def.recipe_material)
                .key('G', center)
        );
    }

    Ok(())
}

// ── Interop registration (for mods that can't depend directly) ───────────────

/// Serialisable arguments for the interop call.
#[derive(::yog_api::rkyv::Archive, ::yog_api::rkyv::Serialize, ::yog_api::rkyv::Deserialize)]
pub struct RegisterPipeArgs {
    /// Raw `YogApi` pointer from `Registry::raw_api()`.
    pub api_ptr: usize,
    /// Pipe definition.
    pub def: PipeDef,
}

/// Interop entry point — called by mods via `registry.interop().call("register_pipe", &args)`.
///
/// This function is exported under the `__yog_wrap_register_pipe` symbol.
#[yog_export]
pub fn register_pipe_interop(args: RegisterPipeArgs) -> Result<(), String> {
    let mut registry = unsafe { yog_api::Registry::from_raw(args.api_ptr as *const yog_api::YogApi) };
    register_pipe(&mut registry, args.def)
}