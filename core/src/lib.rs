//! The transport engine of Yog-Pipes, independent of the game: abstract
//! pipes carrying any number of capabilities, each with its own topology
//! layer and transport model. See `docs/design/transport.md`.

pub mod ids;
pub mod layer;
pub mod registry;
