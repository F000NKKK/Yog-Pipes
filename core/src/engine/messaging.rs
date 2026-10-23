//! Signals, messages between endpoints of one network, and what the
//! networks hold (tooltips, debugging, tests).

use super::{Engine, INBOX, NetInfo};
use crate::ids::{CapId, Face, NetId, Pos, ResId};
use crate::model::{Call, Io};

impl Engine {
    // ── Signals ──────────────────────────────────────────────────────────

    pub fn emit(&mut self, io: &mut dyn Io, cap: CapId, pos: Pos, channel: u8, level: u8) {
        let c = cap.index();
        let (model, mut cx) = cx!(self, c, io);
        model.emit(&mut cx, pos, channel, level);
        self.wake_cap(c, pos);
    }

    pub fn level(&self, cap: CapId, pos: Pos, channel: u8) -> u8 {
        let s = &self.slots[cap.index()];
        s.model.level(&s.layer, pos, channel)
    }

    // ── Messaging ────────────────────────────────────────────────────────

    /// The network a position belongs to: its node, else the first
    /// network its endpoint is connected to.
    pub fn net_at(&self, cap: CapId, pos: Pos) -> Option<NetId> {
        let l = &self.slots[cap.index()].layer;
        if let Some(n) = l.node_at(pos) {
            return l.net_of(n);
        }
        let ep = l.endpoint_at(pos)?;
        Face::ALL.into_iter().find_map(|f| {
            let n = l.node_at(pos.offset(f))?;
            let net = l.net_of(n)?;
            l.net(net)?
                .ports
                .iter()
                .any(|p| p.ep == ep && p.node == n)
                .then_some(net)
        })
    }

    /// Deliver `payload` from `from` to `to` if both are on one network:
    /// to `to`'s `on_message` handler, else its inbox. `false` when not
    /// connected or the inbox is full (nothing is overwritten).
    pub fn post(
        &mut self,
        io: &mut dyn Io,
        cap: CapId,
        from: Pos,
        to: Pos,
        payload: &[u8],
    ) -> bool {
        let (Some(a), Some(b)) = (self.net_at(cap, from), self.net_at(cap, to)) else {
            return false;
        };
        if a != b {
            return false;
        }
        let c = cap.index();
        if let Some(call) = self.message_call(c, to, b)
            && io.message(&self.reg, &call, from, payload)
        {
            return true;
        }
        let q = self.slots[c].inbox.entry(to).or_default();
        if q.len() >= INBOX {
            return false;
        }
        q.push_back(payload.to_vec());
        true
    }

    fn message_call(&self, c: usize, to: Pos, net: NetId) -> Option<Call> {
        let l = &self.slots[c].layer;
        let ep = l.endpoint_at(to)?;
        let ty = self.reg.endpoint(l.endpoint(ep).ty);
        if !ty.listens_messages {
            return None;
        }
        let p = l.net(net)?.ports.iter().find(|p| p.ep == ep)?;
        Some(Call {
            cap: CapId(c as u16),
            ty: l.endpoint(ep).ty,
            handler: ty.handler,
            at: to,
            side: p.face.opposite(),
            pipe: l.node(p.node).pos,
        })
    }

    /// Send `payload` to every `on_message` endpoint of `from`'s network
    /// (except `from`) and keep it as the network's latest value. Returns
    /// how many endpoints received it.
    pub fn broadcast(&mut self, io: &mut dyn Io, cap: CapId, from: Pos, payload: &[u8]) -> u32 {
        let Some(net) = self.net_at(cap, from) else {
            return 0;
        };
        let c = cap.index();
        self.slots[c]
            .latest
            .insert(net.index, (net.generation, payload.to_vec()));
        let l = &self.slots[c].layer;
        let mut targets: Vec<Pos> = l
            .net(net)
            .map(|n| n.ports.iter().map(|p| l.endpoint(p.ep).pos).collect())
            .unwrap_or_default();
        targets.sort();
        targets.dedup();
        let mut sent = 0;
        for to in targets {
            if to == from {
                continue;
            }
            if let Some(call) = self.message_call(c, to, net)
                && io.message(&self.reg, &call, from, payload)
            {
                sent += 1;
            }
        }
        sent
    }

    /// Take the oldest message posted to `pos`.
    pub fn receive(&mut self, cap: CapId, pos: Pos) -> Option<Vec<u8>> {
        let inbox = &mut self.slots[cap.index()].inbox;
        let q = inbox.get_mut(&pos)?;
        let m = q.pop_front();
        if q.is_empty() {
            inbox.remove(&pos);
        }
        m
    }

    /// The last broadcast on `pos`'s network.
    pub fn latest(&self, cap: CapId, pos: Pos) -> Option<&[u8]> {
        let net = self.net_at(cap, pos)?;
        let (g, v) = self.slots[cap.index()].latest.get(&net.index)?;
        (*g == net.generation).then_some(v.as_slice())
    }

    // ── Queries ──────────────────────────────────────────────────────────

    pub fn net_info(&self, cap: CapId, pos: Pos) -> Option<NetInfo> {
        let net = self.net_at(cap, pos)?;
        let s = &self.slots[cap.index()];
        let n = s.layer.net(net)?;
        let mut totals = Vec::new();
        s.model.totals(&s.layer, net, &mut totals);
        Some(NetInfo {
            net,
            size: n.size,
            endpoints: n.ports.len() as u32,
            frozen: n.frozen > 0,
            totals,
        })
    }

    /// Every network of a capability with its totals (conservation tests).
    pub fn totals(&self, cap: CapId) -> Vec<(ResId, u64)> {
        let s = &self.slots[cap.index()];
        let mut out = Vec::new();
        for (net, _) in s.layer.nets() {
            s.model.totals(&s.layer, net, &mut out);
        }
        let mut merged: Vec<(ResId, u64)> = Vec::new();
        for (r, a) in out {
            match merged.iter_mut().find(|m| m.0 == r) {
                Some(m) => m.1 += a,
                None => merged.push((r, a)),
            }
        }
        merged
    }

    /// Indicator values shown at `pos`: `(property name, value)`.
    pub fn indication(&self, pos: Pos) -> Vec<(String, u8)> {
        let mut out = Vec::new();
        for (c, s) in self.slots.iter().enumerate() {
            let Some(n) = s.layer.node_at(pos) else {
                continue;
            };
            let cap = &self.reg.caps[c];
            for (i, &v) in s.ind.shown(n).iter().enumerate() {
                out.push((
                    format!("{}_{}", cap.state_prefix, cap.indicators[i].name),
                    v,
                ));
            }
        }
        out
    }
}
