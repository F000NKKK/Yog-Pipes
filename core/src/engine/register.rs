//! Registration: kinds, capabilities, which blocks carry them, endpoint
//! types and filters. Mismatches are refused here, never discovered at
//! transfer time.

use super::{AttachDef, CapabilityDef, EndpointDef, Engine, Slot, TargetDef, valid_prefix};
use crate::ids::{CapId, EpTypeId, FilterId, FxHashMap, KindId, Pos};
use crate::indicate::{Indication, MAX_INDICATORS};
use crate::layer::Layer;
use crate::registry::{Attach, Capability, EndpointTarget, EndpointType, Filter, Kind};

impl Engine {
    pub fn register_kind(&mut self, kind: Kind) -> Result<KindId, String> {
        self.reg.kinds.register(kind)
    }

    /// Register (or update) a capability. Re-registering a name keeps its
    /// network state and model unless the model mechanic changed.
    pub fn register_capability(&mut self, def: CapabilityDef) -> Result<CapId, String> {
        self.check_capability(&def)?;
        let cap = Capability {
            name: def.name.clone(),
            kinds: self.kind_ids(&def.kinds)?,
            exclusive: def.exclusive,
            state_prefix: def.state_prefix,
            indicators: def.indicators,
            policy: def.policy,
            events: def.events,
        };
        let levels: Vec<u8> = cap.indicators.iter().map(|i| i.levels).collect();
        let tag = def.model.tag();
        let id = match self.reg.cap_id(&def.name) {
            Some(id) => {
                self.reg.caps[id.index()] = cap;
                let slot = &mut self.slots[id.index()];
                slot.ind.levels = levels;
                if slot.tag != tag {
                    slot.model = def.model.build();
                    slot.tag = tag;
                }
                id
            }
            None => {
                self.reg.caps.push(cap);
                self.slots.push(Slot {
                    layer: Layer::default(),
                    model: def.model.build(),
                    tag,
                    ind: Indication::new(levels),
                    inbox: FxHashMap::default(),
                    latest: FxHashMap::default(),
                });
                CapId(self.reg.caps.len() as u16 - 1)
            }
        };
        self.unpark_global(&def.name, id);
        Ok(id)
    }

    /// Why `def` can't be registered, if it can't.
    fn check_capability(&self, def: &CapabilityDef) -> Result<(), String> {
        if !def.name.contains(':') {
            return Err(format!(
                "capability `{}` must be namespaced (`mymod:name`)",
                def.name
            ));
        }
        if !valid_prefix(&def.state_prefix) {
            return Err(format!(
                "state prefix `{}` must be `[a-z0-9_]+`",
                def.state_prefix
            ));
        }
        if let Some(other) = self
            .reg
            .caps
            .iter()
            .find(|c| c.state_prefix == def.state_prefix && c.name != def.name)
        {
            return Err(format!(
                "state prefix `{}` is already used by `{}`",
                def.state_prefix, other.name
            ));
        }
        if def.indicators.len() > MAX_INDICATORS {
            return Err(format!(
                "at most {MAX_INDICATORS} indicators per capability"
            ));
        }
        if let Some(i) = def
            .indicators
            .iter()
            .find(|i| i.levels == 0 || !valid_prefix(&i.name))
        {
            return Err(format!(
                "indicator `{}`: name must be `[a-z0-9_]+` and levels ≥ 1",
                i.name
            ));
        }
        Ok(())
    }

    /// Hand capability `id` (named `name`) its saved global state and inbox.
    pub(super) fn unpark_global(&mut self, name: &str, id: CapId) {
        let Some(p) = self.parked.get_mut(name) else {
            return;
        };
        let slot = &mut self.slots[id.index()];
        if let Some(g) = p.global.take() {
            slot.model.load_global(&mut self.reg, &g);
        }
        for (pos, q) in p.inbox.drain() {
            slot.inbox.insert(pos, q);
        }
    }

    /// The ids of kinds named `names`; an unknown one is refused.
    fn kind_ids(&self, names: &[String]) -> Result<Vec<KindId>, String> {
        names
            .iter()
            .map(|k| {
                self.reg
                    .kinds
                    .id(k)
                    .ok_or_else(|| format!("unknown kind `{k}`"))
            })
            .collect()
    }

    /// Group names as sorted, distinct interned ids.
    fn group_ids(&mut self, names: &[String]) -> Vec<u32> {
        let mut groups: Vec<u32> = names
            .iter()
            .map(|g| self.reg.groups.intern(g.clone()))
            .collect();
        groups.sort_unstable();
        groups.dedup();
        groups
    }

    /// The capability named `name`; an unknown one is refused.
    fn known_cap(&self, name: &str) -> Result<CapId, String> {
        self.reg
            .cap_id(name)
            .ok_or_else(|| format!("unknown capability `{name}`"))
    }

