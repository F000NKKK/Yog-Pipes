//! The types of the public API. Every one is `#[yog_export]`ed, so the
//! generated `yog_pipes_exports` crate carries an identical copy: other
//! mods build these values, and endpoint/event/external handlers receive
//! and return them across the interop boundary.
//!
//! Everything is by name (capabilities, kinds, blocks, dimensions), so
//! nothing here depends on registration order or on this session's ids.

use std::collections::HashMap;

use yog_api::yog_export;

// ── Positions and payloads ───────────────────────────────────────────────────

/// A block position. `dim` accepts `"overworld"` or `"minecraft:overworld"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[yog_export]
pub struct PipePos {
    pub dim: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Opaque data for messaging (`post`, `broadcast`); Yog-Pipes never reads
/// it.
#[derive(Debug, Clone, Default, PartialEq)]
#[yog_export]
pub struct PipePayload {
    /// Bytes the sender and the receiver agree on.
    pub data: Vec<u8>,
    /// Free-form key/value metadata, never inspected.
    pub metadata: Vec<(String, String)>,
}

/// A face of a block, in Minecraft's `Direction` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[yog_export]
pub enum PipeFace {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

/// An exported function of another mod — `#[yog_export] pub fn symbol(..)`
/// of the mod `mod_id` — called by Yog-Pipes. Looked up by name on use, so
/// it survives reloads and restarts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[yog_export]
pub struct Handler {
    pub mod_id: String,
    pub symbol: String,
}

// ── Resources ────────────────────────────────────────────────────────────────

/// Counted in whole units (item stacks) or as a quantity (mB, FE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[yog_export]
pub enum KindForm {
    Discrete,
    Continuous,
}

/// When two portions of a kind are the same resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[yog_export]
pub enum MergeRule {
    /// Same id; data ignored.
    ById,
    /// Same id and same data (item stacks with components).
    ByIdAndData,
}

/// `amount` of the kind is worth `amount * num / den` of `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub struct Conversion {
    pub to: String,
    pub num: u64,
    pub den: u64,
}

/// A resource kind — `"mymod:energy"`, `"mymod:item"`, … Kinds are an open,
/// namespaced registry: Yog-Pipes defines none of its own.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct KindDef {
    pub name: String,
    pub form: KindForm,
    pub merge: MergeRule,
    /// Display unit (`"FE"`, `"mB"`, `"items"`).
    pub unit: String,
    pub conversions: Vec<Conversion>,
}

/// One resource: a kind, an id within it and optional data (an item's
/// components, a fluid's NBT) — opaque bytes compared for equality only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[yog_export]
pub struct Resource {
    pub kind: String,
    pub id: String,
    pub data: Vec<u8>,
}

/// An amount of one resource.
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub struct Stack {
    pub resource: Resource,
    pub amount: u64,
}

/// One entry of a filter.
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub enum FilterRule {
    /// Exactly this resource (kind, id and data).
    Resource(Resource),
    /// Any resource with this id.
    Id(String),
    /// Any resource of this kind.
    Kind(String),
    /// Units tagged with this colour (in-flight sorting).
    Colour(u8),
}

/// Passes what matches a rule (`deny = false`) or what matches none.
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub struct FilterDef {
    pub deny: bool,
    pub rules: Vec<FilterRule>,
}

// ── Capabilities ─────────────────────────────────────────────────────────────

/// How a capability moves its resources — named by mechanic, not by what
/// is carried.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub enum Mechanic {
    /// A shared buffer per network (Forge-Energy cables, Mekanism tanks).
    /// Attach params: `capacity`, `throughput`.
    Buffered,
    /// Instant source→sink transfer with no buffer (EU cables, AE-style
    /// item routing). Attach params: `throughput`, `loss` (‰ per block),
    /// `max_packet`.
    Routed,
    /// Units travelling block by block (BuildCraft / Thermal item pipes).
    /// Attach params: `speed` (blocks/tick), `stack`, `interval`.
    InFlight,
    /// Per-segment levels equalising with pressure (Mekanism / Thermal
    /// fluid ducts). Attach params: `capacity`, `rate`, `head`.
    Flow,
    /// Levels per channel, strongest emitter wins, minus `attenuation` per
    /// block (redstone, bundled cables).
    Signal { channels: u8, attenuation: u8 },
    /// Implemented by another mod: `handler` is called with an
    /// [`ExternalCall`] per network tick; `params` are the attach params
    /// it reads.
    External {
        handler: Handler,
        params: Vec<String>,
    },
}

/// Which sinks get resources first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[yog_export]
pub enum Distribution {
    RoundRobin,
    Priority,
    Nearest,
    EvenSplit,
    Random,
}

