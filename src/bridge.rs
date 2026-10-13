//! Calling other mods' handlers, and converting between the public types
//! (`crate::types`, by name) and the engine's interned ids.
//!
//! No callback ABI is invented here: a handler is an ordinary
//! `#[yog_export]` function, resolved by `(mod id, symbol)` through the
//! interop table and called through the C-ABI wrapper `#[yog_export]`
//! generates, with rkyv-encoded input and output. Resolved pointers are
//! cached until any registration changes ([`invalidate`]), so a reloaded
//! mod's new function is picked up.

use std::sync::atomic::{AtomicU32, Ordering};

use yog_api::rkyv;
use yog_pipes_core::ids::{Face, Pos, ResId, normalize_dim};
use yog_pipes_core::model::Want as CoreWant;
use yog_pipes_core::registry::{Filter, FilterEntry, FilterMode, Registry};

use crate::types::{
    FilterDef, FilterRule, Handler, PipeFace, PipePayload, PipePos, Resource, Stack, Want,
};

type WrapFn = unsafe extern "C" fn(
    input_ptr: *const u8,
    input_len: u32,
    out_data: *mut *mut u8,
    out_len: *mut u32,
    out_cap: *mut u32,
);

/// Bumped by [`invalidate`]; a cached pointer from an older generation is
/// resolved again.
static GENERATION: AtomicU32 = AtomicU32::new(1);

/// Drop every cached handler pointer (a mod registered or reloaded).
pub fn invalidate() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// A handler with its resolved function, if any.
#[derive(Debug, Clone)]
pub struct Target {
    pub handler: Handler,
    cached: Option<(u32, usize)>,
}

impl Target {
    pub fn new(handler: Handler) -> Self {
        Target {
            handler,
            cached: None,
        }
    }

    fn resolve(&mut self) -> Option<WrapFn> {
        let generation = GENERATION.load(Ordering::Relaxed);
        if let Some((g, ptr)) = self.cached
            && g == generation
        {
            // SAFETY: only pointers from `import_raw` of a `#[yog_export]`
            // wrapper are cached, and they all have this signature.
            return Some(unsafe { std::mem::transmute::<usize, WrapFn>(ptr) });
        }
        // SAFETY: the API pointer is the runtime's table, captured when
        // this mod was registered.
        let registry = unsafe {
            yog_api::Registry::from_raw(yog_api::__current_api_ptr() as *const yog_api::YogApi)
        };
        let ptr = unsafe {
            registry
                .interop()
                .import_raw(&self.handler.mod_id, &self.handler.symbol)
        }?;
        self.cached = Some((generation, ptr as usize));
        // SAFETY: as above.
        Some(unsafe { std::mem::transmute::<*const std::ffi::c_void, WrapFn>(ptr) })
    }

    /// Call the handler with rkyv-encoded `input`; its encoded output, or
    /// `None` when the handler isn't (or no longer) exported.
    pub fn invoke(&mut self, input: &[u8]) -> Option<Vec<u8>> {
        let f = self.resolve()?;
        let mut out_data: *mut u8 = std::ptr::null_mut();
        let mut out_len: u32 = 0;
        let mut out_cap: u32 = 0;
        // SAFETY: `f` is a `#[yog_export]` wrapper; it hands back a buffer
        // allocated as a `Vec<u8>` whose ownership passes to us.
        unsafe {
            f(
                input.as_ptr(),
                input.len() as u32,
                &mut out_data,
                &mut out_len,
                &mut out_cap,
            );
            if out_data.is_null() {
                return Some(Vec::new());
            }
            Some(Vec::from_raw_parts(
                out_data,
                out_len as usize,
                out_cap as usize,
            ))
        }
    }
}

/// Call `target` with `input` and decode its reply as `$out`; `None` when
/// the handler is missing or the reply doesn't decode.
macro_rules! call {
    ($target:expr, $input:expr, $out:ty) => {{
        match ::yog_api::rkyv::to_bytes::<::yog_api::rkyv::rancor::Error>($input) {
            Ok(bytes) => $target.invoke(&bytes).and_then(|out| {
                let mut aligned = ::yog_api::rkyv::util::AlignedVec::<16>::new();
                aligned.extend_from_slice(&out);
                ::yog_api::rkyv::from_bytes::<$out, ::yog_api::rkyv::rancor::Error>(&aligned).ok()
            }),
            Err(_) => None,
        }
    }};
}
pub(crate) use call;

/// Encode a value for storage (rkyv, like the interop calls).
pub fn encode<T>(value: &T) -> Vec<u8>
where
    T: for<'a> rkyv::Serialize<
            rkyv::api::high::HighSerializer<
                rkyv::util::AlignedVec,
                rkyv::ser::allocator::ArenaHandle<'a>,
                rkyv::rancor::Error,
            >,
        >,
{
    rkyv::to_bytes::<rkyv::rancor::Error>(value)
        .map(|b| b.to_vec())
        .unwrap_or_default()
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
            FilterRule::Colour(c) => FilterEntry::Colour(*c),
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
                FilterEntry::Colour(c) => FilterRule::Colour(*c),
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
    let mut aligned = rkyv::util::AlignedVec::<16>::new();
    aligned.extend_from_slice(bytes);
    rkyv::from_bytes::<PipePayload, rkyv::rancor::Error>(&aligned).ok()
}
