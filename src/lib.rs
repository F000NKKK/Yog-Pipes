//! Yog Pipes — a transport framework for Yog mods.
//!
//! A framework, not a content mod: it adds no blocks, items or recipes.
//! Mods declare their own pipe blocks as content and tell Yog-Pipes what
//! those pipes carry and how; Yog-Pipes keeps the networks in step with
//! the world, moves resources, signals and messages through them, keeps
//! every unit accounted for and saves it all with the world.
//!
//! - **One abstract pipe.** There are no item/fluid/energy pipe types. A
//!   *capability* (`register_capability`) is one thing pipes carry — named
//!   by its mod, with its resource kinds and a *mechanic* (buffered,
//!   routed, in-flight, flow, signal or one implemented by another mod).
//!   A pipe block carries any number of capabilities (`attach`), each
//!   with its own networks.
//! - **Endpoints** (`register_endpoint`) are the blocks pipes exchange
//!   with; their mod handles each insert/extract/signal/message in an
//!   ordinary `#[yog_export]` function.
//! - **Conservation.** Units move only between a network and an endpoint
//!   that accepted them; anything that has to leave otherwise (a broken
//!   pipe, an overflow) is reported to the capability's owner as an event.
//!
//! The engine itself lives in the `yog-pipes-core` crate (`core/`, no game
//! dependency); this crate is the host. See `docs/design/transport.md` and
//! `examples/` for consumer mods.

mod api;
mod bridge;
mod external;
mod host;
mod ops;
mod persist;
mod types;

use yog_api::{EventPhase, Mod, Registry, Server, info};
use yog_pipes_core::ids::Pos;

pub use api::*;
pub use types::*;

/// Explosion reach: a vanilla blast ray starts at up to 1.3 × power and
/// loses 0.225 per 0.3-block step, so it travels at most ~1.75 × power
/// blocks; 2 × power + 2 leaves margin for rounding.
fn blast_radius(power: f32) -> i32 {
    (power.max(0.0) * 2.0).ceil() as i32 + 2
}

fn world_pos(host: &mut host::Host, dim: &str, x: i32, y: i32, z: i32) -> Pos {
    let pos = PipePos {
        dim: dim.to_string(),
        x,
        y,
        z,
    };
    bridge::pos(&mut host.engine.reg, &pos)
}

pub struct YogPipesMod;

impl Mod for YogPipesMod {
    fn register(registry: &mut Registry) {
        // Keep the networks in step with the world. A block matters only
        // if some mod registered it (as a pipe or an endpoint) — never by
        // a naming convention. Changes made any other way are picked up
        // through `refresh`.
        registry.on_player_place_block(|e, phase, srv| {
            if phase == EventPhase::Post {
                let _ = host::with(|host| {
                    let known = host.engine.reg.block_id(&e.block_id).is_some();
                    let p = world_pos(host, &e.dimension, e.pos.x, e.pos.y, e.pos.z);
                    if known || host.engine.reg.positional.contains_key(&p) {
                        host.run(Some(srv), |e, io, probe| e.place(io, probe, p));
                    }
                });
            }
            true
        });

        registry.on_block_break(|e, phase, _srv| {
            if phase == EventPhase::Post {
                let _ = host::with(|host| {
                    let p = world_pos(host, &e.dimension, e.pos.x, e.pos.y, e.pos.z);
                    host.run(None, |e, io, _| e.remove(io, p));
                });
            }
            true
        });

        // An explosion removes blocks without per-block events: re-check
        // what's known in its reach.
        registry.on_explosion(|e, phase, srv| {
            if phase == EventPhase::Post {
                let _ = host::with(|host| {
                    let (x, y, z) = (e.x.floor() as i32, e.y.floor() as i32, e.z.floor() as i32);
                    let center = world_pos(host, &e.dimension, x, y, z);
                    let r = blast_radius(e.power);
                    host.run(Some(srv), |e, io, probe| {
                        e.revalidate_area(io, probe, center, r)
                    });
                });
            }
            true
        });

        registry.on_server_started(|srv| {
            bridge::invalidate();
            let _ = host::with(|host| persist::load(host, srv));
        });

        // Positions mean nothing once the world is gone: save, then don't
        // carry one world's networks into the next one opened.
        registry.on_server_stopping(|_srv| {
            let _ = host::with(|host| {
                persist::unload(host);
                for def in std::mem::take(&mut host.positional) {
                    if let EndpointTarget::Position(p) = &def.target {
                        let p = bridge::pos(&mut host.engine.reg, p);
                        host.run(None, |e, io, _| {
                            e.unregister_endpoint(io, &def.capability, p)
                        });
                    }
                }
                host.engine.clear_world();
            });
        });

        registry.on_tick(|srv: &dyn Server| {
            let _ = host::with(|host| {
                host.run(Some(srv), |e, io, probe| {
                    e.rediscover(io, probe);
                    e.tick(io);
                });
                persist::tick(host);
            });
        });

        info!("[yog-pipes] ready.");
    }
}

yog_api::export_mod!(YogPipesMod);
