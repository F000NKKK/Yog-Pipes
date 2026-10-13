//! `Mechanic::External`: a transport model implemented by another mod.
//! The engine keeps one opaque state blob per network; this driver hands
//! it to the mod's handler as an [`ExternalCall`] and applies the endpoint
//! I/O it asks for through the engine's checked `Cx` calls, then calls it
//! again with the results — so the model books exactly what moved.

use yog_pipes_core::ids::{NetId, NodeId};
use yog_pipes_core::layer::Port;
use yog_pipes_core::model::external::Driver;
use yog_pipes_core::model::{Activity, Cx};

use crate::bridge::{self, Target, call};
use crate::types::{
    ExternalActivity, ExternalCall, ExternalOp, ExternalPort, ExternalReply, ExternalResult,
    Handler, Stack,
};

/// Calls per network tick (the last one may not ask for more I/O).
const MAX_ROUNDS: usize = 4;

pub struct ExternalDriver {
    capability: String,
    params: usize,
    target: Target,
}

impl ExternalDriver {
    pub fn new(capability: String, handler: Handler, params: usize) -> Self {
        ExternalDriver {
            capability,
            params,
            target: Target::new(handler),
        }
    }

    fn call(&mut self, input: &ExternalCall) -> Option<ExternalReply> {
        let reply = call!(self.target, input, ExternalReply);
        if reply.is_none() {
            log_once(&self.capability, "external handler missing or its reply unreadable");
        }
        reply
    }
}

fn log_once(capability: &str, msg: &str) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.iter().any(|s| s == capability) {
        seen.push(capability.to_string());
        yog_api::warn!("[yog-pipes] {capability}: {msg}");
    }
}

/// Apply `ops` for network `net` (whose ports are `ports`).
fn apply(cx: &mut Cx, net: NetId, ports: &[Port], ops: Vec<ExternalOp>) -> Vec<ExternalResult> {
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        out.push(match op {
            ExternalOp::Insert { port, stack } => {
                let (Some(&p), Ok(r)) = (
                    ports.get(port as usize),
                    bridge::res_id(cx.reg, &stack.resource),
                ) else {
                    out.push(ExternalResult::Inserted(0));
                    continue;
                };
                ExternalResult::Inserted(cx.insert(p, r, stack.amount, 0, false))
            }
            ExternalOp::Extract { port, want, max } => {
                let (Some(&p), Ok(w)) = (
                    ports.get(port as usize),
                    bridge::core_want(cx.reg, &want),
                ) else {
                    out.push(ExternalResult::Extracted(None));
                    continue;
                };
                ExternalResult::Extracted(
                    cx.extract(p, w, max, false)
                        .map(|(r, n)| bridge::stack(cx.reg, r, n)),
                )
            }
            ExternalOp::Spill { at, stack } => {
                if let Ok(r) = bridge::res_id(cx.reg, &stack.resource) {
                    let at = bridge::pos(cx.reg, &at);
                    cx.spill(at, r, stack.amount);
                }
                ExternalResult::Done
            }
            ExternalOp::Indicate { slot, num, den } => {
                cx.indicate_net(net, slot, num, den);
                ExternalResult::Done
            }
        });
    }
    out
}

impl Driver for ExternalDriver {
    fn tick(&mut self, cx: &mut Cx, net: NetId, state: &mut Vec<u8>) -> Activity {
        let Some(n) = cx.layer.net(net) else {
            return Activity::Idle;
        };
        let size = n.size;
        let ports: Vec<Port> = n.ports.clone();
        let port_info: Vec<ExternalPort> = ports
            .iter()
            .map(|&p| {
                let call = cx.call(p);
                ExternalPort {
                    at: bridge::pipe_pos(cx.reg, call.at),
                    pipe: bridge::pipe_pos(cx.reg, call.pipe),
                    side: bridge::face(call.side),
                    sink: cx.sink(p),
                    source: cx.source(p),
                    priority: cx.priority(p),
                }
            })
            .collect();
        let param_min = (0..self.params)
            .map(|i| cx.layer.param_min(net, i))
            .collect();
        let param_sum = (0..self.params)
            .map(|i| cx.layer.param_sum(net, i))
            .collect();
        let mut input = ExternalCall::Tick {
            capability: self.capability.clone(),
            network: (u64::from(net.generation) << 32) | u64::from(net.index),
            size,
            param_min,
            param_sum,
            ports: port_info,
            results: Vec::new(),
            state: std::mem::take(state),
        };
        for round in 0..MAX_ROUNDS {
            let Some(reply) = self.call(&input) else {
                if let ExternalCall::Tick { state: s, .. } = input {
                    *state = s;
                }
                return Activity::Idle;
            };
            *state = reply.state;
            let activity = match reply.activity {
                ExternalActivity::Active => Activity::Active,
                ExternalActivity::Sleep(t) => Activity::Sleep(t),
                ExternalActivity::Idle => Activity::Idle,
            };
            if reply.ops.is_empty() {
                return activity;
            }
            if round + 1 == MAX_ROUNDS {
                // Running these now would move units the model never
                // hears about.
                log_once(&self.capability, "external model asked for I/O in its last round; ignored");
                return activity;
            }
            let results = apply(cx, net, &ports, reply.ops);
            if let ExternalCall::Tick {
                results: r,
                state: s,
                ..
            } = &mut input
            {
                *r = results;
                *s = state.clone();
            }
        }
        Activity::Active
    }

    fn merge(&mut self, into: &mut Vec<u8>, from: Vec<u8>) {
        let input = ExternalCall::Merge {
            capability: self.capability.clone(),
            into: into.clone(),
            from,
        };
        if let Some(reply) = self.call(&input) {
            *into = reply.state;
        }
    }

    fn split(&mut self, from: &mut Vec<u8>, sizes: &[u32]) -> Vec<Vec<u8>> {
        let input = ExternalCall::Split {
            capability: self.capability.clone(),
            state: from.clone(),
            sizes: sizes.to_vec(),
        };
        let Some(reply) = self.call(&input) else {
            return vec![Vec::new(); sizes.len()];
        };
        *from = reply.state;
        let mut parts = reply.parts;
        parts.resize(sizes.len(), Vec::new());
        parts
    }

    fn node_removed(&mut self, cx: &mut Cx, node: NodeId, state: &mut Vec<u8>) {
        let input = ExternalCall::NodeRemoved {
            capability: self.capability.clone(),
            at: bridge::pipe_pos(cx.reg, cx.layer.node(node).pos),
            state: state.clone(),
        };
        let Some(reply) = self.call(&input) else {
            return;
        };
        *state = reply.state;
        let net = cx.layer.net_of(node);
        for op in reply.ops {
            match op {
                ExternalOp::Spill {
                    at,
                    stack: Stack { resource, amount },
                } => {
                    if let Ok(r) = bridge::res_id(cx.reg, &resource) {
                        let at = bridge::pos(cx.reg, &at);
                        cx.spill(at, r, amount);
                    }
                }
                ExternalOp::Indicate { slot, num, den } => {
                    if let Some(net) = net {
                        cx.indicate_net(net, slot, num, den);
                    }
                }
                // Endpoint I/O while a pipe is being removed has no
                // results to report back.
                ExternalOp::Insert { .. } | ExternalOp::Extract { .. } => {
                    log_once(&self.capability, "endpoint I/O on node removal ignored");
                }
            }
        }
    }
}
