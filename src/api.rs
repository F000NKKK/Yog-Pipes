//! The exported functions — what other mods call through
//! `yog_pipes_exports` (`[dependencies.exports] yog-pipes = "0.5"`). All
//! take and return the types of `crate::types`; each is a one-line call
//! into `crate::ops`.
//!
//! Called from inside a pipe handler (an endpoint, event or external
//! handler Yog-Pipes is running), mutations are queued until the current
//! engine step ends and queries fail (see `crate::host`).

use yog_api::yog_export;

use crate::types::{
    AttachDef, CapabilityDef, EndpointDef, KindDef, NetworkInfo, PipeFace, PipeInfo, PipePayload,
    PipePos, SideDef,
};

/// Register (or replace) a resource kind.
#[yog_export]
pub fn register_kind(def: KindDef) -> Result<(), String> {
    crate::ops::register_kind(def)
}

/// Register (or replace) a capability. Re-registering keeps the networks
/// and what they hold unless the mechanic changes.
#[yog_export]
pub fn register_capability(def: CapabilityDef) -> Result<(), String> {
    crate::ops::register_capability(def)
}

/// Make a block a pipe of a capability. A block may carry several
/// capabilities (one call each) unless one of them is exclusive.
#[yog_export]
pub fn attach(def: AttachDef) -> Result<(), String> {
    crate::ops::attach(def)
}

/// Register (or replace) an endpoint: a block type, or one position (saved
/// with the world).
#[yog_export]
pub fn register_endpoint(def: EndpointDef) -> Result<(), String> {
    crate::ops::register_endpoint(def)
}

/// Forget a positional endpoint and disconnect it.
#[yog_export]
pub fn unregister_endpoint(capability: String, pos: PipePos) -> Result<(), String> {
    crate::ops::unregister_endpoint(capability, pos)
}

/// Configure one face of the pipe at `pos` (saved with the world).
#[yog_export]
pub fn set_side(
    capability: String,
    pos: PipePos,
    face: PipeFace,
    side: SideDef,
) -> Result<(), String> {
    crate::ops::set_side(capability, pos, face, side)
}

/// Colour the pipe at `pos` (0 = none): coloured pipes link only to the
/// same colour or to uncoloured ones.
#[yog_export]
pub fn set_colour(capability: String, pos: PipePos, colour: u8) -> Result<(), String> {
    crate::ops::set_colour(capability, pos, colour)
}

/// Re-read the block at `pos` from the world. Placing and breaking by
/// players and explosions are tracked automatically; call this after
/// changing blocks any other way (`set_block`, structures, `/setblock`).
#[yog_export]
pub fn refresh(pos: PipePos) -> Result<(), String> {
    crate::ops::refresh(pos)
}

/// The block at `pos` (an endpoint or a pipe) has something to give or
/// room to take: tick its networks now instead of at the next poll.
#[yog_export]
pub fn wake(pos: PipePos) -> Result<(), String> {
    crate::ops::wake(pos)
}

/// Set the level an emitter at `pos` (a pipe or an endpoint) drives on
/// `channel` of a signal capability; 0 stops emitting.
#[yog_export]
pub fn emit(capability: String, pos: PipePos, channel: u8, level: u8) -> Result<(), String> {
    crate::ops::emit(capability, pos, channel, level)
}

/// The signal level on `channel` at `pos`.
#[yog_export]
pub fn level(capability: String, pos: PipePos, channel: u8) -> Result<u8, String> {
    crate::ops::level(capability, pos, channel)
}

/// Deliver `payload` from `from` to `to` if they share a network: to
/// `to`'s endpoint handler if it listens to messages, else its inbox
/// (`receive`). `false` when not connected or the inbox is full. From
/// inside a handler the post is queued and `true` means "queued".
#[yog_export]
pub fn post(
    capability: String,
    from: PipePos,
    to: PipePos,
    payload: PipePayload,
) -> Result<bool, String> {
    crate::ops::post(capability, from, to, payload)
}

/// Send `payload` to every message-listening endpoint on `from`'s network
/// and keep it as the network's latest value (`latest`). Returns how many
/// endpoints received it (from inside a handler: queued, returns 0).
#[yog_export]
pub fn broadcast(capability: String, from: PipePos, payload: PipePayload) -> Result<u32, String> {
    crate::ops::broadcast(capability, from, payload)
}

/// Take the oldest message waiting at `pos`.
#[yog_export]
pub fn receive(capability: String, pos: PipePos) -> Result<Option<PipePayload>, String> {
    crate::ops::receive(capability, pos)
}

/// The last value broadcast on `pos`'s network.
#[yog_export]
pub fn latest(capability: String, pos: PipePos) -> Result<Option<PipePayload>, String> {
    crate::ops::latest(capability, pos)
}

/// The network of `capability` at `pos` (a pipe, or an endpoint touching
/// one).
#[yog_export]
pub fn network_info(capability: String, pos: PipePos) -> Result<Option<NetworkInfo>, String> {
    crate::ops::network_info(capability, pos)
}

/// Every capability's pipe at `pos`, with its faces.
#[yog_export]
pub fn pipe_info(pos: PipePos) -> Result<Vec<PipeInfo>, String> {
    crate::ops::pipe_info(pos)
}

/// The indicator properties at `pos` — `(<state_prefix>_<name>, value)`
/// for every capability of the pipe there.
#[yog_export]
pub fn indication(pos: PipePos) -> Result<Vec<(String, u8)>, String> {
    crate::ops::indication(pos)
}
