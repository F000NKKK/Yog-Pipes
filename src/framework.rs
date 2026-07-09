//! Pipe framework API — register custom pipe types.
//!
//! See [crate-level docs](crate#quick-start) for usage examples.

use std::sync::{LazyLock, Mutex};

// ── Pipe kind ────────────────────────────────────────────────────────────────

/// What a pipe carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipeKind {
    Item,
    Fluid,
    Signal,
    Energy(YogFluxUnit),
}

/// Yog Flux energy unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum YogFluxUnit {
    /// 1 YF = 1 redstone tick equivalent.
    Flux,
}

// ── Pipe tier ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeTier {
    pub name: &'static str,
    pub speed: u32,
    pub tick_interval: u32,
    pub signal_range: u32,
    pub energy_buffer: u64,
}

impl PipeTier {
    pub const STONE:     PipeTier = PipeTier { name: "Stone",     speed: 1,  tick_interval: 20, signal_range: 8,   energy_buffer: 100  };
    pub const IRON:      PipeTier = PipeTier { name: "Iron",      speed: 2,  tick_interval: 15, signal_range: 16,  energy_buffer: 250  };
    pub const GOLD:      PipeTier = PipeTier { name: "Gold",      speed: 4,  tick_interval: 10, signal_range: 32,  energy_buffer: 500  };
    pub const DIAMOND:   PipeTier = PipeTier { name: "Diamond",   speed: 8,  tick_interval: 5,  signal_range: 64,  energy_buffer: 1000 };
    pub const NETHERITE: PipeTier = PipeTier { name: "Netherite", speed: 16, tick_interval: 3,  signal_range: 128, energy_buffer: 2000 };
}

// ── Pipe definition ──────────────────────────────────────────────────────────

pub struct PipeDef<'a> {
    pub block_id: &'a str,
    pub kind: PipeKind,
    pub tier: PipeTier,
    pub texture: Option<&'a str>,
    pub link_groups: &'a [&'a str],
    pub shape: Option<(f64, f64, f64, f64, f64, f64)>,
}

pub struct PipeRecipe<'a> {
    pub material: &'a str,
    pub center: &'a str,
}

impl Default for PipeRecipe<'_> {
    fn default() -> Self {
        PipeRecipe { material: "minecraft:cobblestone", center: "minecraft:glass_pane" }
    }
}

// ── Registration ─────────────────────────────────────────────────────────────

static RECIPE_COUNTER: LazyLock<Mutex<u64>> = LazyLock::new(|| Mutex::new(0));

/// Register one pipe block + item.
pub fn register_pipe(registry: &mut yog_api::Registry, def: PipeDef, recipe: PipeRecipe) {
    let (x0, y0, z0, x1, y1, z1) = def.shape.unwrap_or((4.0, 4.0, 4.0, 12.0, 12.0, 12.0));

    let mut block = yog_api::BlockDef::new(def.block_id)
        .strength(1.5, 3.0)
        .sound("stone")
        .shape(x0 as f32, y0 as f32, z0 as f32, x1 as f32, y1 as f32, z1 as f32)
        .connects_to_neighbors()
        .connect_groups(def.link_groups);

    if let Some(_tex) = def.texture {
        // TODO: wire texture to block model rendering
    }

    registry.register_block(block);

    let kind_str = match def.kind {
        PipeKind::Item => "Item",
        PipeKind::Fluid => "Fluid",
        PipeKind::Signal => "Signal",
        PipeKind::Energy(_) => "Energy (Yog Flux)",
    };

    registry.register_item(
        yog_api::ItemDef::new(def.block_id)
            .tooltip(format!(
                "§7{} §e{} Pipe\n§7Speed: §b{}§7 | Interval: §a{} ticks\n§7Signal range: §c{}§7 | Buffer: §d{}",
                kind_str, def.tier.name, def.tier.speed, def.tier.tick_interval,
                def.tier.signal_range, def.tier.energy_buffer
            ))
    );

    let mut counter = RECIPE_COUNTER.lock().unwrap();
    *counter += 1;
    let recipe_id = format!("yog-pipes:craft_{}", *counter);

    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new(&recipe_id, def.block_id, 4)
            .row(" M ")
            .row("MGM")
            .row(" M ")
            .key('M', recipe.material)
            .key('G', recipe.center)
    );
}

// ── JSON registration (for interop — no serde dependency) ────────────────────

