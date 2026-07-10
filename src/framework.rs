//! Pipe framework API — universal transport types for Yog mods.
//!
//! All types are `#[yog_export]`-ed so other mods can import them via:
//! ```ignore
//! use yog_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};
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
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

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
    /// Optional: restrict to specific energy type (e.g. `"yog:flux"`, `"forge:energy"`).
    /// Empty string means all energy types are accepted.
    pub energy_type: Option<String>,
}

// ── Energy system ────────────────────────────────────────────────────────────

/// A unique identifier for an energy type.
///
/// Built-in types:
/// - `"yog:flux"` — Yog Flux (YF), the canonical unit
/// - `"forge:energy"` — Forge Energy (FE)
/// - `"redstone:flux"` — Redstone Flux (RF)
/// - `"ic2:eu"` — Energy Unit (EU)
///
/// Mods can register custom types via [`register_energy_type`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnergyTypeId(pub String);

/// A registered energy type with conversion rates to/from Yog Flux (YF).
#[derive(Debug, Clone)]
#[yog_export]
pub struct EnergyType {
    /// Unique id (e.g. `"yog:flux"`, `"forge:energy"`, `"mymod:mana"`).
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// How many YF one unit of this type equals (e.g. 0.1 FE = 1 YF).
    pub yf_per_unit: f64,
    /// How many units of this type one YF equals (e.g. 1 YF = 10 FE).
    pub units_per_yf: f64,
}

/// Global registry of energy types. Keyed by `EnergyTypeId` string.
static ENERGY_TYPES: LazyLock<Mutex<HashMap<String, EnergyType>>> =
    LazyLock::new(|| {
        let mut m = HashMap::new();
        // Yog Flux — the canonical base unit
        m.insert("yog:flux".into(), EnergyType {
            id: "yog:flux".into(),
            display_name: "Yog Flux".into(),
            yf_per_unit: 1.0,
            units_per_yf: 1.0,
        });
        Mutex::new(m)
    });

/// Register a new energy type so other mods can build pipes for it.
///
/// ```ignore
/// register_energy_type(registry, EnergyType {
///     id: "mymod:mana".into(),
///     display_name: "Mana".into(),
///     yf_per_unit: 2.0,
///     units_per_yf: 0.5,
/// }).unwrap();
/// ```
pub fn register_energy_type(registry: &mut yog_api::Registry, et: EnergyType) -> Result<(), String> {
    let mut map = ENERGY_TYPES.lock().map_err(|e| e.to_string())?;
    if map.contains_key(&et.id) {
        return Err(format!("energy type '{}' already registered", et.id));
    }
    map.insert(et.id.clone(), et.clone());

    // Notify other mods via registry
    let _ = registry;

    Ok(())
}

/// Look up an energy type by its string id.
pub fn get_energy_type(id: &str) -> Option<EnergyType> {
    ENERGY_TYPES.lock().ok()?.get(id).cloned()
}

/// Convert `amount` units of `from` type to YF.
#[allow(dead_code)]
pub fn to_yf(from: &EnergyTypeId, amount: f64) -> f64 {
    if let Some(et) = get_energy_type(&from.0) {
        amount * et.yf_per_unit
    } else {
        // Unknown type — treat as raw YF
        amount
    }
}

/// Convert `yf_amount` YF to `amount` units of `to` type.
#[allow(dead_code)]
pub fn from_yf(to: &EnergyTypeId, yf_amount: f64) -> f64 {
    if let Some(et) = get_energy_type(&to.0) {
        yf_amount * et.units_per_yf
    } else {
        // Unknown type — treat as raw YF
        yf_amount
    }
}

/// List all registered energy types.
#[allow(dead_code)]
pub fn list_energy_types() -> Vec<EnergyType> {
    ENERGY_TYPES.lock().ok()
        .map(|map| map.values().cloned().collect())
        .unwrap_or_default()
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

    let link_groups: Vec<&str> = def.link_groups.iter().map(|s| s.as_str()).collect();

    let block = yog_api::BlockDef::new(&def.block_id)
        .strength(1.5, 3.0)
        .sound("stone")
        .shape(shape.0, shape.1, shape.2, shape.3, shape.4, shape.5)
        .connects_to_neighbors()
        .connect_groups(&link_groups);

    registry.register_block(block);

    let kind_str: String = match def.kind {
        PipeKind::Item => "Item".into(),
        PipeKind::Fluid => "Fluid".into(),
        PipeKind::Signal => "Signal".into(),
        PipeKind::Energy => {
            if let Some(ref et) = def.energy_type {
                if let Some(energy_type) = get_energy_type(et) {
                    energy_type.display_name
                } else {
                    "Energy".into()
                }
            } else {
                "Energy".into()
            }
        },
    };

    let energy_type_info = match (&def.kind, &def.energy_type) {
        (PipeKind::Energy, Some(et)) => format!("§7Type: §b{} ", et),
        _ => String::new(),
    };

    registry.register_item(
        yog_api::ItemDef::new(&def.block_id)
            .tooltip(format!(
                "§7{} §e{} Pipe\n{energy_type_info}§7Speed: §b{}§7 | Interval: §a{} ticks\n§7Signal range: §c{}§7 | Buffer: §d{}",
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
#[yog_export]
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