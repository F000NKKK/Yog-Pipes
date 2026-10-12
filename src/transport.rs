//! The universal transport API: connect pipes, route payloads through the
//! network, and dispatch them to whatever handler a mod bound at the
//! destination. Yog-Pipes doesn't know or care whether a payload is an
//! item, a fluid, an energy amount, or a redstone-like signal — that's
//! entirely up to the mods producing and consuming it.

use yog_api::yog_export;

use crate::graph;
use crate::handler;
use crate::payload::PipePayload;

/// A position in the world, as plain data for interop calls.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct PipePos {
    pub dim: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl From<PipePos> for graph::NodeKey {
    /// Bare dimension ids (`"overworld"`) are namespaced on the way in, so
    /// `PipePos` accepts both spellings.
    fn from(p: PipePos) -> Self {
        (graph::normalize_dim(&p.dim), p.x, p.y, p.z)
    }
}

/// Make sure the network around `node` is in the graph, discovering it from
/// the live world if not — a network built before a restart is unknown
/// until something touches it. Without a server (client side) this only
/// reports whether the node is already known.
fn ensure_known(node: &graph::NodeKey) -> bool {
    match yog_api::server() {
        Some(srv) => graph::ensure_known(node, &graph::world_probe(&srv)),
        None => graph::ensure_known(node, &|_: &graph::NodeKey| None),
    }
}

/// Broadcast `payload` from `pos` to every node reachable through the pipe
/// network (a branch fans out to *all* of its connected neighbors, not
/// just one) and dispatch any handler bound at each reached node. Good for
/// signal-like transport where every listener on the network should see
/// the same value — call again whenever the value at the source changes,
/// rather than every tick.
#[yog_export]
pub fn broadcast(
    registry: &mut yog_api::Registry,
    pos: PipePos,
    payload: PipePayload,
) -> Result<(), String> {
    let node: graph::NodeKey = pos.into();
    ensure_known(&node);
    let reached = graph::deliver_broadcast(node, payload.clone());
    for n in &reached {
        handler::dispatch(registry, n, &payload);
    }
    Ok(())
}

/// Send `payload` from `from` directly to `to` via the shortest path in the
/// pipe network (point-to-point — e.g. item/energy/fluid transfer between
/// two specific endpoints) and dispatch any handler bound at `to`. Returns
/// `false` without delivering anything if no path currently connects them.
#[yog_export]
pub fn send(
    registry: &mut yog_api::Registry,
    from: PipePos,
    to: PipePos,
    payload: PipePayload,
) -> Result<bool, String> {
    let from_node: graph::NodeKey = from.into();
    let to_node: graph::NodeKey = to.into();
    if !ensure_known(&from_node) || !ensure_known(&to_node) {
        return Ok(false);
    }
    if graph::find_path(&from_node, &to_node).is_none() {
        return Ok(false);
    }
    graph::store_payload(&to_node, payload.clone());
    handler::dispatch(registry, &to_node, &payload);
    Ok(true)
}

/// Read the payload last delivered to `pos` (by `broadcast` or `send`), if
/// any — for mods that prefer polling over binding a handler. Stored
/// payloads are saved with the world and survive a restart.
#[yog_export]
pub fn read(pos: PipePos) -> Result<Option<PipePayload>, String> {
    Ok(graph::payload_at(&pos.into()))
}

/// Re-read the network around `pos` from the world. Placing and breaking
/// by players and explosions are tracked automatically; call this after
/// changing pipes any other way (your own `set_block`, a structure
/// template, `/setblock`), with `pos` being any pipe of the affected network
/// or the position a pipe was removed from.
#[yog_export]
pub fn refresh(pos: PipePos) -> Result<(), String> {
    let node: graph::NodeKey = pos.into();
    let srv = yog_api::server().ok_or("no server available")?;
    graph::rebuild_around(&node, &graph::world_probe(&srv));
    Ok(())
}

/// Bind a handler at `pos`: whenever a payload is delivered there, the
/// function `mod_id` exported under `symbol` (an ordinary `#[yog_export]`
/// function taking one `PipePayload`) is called with it. Bindings are saved
/// with the world by name and come back after a restart; the function is
/// looked up again on each delivery, so it's whatever `mod_id` exports
/// under `symbol` at that time. Binding the same position again (e.g. from
/// `on_server_started`) simply replaces the saved binding.
#[yog_export]
pub fn bind_handler(pos: PipePos, mod_id: String, symbol: String) -> Result<(), String> {
    handler::bind(pos.into(), mod_id, symbol);
    Ok(())
}

/// Remove whatever handler is bound at `pos`, if any.
#[yog_export]
pub fn unbind_handler(pos: PipePos) -> Result<(), String> {
    handler::unbind(&pos.into());
    Ok(())
}