/// Parse a JSON array of pipe definitions and register each one.
///
/// Expected format (one object per pipe):
/// ```json
/// [{"block_id":"mymod:pipe","kind":"item","tier":{...},"link_groups":["pipe"],"recipe":"mymod:rec"}]
/// ```
///
/// Falls back to simple string extraction — no serde needed.
pub fn register_pipe_from_json(registry: &mut yog_api::Registry, json: &str) {
    // Simple array-of-objects parser — extract each `{...}` block
    let mut depth = 0i32;
    let mut obj_start = 0usize;
    let mut in_string = false;
    let mut prev_char = '\0';

    for (i, ch) in json.char_indices() {
        if prev_char != '\\' && ch == '"' { in_string = !in_string; }
        if in_string { prev_char = ch; continue; }
        match ch {
            '{' => { if depth == 0 { obj_start = i; } depth += 1; }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let obj = &json[obj_start..=i];
                    if let Err(e) = parse_one_pipe_json(registry, obj) {
                        yog_api::warn!("[yog-pipes] bad pipe JSON: {} — {}", &obj[..obj.len().min(80)], e);
                    }
                }
            }
            _ => {}
        }
        prev_char = ch;
    }
}

fn parse_one_pipe_json(registry: &mut yog_api::Registry, obj: &str) -> Result<(), String> {
    let block_id = extract_string(obj, "block_id").ok_or("missing block_id")?;
    let kind_str = extract_string(obj, "kind").unwrap_or("item");
    let tier_name = extract_string(obj, "name").unwrap_or("Basic");
    let speed = extract_u32(obj, "speed").unwrap_or(1);
    let tick_interval = extract_u32(obj, "tick_interval").unwrap_or(20);
    let signal_range = extract_u32(obj, "signal_range").unwrap_or(8);
    let energy_buffer = extract_u64(obj, "energy_buffer").unwrap_or(100);
    let link_groups: Vec<&str> = extract_array(obj, "link_groups").unwrap_or_default();
    let _recipe_id = extract_string(obj, "recipe");

    let kind = match kind_str {
        "fluid"  => PipeKind::Fluid,
        "signal" => PipeKind::Signal,
        "energy" => PipeKind::Energy(YogFluxUnit::Flux),
        _        => PipeKind::Item,
    };

    let tier = PipeTier {
        name: Box::leak(tier_name.to_string().into_boxed_str()),
        speed, tick_interval, signal_range, energy_buffer,
    };

    // We can't use `&str` from the JSON directly (lifetime issues) — leak them
    let bid: &'static str = Box::leak(block_id.to_string().into_boxed_str());
    let lgs: Vec<&'static str> = link_groups.iter()
        .map(|s| Box::leak(s.to_string().into_boxed_str()) as &str)
        .collect();
    let lgs: &'static [&'static str] = Box::leak(lgs.into_boxed_slice());

    register_pipe(registry, PipeDef {
        block_id: bid, kind, tier,
        texture: None,
        link_groups: lgs,
        shape: None,
    }, PipeRecipe::default());

    Ok(())
}

// Simple string extractors (no allocation for common case)

fn extract_string<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{}\"", key);
    let pos = json.find(&needle)?;
    let after = &json[pos + needle.len()..];
    // Skip whitespace and colon
    let after = after.trim_start();
    let after = after.strip_prefix(':')?;
    let after = after.trim_start();
    if after.starts_with('"') {
        let end = after[1..].find('"')?;
        Some(&after[1..=end])
    } else {
        None
    }
}

fn extract_u32(json: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{}\"", key);
    let pos = json.find(&needle)?;
    let after = &json[pos + needle.len()..].trim_start().strip_prefix(':')?.trim_start();
    after.split(&[',', '}', ' '][..]).next()?.parse().ok()
}

fn extract_u64(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\"", key);
    let pos = json.find(&needle)?;
    let after = &json[pos + needle.len()..].trim_start().strip_prefix(':')?.trim_start();
    after.split(&[',', '}', ' '][..]).next()?.parse().ok()
}

fn extract_array<'a>(json: &'a str, key: &str) -> Option<Vec<&'a str>> {
    let needle = format!("\"{}\"", key);
    let pos = json.find(&needle)?;
    let after = &json[pos + needle.len()..].trim_start().strip_prefix(':')?.trim_start();
    let after = after.strip_prefix('[')?;
    let end = after.find(']')?;
    let inner = &after[..end];
    let items: Vec<&str> = inner.split(',')
        .map(|s| s.trim().trim_matches('"').trim())
        .filter(|s| !s.is_empty())
        .collect();
    Some(items)
}
