//! The world as the engine sees it: blocks appearing and going, discovery
//! of what they connect, side and color configuration, and the scheduler.

use std::collections::VecDeque;

use super::{Engine, MAX_DISCOVERY, NodeFace, PRUNE_EVERY, Parked, Probe};
use crate::ids::{
    BlockTypeId, CapId, ChunkKey, DimId, Face, FxHashMap, FxHashSet, NO_NODE, NetId, NodeId, Pos,
};
use crate::layer::{Settled, Split};
use crate::model::{Activity, Cx, Io, Model};
use crate::registry::{Side, SideMode, groups_meet};

/// What one discovery probed: each position's block, and the positions
/// that may be endpoints.
#[derive(Default)]
struct Found {
    probed: FxHashMap<Pos, Option<BlockTypeId>>,
    endpoints: Vec<Pos>,
}

impl Engine {
    // ── World events ─────────────────────────────────────────────────────

    /// A block appeared (or changed) at `pos`: discover what it connects.
    pub fn place(&mut self, io: &mut dyn Io, probe: &mut dyn Probe, pos: Pos) {
        self.discover(io, probe, pos);
    }

    /// The block at `pos` is gone.
    pub fn remove(&mut self, io: &mut dyn Io, pos: Pos) {
        for c in 0..self.slots.len() {
            let (model, mut cx) = cx!(self, c, io);
            model.forget(&mut cx, pos);
            if let Some(node) = cx.layer.node_at(pos) {
                if let Some(net) = cx.layer.net_of(node) {
                    model.node_removed(&mut cx, node, net);
                }
                let split = cx.layer.remove_node(node);
                cx.ind.forget(node);
                Self::after_split(model, &mut cx, split);
            }
            if cx.layer.endpoint_at(pos).is_some() {
                for n in cx.layer.remove_endpoint(pos) {
                    cx.ind.net_changed(n);
                }
            }
        }
    }

    /// Re-read the block at `pos` (changed by `/setblock` or a mod).
    pub fn refresh(&mut self, io: &mut dyn Io, probe: &mut dyn Probe, pos: Pos) {
        let now = probe.block(&self.reg, pos);
        let known = self
            .slots
            .iter()
            .any(|s| s.layer.node_at(pos).is_some() || s.layer.endpoint_at(pos).is_some());
        if known {
            self.remove(io, pos);
        }
        if now.is_some() {
            self.discover(io, probe, pos);
        }
    }

    /// Re-check every known node and endpoint within `radius` of `center`
    /// (explosions remove blocks without per-block events).
    pub fn revalidate_area(
        &mut self,
        io: &mut dyn Io,
        probe: &mut dyn Probe,
        center: Pos,
        radius: i32,
    ) {
        let mut gone = Vec::new();
        for s in &self.slots {
            for (_, n) in s.layer.nodes() {
                let p = n.pos;
                if p.dim == center.dim
                    && (p.x - center.x).abs() <= radius
                    && (p.y - center.y).abs() <= radius
                    && (p.z - center.z).abs() <= radius
                {
                    gone.push(p);
                }
            }
        }
        gone.sort();
        gone.dedup();
        for p in gone {
            let bt = probe.block(&self.reg, p);
            let still = bt.is_some_and(|b| {
                self.slots.iter().enumerate().all(|(c, s)| {
                    s.layer.node_at(p).is_none()
                        || self.reg.blocks[b.index()]
                            .attaches
                            .iter()
                            .any(|a| a.0.index() == c)
                })
            });
            if !still {
                self.remove(io, p);
                if bt.is_some() {
                    self.discover(io, probe, p);
                }
            }
        }
    }

    pub fn set_chunk_loaded(&mut self, dim: DimId, cx_: i32, cz: i32, loaded: bool) {
        let key = ChunkKey { dim, x: cx_, z: cz };
        for s in &mut self.slots {
            s.layer.set_chunk_loaded(key, loaded);
        }
    }

    fn after_split(model: &mut Box<dyn Model>, cx: &mut Cx, split: Split) {
        let Some(net) = split.net else {
            return;
        };
        if split.emptied {
            model.net_removed(cx, net);
            return;
        }
        if !split.parts.is_empty() {
            model.split(cx, net, &split.parts);
            cx.ind.net_changed(net);
            for &(p, _) in &split.parts {
                cx.ind.net_changed(p);
            }
        }
    }

