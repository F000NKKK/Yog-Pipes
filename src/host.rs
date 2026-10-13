//! The one engine of this server and how the game drives it: the world
//! probe (`get_block`), endpoint I/O through other mods' handlers, events
//! to capability owners — and re-entrancy.
//!
//! **Re-entrancy.** Handlers run while the engine is locked (inside a
//! tick, a placement, …), and a handler may well call Yog-Pipes back —
//! wake a network, emit a signal, post a message. Such calls are queued
//! and run right after the current engine step; queries made from inside
//! a handler fail with [`REENTRANT`] instead of deadlocking.

use std::cell::Cell;
use std::sync::{LazyLock, Mutex, MutexGuard};

use yog_api::Server;
use yog_pipes_core::engine::{Engine, Probe};
use yog_pipes_core::ids::{BlockTypeId, CapId, FxHashMap, Pos, ResId};
use yog_pipes_core::model::{Call, Event, Io, Want};
use yog_pipes_core::registry::Registry;

use crate::bridge::{self, Target, call};
use crate::types::{EndpointCall, EndpointDef, EndpointOp, EndpointReply, Handler, PipeEvent};

pub const REENTRANT: &str =
    "yog-pipes: queries aren't available from inside a pipe handler (mutations are queued)";

/// Everything Yog-Pipes keeps for the running server.
pub struct Host {
    pub engine: Engine,
    pub handlers: Handlers,
    /// Positional endpoints, by name — saved with the world (the engine
    /// keeps only this session's ids).
    pub positional: Vec<EndpointDef>,
}

/// Handlers by the engine's `u32` handle (index + 1; 0 = none).
#[derive(Default)]
pub struct Handlers {
    list: Vec<Target>,
    index: FxHashMap<Handler, u32>,
}

impl Handlers {
    pub fn intern(&mut self, h: Option<&Handler>) -> u32 {
        let Some(h) = h else {
            return 0;
        };
        if let Some(&id) = self.index.get(h) {
            return id;
        }
        self.list.push(Target::new(h.clone()));
        let id = self.list.len() as u32;
        self.index.insert(h.clone(), id);
        id
    }

    fn get(&mut self, id: u32) -> Option<&mut Target> {
        self.list.get_mut((id as usize).checked_sub(1)?)
    }
}

static HOST: LazyLock<Mutex<Host>> = LazyLock::new(|| {
    Mutex::new(Host {
        engine: Engine::new(),
        handlers: Handlers::default(),
        positional: Vec::new(),
    })
});

type Deferred = Box<dyn FnOnce(&mut Host) + Send>;
static DEFERRED: Mutex<Vec<Deferred>> = Mutex::new(Vec::new());

thread_local! {
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether this thread is inside an engine step (running a handler).
pub fn busy() -> bool {
    BUSY.get()
}

/// Run `f` on the host, then everything handlers queued meanwhile. Fails
/// with [`REENTRANT`] when called from inside a handler.
pub fn with<R>(f: impl FnOnce(&mut Host) -> R) -> Result<R, String> {
    if BUSY.get() {
        return Err(REENTRANT.into());
    }
    let mut host = lock(&HOST);
    BUSY.set(true);
    let r = f(&mut host);
    loop {
        let queued = std::mem::take(&mut *lock(&DEFERRED));
        if queued.is_empty() {
            break;
        }
        for q in queued {
            q(&mut host);
        }
    }
    BUSY.set(false);
    Ok(r)
}

/// Run `f` now, or — from inside a handler — right after the current
/// engine step.
pub fn mutate(f: impl FnOnce(&mut Host) + Send + 'static) {
    if BUSY.get() {
        lock(&DEFERRED).push(Box::new(f));
    } else {
        let _ = with(f);
    }
}

impl Host {
    /// Run `f` with the engine, endpoint I/O and a probe of the live world
    /// (`srv`, else the global server handle; without one every position
    /// reads as unknown).
    pub fn run<R>(
        &mut self,
        srv: Option<&dyn Server>,
        f: impl FnOnce(&mut Engine, &mut HostIo, &mut WorldProbe) -> R,
    ) -> R {
        let global = if srv.is_none() {
            yog_api::server()
        } else {
            None
        };
        let srv = srv.or(global.as_ref().map(|s| s as &dyn Server));
        let mut io = HostIo {
            handlers: &mut self.handlers,
        };
        let mut probe = WorldProbe { srv };
        f(&mut self.engine, &mut io, &mut probe)
    }
}

// ── World probe ──────────────────────────────────────────────────────────────

pub struct WorldProbe<'a> {
    srv: Option<&'a dyn Server>,
}

