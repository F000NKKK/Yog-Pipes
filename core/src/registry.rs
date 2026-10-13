//! What the engine knows by registration: resource kinds, interned
//! resources, block types, endpoint types, filters and side configuration.
//! Everything is keyed by the integer ids from [`crate::ids`].

use crate::ids::{BlockTypeId, CapId, EpTypeId, FilterId, FxHashMap, Interner, KindId, Pos, ResId};

// ── Resource kinds ───────────────────────────────────────────────────────────

/// Whether a kind is counted in whole units (stacks, packets) or as a
/// continuous quantity in its own base unit (mB, FE). Both are `u64`
/// amounts; the form tells models and UIs how to present and split them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindForm {
    Discrete,
    Continuous,
}

/// When two portions of one kind are the same resource and may share a
/// buffer, a segment or a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRule {
    /// Same id — data is ignored (and not even interned).
    ById,
    /// Same id and same data bytes (item stacks with components).
    ByIdAndData,
}

/// `amount` of this kind is worth `amount * num / den` of `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conversion {
    pub to: KindId,
    pub num: u64,
    pub den: u64,
}

#[derive(Debug, Clone)]
pub struct Kind {
    pub name: String,
    pub form: KindForm,
    pub merge: MergeRule,
    /// Display unit (`"FE"`, `"mB"`), for tooltips and readouts.
    pub unit: String,
    pub conversions: Vec<Conversion>,
}

#[derive(Debug, Default, Clone)]
pub struct Kinds {
    list: Vec<Kind>,
    by_name: FxHashMap<String, KindId>,
}

impl Kinds {
    pub fn register(&mut self, kind: Kind) -> Result<KindId, String> {
        if !kind.name.contains(':') {
            return Err(format!(
                "kind `{}` must be namespaced (`mymod:name`)",
                kind.name
            ));
        }
        if let Some(c) = kind.conversions.iter().find(|c| c.num == 0 || c.den == 0) {
            return Err(format!(
                "kind `{}`: conversion to {:?} has a zero ratio",
                kind.name, c.to
            ));
        }
        if let Some(&id) = self.by_name.get(&kind.name) {
            self.list[id.index()] = kind;
            return Ok(id);
        }
        let id = KindId(self.list.len() as u16);
        self.by_name.insert(kind.name.clone(), id);
        self.list.push(kind);
        Ok(id)
    }

    pub fn id(&self, name: &str) -> Option<KindId> {
        self.by_name.get(name).copied()
    }

    pub fn get(&self, id: KindId) -> &Kind {
        &self.list[id.index()]
    }

    /// Whether something carrying `from` can hand it to something taking
    /// `to` — directly or through one declared conversion.
    pub fn convertible(&self, from: KindId, to: KindId) -> Option<(u64, u64)> {
        if from == to {
            return Some((1, 1));
        }
        self.get(from)
            .conversions
            .iter()
            .find(|c| c.to == to)
            .map(|c| (c.num, c.den))
    }
}

// ── Resources ────────────────────────────────────────────────────────────────

/// A resource: kind + id + optional opaque data. Interned to a [`ResId`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    pub kind: KindId,
    pub id: Box<str>,
    pub data: Box<[u8]>,
}

#[derive(Debug, Default, Clone)]
pub struct Resources {
    interner: Interner<ResourceKey>,
}

impl Resources {
    /// Intern a resource, applying its kind's merge rule (data dropped for
    /// [`MergeRule::ById`]).
    pub fn intern(&mut self, kinds: &Kinds, kind: KindId, id: &str, data: &[u8]) -> ResId {
        let data: Box<[u8]> = match kinds.get(kind).merge {
            MergeRule::ById => Box::new([]),
            MergeRule::ByIdAndData => data.into(),
        };
        ResId(self.interner.intern(ResourceKey {
            kind,
            id: id.into(),
            data,
        }))
    }

    pub fn get(&self, id: ResId) -> &ResourceKey {
        self.interner.resolve(id.0)
    }

    pub fn kind(&self, id: ResId) -> KindId {
        self.get(id).kind
    }
}

