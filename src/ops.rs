//! What the exported functions (`crate::api`) do. Kept apart so the
//! exported items stay one-line calls: `yog publish exports` copies them
//! into the exports crate, which knows nothing of the engine.

use crate::transport::engine::{
    AttachDef as CoreAttach, CapabilityDef as CoreCapability, EndpointDef as CoreEndpoint,
    ModelSpec, TargetDef,
};
use crate::transport::ids::{Face, Pos};
use crate::transport::registry::{
    Conversion as CoreConversion, Indicator, IndicatorSource as CoreSource, Kind,
    KindForm as CoreForm, MergeRule as CoreMerge, Policy, Side, SideMode as CoreMode,
};

use crate::bridge;
use crate::external::ExternalDriver;
use crate::host::{self, Host};
use crate::types::{
    AttachDef, CapabilityDef, Distribution, EndpointDef, EndpointTarget, FaceInfo, IndicatorSource,
    KindDef, KindForm, Mechanic, MergeRule, NetworkInfo, PipeFace, PipeInfo, PipePayload, PipePos,
    SideDef, SideMode,
};

fn cap_id(host: &Host, name: &str) -> Result<crate::transport::ids::CapId, String> {
    host.engine
        .reg
        .cap_id(name)
        .ok_or_else(|| format!("unknown capability `{name}`"))
}

fn network_id(net: crate::transport::ids::NetId) -> u64 {
    (u64::from(net.generation) << 32) | u64::from(net.index)
}

fn mode(m: SideMode) -> CoreMode {
    match m {
        SideMode::Auto => CoreMode::Auto,
        SideMode::In => CoreMode::In,
        SideMode::Out => CoreMode::Out,
        SideMode::Both => CoreMode::Both,
        SideMode::Off => CoreMode::Off,
    }
}

fn side_mode(m: CoreMode) -> SideMode {
    match m {
        CoreMode::Auto => SideMode::Auto,
        CoreMode::In => SideMode::In,
        CoreMode::Out => SideMode::Out,
        CoreMode::Both => SideMode::Both,
        CoreMode::Off => SideMode::Off,
    }
}

// ── Registration ─────────────────────────────────────────────────────────────

pub(crate) fn register_kind(def: KindDef) -> Result<(), String> {
    host::with(|host| {
        let reg = &host.engine.reg;
        let mut conversions = Vec::with_capacity(def.conversions.len());
        for c in &def.conversions {
            conversions.push(CoreConversion {
                to: reg
                    .kinds
                    .id(&c.to)
                    .ok_or_else(|| format!("unknown kind `{}` (register it first)", c.to))?,
                num: c.num,
                den: c.den,
            });
        }
        host.engine
            .register_kind(Kind {
                name: def.name,
                form: match def.form {
                    KindForm::Discrete => CoreForm::Discrete,
                    KindForm::Continuous => CoreForm::Continuous,
                },
                merge: match def.merge {
                    MergeRule::ById => CoreMerge::ById,
                    MergeRule::ByIdAndData => CoreMerge::ByIdAndData,
                },
                unit: def.unit,
                conversions,
            })
            .map(drop)
    })?
}

pub(crate) fn register_capability(def: CapabilityDef) -> Result<(), String> {
    bridge::invalidate();
    host::with(|host| {
        let events = host.handlers.intern(def.events.as_ref());
        let model = match def.mechanic {
            Mechanic::Buffered => ModelSpec::Buffered,
            Mechanic::Routed => ModelSpec::Routed,
            Mechanic::InFlight => ModelSpec::InFlight,
            Mechanic::Flow => ModelSpec::Flow,
            Mechanic::Signal {
                channels,
                attenuation,
            } => ModelSpec::Signal {
                channels,
                attenuation,
            },
            Mechanic::External { handler, params } => ModelSpec::External {
                driver: Box::new(ExternalDriver::new(def.name.clone(), handler, params.len())),
                params,
            },
        };
        host.engine
            .register_capability(CoreCapability {
                name: def.name,
                kinds: def.kinds,
                exclusive: def.exclusive,
                state_prefix: def.state_prefix,
                indicators: def
                    .indicators
                    .into_iter()
                    .map(|i| Indicator {
                        name: i.name,
                        levels: i.levels,
                        source: match i.source {
                            IndicatorSource::Fill => CoreSource::Fill,
                            IndicatorSource::Flow => CoreSource::Flow,
                            IndicatorSource::Units => CoreSource::Units,
                            IndicatorSource::Power => CoreSource::Power,
                            IndicatorSource::External(n) => CoreSource::External(n),
                        },
                    })
                    .collect(),
                policy: match def.distribution {
                    Distribution::RoundRobin => Policy::RoundRobin,
                    Distribution::Priority => Policy::Priority,
                    Distribution::Nearest => Policy::Nearest,
                    Distribution::EvenSplit => Policy::EvenSplit,
                    Distribution::Random => Policy::Random,
                },
                events,
                model,
            })
            .map(drop)
    })?
}

