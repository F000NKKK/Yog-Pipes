# Yog Pipes

Universal transport framework for [Yog mods](https://github.com/F000NKKK/Yog-Mod-Loader).

## What it is

Yog Pipes is **not** a fixed set of pipe blocks and it does **not** know or care what a pipe carries — items, fluid, energy, a redstone-like signal, or anything a mod invents. It only gives you:

- **Pipe blocks** — register one via `PipeDef`, with your own shape/texture/recipes.
- **Connectivity** — pipes auto-link to neighbors through a real, world-checked virtual graph. **Branching is the normal case** — a pipe node can have anywhere from 0 to 6 connected neighbors (T-junctions, crosses, …), not just a single in/out.
- **Routing** — broadcast a payload to every node reachable on the network (signal-like fan-out), or send it point-to-point along the shortest path (item/energy-like transfer).
- **Dispatch** — bind a handler (an ordinary function in your own mod) to a position, and Yog Pipes calls it whenever a payload arrives there.

What the payload actually *means* — item stack, energy amount, redstone level — is entirely up to the mods producing and consuming it.

## How it works

1. **Virtual graph** (`src/graph.rs`) — when a pipe is placed or broken, the graph is rebuilt via BFS, *checking the real world* at every step (`yog_api::World::get_block`) — a neighbor becomes an edge only if it's itself a block that went through `register_pipe`. Never guessed from an id's spelling, never assumed connected without checking. Edges are a proper adjacency map, so every traversal handles any branching factor per node, including loops (a ring of branches reconnecting).
2. **Broadcast** — `broadcast(pos, payload)` reaches every node connected to `pos`, storing the payload and firing any bound handler at each one. Good for signal-like transport where every listener should see the same value.
3. **Point-to-point** — `send(from, to, payload)` finds the shortest path (if any) and delivers only to `to`. Good for item/energy/fluid transfer between two specific endpoints.
4. **Handlers** — `bind_handler(pos, mod_id, symbol)` points at an ordinary `#[yog_export] fn(PipePayload)` in *any* mod; no new callback ABI, it's resolved and called through the same interop mechanism every other cross-mod export uses.

Yog Pipes deliberately does **not** run a tick loop deciding when payloads move — call `broadcast`/`send` when a value actually changes (event-driven, like vanilla redstone), not every tick.

## Layout

```
src/
├── lib.rs        # mod entry point, event wiring
├── kind.rs       # PipeKind
├── model.rs      # 3D model system (ModelDef, ModelElement, FaceDef, ElementRotation)
├── recipe.rs      # RecipeDef (Shaped/Shapeless/Furnace) — recipes as data
├── pipe.rs       # PipeDef + register_pipe (the core entry point)
├── payload.rs    # PipePayload — the opaque data+metadata a pipe carries
├── transport.rs  # broadcast / send / read / bind_handler / unbind_handler
├── handler.rs    # resolves + calls a bound handler via the interop table
└── graph.rs      # the virtual pipe network (BFS, branching-safe)
```

## API

All types are re-exported from `yog_pipes`:

| Type / fn | Description |
|------|-------------|
| `PipeKind` | What a pipe carries: `Item`, `Fluid`, `Signal`, `Energy`, `Custom(String)` — purely descriptive, doesn't affect behavior |
| `PipeDef` | Open-ended pipe definition: `properties` map, `model`, `link_groups`, `recipe` |
| `RecipeDef` | One recipe as data: `Shaped { rows, keys, result_count }`, `Shapeless { ingredients, result_count }`, or `Furnace { input, result_count, experience, cook_time }` |
| `ModelDef` / `ModelElement` / `FaceDef` / `ElementRotation` | 3D block model: cubic elements, per-face textures, rotation |
| `register_pipe(registry, PipeDef)` | Register one pipe block + item + (optional) recipes |
| `PipePayload { data: Vec<u8>, metadata: Vec<(String, String)> }` | Arbitrary payload — Yog Pipes never reads `data` or `metadata`, mods define their own vocabulary |
| `PipePos { dim, x, y, z }` | A world position, for the transport calls below |
| `broadcast(registry, PipePos, PipePayload)` | Deliver to every reachable node + dispatch bound handlers |
| `send(registry, from: PipePos, to: PipePos, PipePayload) -> bool` | Point-to-point delivery via shortest path; `false` if no path exists |
| `read(PipePos) -> Option<PipePayload>` | Poll the last payload delivered to a position |
| `bind_handler(PipePos, mod_id, symbol)` / `unbind_handler(PipePos)` | Attach/remove a handler function at a position |

## Philosophy

- **No fixed tiers** — `PipeDef` has an open `properties: HashMap<String, f64>` map. Mods define exactly the parameters they need (speed, capacity, pressure, temperature, etc.).
- **Recipes as data** — `PipeDef::recipe: Vec<RecipeDef>` carries any number of recipes of any kind (shaped, shapeless, furnace), any pattern/ingredients/output count — not hardcoded to crafting-table-only, single-recipe pipes.
- **3D models** — `ModelDef` lets mods describe block models programmatically with per-face textures, elements, and rotation. No JSON model files required.
- **Link groups** — control which block types pipes connect to via string group names.
- **Extensible kinds** — `PipeKind::Custom("mymod:mana")` for any custom transport type; `PipeKind` itself is descriptive only, never inspected by the routing/dispatch logic.
- **Payload-agnostic transport** — Yog Pipes moves bytes + metadata; whether that's an item stack, energy, fluid, or a signal level is the producing/consuming mod's business, not this framework's.
- **No hardcoded conventions** — a block is a "pipe" because it went through `register_pipe`, never because its id contains some magic substring.

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
    recipe: vec![],
}).unwrap();
```

### Interop (no compile-time linking)

Add `yog-pipes` to `[dependencies]` in `yog.toml` — `yog build`/`yog add` map it to the exports crate. Every function is exported via `#[yog_export]`, so it's called exactly like the direct-dependency version above:

