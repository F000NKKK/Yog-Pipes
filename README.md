# Yog Pipes

Universal transport framework for [Yog mods](https://github.com/F000NKKK/Yog-Mod-Loader).

## What it is

Yog Pipes is **not** a fixed set of pipe blocks — it's a framework that lets modders define **any** kind of transport pipe:

- **Item pipes** — move item stacks between inventories.
- **Fluid pipes** — placeholder for future fluid API.
- **Signal pipes** — propagate redstone signals through the pipe network.
- **Energy pipes** — transfer **Yog Flux (YF)**, a custom energy unit (1 YF = 1 redstone tick equivalent).

## How it works

1. **Virtual graph** — when pipes are placed/broken, the framework rebuilds a graph of all connected pipes and adjacent inventories.
2. **State machine** — each transfer tick cycles through `Extract → Route → Insert` for every pipe in the graph.
3. **BFS routing** — finds shortest paths between source and destination nodes.
4. **Customizable** — modders control textures, link groups (which blocks pipes connect to), shapes, and tiers.

## Quick start

Add `yog-pipes` as a dependency and register your pipes:

```rust
use yog_pipes::{PipeKind, PipeTier, PipeDef, PipeRecipe, register_pipe};

register_pipe(registry, PipeDef {
    block_id:    "mymod:item_pipe_iron",
    kind:        PipeKind::Item,
    tier:        PipeTier { name: "Iron", speed: 2, tick_interval: 15, signal_range: 16, energy_buffer: 250 },
    texture:     Some("mymod:textures/block/item_pipe.png"), // optional custom texture
    link_groups: &["pipe_item", "inventory"],                 // what blocks this connects to
    shape:       None,                                         // optional custom voxel shape
}, PipeRecipe { material: "minecraft:iron_ingot", ..Default::default() });
```

See `examples/standard_pipes.rs` for 15 pipe types (5 tiers × 3 kinds).

## API

- `PipeDef` — block ID, kind, tier, texture, link groups, shape.
- `PipeTier` — fully configurable: speed, tick interval, signal range, energy buffer.
- `PipeRecipe` — material + center item for crafting.
- `register_pipe(registry, def, recipe)` — registers one block + item + recipe.

## License

Apache 2.0
