//! The payload a pipe carries — Yog-Pipes never interprets it.
//!
//! Whether it's an item stack, an energy amount, a fluid volume, a redstone
//! signal level, or something a mod invented — that logic belongs to the
//! mod, not to this framework. Yog-Pipes only connects blocks, routes
//! payloads through the network, and dispatches them to whatever handler a
//! mod bound at the destination.

use yog_api::yog_export;

/// Arbitrary data carried through the pipe network.
#[derive(Debug, Clone, Default, PartialEq)]
#[yog_export]
pub struct PipePayload {
    /// Opaque binary payload — e.g. an rkyv-encoded item stack, an energy
    /// amount, a signal level, or anything else a mod defines. Yog-Pipes
    /// never reads or interprets these bytes.
    pub data: Vec<u8>,
    /// Arbitrary key-value metadata describing the payload (e.g.
    /// `("kind", "item")`, `("count", "64")`). Yog-Pipes never inspects
    /// these keys — mods define their own vocabulary.
    pub metadata: Vec<(String, String)>,
}