    /// Give `block` capability `cap`. Unknown parameter names are refused.
    pub fn attach(&mut self, def: AttachDef) -> Result<(), String> {
        let cap = self.known_cap(&def.cap)?;
        let names = self.slots[cap.index()].model.params();
        let mut params = vec![0.0; names.len()];
        for (k, v) in &def.params {
            let i = names.iter().position(|n| n == k).ok_or_else(|| {
                format!("`{}` has no parameter `{k}` (known: {names:?})", def.cap)
            })?;
            params[i] = *v;
        }
        let bt = self.reg.block_entry(&def.block);
        if self.reg.cap(cap).exclusive
            && let Some(&(other, _)) = self.reg.blocks[bt.index()]
                .attaches
                .iter()
                .find(|&&(c, _)| c != cap && self.reg.cap(c).exclusive)
        {
            return Err(format!(
                "`{}` already carries exclusive capability `{}`",
                def.block,
                self.reg.cap(other).name
            ));
        }
        let att = Attach {
            cap,
            groups: self.group_ids(&def.groups),
            params,
        };
        let existing = self.reg.blocks[bt.index()]
            .attaches
            .iter()
            .find(|a| a.0 == cap)
            .map(|a| a.1);
        let index = self.slots[cap.index()].layer.set_attach(existing, att);
        if existing.is_none() {
            self.reg.blocks[bt.index()].attaches.push((cap, index));
        }
        Ok(())
    }

    /// Register (or replace) an endpoint type. Kind mismatches are refused
    /// here, never discovered at transfer time.
    pub fn register_endpoint(&mut self, def: EndpointDef) -> Result<EpTypeId, String> {
        let cap = self.known_cap(&def.cap)?;
        let kinds = self.kind_ids(&def.kinds)?;
        let c = self.reg.cap(cap);
        if !kinds.is_empty()
            && !c.kinds.is_empty()
            && !kinds.iter().any(|&k| {
                c.carries(&self.reg.kinds, k)
                    || c.kinds
                        .iter()
                        .any(|&ck| self.reg.kinds.convertible(ck, k).is_some())
            })
        {
            return Err(format!(
                "endpoint kinds {:?} don't meet the kinds of `{}` (directly or by a conversion)",
                def.kinds, def.cap
            ));
        }
        let target = match def.target {
            TargetDef::Block(ref b) => EndpointTarget::Block(self.reg.block_entry(b)),
            TargetDef::Position(p) => EndpointTarget::Position(p),
        };
        let ty = EndpointType {
            cap,
            target,
            kinds,
            can_insert: def.can_insert,
            can_extract: def.can_extract,
            listens_signals: def.listens_signals,
            listens_messages: def.listens_messages,
            groups: self.group_ids(&def.groups),
            priority: def.priority,
            max_packet: def.max_packet,
            pressure: def.pressure,
            handler: def.handler,
        };
        Ok(self.put_endpoint_type(ty))
    }

    /// Store `ty`, in place of the type with its capability and target.
    fn put_endpoint_type(&mut self, ty: EndpointType) -> EpTypeId {
        let (cap, target) = (ty.cap, ty.target);
        if let Some(i) = self
            .reg
            .endpoints
            .iter()
            .position(|e| e.cap == cap && e.target == target)
        {
            self.reg.endpoints[i] = ty;
            return EpTypeId(i as u32);
        }
        let id = EpTypeId(self.reg.endpoints.len() as u32);
        self.reg.endpoints.push(ty);
        match target {
            EndpointTarget::Block(b) => self.reg.blocks[b.index()].endpoints.push(id),
            EndpointTarget::Position(p) => self.reg.positional.entry(p).or_default().push(id),
        }
        id
    }

    /// Forget a positional endpoint (a binding the player removed) and
    /// disconnect it.
    pub fn unregister_endpoint(&mut self, cap: &str, pos: Pos) {
        let Some(cap) = self.reg.cap_id(cap) else {
            return;
        };
        if let Some(list) = self.reg.positional.get_mut(&pos) {
            list.retain(|&t| self.reg.endpoints[t.index()].cap != cap);
        }
        // The type entry stays (ids are indices); it just targets nothing.
        for e in &mut self.reg.endpoints {
            if e.cap == cap && e.target == EndpointTarget::Position(pos) {
                e.can_insert = false;
                e.can_extract = false;
                e.listens_messages = false;
                e.listens_signals = false;
            }
        }
        for n in self.slots[cap.index()].layer.remove_endpoint(pos) {
            self.slots[cap.index()].ind.net_changed(n);
        }
    }

    pub fn add_filter(&mut self, f: Filter) -> FilterId {
        self.reg.filters.add(f)
    }
}
