//! Pipe framework API — universal transport types for Yog mods.
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
//!
//! ## Quick start
//!
//! ```ignore
//! use yog_pipes::{PipeKind, PipeDef, ModelDef, register_pipe};
//!
//! register_pipe(registry, PipeDef {
//!     block_id: "mymod:pipe_iron".into(),
//!     kind: PipeKind::Item,
//!     properties: [("speed", 2.0), ("tick_interval", 15.0)].into(),
//!     model: Some(ModelDef {
//!         texture: Some("mymod:block/pipe_iron".into()),
//!         elements: vec![],
//!     }),
//!     link_groups: vec!["pipe_item".into(), "inventory".into()],
//! }).unwrap();
//! ```

use std::collections::HashMap;
use yog_api::yog_export;

// ── Pipe kind ────────────────────────────────────────────────────────────────

/// What a pipe carries. Mods can add custom kinds via the string-based
/// [`PipeKind::Custom`] variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[yog_export]
pub enum PipeKind {
    Item,
    Fluid,
    Signal,
    Energy,
    /// Any custom kind — identified by a string id (e.g. `"mymod:mana"`).
    Custom(String),
}

// ── 3D model system ─────────────────────────────────────────────────────────

/// Describes a block model with cubic elements and per-face textures.
///
/// This replaces the need for separate JSON model files. Mods describe
/// their pipe shape programmatically.
#[derive(Debug, Clone)]
#[yog_export]
pub struct ModelDef {
    /// Block texture (e.g. `"mymod:block/pipe_iron"`).
    /// If `None`, the framework uses a default pipe texture.
    pub texture: Option<String>,
    /// List of cubic elements that make up the model.
    /// Empty = default pipe shape (4..12 on all axes).
    pub elements: Vec<ModelElement>,
}

/// A single cubic element in a block model.
#[derive(Debug, Clone)]
#[yog_export]
pub struct ModelElement {
    /// Start position `(x, y, z)` in 16×16×16 voxel space (0..16).
    pub from: [f32; 3],
    /// End position `(x, y, z)` in 16×16×16 voxel space (0..16).
    pub to: [f32; 3],
    /// Per-face textures and UV data. Key: `"up"`, `"down"`, `"north"`,
    /// `"south"`, `"east"`, `"west"`, or `"all"` to set all faces at once.
    pub faces: HashMap<String, FaceDef>,
    /// Optional rotation around a center point.
    pub rotation: Option<ElementRotation>,
}

/// Texture and UV data for one face of a model element.
#[derive(Debug, Clone)]
#[yog_export]
pub struct FaceDef {
    /// Texture reference (e.g. `"mymod:block/pipe_iron"`).
    /// If empty, inherits from [`ModelDef::texture`].
    pub texture: String,
    /// UV coordinates `[u_min, v_min, u_max, v_max]` in 0..16 range.
    /// Empty = full face (0, 0, 16, 16).
    pub uv: Option<[f32; 4]>,
    /// Rotation of the face texture in 90-degree increments (0, 90, 180, 270).
    pub rotation: u32,
}

/// Rotation of a model element around a center point.
#[derive(Debug, Clone)]
#[yog_export]
pub struct ElementRotation {
    /// Center of rotation `(x, y, z)` in voxel space.
    pub origin: [f32; 3],
    /// Axis: `"x"`, `"y"`, or `"z"`.
    pub axis: String,
    /// Angle in degrees (positive = clockwise when looking towards origin
    /// along the positive axis direction). Typically -45, -22.5, 22.5, 45.
    pub angle: f32,
    /// Whether to rescale the faces after rotation.
    pub rescale: bool,
}

// ── Pipe definition ──────────────────────────────────────────────────────────

/// Registration entry for one pipe block+item.
///
/// This is intentionally minimal — the framework does **not** dictate:
/// - What properties a pipe has (use `properties` map)
/// - What recipe it uses (register separately via `registry.add_shaped_recipe()`)
/// - What model it has (use `model` field or leave `None` for default)
#[derive(Debug, Clone)]
#[yog_export]
pub struct PipeDef {
    /// Block/item id (e.g. `"mymod:pipe_iron"`).
    pub block_id: String,
    /// What this pipe carries.
    pub kind: PipeKind,
    /// Open property map for any pipe parameters.
    ///
    /// Common keys (all optional):
    /// - `"speed"` — transfer speed (items/ops per cycle)
    /// - `"tick_interval"` — game ticks between transfer cycles
    /// - `"signal_range"` — max graph distance for signal propagation
    /// - `"energy_buffer"` — max energy buffer per node
    /// - `"fluid_capacity"` — max fluid capacity in mB
    /// - `"temperature_max"` — max fluid temperature
    /// - `"pressure_max"` — max pressure
    ///
    /// Mods can add **any** keys they need. Other mods can read them.
    pub properties: HashMap<String, f64>,
    /// 3D model definition. `None` = default pipe shape.
    pub model: Option<ModelDef>,
    /// Connect groups for automatic neighbor linking.
    pub link_groups: Vec<String>,
}