/// What an indicator shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[yog_export]
pub enum IndicatorSource {
    /// Stored / capacity of the node or network.
    Fill,
    /// Moved last tick / throughput.
    Flow,
    /// Units in flight in the node.
    Units,
    /// Signal level.
    Power,
    /// Slot `n` of an external model's indicators.
    External(u8),
}

/// A visual indicator: the block-state property `<state_prefix>_<name>`
/// with `levels + 1` values (0 = off).
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub struct IndicatorDef {
    pub name: String,
    pub levels: u8,
    pub source: IndicatorSource,
}

/// One thing pipes can carry. A pipe block may carry several capabilities
/// at once; each has its own networks.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct CapabilityDef {
    /// Namespaced (`"mymod:energy"`).
    pub name: String,
    /// Kinds it carries; empty = any kind (or none, for pure signals).
    pub kinds: Vec<String>,
    /// A block carrying this capability carries nothing else.
    pub exclusive: bool,
    /// Prefix of its indicator properties (`[a-z0-9_]+`).
    pub state_prefix: String,
    pub indicators: Vec<IndicatorDef>,
    pub distribution: Distribution,
    /// Called with every [`PipeEvent`] of this capability (spills, loss,
    /// overloads, indicator changes).
    pub events: Option<Handler>,
    pub mechanic: Mechanic,
}

/// Makes a block a pipe of a capability.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct AttachDef {
    /// Block id (`"mymod:copper_cable"`).
    pub block: String,
    pub capability: String,
    /// Adjacent pipes link when their groups share an entry (empty = links
    /// to every pipe of the capability). Pass the block's `connect_groups`
    /// to link exactly what the player sees connected.
    pub groups: Vec<String>,
    /// Mechanic parameters (see [`Mechanic`]); unknown names are refused.
    pub params: HashMap<String, f64>,
}

// ── Endpoints ────────────────────────────────────────────────────────────────

/// What an endpoint registration applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub enum EndpointTarget {
    /// Every block of this id.
    Block(String),
    /// One position (a block of another mod, a vanilla block); saved with
    /// the world.
    Position(PipePos),
}

/// A block pipes connect to and exchange resources, signals or messages
/// with.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct EndpointDef {
    pub capability: String,
    pub target: EndpointTarget,
    /// Kinds it handles (empty = whatever the capability carries).
    pub kinds: Vec<String>,
    pub can_insert: bool,
    pub can_extract: bool,
    pub listens_signals: bool,
    pub listens_messages: bool,
    /// Pipes connect only when the groups meet (empty = any pipe).
    pub groups: Vec<String>,
    /// Higher first under [`Distribution::Priority`].
    pub priority: i32,
    /// Largest accepted transfer (0 = no limit); bigger ones overload.
    pub max_packet: u64,
    /// Pump pressure for [`Mechanic::Flow`].
    pub pressure: u32,
    /// Called with an [`EndpointCall`] for every operation.
    pub handler: Option<Handler>,
}

/// How a pipe's face treats what it touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[yog_export]
pub enum SideMode {
    /// Insert into anything that takes, extract only from pure sources.
    Auto,
    In,
    Out,
    Both,
    Off,
}

/// The configuration of one face of one pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub struct SideDef {
    pub mode: SideMode,
    /// Temporarily off (a redstone-controlled side).
    pub disabled: bool,
    pub priority: i32,
    pub filter: Option<FilterDef>,
    /// Colour tag of units leaving here; colour filters match it.
    pub colour: u8,
}

/// What an extraction asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub enum Want {
    Any,
    Resource(Resource),
    Filter(FilterDef),
}

/// One operation on an endpoint.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub enum EndpointOp {
    /// Take up to `amount`; reply [`EndpointReply::Accepted`]. With
    /// `simulate` nothing may change.
    Insert { stack: Stack, simulate: bool },
    /// Give up to `max`; reply [`EndpointReply::Gave`].
    Extract {
        want: Want,
        max: u64,
        simulate: bool,
    },
    /// A listened signal level changed.
    Signal { channel: u8, level: u8 },
    /// A message; reply [`EndpointReply::Handled`] (`false` keeps it in
    /// the inbox for `receive`).
    Message { from: PipePos, payload: PipePayload },
}

/// A call to an endpoint handler.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct EndpointCall {
    pub capability: String,
    /// The endpoint block.
    pub at: PipePos,
    /// Its face the pipe touches.
    pub side: PipeFace,
    /// The pipe.
    pub pipe: PipePos,
    pub op: EndpointOp,
}