// ── Filters ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterEntry {
    /// Exactly this resource (kind, id and data).
    Resource(ResId),
    /// Any resource with this id, whatever its data.
    Id(Box<str>),
    /// Any resource of this kind.
    Kind(KindId),
    /// Units tagged with this colour (in-flight sorting).
    Colour(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub mode: FilterMode,
    pub entries: Vec<FilterEntry>,
}

impl Filter {
    /// Whether a portion of `res` (tagged `colour`, 0 = none) passes.
    pub fn passes(&self, resources: &Resources, res: ResId, colour: u8) -> bool {
        let key = resources.get(res);
        let hit = self.entries.iter().any(|e| match e {
            FilterEntry::Resource(r) => *r == res,
            FilterEntry::Id(id) => *key.id == **id,
            FilterEntry::Kind(k) => key.kind == *k,
            FilterEntry::Colour(c) => *c == colour,
        });
        match self.mode {
            FilterMode::Allow => hit,
            FilterMode::Deny => !hit,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Filters {
    list: Vec<Filter>,
}

impl Filters {
    pub fn add(&mut self, f: Filter) -> FilterId {
        if let Some(i) = self.list.iter().position(|x| *x == f) {
            return FilterId(i as u32);
        }
        self.list.push(f);
        FilterId(self.list.len() as u32 - 1)
    }

    pub fn get(&self, id: FilterId) -> &Filter {
        &self.list[id.index()]
    }

    /// `true` when there is no filter.
    pub fn passes(&self, id: Option<FilterId>, res: &Resources, r: ResId, colour: u8) -> bool {
        id.is_none_or(|f| self.get(f).passes(res, r, colour))
    }
}

// ── Side configuration ───────────────────────────────────────────────────────

/// What a node's face does: toward a pipe, `Off` cuts the link; toward an
/// endpoint, `In` = the network only inserts there, `Out` = only extracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideMode {
    #[default]
    Both,
    In,
    Out,
    Off,
}

impl SideMode {
    pub fn inserts(self) -> bool {
        matches!(self, SideMode::Both | SideMode::In)
    }
    pub fn extracts(self) -> bool {
        matches!(self, SideMode::Both | SideMode::Out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Side {
    pub mode: SideMode,
    /// `false` pauses this side (redstone control).
    pub disabled: bool,
    /// Added to the endpoint's own priority.
    pub priority: i32,
    pub filter: Option<FilterId>,
    /// Colour tag given to units extracted through this side.
    pub colour: u8,
}

// ── Block and endpoint types ─────────────────────────────────────────────────

/// One capability attached to a block type.
#[derive(Debug, Clone)]
pub struct Attach {
    pub cap: CapId,
    /// Interned link groups, sorted.
    pub groups: Vec<u32>,
    /// Values for the model's parameters, in `Model::params()` order
    /// (absent ones are 0).
    pub params: Vec<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct BlockType {
    pub name: String,
    /// `(capability, index into that layer's attach table)`.
    pub attaches: Vec<(CapId, u16)>,
    pub endpoints: Vec<EpTypeId>,
}

/// Where an endpoint type applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointTarget {
    Block(BlockTypeId),
    Position(Pos),
}

#[derive(Debug, Clone)]
pub struct EndpointType {
    pub cap: CapId,
    pub target: EndpointTarget,
    pub kinds: Vec<KindId>,
    pub can_insert: bool,
    pub can_extract: bool,
    pub listens_signals: bool,
    pub listens_messages: bool,
    /// Interned link groups; empty = plugs into any pipe of the capability.
    pub groups: Vec<u32>,
    pub priority: i32,
    /// 0 = no limit; a transfer above it raises `Event::Overload`.
    pub max_packet: u64,
    /// Pump strength for `flow` (range in blocks); 0 = not a pump.
    pub pressure: u32,
    /// The host's handle for dispatching this endpoint's handlers.
    pub handler: u32,
}

/// Whether two sorted group lists share an entry (empty `b` = any).
pub fn groups_meet(a: &[u32], b: &[u32]) -> bool {
    if b.is_empty() {
        return true;
    }
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => return true,
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    false
}

// ── Capabilities ─────────────────────────────────────────────────────────────

/// How a model orders the endpoints it hands resources to (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Rotating start, so every sink gets its turn.
    #[default]
    RoundRobin,
    /// Highest endpoint + side priority first; round-robin within a level.
    Priority,
    /// Fewest pipe hops from the source first.
    Nearest,
    /// Simulate every sink, then water-fill: everyone gets an equal share
    /// of what they can take.
    EvenSplit,
    /// Seeded shuffle (deterministic per world tick).
    Random,
}

/// What an indicator shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorSource {
    /// How full the buffer/segment is.
    Fill,
    /// Whether something moved recently (0/max).
    Flow,
    /// Units in a node.
    Units,
    /// Signal level at a node (max over channels).
    Power,
    /// Value `n` published by an external model.
    External(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indicator {
    /// Property suffix: the block-state property is `<prefix>_<name>`.
    pub name: String,
    /// The property's maximum (`0..=levels`).
    pub levels: u8,
    pub source: IndicatorSource,
}

/// A registered capability (its model lives in the engine, by the same id).
#[derive(Debug, Clone)]
pub struct Capability {
    pub name: String,
    pub kinds: Vec<KindId>,
    pub exclusive: bool,
    pub state_prefix: String,
    pub indicators: Vec<Indicator>,
    pub policy: Policy,
    /// The host's handle for this capability's event hook (0 = none).
    pub events: u32,
}

impl Capability {
    /// Whether this capability carries resources of `kind`.
    pub fn carries(&self, kinds: &Kinds, kind: KindId) -> bool {
        self.kinds.iter().any(|&k| kinds.convertible(kind, k).is_some())
    }
}

/// Every registration the engine knows, in one place so models can borrow
/// it next to their layer.
#[derive(Debug, Default)]
pub struct Registry {
    pub kinds: Kinds,
    pub resources: Resources,
    pub filters: Filters,
    pub caps: Vec<Capability>,
    pub blocks: Vec<BlockType>,
    pub block_index: FxHashMap<String, BlockTypeId>,
    pub endpoints: Vec<EndpointType>,
    /// Positional endpoint types by position.
    pub positional: FxHashMap<Pos, Vec<EpTypeId>>,
    pub groups: Interner<String>,
    pub dims: Interner<String>,
}

impl Registry {
    pub fn cap(&self, id: CapId) -> &Capability {
        &self.caps[id.index()]
    }

    pub fn cap_id(&self, name: &str) -> Option<CapId> {
        self.caps
            .iter()
            .position(|c| c.name == name)
            .map(|i| CapId(i as u16))
    }

    pub fn block_id(&self, name: &str) -> Option<BlockTypeId> {
        self.block_index.get(name).copied()
    }

    /// Get or create the block type for a block id.
    pub fn block_entry(&mut self, name: &str) -> BlockTypeId {
        if let Some(&id) = self.block_index.get(name) {
            return id;
        }
        let id = BlockTypeId(self.blocks.len() as u32);
        self.blocks.push(BlockType {
            name: name.to_string(),
            ..Default::default()
        });
        self.block_index.insert(name.to_string(), id);
        id
    }

    pub fn endpoint(&self, id: EpTypeId) -> &EndpointType {
        &self.endpoints[id.index()]
    }

    /// Endpoint types of capability `cap` that apply at `pos` holding
    /// block `block` (positional ones win over block ones).
    pub fn endpoint_type(&self, cap: CapId, pos: Pos, block: Option<BlockTypeId>) -> Option<EpTypeId> {
        if let Some(list) = self.positional.get(&pos)
            && let Some(&t) = list.iter().find(|&&t| self.endpoint(t).cap == cap)
        {
            return Some(t);
        }
        let b = block?;
        self.blocks[b.index()]
            .endpoints
            .iter()
            .copied()
            .find(|&t| self.endpoint(t).cap == cap)
    }

    pub fn dim(&mut self, name: &str) -> crate::ids::DimId {
        crate::ids::DimId(self.dims.intern(crate::ids::normalize_dim(name)) as u16)
    }

    pub fn dim_name(&self, id: crate::ids::DimId) -> &str {
        self.dims.resolve(id.0 as u32)
    }
}
