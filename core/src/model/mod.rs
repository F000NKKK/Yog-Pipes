//! The model trait (§5.1 of the design) and everything a model works
//! with: [`Cx`] — its layer, the registry, endpoint I/O through the host's
//! [`Io`], events and indication — plus per-network storage.
//!
//! Endpoint I/O goes through [`Cx::insert`] / [`Cx::extract`] only. Those
//! check side modes, filters and kinds, apply kind conversions exactly and
//! clamp handlers that claim more than they were asked for, so every model
//! gets the same conservation guarantees for free.

pub mod buffered;
pub mod external;
pub mod flow;
pub mod in_flight;
pub mod routed;
pub mod signal;

use crate::ids::{CapId, EpTypeId, Face, NetId, NodeId, Pos, ResId};
use crate::indicate::Indication;
use crate::layer::{Layer, Port};
use crate::registry::Registry;
use yog_sync::Mutex;

// ── Host interface ───────────────────────────────────────────────────────────

/// Which endpoint a call is for, as the host sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call {
    pub cap: CapId,
    pub ty: EpTypeId,
    /// The host's handler handle (from the endpoint type).
    pub handler: u32,
    /// The endpoint block.
    pub at: Pos,
    /// The endpoint's face the pipe touches.
    pub side: Face,
    /// The pipe node.
    pub pipe: Pos,
}

/// What an extraction asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Anything the capability carries.
    Any,
    /// Exactly this resource.
    Res(ResId),
    /// Anything passing this filter.
    Filter(crate::ids::FilterId),
}

/// Something the capability's mod should hear about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Units left the network at `at` (pipe broken, merge of different
    /// buffers, overflow): the mod decides what that means (drop items,
    /// void energy).
    Spilled { at: Pos, res: ResId, amount: u64 },
    /// Units consumed by transport loss (`routed` with `loss`).
    Lost { at: Pos, res: ResId, amount: u64 },
    /// A transfer above a pipe's or endpoint's `max_packet`.
    Overload { at: Pos, amount: u64, limit: u64 },
}

/// The world side of endpoint I/O, implemented by the host (the mod's
/// handler dispatch, or a mock in tests).
pub trait Io {
    /// Offer `amount` of `res`; returns what was (or would be) accepted.
    fn insert(
        &mut self,
        reg: &Registry,
        call: &Call,
        res: ResId,
        amount: u64,
        simulate: bool,
    ) -> u64;
    /// Take up to `max`; returns what was (or would be) given.
    fn extract(
        &mut self,
        reg: &mut Registry,
        call: &Call,
        want: Want,
        max: u64,
        simulate: bool,
    ) -> Option<(ResId, u64)>;
    /// A listened signal level changed.
    fn signal(&mut self, reg: &Registry, call: &Call, channel: u8, level: u8);
    /// A message for an endpoint with an `on_message` handler; `false` to
    /// leave it in the inbox instead.
    fn message(&mut self, reg: &Registry, call: &Call, from: Pos, payload: &[u8]) -> bool;
    fn event(&mut self, reg: &Registry, cap: CapId, event: &Event);
    /// Indicator `index` of capability `cap` at `at` is now `value`.
    fn indicate(&mut self, reg: &Registry, cap: CapId, at: Pos, index: u8, value: u8);
}

// ── Model interface ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Tick again next tick.
    Active,
    /// Tick again in `n` ticks (source polling interval).
    Sleep(u32),
    /// Nothing to do until something wakes the network.
    Idle,
}

/// One saved piece of model state, keyed by a block position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub pos: Pos,
    pub bytes: Vec<u8>,
}

/// A transport mechanic. One instance per capability; all callbacks get
/// that capability's layer through [`Cx`].
pub trait Model: Send {
    /// Attach parameter names this model reads (others are refused).
    fn params(&self) -> &'static [&'static str];
    fn node_added(&mut self, _cx: &mut Cx, _node: NodeId, _net: NetId) {}
    /// Called before the node is unlinked: spill its share.
    fn node_removed(&mut self, _cx: &mut Cx, _node: NodeId, _net: NetId) {}
    fn merged(&mut self, _cx: &mut Cx, _into: NetId, _from: NetId) {}
    fn split(&mut self, _cx: &mut Cx, _from: NetId, _parts: &[(NetId, u32)]) {}
    /// The network's last node is gone (its state was already spilled
    /// through `node_removed`).
    fn net_removed(&mut self, _cx: &mut Cx, _net: NetId) {}
    fn tick(&mut self, cx: &mut Cx, net: NetId) -> Activity;
    fn save(&self, layer: &Layer, reg: &Registry, out: &mut Vec<Record>);
    /// Saved bytes for the node now at `node`.
    fn restore(&mut self, cx: &mut Cx, node: NodeId, net: NetId, bytes: &[u8]) -> bool;
    /// Resources held by a network (info, conservation tests).
    fn totals(&self, layer: &Layer, net: NetId, out: &mut Vec<(ResId, u64)>);
    /// A signal emitter changed (signal models only).
    fn emit(&mut self, _cx: &mut Cx, _at: Pos, _channel: u8, _level: u8) {}
    /// Signal level at `at` (a node or an endpoint next to nodes).
    fn level(&self, _layer: &Layer, _at: Pos, _channel: u8) -> u8 {
        0
    }
    /// The block at `at` is gone (whatever it was): drop state keyed by
    /// that position (signal emitters).
    fn forget(&mut self, _cx: &mut Cx, _at: Pos) {}
    /// Position-independent state, saved once per capability.
    fn save_global(&self, _reg: &Registry) -> Vec<u8> {
        Vec::new()
    }
    fn load_global(&mut self, _reg: &mut Registry, _bytes: &[u8]) {}
    /// Drop all world state (the world closed); configuration stays.
    fn reset(&mut self);
}

