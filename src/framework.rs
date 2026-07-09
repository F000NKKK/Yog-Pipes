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
        .shape(x0, y0, z0, x1, y1, z1)
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
