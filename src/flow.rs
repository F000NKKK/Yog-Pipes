//! `Flow`, Yog-Pipes' mirror (`docs/design/bus.md`, "Mirrors"): every pipe
//! of a capability in a dimension, read in place by other mods (a gauge, a
//! HUD) instead of asking `pipe_info` / `indication` each tick. Its key
//! space is a family, a member a capability in a dimension
//! (`<SPACE>/<capability>/<dimension>`), a pipe keyed by its packed
//! position:
//!
//! ```ignore
//! let flow = Flow::declare_in("mymod:energy/minecraft:overworld", None::<&()>, Flow::LEVELS, Rate::Every(4))?;
//! let fill = flow.get(BlockPos::new(x, y, z).packed()).map(|p| p.levels()[0]);
//! ```

use yog_api::mirror::{Mirror, MirrorWriter};
use yog_api::{BlockPos, yog_mirror};

use crate::host;
use crate::transport::indicate::MAX_INDICATORS;

/// Flow's key space family: a member a capability in a dimension.
pub const SPACE: &str = "yog-pipes:keys/flow";

/// What separates a member's capability from its dimension.
const MEMBER_SEP: char = '/';

/// A pipe of a capability.
#[yog_mirror(space = SPACE, family)]
pub struct Flow {
    /// Its network (stable while the network is; changes as it merges or splits).
    pub network: u64,
    /// Its indicators' levels as the world shows them, in the capability's order.
    pub levels: [u8; MAX_INDICATORS],
}

/// A pipe's row as the writer fills it.
struct Pipe {
    key: u64,
    network: u64,
    levels: [u8; MAX_INDICATORS],
}

/// The pipes of member `member` (`<capability>/<dimension>`).
fn pipes(member: &str) -> Vec<Pipe> {
    let Some((cap, dim)) = member.rsplit_once(MEMBER_SEP) else {
        return Vec::new();
    };
    host::with(|host| {
        let e = &mut host.engine;
        let Some(c) = e.reg.cap_id(cap) else {
            return Vec::new();
        };
        let d = e.reg.dim(dim);
        let layer = e.layer(c);
        layer
            .nodes()
            .filter(|(_, n)| n.pos.dim == d)
            .map(|(id, n)| {
                let mut levels = [0; MAX_INDICATORS];
                for (l, v) in levels.iter_mut().zip(e.shown(c, id)) {
                    *l = *v;
                }
                Pipe {
                    key: BlockPos::new(n.pos.x, n.pos.y, n.pos.z).packed(),
                    network: crate::ops::network_id(layer.net_id(n.net_index())),
                    levels,
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

/// A turn: the member's pipes keyed and their due fields written.
pub fn write(w: &mut MirrorWriter<'_, Flow>) {
    let Some(member) = w.member().map(str::to_owned) else {
        return;
    };
    let pipes = pipes(&member);
    let keys: Vec<(u64, Option<u64>)> = pipes.iter().map(|p| (p.key, None)).collect();
    w.keep_keys(&keys);
    for p in &pipes {
        if let Some(row) = w.row(p.key) {
            row.network(|| p.network).levels(|| p.levels);
        }
    }
}

/// Provides `Flow`.
pub fn register(registry: &mut yog_api::Registry) {
    if !registry.provide_mirror_of::<Flow>(write) {
        yog_api::warn!("[yog-pipes] {} isn't provided", Flow::address());
    }
}
