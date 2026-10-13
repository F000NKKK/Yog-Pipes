//! Which blocks are pipes.
//!
//! Yog-Pipes defines no blocks, items or recipes. A mod declares its pipe
//! blocks like any other content — a `content/**/*.yblock` with the model,
//! `states.connect` arms, `block.connect_groups` and a name/tooltip from
//! its `.yres` — and then marks them as pipes with [`register_pipe`], so the
//! graph (`crate::graph`) knows which world positions are nodes and which
//! neighbors they link to. Nothing here guesses from a block id's spelling.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::yog_export;

use crate::graph::lock;
use crate::kind::PipeKind;

/// Marks one block as a pipe.
#[derive(Debug, Clone)]
#[yog_export]
pub struct PipeDef {
    /// Block id (e.g. `"mymod:pipe_iron"`) of a block the mod registers
    /// itself (normally from a `.yblock`).
    pub block_id: String,
    /// What this pipe carries. Descriptive only, never used for routing.
    pub kind: PipeKind,
    /// Two adjacent pipes are linked when their groups share an entry —
    /// pass the block's own `connect_groups` (`Blocks::x().connect_groups`)
    /// so the graph links exactly what the player sees connected.
    pub link_groups: Vec<String>,
    /// Open parameter map (`"speed"`, `"capacity"`, anything). Yog-Pipes
    /// never reads it; other mods get it back through [`pipe_def`].
    pub properties: HashMap<String, f64>,
}

/// Every block marked via [`register_pipe`], by id.
static PIPES: LazyLock<Mutex<HashMap<String, PipeDef>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether `block_id` was marked as a pipe via [`register_pipe`].
pub fn is_pipe_block(block_id: &str) -> bool {
    lock(&PIPES).contains_key(block_id)
}

/// The link groups of `block_id` if it's a pipe, else `None`.
pub fn pipe_groups(block_id: &str) -> Option<Vec<String>> {
    lock(&PIPES).get(block_id).map(|d| d.link_groups.clone())
}

/// Mark `def.block_id` as a pipe. Registering the same id again replaces
/// its definition.
#[yog_export]
pub fn register_pipe(def: PipeDef) -> Result<(), String> {
    if def.link_groups.is_empty() {
        return Err(format!(
            "{}: a pipe needs at least one link group (the block's connect_groups)",
            def.block_id
        ));
    }
    lock(&PIPES).insert(def.block_id.clone(), def);
    Ok(())
}

/// The definition `block_id` was registered with, if it's a pipe.
#[yog_export]
pub fn pipe_def(block_id: String) -> Result<Option<PipeDef>, String> {
    Ok(lock(&PIPES).get(&block_id).cloned())
}
