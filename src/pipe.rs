//! Pipe block registration — item/fluid/energy pipes with auto-connection.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::{BlockDef, ItemDef, Registry};

// ── Tier definitions ────────────────────────────────────────────────────────

/// Pipe tiers with transfer speed (items per operation) and tick interval.
#[derive(Debug, Clone, Copy)]
pub struct PipeTier {
    pub name: &'static str,
    pub item_id: &'static str,
    pub speed: u32,       // items transferred per operation
    pub tick_interval: u32, // game ticks between transfers
}

const TIERS: &[PipeTier] = &[
    PipeTier { name: "Stone",     item_id: "yog-pipes:item_pipe_stone",     speed: 1,  tick_interval: 20 },
    PipeTier { name: "Iron",      item_id: "yog-pipes:item_pipe_iron",      speed: 2,  tick_interval: 15 },
    PipeTier { name: "Gold",      item_id: "yog-pipes:item_pipe_gold",      speed: 4,  tick_interval: 10 },
    PipeTier { name: "Diamond",   item_id: "yog-pipes:item_pipe_diamond",   speed: 8,  tick_interval: 5  },
    PipeTier { name: "Netherite", item_id: "yog-pipes:item_pipe_netherite", speed: 16, tick_interval: 3  },
];

/// Active pipe networks: (dimension, x, y, z) → PipeTier
pub static PIPE_NETWORK: LazyLock<Mutex<HashMap<(String, i32, i32, i32), PipeTier>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register(registry: &mut Registry) {
    for tier in TIERS {
        let name = format!("{} Item Pipe", tier.name);

        // Pipe block — connects to neighbors automatically
        registry.register_block(
            BlockDef::new(tier.item_id)
                .strength(1.5, 3.0)
                .sound("stone")
                .shape(4.0, 4.0, 4.0, 12.0, 12.0, 12.0)
                .connects_to_neighbors()
                .connect_groups(&["pipe_item"])
        );

        registry.register_item(
            ItemDef::new(tier.item_id)
                .tooltip(format!("§7Tier: §e{}§7 | Speed: §b{} items/op§7 | Interval: §a{} ticks",
                    tier.name, tier.speed, tier.tick_interval))
        );

        // Crafting recipes (simplified)
        let material = match tier.name {
            "Stone"     => "minecraft:cobblestone",
            "Iron"      => "minecraft:iron_ingot",
            "Gold"      => "minecraft:gold_ingot",
            "Diamond"   => "minecraft:diamond",
            "Netherite" => "minecraft:netherite_ingot",
            _ => "minecraft:cobblestone",
        };

        registry.add_shaped_recipe(
            yog_api::ShapedRecipe::new(
                &format!("yog-pipes:{}_craft", tier.item_id.replace(':', "_")),
                tier.item_id, 4,
            )
            .row(" M ")
            .row("MGM")
            .row(" M ")
            .key('M', material)
            .key('G', "minecraft:glass_pane")
        );
    }

    // ── Transfer tick ──────────────────────────────────────────────────────
    registry.on_tick(|_srv| {
        // TODO: iterate pipe network, transfer items between adjacent inventories
    });
}
