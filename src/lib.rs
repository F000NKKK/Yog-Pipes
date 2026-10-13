//! Yog Pipes — universal transport framework for Yog mods.
//!
//! A framework, not a content mod: it adds no blocks, items or recipes of
//! its own. It keeps a virtual graph of pipe networks in step with the
//! world, routes payloads through it and dispatches them to handlers —
//! without dictating what a pipe looks like or what it carries.
//!
//! - **Pipes are ordinary content.** A mod declares its pipe blocks in
//!   `content/**/*.yblock` (model, `states.connect` arms,
//!   `block.connect_groups`, name/tooltip from `.yres`) and marks them with
//!   [`register_pipe`]. Blocks are recognized as pipes only that way, never
//!   by guessing from an id's spelling.
//! - **Link groups** — two adjacent pipes link when their groups share an
//!   entry, the same rule the loader uses to grow the visual arms.
//! - **Payload-agnostic transport** — what a [`PipePayload`] means is up to
//!   the mods producing and consuming it; Yog-Pipes only connects, routes
//!   and dispatches.
//! - **Branching is normal** — a node can have 0–6 connected neighbors,
//!   loops included.
//!
//! ## Usage (from another mod, `[dependencies.exports] yog-pipes = "0.4"`)
//!
//! ```ignore
//! use crate::content::Blocks;
//! use yog_pipes::{register_pipe, PipeDef, PipeKind};
//!
//! let block = Blocks::copper_pipe(); // content/blocks/copper_pipe.yblock
//! register_pipe(PipeDef {
//!     block_id: block.id,
//!     kind: PipeKind::Item,
//!     link_groups: block.connect_groups,
//!     properties: [("speed".into(), 2.0)].into(),
//! }).unwrap();
//! ```
//!
//! Transport: [`broadcast`] (every connected node), [`send`] (shortest
//! path to one node), [`read`] (poll), [`bind_handler`] (call a
//! `#[yog_export] fn(PipePayload)` of the bound mod on each delivery).
//! See `examples/copper-pipes/` for a complete consumer mod.

mod graph;
mod handler;
mod kind;
mod payload;
mod persist;
mod pipe;
mod transport;

use yog_api::{info, Mod, Registry};

pub use kind::PipeKind;
pub use payload::PipePayload;
pub use pipe::{pipe_def, register_pipe, PipeDef};
pub use transport::{bind_handler, broadcast, read, refresh, send, unbind_handler, PipePos};

// ── Mod entry point ──────────────────────────────────────────────────────────

pub struct YogPipesMod;

impl Mod for YogPipesMod {
    fn register(registry: &mut Registry) {
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