/// An endpoint handler's answer.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub enum EndpointReply {
    Accepted(u64),
    Gave(Option<Stack>),
    Handled(bool),
    /// For signals (and anything not handled).
    Done,
}

/// Sent to a capability's `events` handler.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub enum PipeEvent {
    /// Units left the network at `at` (pipe broken, overflow, a merge of
    /// different contents) — drop them as items, void them, …
    Spilled {
        capability: String,
        at: PipePos,
        stack: Stack,
    },
    /// Units consumed by transport loss.
    Lost {
        capability: String,
        at: PipePos,
        stack: Stack,
    },
    /// A transfer above a `max_packet`.
    Overload {
        capability: String,
        at: PipePos,
        amount: u64,
        limit: u64,
    },
    /// The indicator property `property` at `at` is now `value`.
    Indicator {
        capability: String,
        at: PipePos,
        property: String,
        value: u8,
    },
}

// ── External mechanics ───────────────────────────────────────────────────────

/// One connection of an external model's network to an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
#[yog_export]
pub struct ExternalPort {
    pub at: PipePos,
    pub pipe: PipePos,
    pub side: PipeFace,
    /// The side and endpoint allow inserting / extracting now.
    pub sink: bool,
    pub source: bool,
    pub priority: i32,
}

/// Endpoint I/O an external model asks for; ports index
/// [`ExternalCall::Tick::ports`].
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub enum ExternalOp {
    Insert {
        port: u32,
        stack: Stack,
    },
    Extract {
        port: u32,
        want: Want,
        max: u64,
    },
    /// Units leave the network at `at` (reported as [`PipeEvent::Spilled`]).
    Spill {
        at: PipePos,
        stack: Stack,
    },
    /// Indicator slot `slot` of the whole network: `num / den`.
    Indicate {
        slot: u8,
        num: u64,
        den: u64,
    },
}

/// The outcome of one [`ExternalOp`], in order.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub enum ExternalResult {
    Inserted(u64),
    Extracted(Option<Stack>),
    Done,
}

/// When an external network wants its next tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[yog_export]
pub enum ExternalActivity {
    Active,
    Sleep(u32),
    Idle,
}

/// A call to an external model. `state` is the network's own opaque
/// state, kept and saved by Yog-Pipes.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub enum ExternalCall {
    /// Tick one network. Ops in the reply run at once and the model is
    /// called again with their `results` (up to 4 rounds a tick), so it
    /// always books what actually moved.
    Tick {
        capability: String,
        network: u64,
        size: u32,
        /// Per attach param (in `Mechanic::External::params` order): the
        /// smallest value on the network (a bottleneck) and the sum (a
        /// capacity).
        param_min: Vec<f64>,
        param_sum: Vec<f64>,
        ports: Vec<ExternalPort>,
        results: Vec<ExternalResult>,
        state: Vec<u8>,
    },
    /// Two networks joined; reply with the joined state.
    Merge {
        capability: String,
        into: Vec<u8>,
        from: Vec<u8>,
    },
    /// A network split into parts of `sizes` nodes; reply with `parts`
    /// (one per size) and the remaining `state`.
    Split {
        capability: String,
        state: Vec<u8>,
        sizes: Vec<u32>,
    },
    /// The pipe at `at` is about to go: spill its share (ops `Spill`).
    NodeRemoved {
        capability: String,
        at: PipePos,
        state: Vec<u8>,
    },
}

/// An external model's answer.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct ExternalReply {
    pub state: Vec<u8>,
    pub parts: Vec<Vec<u8>>,
    pub ops: Vec<ExternalOp>,
    pub activity: ExternalActivity,
}

// ── Queries ──────────────────────────────────────────────────────────────────

/// One network.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct NetworkInfo {
    /// Stable while the network exists; changes when it merges or splits.
    pub network: u64,
    pub size: u32,
    pub endpoints: u32,
    /// Part of it is in unloaded chunks: it doesn't tick.
    pub frozen: bool,
    pub contents: Vec<Stack>,
}

/// One face of one pipe.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct FaceInfo {
    pub face: PipeFace,
    /// Linked to a pipe of the same capability.
    pub linked: bool,
    /// Connected to an endpoint.
    pub endpoint: bool,
    pub mode: SideMode,
    pub disabled: bool,
    pub priority: i32,
    pub filtered: bool,
}

/// One pipe of one capability.
#[derive(Debug, Clone, PartialEq)]
#[yog_export]
pub struct PipeInfo {
    pub capability: String,
    pub network: u64,
    pub colour: u8,
    pub faces: Vec<FaceInfo>,
}
