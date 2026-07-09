//! Example: Standard item/signal/energy pipes (5 tiers × 3 kinds = 15 pipes).
//!
//! Copy this example into your own mod to customize textures, recipes, and tiers.

use yog_pipes::{
    PipeDef, PipeKind, PipeRecipe, PipeTier, YogFluxUnit, register_pipe,
};

use yog_api::{Mod, Registry};

pub struct ExamplePipes;

impl Mod for ExamplePipes {
    fn register(registry: &mut Registry) {
        let tiers = [
            PipeTier { name: "Stone",     speed: 1,  tick_interval: 20, signal_range: 8,   energy_buffer: 100  },
            PipeTier { name: "Iron",      speed: 2,  tick_interval: 15, signal_range: 16,  energy_buffer: 250  },
            PipeTier { name: "Gold",      speed: 4,  tick_interval: 10, signal_range: 32,  energy_buffer: 500  },
            PipeTier { name: "Diamond",   speed: 8,  tick_interval: 5,  signal_range: 64,  energy_buffer: 1000 },
            PipeTier { name: "Netherite", speed: 16, tick_interval: 3,  signal_range: 128, energy_buffer: 2000 },
        ];

        for tier in &tiers {
            let mat = match tier.name {
                "Stone"     => "minecraft:cobblestone",
                "Iron"      => "minecraft:iron_ingot",
                "Gold"      => "minecraft:gold_ingot",
                "Diamond"   => "minecraft:diamond",
                "Netherite" => "minecraft:netherite_ingot",
                _           => "minecraft:cobblestone",
            };

            // Item pipe
            register_pipe(registry, PipeDef {
                block_id:    &format!("example:item_pipe_{}", tier.name.to_lowercase()),
                kind:        PipeKind::Item,
                tier:        *tier,
                texture:     None,
                link_groups: &["pipe_item", "inventory"],
                shape:       None,
            }, PipeRecipe { material: mat, ..Default::default() });

            // Signal pipe
            register_pipe(registry, PipeDef {
                block_id:    &format!("example:signal_pipe_{}", tier.name.to_lowercase()),
                kind:        PipeKind::Signal,
                tier:        *tier,
                texture:     None,
                link_groups: &["pipe_signal", "redstone"],
                shape:       None,
            }, PipeRecipe { material: mat, center: "minecraft:redstone" });

            // Energy (Yog Flux) pipe
            register_pipe(registry, PipeDef {
                block_id:    &format!("example:flux_pipe_{}", tier.name.to_lowercase()),
                kind:        PipeKind::Energy(YogFluxUnit::Flux),
                tier:        *tier,
                texture:     None,
                link_groups: &["pipe_energy", "energy_producer", "energy_consumer"],
                shape:       None,
            }, PipeRecipe { material: mat, center: "minecraft:glowstone_dust" });
        }
    }
}

yog_api::export_mod!(ExamplePipes);
