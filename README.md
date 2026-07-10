# Yog Pipes

Universal transport framework for [Yog mods](https://github.com/F000NKKK/Yog-Mod-Loader).

## What it is

Yog Pipes is **not** a fixed set of pipe blocks — it's a framework that lets modders define **any** kind of transport pipe:

- **Item pipes** — move item stacks between inventories.
- **Fluid pipes** — placeholder for future fluid API.
- **Signal pipes** — propagate redstone signals through the pipe network.
- **Energy pipes** — transfer **Yog Flux (YF)**, a custom energy unit.

## How it works

1. **Virtual graph** — when pipes are placed/broken, the framework rebuilds a graph of all connected pipes and adjacent inventories.
2. **State machine** — each transfer tick cycles through `Extract → Route → Insert` for every pipe in the graph.
3. **BFS routing** — finds shortest paths between source and destination nodes.
4. **Customizable** — modders control textures, link groups (which blocks pipes connect to), shapes, and tiers.

## API

All public types are re-exported from `yog_pipes`:

| Type | Description |
|------|-------------|
| `PipeKind` | What a pipe carries: `Item`, `Fluid`, `Signal`, `Energy` |
| `PipeTier` | Speed, tick interval, signal range, energy buffer |
| `PipeDef` | Full pipe definition: block id, kind, tier, texture, shape, link groups, recipe |
| `RegisterPipeArgs` | Serialisable args for interop calls (rkyv) |
| `register_pipe(registry, def)` | Register one pipe block + item + recipe |

## Usage

### 1. Direct dependency (recommended)

Add `yog-pipes` to your `Cargo.toml` and call `register_pipe` directly:

```rust
use yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};

fn register(registry: &mut Registry) {
    register_pipe(registry, PipeDef {
        block_id: "mymod:item_pipe_iron".into(),
        kind: PipeKind::Item,
        tier: PipeTier {
            name: "Iron".into(),
            speed: 2,
            tick_interval: 15,
            signal_range: 16,
            energy_buffer: 250,
        },
        texture: Some("mymod:textures/block/item_pipe.png".into()),
        shape: None,
        link_groups: vec!["pipe_item".into(), "inventory".into()],
        recipe_material: "minecraft:iron_ingot".into(),
        recipe_center: String::new(),
    }).unwrap();
}
```

### 2. Interop (no direct dependency)

If your mod can't depend on `yog-pipes` directly (e.g. it's loaded via Yog Mod Loader without Cargo linking), use the interop export:

```rust
use yog_pipes_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, RegisterPipeArgs};

fn register(registry: &mut Registry) {
    // Get the raw YogApi pointer
    let api_ptr = registry.raw_api() as usize;

    // Call the exported function via interop
    registry.interop().call("register_pipe", &RegisterPipeArgs {
        api_ptr,
        def: PipeDef {
            block_id: "mymod:item_pipe_iron".into(),
            kind: PipeKind::Item,
            tier: PipeTier { name: "Iron".into(), speed: 2, tick_interval: 15,
                             signal_range: 16, energy_buffer: 250 },
            link_groups: vec!["pipe_item".into(), "inventory".into()],
            recipe_material: "minecraft:iron_ingot".into(),
            recipe_center: String::new(),
        },
    }).unwrap();
}
```

## Example

See `examples/standard_pipes.rs` for a complete example that registers 15 pipe types (5 tiers × 3 kinds).

## License

Apache 2.0