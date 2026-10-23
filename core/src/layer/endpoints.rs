//! Endpoints and their ports, sides and colors, and which chunks are
//! loaded (networks with a node in an unloaded one are frozen).

use super::{Endpoint, Layer, Port};
use crate::ids::{ChunkKey, EpId, EpTypeId, Face, NetId, NodeId, Pos};
use crate::registry::Side;

impl Layer {
    // ── Endpoints and ports ──────────────────────────────────────────────

    pub fn endpoint_at(&self, pos: Pos) -> Option<EpId> {
        self.ep_index.get(&pos).copied()
    }

    /// The endpoint `id` names (a port's endpoint is always live).
    pub fn endpoint(&self, id: EpId) -> &Endpoint {
        &self.eps[id.index()]
    }

    /// Get or create the endpoint instance at `pos`.
    pub fn ensure_endpoint(&mut self, pos: Pos, ty: EpTypeId) -> EpId {
        if let Some(&e) = self.ep_index.get(&pos) {
            self.eps[e.index()].ty = ty;
            return e;
        }
        let ep = Endpoint {
            pos,
            ty,
            ports: 0,
            live: true,
        };
        let id = match self.free_eps.pop() {
            Some(i) => {
                self.eps[i as usize] = ep;
                EpId(i)
            }
            None => {
                self.eps.push(ep);
                EpId(self.eps.len() as u32 - 1)
            }
        };
        self.ep_index.insert(pos, id);
        id
    }

    /// Connect `ep` to `node` through the node's `face` (idempotent).
    pub fn add_port(&mut self, ep: EpId, node: NodeId, face: Face) {
        let Some(id) = self.net_of(node) else {
            return;
        };
        let port = Port { ep, node, face };
        let Some(net) = self.net_mut(id) else {
            return;
        };
        if net.ports.contains(&port) {
            return;
        }
        net.ports.push(port);
        self.eps[ep.index()].ports += 1;
        self.touch(id);
    }

    pub(super) fn drop_port_count(&mut self, ep: EpId) {
        let e = &mut self.eps[ep.index()];
        e.ports = e.ports.saturating_sub(1);
        if e.ports == 0 {
            let pos = e.pos;
            self.free_endpoint(ep);
            self.ep_index.remove(&pos);
        }
    }

    /// Put `ep`'s slot on the free list (once).
    fn free_endpoint(&mut self, ep: EpId) {
        let e = &mut self.eps[ep.index()];
        if e.live {
            e.live = false;
            self.free_eps.push(ep.0);
        }
    }

    /// Disconnect and forget the endpoint at `pos`. Returns the networks
    /// it was connected to.
    pub fn remove_endpoint(&mut self, pos: Pos) -> Vec<NetId> {
        let Some(ep) = self.ep_index.get(&pos).copied() else {
            return Vec::new();
        };
        let mut nets = Vec::new();
        for f in Face::ALL {
            let Some(node) = self.node_at(pos.offset(f)) else {
                continue;
            };
            let Some(id) = self.net_of(node) else {
                continue;
            };
            let Some(net) = self.net_mut(id) else {
                continue;
            };
            let before = net.ports.len();
            net.ports.retain(|p| !(p.ep == ep && p.node == node));
            if net.ports.len() != before && !nets.contains(&id) {
                nets.push(id);
            }
        }
        self.free_endpoint(ep);
        self.ep_index.remove(&pos);
        for &n in &nets {
            self.touch(n);
        }
        nets
    }

    // ── Side configuration and color ────────────────────────────────────

    pub fn set_side_raw(&mut self, node: NodeId, face: Face, side: Side) {
        if side == Side::default() {
            self.sides.remove(&(node, face));
        } else {
            self.sides.insert((node, face), side);
        }
        if let Some(id) = self.net_of(node) {
            self.touch(id);
        }
    }

    pub fn set_color_raw(&mut self, node: NodeId, color: u8) {
        self.nodes[node.index()].color = color;
    }

    /// Every side configured away from the default (for saving).
    pub fn configured_sides(&self) -> impl Iterator<Item = (Pos, Face, Side)> + '_ {
        self.sides
            .iter()
            .map(|(&(n, f), &s)| (self.node(n).pos, f, s))
    }

    // ── Chunk loading ────────────────────────────────────────────────────

    /// Track a chunk's load state; networks with a node in an unloaded
    /// chunk are frozen (not ticked). O(nodes in the chunk).
    pub fn set_chunk_loaded(&mut self, chunk: ChunkKey, loaded: bool) {
        let changed = if loaded {
            self.unloaded.remove(&chunk)
        } else {
            self.unloaded.insert(chunk)
        };
        if !changed {
            return;
        }
        let nodes = self.chunks.get(&chunk).cloned().unwrap_or_default();
        for n in nodes {
            let Some(id) = self.net_of(n) else {
                continue;
            };
            let Some(net) = self.net_mut(id) else {
                continue;
            };
            if loaded {
                net.frozen -= 1;
            } else {
                net.frozen += 1;
            }
            self.activate(id, 0);
        }
    }
}
