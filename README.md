# Yog Pipes

Universal bridge framework for [Yog mods](https://github.com/F000NKKK/Yog-Mod-Loader).

## What it is

Yog Pipes is **not** a fixed set of pipe blocks — it's a framework that lets modders define **any** kind of transport or signal pipe:

- **Item pipes** — move item stacks between inventories.
- **Fluid pipes** — transfer fluids between tanks and machines.
- **Signal pipes** — propagate redstone-like signals through the pipe network, letting blocks communicate without direct wiring.
- **Energy pipes** — transfer any energy type: Yog Flux (YF), Redstone Flux (RF), Forge Energy (FE), EU, or custom types defined by other mods.

The energy system is **extensible**: any mod can register a new `EnergyType` and other mods can build pipes, generators, and consumers for it.

## How it works

1. **Virtual graph** — when pipes are placed/broken, the framework rebuilds a graph of all connected pipes and adjacent inventories/machines.
2. **State machine** — each transfer tick cycles through `Extract → Route → Insert` for every pipe in the graph.
3. **BFS routing** — finds shortest paths between source and destination nodes.
4. **Signal propagation** — signals travel through the pipe network; any block can emit or listen on a signal channel.
5. **Customizable** — modders control textures, link groups (which blocks pipes connect to), shapes, and tiers.

## API

All public types are re-exported from `yog_pipes`:

| Type | Description |
|------|-------------|
| `PipeKind` | What a pipe carries: `Item`, `Fluid`, `Signal`, `Energy` |
| `PipeTier` | Speed, tick interval, signal range, energy buffer |
| `PipeDef` | Full pipe definition: block id, kind, tier, texture, shape, link groups, recipe |
| `EnergyType` | An energy type identifier (YF, RF, FE, EU, or custom) |
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

### 2. Interop (no compile-time linking)

If your mod can't link `yog-pipes` at compile time (loaded dynamically by the runtime),
you still add it to `[dependencies]` in `yog.toml`. The `yog build` tool automatically
maps `yog-pipes` → `yog_exports = { package = "yog_pipes_exports", ... }`.

Your code uses the **same types and functions**, but from the `yog_exports` namespace.
The generated exports crate exposes normal Rust functions — the interop C-ABI
serialization happens transparently under the hood via `import!`:

```rust
use yog_exports::yog_pipes::{PipeKind, PipeTier, PipeDef, register_pipe};

fn register(registry: &mut Registry) {
    register_pipe(registry, PipeDef {
        block_id: "mymod:item_pipe_iron".into(),
        kind: PipeKind::Item,
        tier: PipeTier { name: "Iron".into(), speed: 2, tick_interval: 15,
                         signal_range: 16, energy_buffer: 250 },
        link_groups: vec!["pipe_item".into(), "inventory".into()],
        recipe_material: "minecraft:iron_ingot".into(),
        recipe_center: String::new(),
        energy_type: None,
    }).unwrap();
}
```

No manual `registry.interop().call(...)` — the `import!` macro inside the
generated exports crate handles all serialization and binding automatically.

### 3. Custom energy types

Any mod can register a new energy type:

```rust
use yog_pipes::{EnergyType, register_energy_type};

// Register a custom energy type (e.g. "mana" from a magic mod)
register_energy_type(registry, EnergyType {
    id: "mymod:mana".into(),
    display_name: "Mana".into(),
    // Conversion rates to Yog Flux (YF)
    yf_per_unit: 2.0,
    units_per_yf: 0.5,
}).unwrap();
```

Then other mods can build pipes that transfer "mana" by specifying `EnergyTypeId("mymod:mana")` in their pipe definition.

## Example

See `examples/standard_pipes.rs` for a complete example that registers 15 pipe types (5 tiers × 3 kinds).

## License

Apache 2.0