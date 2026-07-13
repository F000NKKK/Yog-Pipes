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
mod pipe;
mod recipe;
mod transport;

use yog_api::{Mod, Registry};

// Re-export all public types
pub use kind::PipeKind;
pub use model::{ElementRotation, FaceDef, ModelDef, ModelElement};
pub use payload::PipePayload;
pub use pipe::{register_pipe, PipeDef};
pub use recipe::RecipeDef;
pub use transport::{bind_handler, broadcast, read, send, unbind_handler, PipePos};

// ── Mod entry point ──────────────────────────────────────────────────────────

pub struct YogPipesMod;

impl Mod for YogPipesMod {
    fn register(registry: &mut Registry) {
        yog_api::info!("[yog-pipes] pipe transport framework ready.");

        // `register_pipe`/`broadcast`/`send`/etc. are auto-registered for
        // interop consumers via their own `#[yog_export]` ctors — no manual
        // `registry.interop().export()` call needed here.

        // Infrastructure: rebuild graph when any pipe block is placed or
        // broken. `pipe::is_pipe_block` is the only thing that decides
        // whether a block id is a pipe — never a naming convention.
        //
        // Dimension is hardcoded to "overworld": `PlaceBlockEvent`/
        // `BlockBreakEvent` in yog-event carry no dimension field at all, so
        // there is currently no way to learn it from these callbacks. Fixing
        // that is an upstream Yog-Mod-Loader change (new event field, ABI
        // bump, Java/Mixin plumbing), not something this framework can work
        // around on its own.
        registry.on_player_place_block(|e, phase, srv| {
            if phase != yog_api::EventPhase::Post {
                return true;
            }
            if !pipe::is_pipe_block(&e.block_id) {
                return true;
            }
            graph::rebuild_graph(srv, "overworld", e.pos.x, e.pos.y, e.pos.z);
            true
        });

        registry.on_block_break(|e, phase, srv| {
            if phase != yog_api::EventPhase::Post {
                return true;
            }
            if !pipe::is_pipe_block(&e.block_id) {
                return true;
            }
            graph::rebuild_graph(srv, "overworld", e.pos.x, e.pos.y, e.pos.z);
            true
        });

        // Transport is event-driven (call `broadcast`/`send` when a value
        // actually changes), not tick-polled — there is deliberately no
        // `on_tick` hook here dictating how/when payloads move; that's each
        // producing mod's call.
    }
}

yog_api::export_mod!(YogPipesMod);