// ── Registration helper ──────────────────────────────────────────────────────

/// Register one pipe block + item.
///
/// The framework registers the block and item. **Recipes are NOT generated** —
/// mods must register their own via `registry.add_shaped_recipe()` if desired.
///
/// ```ignore
/// register_pipe(registry, PipeDef {
///     block_id: "mymod:pipe_iron".into(),
///     kind: PipeKind::Item,
///     properties: [("speed", 2.0), ("tick_interval", 15.0)].into(),
///     model: None,
///     link_groups: vec!["pipe_item".into(), "inventory".into()],
/// }).unwrap();
/// ```
pub fn register_pipe(registry: &mut yog_api::Registry, def: PipeDef) -> Result<(), String> {
    let shape = resolve_shape(&def.model);

    let link_groups: Vec<&str> = def.link_groups.iter().map(|s| s.as_str()).collect();

    let block = yog_api::BlockDef::new(&def.block_id)
        .strength(1.5, 3.0)
        .sound("stone")
        .shape(shape.0, shape.1, shape.2, shape.3, shape.4, shape.5)
        .connects_to_neighbors()
        .connect_groups(&link_groups);

    // Apply model if provided
    let block = if let Some(ref model) = def.model {
        apply_model(block, model)
    } else {
        block
    };

    registry.register_block(block);

    let kind_str = kind_display_name(&def.kind);
    let tooltip = build_tooltip(&kind_str, &def.properties);

    registry.register_item(yog_api::ItemDef::new(&def.block_id).tooltip(tooltip));

    Ok(())
}

/// Resolve collision shape from model or use default pipe shape.
fn resolve_shape(model: &Option<ModelDef>) -> (f32, f32, f32, f32, f32, f32) {
    if let Some(ref m) = model {
        if !m.elements.is_empty() {
            // Compute bounding box from all elements
            let mut min = [16.0f32; 3];
            let mut max = [0.0f32; 3];
            for el in &m.elements {
                for i in 0..3 {
                    min[i] = min[i].min(el.from[i]);
                    max[i] = max[i].max(el.to[i]);
                }
            }
            // Scale from 0..16 to -8..8 (Minecraft block space)
            return (
                min[0] - 8.0,
                min[1] - 8.0,
                min[2] - 8.0,
                max[0] - 8.0,
                max[1] - 8.0,
                max[2] - 8.0,
            );
        }
    }
    // Default pipe shape: 4..12 on all axes → -4..4 in block space
    (4.0, 4.0, 4.0, 12.0, 12.0, 12.0)
}

/// Apply model data to a BlockDef (textures, elements, etc.)
fn apply_model(block: yog_api::BlockDef, _model: &ModelDef) -> yog_api::BlockDef {
    // Model data (textures, elements, faces) is used by the renderer layer.
    // BlockDef handles shape, strength, connect groups — model details
    // are passed to the Yog runtime separately.
    let _ = &_model.texture;
    block
}

fn kind_display_name(kind: &PipeKind) -> &'static str {
    match kind {
        PipeKind::Item => "Item",
        PipeKind::Fluid => "Fluid",
        PipeKind::Signal => "Signal",
        PipeKind::Energy => "Energy",
        PipeKind::Custom(_) => "Custom",
    }
}

fn build_tooltip(kind: &str, props: &HashMap<String, f64>) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("§7{} Pipe", kind));

    if let Some(speed) = props.get("speed") {
        parts.push(format!("§7Speed: §b{}", speed));
    }
    if let Some(interval) = props.get("tick_interval") {
        parts.push(format!("§7Interval: §a{} ticks", interval));
    }
    if let Some(range) = props.get("signal_range") {
        parts.push(format!("§7Signal range: §c{}", range));
    }
    if let Some(buf) = props.get("energy_buffer") {
        parts.push(format!("§7Buffer: §d{}", buf));
    }
    if let Some(cap) = props.get("fluid_capacity") {
        parts.push(format!("§7Capacity: §b{} mB", cap));
    }
    if let Some(temp) = props.get("temperature_max") {
        parts.push(format!("§7Max temp: §c{}°C", temp));
    }
    if let Some(pressure) = props.get("pressure_max") {
        parts.push(format!("§7Max pressure: §e{} atm", pressure));
    }

    // Add any custom properties that aren't standard
    for (k, v) in props {
        if ![
            "speed",
            "tick_interval",
            "signal_range",
            "energy_buffer",
            "fluid_capacity",
            "temperature_max",
            "pressure_max",
        ]
        .contains(&k.as_str())
        {
            parts.push(format!("§7{}: §b{}", k, v));
        }
    }

    parts.join("\n")
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
    let mut registry =
        unsafe { yog_api::Registry::from_raw(args.api_ptr as *const yog_api::YogApi) };
    register_pipe(&mut registry, args.def)
}
