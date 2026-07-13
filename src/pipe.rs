//! Pipe definition and registration — the framework's core entry point.
//!
//! This is intentionally minimal — the framework does **not** dictate:
//! - What properties a pipe has (use `properties` map)
//! - What recipe(s) it uses (use `recipe`, or leave it empty and register
//!   recipes separately)
//! - What model it has (use `model` field or leave `None` for default)

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::yog_export;

use crate::kind::PipeKind;
use crate::model::{self, ModelDef};
use crate::recipe::{self, RecipeDef};

/// Registration entry for one pipe block+item.
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
    /// Crafting recipes for this pipe's item — zero, one, or several (e.g. a
    /// shaped recipe AND a furnace recipe for the same output). Empty means
    /// no recipe is registered (mods can still add their own separately).
    pub recipe: Vec<RecipeDef>,
}

/// Every block id ever registered via [`register_pipe`], with the
/// [`PipeKind`] it carries — the graph (`crate::graph`) consults this to
/// decide whether a world position holds a pipe, instead of guessing from
/// the block id's spelling.
static PIPE_BLOCKS: LazyLock<Mutex<HashMap<String, PipeKind>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether `block_id` was registered as a pipe via [`register_pipe`].
pub fn is_pipe_block(block_id: &str) -> bool {
    PIPE_BLOCKS.lock().unwrap().contains_key(block_id)
}

/// Register one pipe block + item.
///
/// ```ignore
/// register_pipe(registry, PipeDef {
///     block_id: "mymod:pipe_iron".into(),
///     kind: PipeKind::Item,
///     properties: [("speed", 2.0), ("tick_interval", 15.0)].into(),
///     model: None,
///     link_groups: vec!["pipe_item".into(), "inventory".into()],
///     recipe: vec![],
/// }).unwrap();
/// ```
#[yog_export]
pub fn register_pipe(registry: &mut yog_api::Registry, def: PipeDef) -> Result<(), String> {
    let shape = model::resolve_shape(&def.model);

    let link_groups: Vec<&str> = def.link_groups.iter().map(|s| s.as_str()).collect();

    let block = yog_api::BlockDef::new(&def.block_id)
        .strength(1.5, 3.0)
        .sound("stone")
        .shape(shape.0, shape.1, shape.2, shape.3, shape.4, shape.5)
        .connects_to_neighbors()
        .connect_groups(&link_groups);

    let block = if let Some(ref model) = def.model {
        model::apply(block, model)
    } else {
        block
    };

    registry.register_block(block);

    let kind_str = crate::kind::display_name(&def.kind);
    let tooltip = build_tooltip(kind_str, &def.properties);
    registry.register_item(yog_api::ItemDef::new(&def.block_id).tooltip(tooltip));

    for r in &def.recipe {
        recipe::register(registry, &def.block_id, r);
    }

    PIPE_BLOCKS
        .lock()
        .unwrap()
        .insert(def.block_id.clone(), def.kind.clone());

    Ok(())
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