    fn after_settle(
        model: &mut Box<dyn Model>,
        cx: &mut Cx,
        s: &Settled,
        nodes: &[NodeId],
        parked: Option<&mut Parked>,
    ) {
        let Some(net) = s.net else {
            return;
        };
        for &(from, _) in &s.absorbed {
            model.merged(cx, net, from);
        }
        if !s.absorbed.is_empty() {
            cx.ind.net_changed(net);
        }
        for &n in nodes {
            model.node_added(cx, n, net);
        }
        if let Some(p) = parked
            && !p.records.is_empty()
        {
            for &n in nodes {
                Self::restore_records(model, cx, n, p);
            }
        }
    }

    /// Hand node `n` its saved records; those it can't take yet stay parked.
    fn restore_records(model: &mut Box<dyn Model>, cx: &mut Cx, n: NodeId, p: &mut Parked) {
        let pos = cx.layer.node(n).pos;
        let Some(list) = p.records.remove(&pos) else {
            return;
        };
        let mut left = Vec::new();
        for bytes in list {
            // The network may have changed id through merges caused by
            // earlier restores: look it up again.
            let Some(net) = cx.layer.net_of(n) else {
                left.push(bytes);
                continue;
            };
            if !model.restore(cx, n, net, &bytes) {
                left.push(bytes);
            }
        }
        if !left.is_empty() {
            p.records.insert(pos, left);
        }
    }

    /// BFS over unknown positions from `start`, adding nodes to every
    /// layer their block belongs to, then linking, settling networks and
    /// connecting endpoints.
    pub(super) fn discover(&mut self, io: &mut dyn Io, probe: &mut dyn Probe, start: Pos) {
        let mut region = std::mem::take(&mut self.region);
        region.resize_with(self.slots.len(), Vec::new);
        for r in &mut region {
            r.clear();
        }
        let mut found = self.probe_region(probe, start, &mut region);
        for (c, nodes) in region.iter().enumerate() {
            if !nodes.is_empty() {
                self.link_new(c, nodes);
                self.settle_new(io, c, nodes);
            }
        }
        self.connect_found(probe, &mut found, &region);
        self.region = region;
    }

    /// The BFS of [`Engine::discover`]: new nodes into `region` by
    /// capability, the probed blocks and possible endpoints returned.
    fn probe_region(
        &mut self,
        probe: &mut dyn Probe,
        start: Pos,
        region: &mut [Vec<NodeId>],
    ) -> Found {
        let mut found = Found::default();
        let mut queue: VecDeque<Pos> = VecDeque::from([start]);
        found.probed.insert(start, probe.block(&self.reg, start));
        let mut grown = 0usize;
        while let Some(pos) = queue.pop_front() {
            let bt = found.probed.get(&pos).copied().flatten();
            let grew = bt.is_some_and(|b| self.add_nodes(pos, b, region));
            grown += grew as usize;
            if bt.is_some() || self.reg.positional.contains_key(&pos) {
                found.endpoints.push(pos);
            }
            if !grew {
                continue;
            }
            for f in Face::ALL {
                let np = pos.offset(f);
                if found.probed.contains_key(&np) {
                    continue;
                }
                if grown >= MAX_DISCOVERY {
                    break;
                }
                let nb = probe.block(&self.reg, np);
                found.probed.insert(np, nb);
                // Expand into blocks with a capability not yet known there;
                // endpoints are noted without expanding.
                if nb.is_some_and(|b| self.unknown_here(b, np)) {
                    queue.push_back(np);
                } else if nb.is_some() || self.reg.positional.contains_key(&np) {
                    found.endpoints.push(np);
                }
            }
        }
        found
    }

    /// Whether block `b` at `pos` has a capability with no node there yet.
    fn unknown_here(&self, b: BlockTypeId, pos: Pos) -> bool {
        self.reg.blocks[b.index()]
            .attaches
            .iter()
            .any(|&(c, _)| self.slots[c.index()].layer.node_at(pos).is_none())
    }

    /// A node at `pos` in every layer block `b` belongs to and has none
    /// there yet, with its parked side and color. Whether any was added.
    fn add_nodes(&mut self, pos: Pos, b: BlockTypeId, region: &mut [Vec<NodeId>]) -> bool {
        let mut grew = false;
        for k in 0..self.reg.blocks[b.index()].attaches.len() {
            let (cap, att) = self.reg.blocks[b.index()].attaches[k];
            let c = cap.index();
            let layer = &mut self.slots[c].layer;
            if layer.node_at(pos).is_some() {
                continue;
            }
            let id = layer.insert_node(pos, att);
            if let Some(p) = self.parked.get_mut(&self.reg.caps[c].name) {
                for (f, s) in p.sides.remove(&pos).unwrap_or_default() {
                    layer.set_side_raw(id, f, s);
                }
                if let Some(col) = p.colors.remove(&pos) {
                    layer.set_color_raw(id, col);
                }
            }
            region[c].push(id);
            grew = true;
        }
        grew
    }