// ── Context ──────────────────────────────────────────────────────────────────

/// Seeded xorshift64*: deterministic, for `Policy::Random` and tests.
#[derive(Debug, Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

pub struct Cx<'a> {
    pub cap: CapId,
    pub layer: &'a mut Layer,
    pub reg: &'a mut Registry,
    pub io: &'a mut dyn Io,
    pub ind: &'a mut Indication,
    pub rng: &'a mut Rng,
    pub tick: u64,
}

impl Cx<'_> {
    /// The host's view of a port.
    pub fn call(&self, port: Port) -> Call {
        let ep = self.layer.endpoint(port.ep);
        let ty = self.reg.endpoint(ep.ty);
        Call {
            cap: self.cap,
            ty: ep.ty,
            handler: ty.handler,
            at: ep.pos,
            side: port.face.opposite(),
            pipe: self.layer.node(port.node).pos,
        }
    }

    /// Whether the network may insert into this port now.
    pub fn sink(&self, port: Port) -> bool {
        let side = self.layer.side(port.node, port.face);
        let ty = self.reg.endpoint(self.layer.endpoint(port.ep).ty);
        !side.disabled && side.mode.inserts(ty.can_insert)
    }

    /// Whether the network may extract from this port now.
    pub fn source(&self, port: Port) -> bool {
        let side = self.layer.side(port.node, port.face);
        let ty = self.reg.endpoint(self.layer.endpoint(port.ep).ty);
        !side.disabled && side.mode.extracts(ty.can_insert, ty.can_extract)
    }

    /// Endpoint plus side priority.
    pub fn priority(&self, port: Port) -> i32 {
        self.reg.endpoint(self.layer.endpoint(port.ep).ty).priority
            + self.layer.side(port.node, port.face).priority
    }

    /// The resource-acceptance check of a port: side filter and the
    /// endpoint's kinds (directly or by conversion). Returns the kind
    /// conversion `(num, den)`.
    fn accepts(&self, port: Port, res: ResId, color: u8) -> Option<(u64, u64)> {
        let side = self.layer.side(port.node, port.face);
        if !self
            .reg
            .filters
            .passes(side.filter, &self.reg.resources, res, color)
        {
            return None;
        }
        let kind = self.reg.resources.kind(res);
        let ty = self.reg.endpoint(self.layer.endpoint(port.ep).ty);
        if ty.kinds.is_empty() {
            return Some((1, 1));
        }
        ty.kinds
            .iter()
            .filter_map(|&k| self.reg.kinds.convertible(kind, k))
            .next()
    }

    /// Offer `amount` of `res` (tagged `color`) to a sink port. Returns
    /// what it accepted, in the network's units. Never more than `amount`.
    ///
    /// With a kind conversion `num/den` only whole conversion steps are
    /// offered, so the sink always receives exactly the value debited; a
    /// sink that accepts a partial step is debited the whole step and the
    /// difference is reported as [`Event::Lost`].
    pub fn insert(
        &mut self,
        port: Port,
        res: ResId,
        amount: u64,
        color: u8,
        simulate: bool,
    ) -> u64 {
        if amount == 0 || !self.sink(port) {
            return 0;
        }
        let Some((num, den)) = self.accepts(port, res, color) else {
            return 0;
        };
        let call = self.call(port);
        if (num, den) == (1, 1) {
            let got = self.io.insert(self.reg, &call, res, amount, simulate);
            return self.clamp(got, amount, &call);
        }
        let g = gcd(num, den);
        let (step_net, step_sink) = (den / g, num / g);
        let steps = amount / step_net;
        if steps == 0 {
            return 0;
        }
        let ty = self.reg.endpoint(call.ty);
        // `accepts` found the ratio through one of these kinds.
        let Some(target_kind) = ty.kinds.iter().copied().find(|&k| {
            self.reg
                .kinds
                .convertible(self.reg.resources.kind(res), k)
                .is_some()
        }) else {
            return 0;
        };
        let name = self.reg.kinds.get(target_kind).name.clone();
        let converted = self
            .reg
            .resources
            .intern(&self.reg.kinds, target_kind, &name, &[]);
        let offer = steps * step_sink;
        let got = self.io.insert(self.reg, &call, converted, offer, simulate);
        let got = self.clamp(got, offer, &call);
        let debit = got.div_ceil(step_sink) * step_net;
        if !simulate && !got.is_multiple_of(step_sink) {
            let value = got * step_net / step_sink;
            self.event(Event::Lost {
                at: call.at,
                res,
                amount: debit - value,
            });
        }
        debit
    }

    /// Take up to `max` from a source port. The result is clamped to
    /// `max`, must be a kind the capability carries and must pass the
    /// side filter; anything else is refused (a real extraction that
    /// returns something unexpected is put back, or spilled).
    pub fn extract(
        &mut self,
        port: Port,
        want: Want,
        max: u64,
        simulate: bool,
    ) -> Option<(ResId, u64)> {
        if max == 0 || !self.source(port) {
            return None;
        }
        let call = self.call(port);
        let side = self.layer.side(port.node, port.face);
        let want = match (want, side.filter) {
            (Want::Any, Some(f)) => Want::Filter(f),
            (w, _) => w,
        };
        let (res, got) = self.io.extract(self.reg, &call, want, max, simulate)?;
        let got = self.clamp(got, max, &call);
        if got == 0 {
            return None;
        }
        let ok = match want {
            Want::Res(r) => r == res,
            _ => true,
        } && self
            .reg
            .filters
            .passes(side.filter, &self.reg.resources, res, side.color)
            && self
                .reg
                .cap(self.cap)
                .carries(&self.reg.kinds, self.reg.resources.kind(res));
        if ok {
            return Some((res, got));
        }
        if !simulate {
            // Not what was asked for: give it back, or spill it.
            let back = self.io.insert(self.reg, &call, res, got, false).min(got);
            if back < got {
                self.event(Event::Spilled {
                    at: call.at,
                    res,
                    amount: got - back,
                });
            }
        }
        None
    }

    /// Return units to where they came from, bypassing side modes (a
    /// source that refused them keeps nothing). Returns what it took.
    pub fn give_back(&mut self, port: Port, res: ResId, amount: u64) -> u64 {
        if amount == 0 {
            return 0;
        }
        let call = self.call(port);
        let got = self.io.insert(self.reg, &call, res, amount, false);
        self.clamp(got, amount, &call)
    }

    fn clamp(&mut self, got: u64, asked: u64, call: &Call) -> u64 {
        if got > asked {
            log_once(
                call.handler,
                "handler claimed more than it was asked for; clamped",
            );
            asked
        } else {
            got
        }
    }

    pub fn event(&mut self, event: Event) {
        self.io.event(self.reg, self.cap, &event);
    }

    pub fn spill(&mut self, at: Pos, res: ResId, amount: u64) {
        if amount > 0 {
            self.event(Event::Spilled { at, res, amount });
        }
    }

    /// Publish a per-node indicator value `num/den` (quantised by the
    /// engine).
    pub fn indicate_node(&mut self, node: NodeId, index: u8, num: u64, den: u64) {
        self.ind.node(node, index, num, den);
    }

    /// Publish a per-network indicator value `num/den`.
    pub fn indicate_net(&mut self, net: NetId, index: u8, num: u64, den: u64) {
        self.ind.net(net, index, num, den);
    }

    /// Index of the capability's indicator with this source, if declared.
    pub fn indicator(&self, source: crate::registry::IndicatorSource) -> Option<u8> {
        self.reg
            .cap(self.cap)
            .indicators
            .iter()
            .position(|i| i.source == source)
            .map(|i| i as u8)
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// Warns about a misbehaving endpoint handler the first time only.
fn log_once(handler: u32, msg: &str) {
    static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock();
    if !seen.contains(&handler) {
        seen.push(handler);
        yog_logging::warn!("[yog-pipes] endpoint handler {handler}: {msg}");
    }
}

// ── Per-network storage ──────────────────────────────────────────────────────

/// Model state per network, dense by network index and checked by
/// generation, so a stale `NetId` never reads another network's state.
#[derive(Debug, Clone)]
pub struct NetMap<T> {
    slots: Vec<Option<(u32, T)>>,
}

impl<T> Default for NetMap<T> {
    fn default() -> Self {
        NetMap { slots: Vec::new() }
    }
}

impl<T: Default> NetMap<T> {
    pub fn get(&self, id: NetId) -> Option<&T> {
        match self.slots.get(id.index as usize) {
            Some(Some((g, v))) if *g == id.generation => Some(v),
            _ => None,
        }
    }

    /// The state of `id`, created (default) if missing or stale.
    pub fn entry(&mut self, id: NetId) -> &mut T {
        let i = id.index as usize;
        if self.slots.len() <= i {
            self.slots.resize_with(i + 1, || None);
        }
        let slot = &mut self.slots[i];
        if matches!(slot, Some((g, _)) if *g != id.generation) {
            *slot = None;
        }
        &mut slot.get_or_insert_with(|| (id.generation, T::default())).1
    }

    pub fn take(&mut self, id: NetId) -> Option<T> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if matches!(slot, Some((g, _)) if *g == id.generation) {
            slot.take().map(|(_, v)| v)
        } else {
            None
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (NetId, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.as_ref().map(|(g, v)| {
                (
                    NetId {
                        index: i as u32,
                        generation: *g,
                    },
                    v,
                )
            })
        })
    }
}