```rust
use yog_exports::yog_pipes::{PipeKind, PipeDef};

yog_exports::yog_pipes::register_pipe(PipeDef {
    block_id: "mymod:pipe_iron".into(),
    kind: PipeKind::Item,
    properties: [("speed", 2.0), ("tick_interval", 15.0)].into(),
    model: None,
    link_groups: vec!["pipe_item".into(), "inventory".into()],
    recipe: vec![],
}).unwrap();
```

The exports crate (`yog-pipes-exports` on crates.io) is published under **MIT OR Apache-2.0**, regardless of this mod's own AGPL license — `yog-cli` defaults every generated exports crate to that permissive license so depending on it never forces a consumer's mod under AGPL too (see `[mod] exports_license` in `yog.toml` if a mod wants the opposite).

### Recipes as data (any kind, any station)

```rust
use yog_pipes::RecipeDef;

register_pipe(registry, PipeDef {
    block_id: "mymod:pipe_iron".into(),
    kind: PipeKind::Item,
    properties: [("speed", 2.0)].into(),
    model: None,
    link_groups: vec!["pipe_item".into()],
    recipe: vec![
        RecipeDef::Shaped {
            rows: vec!["III".into()],
            keys: vec![('I', "minecraft:iron_ingot".into())],
            result_count: 4,
        },
        RecipeDef::Furnace {
            input: "mymod:raw_pipe".into(),
            result_count: 1,
            experience: 0.2,
            cook_time: 100,
        },
    ],
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
    recipe: vec![],
}).unwrap();
```

### Transport: broadcast (signal-like) and point-to-point (item/energy-like)

```rust
use yog_exports::yog_pipes::{PipePos, PipePayload, broadcast, send, read, bind_handler};

let pos = |x, y, z| PipePos { dim: "minecraft:overworld".into(), x, y, z };

// Every node reachable from (0, 64, 0) sees this — redstone-like fan-out.
// Call again whenever the source value changes, not every tick.
broadcast(pos(0, 64, 0), PipePayload {
    data: vec![15],
    metadata: vec![("kind".into(), "redstone".into())],
}).unwrap();

// Point-to-point — e.g. moving one item stack from a specific source to a
// specific destination. Returns `false` if no path currently connects them.
let delivered = send(
    pos(0, 64, 0), pos(4, 64, 0),
    PipePayload { data: item_bytes, metadata: vec![] },
).unwrap();

// Either poll...
let payload = read(pos(4, 64, 0)).unwrap();

// ...or bind a handler once — an ordinary `#[yog_export] fn(PipePayload)`
// in your own mod, called automatically whenever a payload arrives.
bind_handler(pos(4, 64, 0), "my-mod".into(), "on_pipe_payload".into()).unwrap();
```

## Known limitations

- **Dimension is hardcoded to `"overworld"`** in the graph rebuild trigger. `PlaceBlockEvent`/`BlockBreakEvent` in `yog-event` carry no dimension field at all, so there's currently no way to learn it from those callbacks — fixing that is an upstream Yog Mod Loader change (new event field, ABI bump, Java/Mixin plumbing), not something this framework can work around on its own.

## License

AGPL-v3.0 (the mod itself). The generated exports crate is MIT OR Apache-2.0 — see [Interop](#interop-no-compile-time-linking) above.
