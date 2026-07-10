# Yog Pipes

Universal transport framework for [Yog mods](https://github.com/F000NKKK/Yog-Mod-Loader).

## What it is

Yog Pipes is **not** a fixed set of pipe blocks — it's a **framework** that lets modders define **any** kind of transport or signal pipe:

- **Item pipes** — move item stacks between inventories.
- **Fluid pipes** — transfer fluids between tanks and machines.
- **Signal pipes** — propagate redstone-like signals through the pipe network, letting blocks communicate without direct wiring.
- **Energy pipes** — transfer any energy type: Yog Flux (YF), Forge Energy (FE), Redstone Flux (RF), EU, or custom types.
- **Custom pipes** — `PipeKind::Custom("mymod:mana")` for anything else.

## How it works

1. **Virtual graph** — when pipes are placed/broken, the framework rebuilds a graph of all connected pipes and adjacent blocks.
2. **State machine** — each transfer tick cycles through `Extract → Route → Insert` for every pipe in the graph.
3. **BFS routing** — finds shortest paths between source and destination nodes.
4. **Signal propagation** — signals travel through the pipe network; any block can emit or listen on a signal channel.

## API

All types are re-exported from `yog_pipes`:

| Type | Description |
|------|-------------|
| `PipeKind` | What a pipe carries: `Item`, `Fluid`, `Signal`, `Energy`, `Custom(String)` |
| `PipeDef` | Open-ended pipe definition with `properties` map, `model`, `link_groups` |
| `ModelDef` | 3D block model with cubic elements, per-face textures, rotation |
| `ModelElement` | A single cube in the model (`from`/`to` in voxel space) |
| `FaceDef` | Per-face texture + UV coordinates |
| `ElementRotation` | Rotation of a model element |
| `RegisterPipeArgs` | Serialisable args for interop calls (rkyv) |
| `register_pipe()` | Register one pipe block + item |

## Philosophy

- **No fixed tiers** — `PipeDef` has an open `properties: HashMap<String, f64>` map. Mods define exactly the parameters they need (speed, capacity, pressure, temperature, etc.).
- **No recipes** — the framework never generates recipes. Mods register their own via `registry.add_shaped_recipe()` if they want crafting.
- **3D models** — `ModelDef` lets mods describe block models programmatically with per-face textures, elements, and rotation. No JSON model files required.
- **Link groups** — control which block types pipes connect to via string group names.
- **Extensible kinds** — `PipeKind::Custom("mymod:mana")` for any custom transport type.

## Usage

### Direct dependency

```rust
use yog_pipes::{PipeKind, PipeDef, register_pipe};

register_pipe(registry, PipeDef {
    block_id: "mymod:pipe_iron".into(),
    kind: PipeKind::Item,
    properties: [("speed", 2.0), ("tick_interval", 15.0)].into(),
    model: None,
    link_groups: vec!["pipe_item".into(), "inventory".into()],
}).unwrap();
```

### Interop (no compile-time linking)

Add `yog-pipes` to `[dependencies]` in `yog.toml` — `yog build` maps it to the exports crate:

```rust
use yog_exports::yog_pipes::{PipeKind, PipeDef, RegisterPipeArgs};

yog_exports::yog_pipes::register_pipe(RegisterPipeArgs {
    api_ptr: registry.raw_api() as usize,
    def: PipeDef { ... },
}).unwrap();
```

### Custom 3D model

```rust
use yog_pipes::{
    PipeKind, PipeDef, ModelDef, ModelElement, FaceDef,
    ElementRotation, register_pipe,
};

register_pipe(registry, PipeDef {
    block_id: "mymod:glass_pipe".into(),
    kind: PipeKind::Fluid,
    properties: [("fluid_capacity", 8000.0)].into(),
    model: Some(ModelDef {
        texture: Some("mymod:block/glass_pipe".into()),
        elements: vec![ModelElement {
            from: [5.0, 0.0, 5.0],
            to: [11.0, 16.0, 11.0],
            faces: [].into(),
            rotation: None,
        }],
    }),
    link_groups: vec!["pipe_fluid".into(), "tank".into()],
}).unwrap();
```

### Custom property (fluid temperature limit)

```rust
register_pipe(registry, PipeDef {
    block_id: "mymod:lava_proof_pipe".into(),
    kind: PipeKind::Fluid,
    properties: [
        ("speed", 1.0),
        ("fluid_capacity", 2000.0),
        ("temperature_max", 1500.0),
    ].into(),
    model: None,
    link_groups: vec!["pipe_fluid".into(), "tank".into()],
}).unwrap();
```

## License

Apache 2.0