pub(crate) fn attach(def: AttachDef) -> Result<(), String> {
    host::with(|host| {
        let mut params: Vec<(String, f64)> = def.params.into_iter().collect();
        params.sort_by(|a, b| a.0.cmp(&b.0));
        host.engine.attach(CoreAttach {
            block: def.block,
            cap: def.capability,
            groups: def.groups,
            params,
        })
    })?
}

pub(crate) fn register_endpoint(def: EndpointDef) -> Result<(), String> {
    bridge::invalidate();
    host::with(|host| register_endpoint_in(host, def))?
}

pub(crate) fn register_endpoint_in(host: &mut Host, def: EndpointDef) -> Result<(), String> {
    let handler = host.handlers.intern(def.handler.as_ref());
    let target = match &def.target {
        EndpointTarget::Block(b) => TargetDef::Block(b.clone()),
        EndpointTarget::Position(p) => TargetDef::Position(bridge::pos(&mut host.engine.reg, p)),
    };
    host.engine.register_endpoint(CoreEndpoint {
        cap: def.capability.clone(),
        target: target.clone(),
        kinds: def.kinds.clone(),
        can_insert: def.can_insert,
        can_extract: def.can_extract,
        listens_signals: def.listens_signals,
        listens_messages: def.listens_messages,
        groups: def.groups.clone(),
        priority: def.priority,
        max_packet: def.max_packet,
        pressure: def.pressure,
        handler,
    })?;
    if let TargetDef::Position(p) = target {
        host.positional
            .retain(|d| !(d.capability == def.capability && d.target == def.target));
        host.positional.push(def);
        // Connect it to pipes already next to it.
        host.run(None, |e, io, probe| e.place(io, probe, p));
    }
    Ok(())
}

pub(crate) fn unregister_endpoint(capability: String, pos: PipePos) -> Result<(), String> {
    host::mutate(move |host| {
        let p = bridge::pos(&mut host.engine.reg, &pos);
        let target = EndpointTarget::Position(pos);
        host.positional
            .retain(|d| !(d.capability == capability && d.target == target));
        host.run(None, |e, io, _| e.unregister_endpoint(io, &capability, p));
    });
    Ok(())
}

// ── Configuration ────────────────────────────────────────────────────────────

pub(crate) fn set_side(
    capability: String,
    pos: PipePos,
    face: PipeFace,
    side: SideDef,
) -> Result<(), String> {
    host::mutate(move |host| {
        let Ok(cap) = cap_id(host, &capability) else {
            return;
        };
        let reg = &mut host.engine.reg;
        let p = bridge::pos(reg, &pos);
        let filter = match side.filter.as_ref().map(|f| bridge::filter(reg, f)) {
            Some(Ok(f)) => Some(reg.filters.add(f)),
            Some(Err(e)) => {
                yog_api::warn!("[yog-pipes] set_side: {e}");
                return;
            }
            None => None,
        };
        let side = Side {
            mode: mode(side.mode),
            disabled: side.disabled,
            priority: side.priority,
            filter,
            color: side.color,
        };
        host.run(None, |e, io, probe| {
            e.set_side(io, probe, cap, p, bridge::core_face(face), side)
        });
    });
    Ok(())
}

pub(crate) fn set_color(capability: String, pos: PipePos, color: u8) -> Result<(), String> {
    host::mutate(move |host| {
        let Ok(cap) = cap_id(host, &capability) else {
            return;
        };
        let p = bridge::pos(&mut host.engine.reg, &pos);
        host.run(None, |e, io, probe| e.set_color(io, probe, cap, p, color));
    });
    Ok(())
}

pub(crate) fn refresh(pos: PipePos) -> Result<(), String> {
    host::mutate(move |host| {
        let p = bridge::pos(&mut host.engine.reg, &pos);
        host.run(None, |e, io, probe| e.refresh(io, probe, p));
    });
    Ok(())
}

// ── Transport ────────────────────────────────────────────────────────────────

pub(crate) fn wake(pos: PipePos) -> Result<(), String> {
    host::mutate(move |host| {
        let p = bridge::pos(&mut host.engine.reg, &pos);
        host.engine.wake(p);
    });
    Ok(())
}

pub(crate) fn emit(capability: String, pos: PipePos, channel: u8, level: u8) -> Result<(), String> {
    host::mutate(move |host| {
        let Ok(cap) = cap_id(host, &capability) else {
            return;
        };
        let p = bridge::pos(&mut host.engine.reg, &pos);
        host.run(None, |e, io, _| e.emit(io, cap, p, channel, level));
    });
    Ok(())
}

