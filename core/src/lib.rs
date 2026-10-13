//! The transport engine of Yog-Pipes, independent of the game: abstract
//! pipes carrying any number of capabilities, each with its own topology
//! layer and transport model. See `docs/design/transport.md`.
//!
//! - [`ids`] — integer ids, positions, faces, hashing.
//! - [`registry`] — resource kinds, resources, filters, capabilities,
//!   block and endpoint types.
//! - [`layer`] — one capability's topology: nodes, networks, ports.
//! - [`model`] — the model trait and the built-in mechanics.
//! - [`policy`], [`indicate`], [`persist`] — shared pieces.
//! - [`engine`] — everything wired together; what a host drives.

pub mod engine;
pub mod ids;
pub mod indicate;
pub mod layer;
pub mod model;
pub mod persist;
pub mod policy;
pub mod registry;
