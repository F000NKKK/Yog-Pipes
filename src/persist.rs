//! Saving transport state with the world.
//!
//! What is saved: the engine's state (`Engine::save` — what every network
//! holds, in-flight units, side configuration, colours, signal emitters,
//! inboxes, by position and by name) and the positional endpoints mods
//! registered (by name, with their handler names). The topology is not:
//! it's rediscovered from the world, starting at saved positions.
//!
//! **Nothing saved is thrown away on load.** State whose pipe or
//! capability is missing (a pipe removed while the server was down, a mod
//! not installed this time) stays parked in the engine and is saved again
//! unchanged.
//!
//! Where: the yog-api exposes no world-save hook and no world directory,
//! only `Server::game_dir` (shared by every singleplayer world). The world
//! being played is found next to it as the save holding the newest
//! `session.lock` — Minecraft rewrites that file when it opens a world —
//! and the state goes into a `yog_api::Storage` rooted there
//! (`<world>/yog-data/yog-pipes/global.kv`), so it travels with the save.
//! It's written on server stop and every [`AUTOSAVE_TICKS`] in between if
//! anything changed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use yog_api::rkyv;

use crate::host::Host;
use crate::types::EndpointDef;

const MOD_ID: &str = "yog-pipes";
/// `Storage` key of the engine state.
const ENGINE_KEY: &str = "transport";
/// `Storage` key of the positional endpoints.
const ENDPOINTS_KEY: &str = "endpoints";
/// Where saved state this version can't read is set aside.
const UNREADABLE_SUFFIX: &str = ".unreadable";
/// Same cadence as vanilla's autosave (5 minutes).
const AUTOSAVE_TICKS: u32 = 6000;

/// Where the current world's state lives and the bytes last written there
/// (to skip rewriting unchanged state). `None` while no world is open or
/// its directory couldn't be found.
static TARGET: Mutex<Option<(PathBuf, Vec<u8>, Vec<u8>)>> = Mutex::new(None);
static TICKS: AtomicU32 = AtomicU32::new(0);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_aside(store: &mut yog_api::Storage, key: &str, bytes: Vec<u8>, err: &str) {
    yog_api::warn!(
        "[yog-pipes] saved `{key}` in {} is unreadable ({err}); kept as `{key}{UNREADABLE_SUFFIX}`",
        store.path().display()
    );
    store.set(&format!("{key}{UNREADABLE_SUFFIX}"), bytes);
    store.remove(key);
    if let Err(e) = store.flush() {
        yog_api::error!("[yog-pipes] failed to set unreadable state aside: {e}");
    }
}

fn decode_endpoints(bytes: &[u8]) -> Result<Vec<EndpointDef>, String> {
    let mut aligned = rkyv::util::AlignedVec::<16>::new();
    aligned.extend_from_slice(bytes);
    rkyv::from_bytes::<Vec<EndpointDef>, rkyv::rancor::Error>(&aligned).map_err(|e| e.to_string())
}

/// Load the world's state into `host` (called on server start).
pub fn load(host: &mut Host, srv: &dyn yog_api::Server) {
    TICKS.store(0, Ordering::Relaxed);
    let Some(dir) = find_world_dir(Path::new(&srv.game_dir())) else {
        yog_api::warn!("[yog-pipes] world directory not found; pipe state won't be saved");
        *lock(&TARGET) = None;
        return;
    };
    let mut store = yog_api::Storage::open(&dir.to_string_lossy(), MOD_ID);
    let engine = store.get_bytes(ENGINE_KEY).map(<[u8]>::to_vec);
    let endpoints = store.get_bytes(ENDPOINTS_KEY).map(<[u8]>::to_vec);
    let (mut last_engine, mut last_endpoints) = (Vec::new(), Vec::new());
    if let Some(bytes) = engine {
        match host.engine.load(&bytes) {
            Ok(()) => last_engine = bytes,
            Err(e) => set_aside(&mut store, ENGINE_KEY, bytes, &e),
        }
    }
    if let Some(bytes) = endpoints {
        match decode_endpoints(&bytes) {
            Ok(defs) => {
                for def in defs {
                    if let Err(e) = crate::api::register_endpoint_in(host, def.clone()) {
                        // Its capability isn't registered this session:
                        // keep it for the next save anyway.
                        yog_api::warn!("[yog-pipes] saved endpoint kept but inactive: {e}");
                        host.positional.push(def);
                    }
                }
                last_endpoints = bytes;
            }
            Err(e) => set_aside(&mut store, ENDPOINTS_KEY, bytes, &e),
        }
    }
    yog_api::info!(
        "[yog-pipes] restored state ({} parked record(s), {} positional endpoint(s))",
        host.engine.parked_records(),
        host.positional.len()
    );
    *lock(&TARGET) = Some((dir, last_engine, last_endpoints));
}

pub fn save(host: &Host) {
    let mut target = lock(&TARGET);
    let Some((dir, last_engine, last_endpoints)) = target.as_mut() else {
        return;
    };
    let engine = host.engine.save();
    let endpoints = crate::bridge::encode(&host.positional);
    if engine == *last_engine && endpoints == *last_endpoints {
        return;
    }
    let mut store = yog_api::Storage::open(&dir.to_string_lossy(), MOD_ID);
    store.set(ENGINE_KEY, engine.clone());
    store.set(ENDPOINTS_KEY, endpoints.clone());
    match store.flush() {
        Ok(()) => {
            *last_engine = engine;
            *last_endpoints = endpoints;
        }
        Err(e) => yog_api::error!(
            "[yog-pipes] failed to save pipe state to {}: {e}",
            store.path().display()
        ),
    }
}

pub fn tick(host: &Host) {
    if TICKS.fetch_add(1, Ordering::Relaxed) + 1 >= AUTOSAVE_TICKS {
        TICKS.store(0, Ordering::Relaxed);
        save(host);
    }
}

/// Save, then forget the world (server stopping).
pub fn unload(host: &Host) {
    save(host);
    *lock(&TARGET) = None;
}

/// The save directory of the world being played: the one under `game_dir`
/// (or `game_dir/saves`) with the newest `session.lock`.
pub fn find_world_dir(game_dir: &Path) -> Option<PathBuf> {
    [game_dir.join("saves"), game_dir.to_path_buf()]
        .iter()
        .filter_map(|parent| std::fs::read_dir(parent).ok())
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let modified = std::fs::metadata(dir.join("session.lock"))
                .and_then(|m| m.modified())
                .ok()?;
            Some((modified, dir))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, dir)| dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_dir_is_newest_session_lock() {
        let root = std::env::temp_dir().join(format!("yog-pipes-test-{}", std::process::id()));
        let saves = root.join("saves");
        for (name, delay) in [("old", 0), ("new", 20)] {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            let d = saves.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("session.lock"), b"x").unwrap();
        }
        assert_eq!(find_world_dir(&root), Some(saves.join("new")));
        std::fs::remove_dir_all(&root).ok();
    }
}
