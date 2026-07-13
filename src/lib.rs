//! Yog Pipes — universal transport framework for Yog mods.
//!
//! This is a **framework**, not a mod — it's a set of pipe building blocks
//! plus its own small runtime (a virtual graph + tick loop) for mods to
//! build transport/signal networks on, without dictating recipes, tiers, or
//! models.
//!
//! ## Philosophy
//!
//! - **No fixed fields** — `PipeDef` carries an open `properties` map instead
//!   of a rigid `PipeTier` struct. Mods define whatever parameters they need.
//! - **Recipes as data** — `PipeDef::recipe` carries an arbitrary shaped
//!   recipe (any pattern, any ingredients, any output count). Leave it
//!   `None` to register no recipe, or add one separately.
//! - **3D models** — `ModelDef` lets mods describe block models with
//!   per-face textures, elements, and rotation — no JSON files needed.
//! - **Link groups** — control which blocks pipes connect to.
//! - **Extensible pipe kinds** — `PipeKind::Custom("mymod:mana")` for custom types.
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
//!     recipe: None,
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
//! ### Signal transport (push/pull, tick-driven from any mod)
//!
//! ```ignore
//! use yog_exports::yog_pipes::{PipePos, push_signal, pull_signal};
//!
//! push_signal(PipePos { dim: "minecraft:overworld".into(), x, y, z }, 15).unwrap();
//! let level = pull_signal(PipePos { dim: "minecraft:overworld".into(), x: ox, y: oy, z: oz }).unwrap();
//! ```

mod graph;
mod kind;
mod model;
mod pipe;
mod recipe;
mod signal;
mod transfer;

use yog_api::{Mod, Registry};

// Re-export all public types
pub use kind::PipeKind;
pub use model::{ElementRotation, FaceDef, ModelDef, ModelElement};
pub use pipe::{register_pipe, PipeDef};
pub use recipe::RecipeDef;
pub use signal::{pull_signal, push_signal, PipePos};

// ── Mod entry point ──────────────────────────────────────────────────────────

pub struct YogPipesMod;

impl Mod for YogPipesMod {
    fn register(registry: &mut Registry) {
        yog_api::info!("[yog-pipes] pipe transport framework ready.");

        // `register_pipe`/`push_signal`/`pull_signal` are auto-registered for
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

        registry.on_tick(|srv| transfer::transfer_tick(srv));
    }
}

yog_api::export_mod!(YogPipesMod);
