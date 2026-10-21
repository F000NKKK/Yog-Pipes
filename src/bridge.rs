//! Calling other mods' handlers, and converting between the public types
//! (`crate::types`, by name) and the engine's interned ids.
//!
//! No callback ABI is invented here: a handler is an ordinary
//! `#[yog_export]` function of one argument, called by `(mod id, symbol)`
//! on the bus — its argument and answer as Yog Data. It's looked up on
//! each call, so a reloaded mod's new function is picked up.

use yog_api::yog_data::{self, Schema};

use crate::transport::ids::{Face, Pos, ResId, normalize_dim};
use crate::transport::model::Want as CoreWant;
use crate::transport::registry::{Filter, FilterEntry, FilterMode, Registry};

use crate::types::{
    FilterDef, FilterRule, Handler, PipeFace, PipePayload, PipePos, Resource, Stack, Want,
};

/// A handler to call.
#[derive(Debug, Clone)]
pub struct Target {
    pub handler: Handler,
}

impl Target {
    pub fn new(handler: Handler) -> Self {
        Target { handler }
    }

    /// Calls the handler with `input`: its answer, `None` when it isn't
    /// (or no longer) exported or failed.
    pub fn call<A: Schema, R: Schema>(&self, input: &A) -> Option<R> {
        yog_api::call_export(&self.handler.mod_id, &self.handler.symbol, input)
    }
}

/// A value as stored (binary Yog Data, as the calls carry it).
pub fn encode<T: Schema>(value: &T) -> Vec<u8> {
    yog_data::schema::to_bytes(value)
}

/// A stored value back; `None` if the bytes aren't one.
pub fn decode<T: Schema>(bytes: &[u8]) -> Option<T> {
    yog_data::read_bytes(bytes).ok()
}

// ── Conversions ──────────────────────────────────────────────────────────────

pub fn face(f: Face) -> PipeFace {
    match f {
        Face::Down => PipeFace::Down,
        Face::Up => PipeFace::Up,
        Face::North => PipeFace::North,
        Face::South => PipeFace::South,
        Face::West => PipeFace::West,
        Face::East => PipeFace::East,
    }
}

pub fn core_face(f: PipeFace) -> Face {
    Face::from_index(f as usize)
}

pub fn pipe_pos(reg: &Registry, p: Pos) -> PipePos {
    PipePos {
        dim: reg.dim_name(p.dim).to_string(),
        x: p.x,
        y: p.y,
        z: p.z,
    }
}

pub fn pos(reg: &mut Registry, p: &PipePos) -> Pos {
    Pos::new(reg.dim(&normalize_dim(&p.dim)), p.x, p.y, p.z)
}

pub fn resource(reg: &Registry, r: ResId) -> Resource {
    let key = reg.resources.get(r);
    Resource {
        kind: reg.kinds.get(key.kind).name.clone(),
        id: key.id.to_string(),
        data: key.data.to_vec(),
    }
}

pub fn stack(reg: &Registry, r: ResId, amount: u64) -> Stack {
    Stack {
        resource: resource(reg, r),
        amount,
    }
}

/// The engine's id of `r`; an error when its kind isn't registered.
pub fn res_id(reg: &mut Registry, r: &Resource) -> Result<ResId, String> {
    let kind = reg
        .kinds
        .id(&r.kind)
        .ok_or_else(|| format!("unknown kind `{}`", r.kind))?;
    Ok(reg.resources.intern(&reg.kinds, kind, &r.id, &r.data))
}

pub fn filter(reg: &mut Registry, f: &FilterDef) -> Result<Filter, String> {
    let mut entries = Vec::with_capacity(f.rules.len());
    for rule in &f.rules {
        entries.push(match rule {
            FilterRule::Resource(r) => FilterEntry::Resource(res_id(reg, r)?),
            FilterRule::Id(id) => FilterEntry::Id(id.as_str().into()),
            FilterRule::Kind(k) => FilterEntry::Kind(
                reg.kinds
                    .id(k)
                    .ok_or_else(|| format!("unknown kind `{k}`"))?,
            ),
            FilterRule::Color(c) => FilterEntry::Color(*c),
        });
    }
    Ok(Filter {
        mode: if f.deny {
            FilterMode::Deny
        } else {
            FilterMode::Allow
        },
        entries,
    })
}

pub fn filter_def(reg: &Registry, f: &Filter) -> FilterDef {
    FilterDef {
        deny: f.mode == FilterMode::Deny,
        rules: f
            .entries
            .iter()
            .map(|e| match e {
                FilterEntry::Resource(r) => FilterRule::Resource(resource(reg, *r)),
                FilterEntry::Id(id) => FilterRule::Id(id.to_string()),
                FilterEntry::Kind(k) => FilterRule::Kind(reg.kinds.get(*k).name.clone()),
                FilterEntry::Color(c) => FilterRule::Color(*c),
            })
            .collect(),
    }
}

pub fn want(reg: &Registry, w: CoreWant) -> Want {
    match w {
        CoreWant::Any => Want::Any,
        CoreWant::Res(r) => Want::Resource(resource(reg, r)),
        CoreWant::Filter(f) => Want::Filter(filter_def(reg, reg.filters.get(f))),
    }
}

pub fn core_want(reg: &mut Registry, w: &Want) -> Result<CoreWant, String> {
    Ok(match w {
        Want::Any => CoreWant::Any,
        Want::Resource(r) => CoreWant::Res(res_id(reg, r)?),
        Want::Filter(f) => {
            let f = filter(reg, f)?;
            CoreWant::Filter(reg.filters.add(f))
        }
    })
}

/// Pack a payload for the engine's byte-only messaging.
pub fn pack(p: &PipePayload) -> Vec<u8> {
    encode(p)
}

pub fn unpack(bytes: &[u8]) -> Option<PipePayload> {
    decode(bytes)
}