pub(crate) fn level(capability: String, pos: PipePos, channel: u8) -> Result<u8, String> {
    host::with(|host| {
        let cap = cap_id(host, &capability)?;
        let p = bridge::pos(&mut host.engine.reg, &pos);
        Ok(host.engine.level(cap, p, channel))
    })?
}

// ── Messaging ────────────────────────────────────────────────────────────────

pub(crate) fn post(
    capability: String,
    from: PipePos,
    to: PipePos,
    payload: PipePayload,
) -> Result<bool, String> {
    let run = move |host: &mut Host| -> Result<bool, String> {
        let cap = cap_id(host, &capability)?;
        let (f, t) = (
            bridge::pos(&mut host.engine.reg, &from),
            bridge::pos(&mut host.engine.reg, &to),
        );
        let bytes = bridge::pack(&payload);
        Ok(host.run(None, |e, io, _| e.post(io, cap, f, t, &bytes)))
    };
    match host::busy() {
        false => host::with(run)?,
        true => {
            host::mutate(move |host| {
                let _ = run(host);
            });
            Ok(true)
        }
    }
}

pub(crate) fn broadcast(
    capability: String,
    from: PipePos,
    payload: PipePayload,
) -> Result<u32, String> {
    let run = move |host: &mut Host| -> Result<u32, String> {
        let cap = cap_id(host, &capability)?;
        let f = bridge::pos(&mut host.engine.reg, &from);
        let bytes = bridge::pack(&payload);
        Ok(host.run(None, |e, io, _| e.broadcast(io, cap, f, &bytes)))
    };
    match host::busy() {
        false => host::with(run)?,
        true => {
            host::mutate(move |host| {
                let _ = run(host);
            });
            Ok(0)
        }
    }
}

pub(crate) fn receive(capability: String, pos: PipePos) -> Result<Option<PipePayload>, String> {
    host::with(|host| {
        let cap = cap_id(host, &capability)?;
        let p = bridge::pos(&mut host.engine.reg, &pos);
        Ok(host.engine.receive(cap, p).and_then(|b| bridge::unpack(&b)))
    })?
}

pub(crate) fn latest(capability: String, pos: PipePos) -> Result<Option<PipePayload>, String> {
    host::with(|host| {
        let cap = cap_id(host, &capability)?;
        let p = bridge::pos(&mut host.engine.reg, &pos);
        Ok(host.engine.latest(cap, p).and_then(bridge::unpack))
    })?
}

// ── Queries ──────────────────────────────────────────────────────────────────

pub(crate) fn network_info(
    capability: String,
    pos: PipePos,
) -> Result<Option<NetworkInfo>, String> {
    host::with(|host| {
        let cap = cap_id(host, &capability)?;
        let p = bridge::pos(&mut host.engine.reg, &pos);
        let reg = &host.engine.reg;
        Ok(host.engine.net_info(cap, p).map(|i| NetworkInfo {
            network: network_id(i.net),
            size: i.size,
            endpoints: i.endpoints,
            frozen: i.frozen,
            contents: i
                .totals
                .into_iter()
                .map(|(r, n)| bridge::stack(reg, r, n))
                .collect(),
        }))
    })?
}

pub(crate) fn pipe_info(pos: PipePos) -> Result<Vec<PipeInfo>, String> {
    host::with(|host| {
        let p: Pos = bridge::pos(&mut host.engine.reg, &pos);
        let e = &host.engine;
        let mut out = Vec::new();
        for (c, cap) in e.reg.caps.iter().enumerate() {
            let layer = e.layer(crate::transport::ids::CapId(c as u16));
            let Some(node) = layer.node_at(p) else {
                continue;
            };
            let n = layer.node(node);
            let faces = Face::ALL
                .into_iter()
                .map(|f| {
                    let s = layer.side(node, f);
                    FaceInfo {
                        face: bridge::face(f),
                        linked: n.links[f as usize] != crate::transport::ids::NO_NODE,
                        endpoint: layer.endpoint_at(p.offset(f)).is_some(),
                        mode: side_mode(s.mode),
                        disabled: s.disabled,
                        priority: s.priority,
                        filtered: s.filter.is_some(),
                    }
                })
                .collect();
            out.push(PipeInfo {
                capability: cap.name.clone(),
                network: layer.net_of(node).map_or(0, network_id),
                color: n.color,
                faces,
            });
        }
        out
    })
}

pub(crate) fn indication(pos: PipePos) -> Result<Vec<(String, u8)>, String> {
    host::with(|host| {
        let p = bridge::pos(&mut host.engine.reg, &pos);
        host.engine.indication(p)
    })
}