    /// Link capability `c`'s new nodes to every linkable neighbour.
    fn link_new(&mut self, c: usize, nodes: &[NodeId]) {
        let layer = &mut self.slots[c].layer;
        for &id in nodes {
            let pos = layer.node(id).pos;
            for f in Face::ALL {
                if let Some(nb) = layer.node_at(pos.offset(f))
                    && layer.linkable(id, f, nb)
                {
                    layer.link_raw(id, f, nb);
                }
            }
        }
    }

    /// Settle each connected piece of capability `c`'s new nodes.
    fn settle_new(&mut self, io: &mut dyn Io, c: usize, nodes: &[NodeId]) {
        let name = self.reg.caps[c].name.clone();
        let mut parked = self.parked.remove(&name);
        let mut comp = Vec::new();
        for &id in nodes {
            if self.slots[c].layer.net_of(id).is_some() {
                continue;
            }
            self.unsettled_piece(c, id, &mut comp);
            let (model, mut cx) = cx!(self, c, io);
            let settled = cx.layer.settle(&comp);
            Self::after_settle(model, &mut cx, &settled, &comp, parked.as_mut());
        }
        if let Some(p) = parked {
            self.parked.insert(name, p);
        }
    }

    /// The unsettled nodes linked to `id` (itself included) into `comp`.
    fn unsettled_piece(&self, c: usize, id: NodeId, comp: &mut Vec<NodeId>) {
        let layer = &self.slots[c].layer;
        comp.clear();
        comp.push(id);
        let mut seen: FxHashSet<NodeId> = FxHashSet::from_iter([id]);
        let mut i = 0;
        while i < comp.len() {
            let cur = comp[i];
            i += 1;
            for l in layer.node(cur).links {
                if l != NO_NODE && layer.net_of(l).is_none() && seen.insert(l) {
                    comp.push(l);
                }
            }
        }
    }

    /// Connect the endpoints discovery found, and those next to new nodes
    /// it didn't probe.
    fn connect_found(&mut self, probe: &mut dyn Probe, found: &mut Found, region: &[Vec<NodeId>]) {
        found.endpoints.sort();
        found.endpoints.dedup();
        for &ep in &found.endpoints {
            let bt = found.probed.get(&ep).copied().flatten();
            self.connect_endpoint(ep, bt);
        }
        for (c, nodes) in region.iter().enumerate() {
            for &id in nodes {
                let pos = self.slots[c].layer.node(id).pos;
                for f in Face::ALL {
                    let np = pos.offset(f);
                    if found.probed.contains_key(&np) {
                        continue;
                    }
                    let bt = probe.block(&self.reg, np);
                    found.probed.insert(np, bt);
                    self.connect_endpoint(np, bt);
                }
            }
        }
    }

    /// Connect the block at `pos` (type `bt`) as an endpoint of every
    /// capability it is one for, to every adjacent node that takes it.
    fn connect_endpoint(&mut self, pos: Pos, bt: Option<BlockTypeId>) {
        for c in 0..self.slots.len() {
            let cap = CapId(c as u16);
            let Some(ty) = self.reg.endpoint_type(cap, pos, bt) else {
                continue;
            };
            let ept = &self.reg.endpoints[ty.index()];
            let layer = &mut self.slots[c].layer;
            if layer.node_at(pos).is_some() {
                // A pipe of this capability is not its own endpoint.
                continue;
            }
            for f in Face::ALL {
                let Some(node) = layer.node_at(pos.offset(f)) else {
                    continue;
                };
                let face = f.opposite();
                if layer.side(node, face).mode == SideMode::Off
                    || !groups_meet(
                        &layer.atts[layer.node(node).att as usize].groups,
                        &ept.groups,
                    )
                {
                    continue;
                }
                let ep = layer.ensure_endpoint(pos, ty);
                layer.add_port(ep, node, face);
            }
        }
    }

    // ── Configuration ────────────────────────────────────────────────────

    /// Configure the face `at` of a capability's node.
    pub fn set_side(&mut self, io: &mut dyn Io, probe: &mut dyn Probe, at: NodeFace, side: Side) {
        let c = at.cap.index();
        let Some(node) = self.slots[c].layer.node_at(at.pos) else {
            return;
        };
        let old = self.slots[c].layer.side(node, at.face);
        self.slots[c].layer.set_side_raw(node, at.face, side);
        if (old.mode == SideMode::Off) != (side.mode == SideMode::Off) {
            self.relink(io, probe, c, node, at.face);
        }
    }

