//! Yog Pipes — universal transport framework for Yog mods.
//!
//! This is a **framework**, not a mod — it's a set of pipe building blocks
//! plus its own small runtime (a virtual graph) for mods to build
//! transport/signal networks on, without dictating recipes, tiers, models,
//! or what a pipe actually carries.
//!
//! ## Philosophy
//!
//! - **No fixed fields** — `PipeDef` carries an open `properties` map instead
//!   of a rigid `PipeTier` struct. Mods define whatever parameters they need.
//! - **Recipes as data** — `PipeDef::recipe` carries any number of recipes
//!   (shaped, shapeless, furnace, …), any pattern/ingredients/output. Leave
//!   it empty to register none, or add recipes separately.
//! - **3D models** — `ModelDef` lets mods describe block models with
//!   per-face textures, elements, and rotation — no JSON files needed.
//! - **Link groups** — control which blocks pipes connect to.
//! - **Extensible pipe kinds** — `PipeKind::Custom("mymod:mana")` for custom types.
//! - **Payload-agnostic transport** — whether a pipe carries items, fluid,
//!   energy, or a redstone-like signal is entirely up to the mods
//!   producing/consuming `PipePayload`; Yog-Pipes only connects, routes,
//!   and dispatches.
//! - **Branching is normal** — a pipe node can have any number of connected
//!   neighbors (0–6); every graph traversal here treats that as the
//!   default case, not something bolted on for T-junctions.
//! - **No hardcoded conventions** — pipe blocks are recognized by having
//!   gone through `register_pipe`, never by guessing from an id's spelling.
//!
//! ## Usage
//!
//! ### Direct dependency
//!
//! ```ignore
//! use yog_pipes::{PipeKind, PipeDef, register_pipe};
//!
//! register_pipe(registry, PipeDef {
//!     block_id: "mymod:pipe_iron".into(),
//!     kind: PipeKind::Item,
//!     properties: [("speed", 2.0), ("tick_interval", 15.0)].into(),
//!     model: None,
//!     link_groups: vec!["pipe_item".into(), "inventory".into()],
//!     recipe: vec![],
//! }).unwrap();
//! ```
//!
//! ### Interop
//!
//! ```ignore
//! use yog_exports::yog_pipes::{PipeKind, PipeDef};
//!
//! yog_exports::yog_pipes::register_pipe(PipeDef { ... }).unwrap();
//! ```
//!
//! ### Transport — broadcast (signal-like), point-to-point (item/energy-like)
//!
//! ```ignore
//! use yog_exports::yog_pipes::{PipePos, PipePayload, broadcast, send, read, bind_handler};
//!
//! let pos = |x, y, z| PipePos { dim: "minecraft:overworld".into(), x, y, z };
//!
//! // Every node reachable from `pos` sees this payload (redstone-like fan-out).
//! broadcast(pos(0, 64, 0), PipePayload {
//!     data: vec![15],
//!     metadata: vec![("kind".into(), "redstone".into())],
//! }).unwrap();
//!
//! // Point-to-point, e.g. moving an item stack from one endpoint to another.
//! send(pos(0, 64, 0), pos(4, 64, 0), PipePayload { data: item_bytes, metadata: vec![] }).unwrap();
//!
//! // Poll, or bind a handler (an ordinary `#[yog_export] fn(PipePayload)`
//! // in your own mod) to be called whenever a payload arrives instead.
//! let payload = read(pos(4, 64, 0)).unwrap();
//! bind_handler(pos(4, 64, 0), "my-mod".into(), "on_pipe_payload".into()).unwrap();
//! ```

mod graph;
mod handler;
mod kind;
mod model;
mod payload;
mod persist;
mod pipe;
mod recipe;
mod transport;

use yog_api::{info, Mod, Registry};

// Re-export all public types
pub use kind::PipeKind;
pub use model::{ElementRotation, FaceDef, ModelDef, ModelElement};
pub use payload::PipePayload;
pub use pipe::{register_pipe, PipeDef};
pub use recipe::RecipeDef;
pub use transport::{bind_handler, broadcast, read, refresh, send, unbind_handler, PipePos};

// ── Mod entry point ──────────────────────────────────────────────────────────

pub struct YogPipesMod;

impl Mod for YogPipesMod {
    fn register(registry: &mut Registry) {
        info!("[yog-pipes] pipe transport framework ready.");

        // `register_pipe`/`broadcast`/`send`/etc. are auto-registered for
        // interop consumers via their own `#[yog_export]` ctors — no manual
        // `registry.interop().export()` call needed here.

        // Keep the graph in step with the world. `pipe::is_pipe_block` is the
        // only thing that decides whether a block id is a pipe — never a
        // naming convention. Changes made any other way (`/setblock`, a
        // mod's own `set_block`) are picked up lazily or via `refresh`.
        registry.on_player_place_block(|e, phase, srv| {
            if phase == yog_api::EventPhase::Post && pipe::is_pipe_block(&e.block_id) {
                let node = (
                    graph::normalize_dim(&e.dimension),
                    e.pos.x,
                    e.pos.y,
                    e.pos.z,
                );
                graph::on_placed(node, &graph::world_probe(srv));
            }
            true
        });

        registry.on_block_break(|e, phase, _srv| {
            if phase == yog_api::EventPhase::Post && pipe::is_pipe_block(&e.block_id) {
                graph::on_removed(&(
                    graph::normalize_dim(&e.dimension),
                    e.pos.x,
                    e.pos.y,
                    e.pos.z,
                ));
            }
            true
        });

        // An explosion can take out any number of pipes at once without a
        // per-block event: re-check the known nodes in its reach. A vanilla
        // blast ray starts at up to 1.3 × power and loses 0.225 per 0.3-block
        // step, so it travels at most ~1.75 × power blocks; 2 × power + 2
        // leaves margin for rounding.
        registry.on_explosion(|e, phase, srv| {
            if phase == yog_api::EventPhase::Post {
                let radius = (e.power.max(0.0) * 2.0).ceil() as i32 + 2;
                let center = (e.x.floor() as i32, e.y.floor() as i32, e.z.floor() as i32);
                graph::revalidate_area(
                    &graph::normalize_dim(&e.dimension),
                    center,
                    radius,
                    &graph::world_probe(srv),
                );
            }
            true
        });

        // Stored payloads and handler bindings are saved with the world
        // (see `persist`); the topology is rediscovered from the world.
        registry.on_server_started(persist::load);

        // Positions mean nothing once the world is gone — save, then don't
        // carry one world's networks into the next one opened in the same
        // session.
        registry.on_server_stopping(|_srv| {
            persist::unload();
            graph::clear();
            handler::clear();
        });

        // Transport is event-driven (call `broadcast`/`send` when a value
        // actually changes), not tick-polled — the tick hook below only
        // autosaves; how/when payloads move is each producing mod's call.
        registry.on_tick(|_srv| persist::tick());

        info!("[yog-pipes] ready.");
    }
}

yog_api::export_mod!(YogPipesMod);
