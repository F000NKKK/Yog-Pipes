//! Signal transport: push a value into the pipe graph at a source position,
//! pull the propagated value back out at any other position. Pure in-memory
//! graph math — no `Registry`/world access needed, so other mods (via the
//! generated `yog_exports` crate) can drive it every tick without touching
//! the interop boundary's `&mut Registry` machinery at all.

use yog_api::yog_export;

/// A position in the world, as plain data for interop calls.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct PipePos {
    pub dim: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl From<PipePos> for crate::graph::NodeKey {
    fn from(p: PipePos) -> Self {
        (p.dim, p.x, p.y, p.z)
    }
}

/// Push `strength` (0..255) as the signal emitted at `pos`, and propagate it
/// through the pipe graph immediately (attenuating by 1 per hop, like
/// vanilla redstone). Call this whenever the value at a source changes —
/// not every tick — the graph doesn't need re-propagation otherwise.
#[yog_export]
pub fn push_signal(pos: PipePos, strength: u8) -> Result<(), String> {
    crate::graph::propagate_signals(pos.into(), strength);
    Ok(())
}

/// Read the signal level the graph has currently propagated to `pos`.
#[yog_export]
pub fn pull_signal(pos: PipePos) -> Result<u8, String> {
    Ok(crate::graph::signal_at(&pos.into()))
}