    /// Set the color of the `cap` node at `pos` (0 = uncolored) and
    /// re-evaluate its links.
    pub fn set_color(
        &mut self,
        io: &mut dyn Io,
        probe: &mut dyn Probe,
        cap: CapId,
        pos: Pos,
        color: u8,
    ) {
        let c = cap.index();
        let Some(node) = self.slots[c].layer.node_at(pos) else {
            return;
        };
        self.slots[c].layer.set_color_raw(node, color);
        for f in Face::ALL {
            self.relink(io, probe, c, node, f);
        }
    }

    /// Make the link (or port) on `node`'s `face` match the rules again.
    fn relink(
        &mut self,
        io: &mut dyn Io,
        probe: &mut dyn Probe,
        c: usize,
        node: NodeId,
        face: Face,
    ) {
        let pos = self.slots[c].layer.node(node).pos;
        let np = pos.offset(face);
        let linked = self.slots[c].layer.node(node).links[face as usize];
        match self.slots[c].layer.node_at(np) {
            Some(nb) => {
                let ok = self.slots[c].layer.linkable(node, face, nb);
                if linked != NO_NODE && !ok {
                    let (model, mut cx) = cx!(self, c, io);
                    let split = cx.layer.unlink(node, face);
                    Self::after_split(model, &mut cx, split);
                } else if linked == NO_NODE && ok {
                    let (model, mut cx) = cx!(self, c, io);
                    if let Some((into, from, _)) = cx.layer.link_settled(node, face, nb) {
                        model.merged(&mut cx, into, from);
                        cx.ind.net_changed(into);
                    }
                }
            }
            None => {
                let bt = probe.block(&self.reg, np);
                self.connect_endpoint(np, bt);
            }
        }
    }

    // ── Scheduling ───────────────────────────────────────────────────────

    /// A machine at (or a pipe at) `pos` has something to give or room to
    /// take: tick its networks now.
    pub fn wake(&mut self, pos: Pos) {
        for c in 0..self.slots.len() {
            self.wake_cap(c, pos);
        }
    }

    /// Wake capability `c`'s networks at and around `pos`.
    pub(super) fn wake_cap(&mut self, c: usize, pos: Pos) {
        let tick = self.tick;
        let l = &mut self.slots[c].layer;
        let mut nets: Vec<NetId> = Vec::new();
        if let Some(n) = l.node_at(pos) {
            nets.extend(l.net_of(n));
        }
        for f in Face::ALL {
            if let Some(n) = l.node_at(pos.offset(f)) {
                nets.extend(l.net_of(n));
            }
        }
        for n in nets {
            l.activate(n, tick);
        }
    }

    pub fn tick(&mut self, io: &mut dyn Io) {
        self.tick += 1;
        let tick = self.tick;
        for c in 0..self.slots.len() {
            let mut list = std::mem::take(&mut self.slots[c].layer.active);
            list.sort_unstable();
            list.dedup();
            let mut keep = Vec::with_capacity(list.len());
            for idx in list {
                if self.tick_net(io, c, idx) {
                    keep.push(idx);
                }
            }
            let slot = &mut self.slots[c];
            keep.append(&mut slot.layer.active);
            slot.layer.active = keep;
            if tick.is_multiple_of(self.indication_interval.max(1)) && !slot.ind.is_empty() {
                let reg = &self.reg;
                let cap = CapId(c as u16);
                slot.ind.flush(&mut slot.layer, |pos, i, v| {
                    io.indicate(reg, cap, pos, i, v)
                });
            }
            if tick.is_multiple_of(PRUNE_EVERY) {
                slot.layer.prune_caches();
            }
        }
    }

    /// Tick capability `c`'s network at index `idx` if it's due. Whether it
    /// stays on the active list.
    fn tick_net(&mut self, io: &mut dyn Io, c: usize, idx: u32) -> bool {
        let tick = self.tick;
        let layer = &mut self.slots[c].layer;
        let id = layer.net_id(idx);
        let Some(n) = layer.net_mut(id) else {
            return false;
        };
        if !n.active {
            return false;
        }
        if n.frozen > 0 {
            n.active = false;
            return false;
        }
        if n.wake_at > tick {
            return true;
        }
        let (model, mut cx) = cx!(self, c, io);
        let act = model.tick(&mut cx, id);
        let Some(n) = cx.layer.net_mut(id) else {
            return false;
        };
        match act {
            Activity::Active => n.wake_at = tick + 1,
            Activity::Sleep(t) => n.wake_at = tick + t.max(1) as u64,
            Activity::Idle => n.active = false,
        }
        n.active
    }
}
