//! Saving transport state with the world.
//!
//! What is saved: the engine's state (`Engine::save` — what every network
//! holds, in-flight units, side configuration, colors, signal emitters,
//! inboxes, by position and by name) and the positional endpoints mods
//! registered (by name, with their handler names). The topology is not:
//! it's rediscovered from the world, starting at saved positions.
//!
//! **Nothing saved is thrown away on load.** State whose pipe or
//! capability is missing (a pipe removed while the server was down, a mod
//! not installed this time) stays parked in the engine and is saved again
//! unchanged.
//!
//! Where: the mod's world store (`yog_api::storage::world`), so it travels
//! with the save. It's put there as the world is saved (and before the
//! world is cleared as the server stops); the world's save writes it.

use std::sync::Mutex;

use yog_api::storage::Store;
use yog_api::yog_data::Value;

use crate::host::Host;
use crate::types::EndpointDef;

/// Where the engine state and the positional endpoints are kept in the world store.
const ENGINE_KEY: &str = "transport";
const ENDPOINTS_KEY: &str = "endpoints";
/// Where saved state this version can't read is set aside.
const UNREADABLE_SUFFIX: &str = ".unreadable";

/// The bytes last kept (unchanged state isn't put again): engine, endpoints.
/// `None` while no world is open.
static LAST: Mutex<Option<(Vec<u8>, Vec<u8>)>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The bytes kept at `key`.
fn bytes_at(store: &Store, key: &str) -> Option<Vec<u8>> {
    match store.get::<Value>(key)? {
        Value::Bytes(b) => Some(b),
        _ => None,
    }
}

fn set_aside(store: &Store, key: &str, bytes: Vec<u8>, err: &str) {
    yog_api::warn!(
        "[yog-pipes] saved `{key}` is unreadable ({err}); kept as `{key}{UNREADABLE_SUFFIX}`"
    );
    store.set(&format!("{key}{UNREADABLE_SUFFIX}"), &Value::Bytes(bytes));
    store.remove(key);
}

fn decode_endpoints(bytes: &[u8]) -> Result<Vec<EndpointDef>, String> {
    crate::bridge::decode(bytes).ok_or_else(|| "not a list of endpoints".to_string())
}

/// Load the open world's state into `host` (called on server start).
pub fn load(host: &mut Host) {
    let Some(store) = yog_api::storage::world() else {
        return;
    };
    let (mut last_engine, mut last_endpoints) = (Vec::new(), Vec::new());
    if let Some(bytes) = bytes_at(&store, ENGINE_KEY) {
        match host.engine.load(&bytes) {
            Ok(()) => last_engine = bytes,
            Err(e) => set_aside(&store, ENGINE_KEY, bytes, &e),
        }
    }
    if let Some(bytes) = bytes_at(&store, ENDPOINTS_KEY) {
        match decode_endpoints(&bytes) {
            Ok(defs) => {
                for def in defs {
                    if let Err(e) = crate::ops::register_endpoint_in(host, def.clone()) {
                        // Its capability isn't registered this session:
                        // keep it for the next save anyway.
                        yog_api::warn!("[yog-pipes] saved endpoint kept but inactive: {e}");
                        host.positional.push(def);
                    }
                }
                last_endpoints = bytes;
            }
            Err(e) => set_aside(&store, ENDPOINTS_KEY, bytes, &e),
        }
    }
    yog_api::info!(
        "[yog-pipes] restored state ({} parked record(s), {} positional endpoint(s))",
        host.engine.parked_records(),
        host.positional.len()
    );
    *lock(&LAST) = Some((last_engine, last_endpoints));
}

/// Puts the state in the world store if it changed (the world's save writes it).
pub fn save(host: &Host) {
    let mut last = lock(&LAST);
    let Some((last_engine, last_endpoints)) = last.as_mut() else {
        return;
    };
    let Some(store) = yog_api::storage::world() else {
        return;
    };
    let engine = host.engine.save();
    let endpoints = crate::bridge::encode(&host.positional);
    if engine == *last_engine && endpoints == *last_endpoints {
        return;
    }
    store.set(ENGINE_KEY, &Value::Bytes(engine.clone()));
    store.set(ENDPOINTS_KEY, &Value::Bytes(endpoints.clone()));
    *last_engine = engine;
    *last_endpoints = endpoints;
}

/// Puts the state in the store, then forgets the world (server stopping:
/// its last save writes it).
pub fn unload(host: &Host) {
    save(host);
    *lock(&LAST) = None;
}
