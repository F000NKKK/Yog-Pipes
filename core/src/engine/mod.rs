//! The engine: registrations, one [`Layer`] + [`Model`] per capability,
//! world discovery through a [`Probe`], the scheduler, messaging and
//! saving. The host (the Yog-Pipes mod, or a test) drives it with world
//! events and ticks and implements [`Io`] for endpoint calls.

use std::collections::VecDeque;

use crate::ids::{BlockTypeId, CapId, Face, FxHashMap, NetId, NodeId, Pos, ResId};
use crate::indicate::{self, Indication};
use crate::layer::Layer;
use crate::model::buffered::Buffered;
use crate::model::external::{Driver, External};
use crate::model::flow::Flow;
use crate::model::in_flight::InFlight;
use crate::model::routed::Routed;
use crate::model::signal::Signal;
use crate::model::{Model, Rng};
use crate::registry::{Indicator, Policy, Registry, Side};

/// Most nodes one discovery adds (a runaway guard, not a limit on network
/// size: a bigger network is completed by the next discovery that touches
/// it). Counted in nodes, not probed positions — every node also probes
/// the air around it.
pub const MAX_DISCOVERY: usize = 262_144;

/// Bounded per-endpoint inbox for `post`.
pub const INBOX: usize = 64;

/// Save format version.
pub const FORMAT: u32 = 1;
const MAGIC: &[u8; 4] = b"YPT1";

/// The scheduler's random seed: the golden ratio's fraction, a fixed
/// start so a world runs the same every time.
const RNG_SEED: u64 = 0x9e37_79b9_7f4a_7c15;

/// Ticks between cache housekeeping passes.
const PRUNE_EVERY: u64 = 1200;

/// The world, as discovery sees it.
pub trait Probe {
    /// The engine's type of the block at `pos` (`reg.block_id(...)` of its
    /// id), or `None` for a block the engine doesn't know.
    fn block(&mut self, reg: &Registry, pos: Pos) -> Option<BlockTypeId>;
}

/// Which built-in mechanic a capability uses, or an external one.
pub enum ModelSpec {
    Buffered,
    Routed,
    InFlight,
    Flow,
    Signal {
        channels: u16,
        attenuation: u8,
    },
    External {
        driver: Box<dyn Driver>,
        params: Vec<String>,
    },
}

impl ModelSpec {
    fn build(self) -> Box<dyn Model> {
        match self {
            ModelSpec::Buffered => Box::new(Buffered::new()),
            ModelSpec::Routed => Box::new(Routed::new()),
            ModelSpec::InFlight => Box::new(InFlight::new()),
            ModelSpec::Flow => Box::new(Flow::new()),
            ModelSpec::Signal {
                channels,
                attenuation,
            } => Box::new(Signal::new(channels, attenuation)),
            ModelSpec::External { driver, params } => Box::new(External::new(driver, params)),
        }
    }

    fn tag(&self) -> u8 {
        match self {
            ModelSpec::Buffered => 0,
            ModelSpec::Routed => 1,
            ModelSpec::InFlight => 2,
            ModelSpec::Flow => 3,
            ModelSpec::Signal { .. } => 4,
            ModelSpec::External { .. } => 5,
        }
    }
}

pub struct CapabilityDef {
    pub name: String,
    pub kinds: Vec<String>,
    pub exclusive: bool,
    pub state_prefix: String,
    pub indicators: Vec<Indicator>,
    pub policy: Policy,
    pub events: u32,
    pub model: ModelSpec,
}

pub struct AttachDef {
    pub block: String,
    pub cap: String,
    pub groups: Vec<String>,
    pub params: Vec<(String, f64)>,
}

#[derive(Debug, Clone)]
pub enum TargetDef {
    Block(String),
    Position(Pos),
}

#[derive(Debug, Clone)]
pub struct EndpointDef {
    pub cap: String,
    pub target: TargetDef,
    pub kinds: Vec<String>,
    pub can_insert: bool,
    pub can_extract: bool,
    pub listens_signals: bool,
    pub listens_messages: bool,
    pub groups: Vec<String>,
    pub priority: i32,
    pub max_packet: u64,
    pub pressure: u32,
    pub handler: u32,
}

/// One face of a capability's node: what [`Engine::set_side`] configures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeFace {
    pub cap: CapId,
    pub pos: Pos,
    pub face: Face,
}

/// A snapshot of one network, for tooltips, debugging and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetInfo {
    pub net: NetId,
    pub size: u32,
    pub endpoints: u32,
    pub frozen: bool,
    pub totals: Vec<(ResId, u64)>,
}

#[derive(Default)]
struct Parked {
    seeds: Vec<Pos>,
    sides: FxHashMap<Pos, Vec<(Face, Side)>>,
    colors: FxHashMap<Pos, u8>,
    records: FxHashMap<Pos, Vec<Vec<u8>>>,
    global: Option<Vec<u8>>,
    inbox: FxHashMap<Pos, VecDeque<Vec<u8>>>,
}

struct Slot {
    layer: Layer,
    model: Box<dyn Model>,
    tag: u8,
    ind: Indication,
    inbox: FxHashMap<Pos, VecDeque<Vec<u8>>>,
    latest: FxHashMap<u32, (u32, Vec<u8>)>,
}

pub struct Engine {
    pub reg: Registry,
    slots: Vec<Slot>,
    rng: Rng,
    tick: u64,
    /// Saved state waiting for its capability or its nodes, by capability
    /// name.
    parked: FxHashMap<String, Parked>,
    /// Ticks between indication flushes.
    pub indication_interval: u64,
    region: Vec<Vec<NodeId>>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine {
            reg: Registry::default(),
            slots: Vec::new(),
            rng: Rng(RNG_SEED),
            tick: 0,
            parked: FxHashMap::default(),
            indication_interval: indicate::DEFAULT_INTERVAL,
            region: Vec::new(),
        }
    }
}

/// Build a `Cx` for slot `c` out of disjoint engine fields.
macro_rules! cx {
    ($e:ident, $c:expr, $io:expr) => {{
        let slot = &mut $e.slots[$c];
        (
            &mut slot.model,
            $crate::model::Cx {
                cap: CapId($c as u16),
                layer: &mut slot.layer,
                reg: &mut $e.reg,
                io: $io,
                ind: &mut slot.ind,
                rng: &mut $e.rng,
                tick: $e.tick,
            },
        )
    }};
}

mod messaging;
mod register;
mod saving;
mod world;

fn valid_prefix(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    pub fn layer(&self, cap: CapId) -> &Layer {
        &self.slots[cap.index()].layer
    }

    /// The indicator levels the world shows for `node` of `cap`, in the
    /// capability's indicator order.
    pub fn shown(&self, cap: CapId, node: NodeId) -> &[u8] {
        self.slots[cap.index()].ind.shown(node)
    }

    pub fn model(&self, cap: CapId) -> &dyn Model {
        &*self.slots[cap.index()].model
    }

    pub fn model_mut(&mut self, cap: CapId) -> &mut dyn Model {
        &mut *self.slots[cap.index()].model
    }

    pub fn dim(&mut self, name: &str) -> crate::ids::DimId {
        self.reg.dim(name)
    }
}