impl Probe for WorldProbe<'_> {
    fn block(&mut self, reg: &Registry, pos: Pos) -> Option<BlockTypeId> {
        let id = self.srv?.get_block(
            reg.dim_name(pos.dim),
            yog_api::BlockPos {
                x: pos.x,
                y: pos.y,
                z: pos.z,
            },
        )?;
        reg.block_id(&id)
    }
}

// ── Endpoint I/O ─────────────────────────────────────────────────────────────

pub struct HostIo<'a> {
    handlers: &'a mut Handlers,
}

impl HostIo<'_> {
    fn endpoint(&mut self, reg: &Registry, call: &Call, op: EndpointOp) -> Option<EndpointReply> {
        let target = self.handlers.get(call.handler)?;
        let input = EndpointCall {
            capability: reg.cap(call.cap).name.clone(),
            at: bridge::pipe_pos(reg, call.at),
            side: bridge::face(call.side),
            pipe: bridge::pipe_pos(reg, call.pipe),
            op,
        };
        call!(target, &input, EndpointReply)
    }

    fn notify(&mut self, reg: &Registry, cap: CapId, event: PipeEvent) {
        if let Some(target) = self.handlers.get(reg.cap(cap).events) {
            let _: Option<()> = call!(target, &event, ());
        }
    }
}

impl Io for HostIo<'_> {
    fn insert(
        &mut self,
        reg: &Registry,
        call: &Call,
        res: ResId,
        amount: u64,
        simulate: bool,
    ) -> u64 {
        let op = EndpointOp::Insert {
            stack: bridge::stack(reg, res, amount),
            simulate,
        };
        match self.endpoint(reg, call, op) {
            Some(EndpointReply::Accepted(n)) => n,
            _ => 0,
        }
    }

    fn extract(
        &mut self,
        reg: &mut Registry,
        call: &Call,
        want: Want,
        max: u64,
        simulate: bool,
    ) -> Option<(ResId, u64)> {
        let op = EndpointOp::Extract {
            want: bridge::want(reg, want),
            max,
            simulate,
        };
        let Some(EndpointReply::Gave(Some(stack))) = self.endpoint(reg, call, op) else {
            return None;
        };
        match bridge::res_id(reg, &stack.resource) {
            Ok(r) => Some((r, stack.amount)),
            Err(e) => {
                yog_api::warn!(
                    "[yog-pipes] endpoint at {:?} gave {} of an unregistered kind: {e}",
                    call.at,
                    stack.amount
                );
                None
            }
        }
    }

    fn signal(&mut self, reg: &Registry, call: &Call, channel: u8, level: u8) {
        self.endpoint(reg, call, EndpointOp::Signal { channel, level });
    }

    fn message(&mut self, reg: &Registry, call: &Call, from: Pos, payload: &[u8]) -> bool {
        let Some(payload) = bridge::unpack(payload) else {
            return false;
        };
        let op = EndpointOp::Message {
            from: bridge::pipe_pos(reg, from),
            payload,
        };
        matches!(
            self.endpoint(reg, call, op),
            Some(EndpointReply::Handled(true))
        )
    }

    fn event(&mut self, reg: &Registry, cap: CapId, event: &Event) {
        let capability = reg.cap(cap).name.clone();
        let event = match *event {
            Event::Spilled { at, res, amount } => PipeEvent::Spilled {
                capability,
                at: bridge::pipe_pos(reg, at),
                stack: bridge::stack(reg, res, amount),
            },
            Event::Lost { at, res, amount } => PipeEvent::Lost {
                capability,
                at: bridge::pipe_pos(reg, at),
                stack: bridge::stack(reg, res, amount),
            },
            Event::Overload { at, amount, limit } => PipeEvent::Overload {
                capability,
                at: bridge::pipe_pos(reg, at),
                amount,
                limit,
            },
        };
        self.notify(reg, cap, event);
    }

    fn indicate(&mut self, reg: &Registry, cap: CapId, at: Pos, index: u8, value: u8) {
        // The loader can't set arbitrary block-state properties of mod
        // blocks yet; until it can, indicator changes go to the owner as
        // events (and `indication(pos)` answers queries).
        let c = reg.cap(cap);
        let Some(ind) = c.indicators.get(index as usize) else {
            return;
        };
        let event = PipeEvent::Indicator {
            capability: c.name.clone(),
            at: bridge::pipe_pos(reg, at),
            property: format!("{}_{}", c.state_prefix, ind.name),
            value,
        };
        self.notify(reg, cap, event);
    }
}
