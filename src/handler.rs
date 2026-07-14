//! Handler binding — lets a mod attach its own `#[yog_export]` function to a
//! pipe network position, so it gets called whenever a payload is delivered
//! there, instead of having to poll `transport::read` every tick.
//!
//! No new callback ABI is invented here: a bound handler is just an
//! ordinary interop export (the same C-ABI wrapper `#[yog_export]` already
//! generates for any function), resolved through `Registry::interop` and
//! invoked exactly like any other cross-mod call.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::graph::NodeKey;
use crate::payload::PipePayload;

/// Position → (mod id, exported symbol name) that should receive payloads
/// delivered to that position.
static HANDLERS: LazyLock<Mutex<HashMap<NodeKey, (String, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn bind(pos: NodeKey, mod_id: String, symbol: String) {
    HANDLERS.lock().unwrap().insert(pos, (mod_id, symbol));
}

pub fn unbind(pos: &NodeKey) {
    HANDLERS.lock().unwrap().remove(pos);
}

/// If a handler is bound at `pos`, resolve it via the interop table and
/// call it with `payload`. Silently does nothing if no handler is bound,
/// or if the bound mod/symbol isn't (yet) resolvable — the same tolerance
/// `__yog_resolve_pending_imports` already applies to ordinary imports.
pub fn dispatch(registry: &mut yog_api::Registry, pos: &NodeKey, payload: &PipePayload) {
    let Some((mod_id, symbol)) = HANDLERS.lock().unwrap().get(pos).cloned() else {
        return;
    };
    let Some(ptr) = (unsafe { registry.interop().import_raw(&mod_id, &symbol) }) else {
        return;
    };

    type WrapFn = unsafe extern "C" fn(
        input_ptr: *const u8,
        input_len: u32,
        out_data: *mut *mut u8,
        out_len: *mut u32,
        out_cap: *mut u32,
    );
    let f: WrapFn = unsafe { std::mem::transmute(ptr) };

    let Ok(aligned) = yog_api::rkyv::to_bytes::<yog_api::rkyv::rancor::Error>(payload) else {
        return;
    };
    let input_bytes: Vec<u8> = aligned.to_vec();

    let mut out_data: *mut u8 = std::ptr::null_mut();
    let mut out_len: u32 = 0;
    let mut out_cap: u32 = 0;
    unsafe {
        f(
            input_bytes.as_ptr(),
            input_bytes.len() as u32,
            &mut out_data,
            &mut out_len,
            &mut out_cap,
        );
        // The handler's return value (if any) isn't meaningful to us — we
        // only need to free the buffer the C-ABI call allocated.
        if !out_data.is_null() {
            let _ = Vec::from_raw_parts(out_data, out_len as usize, out_cap as usize);
        }
    }
}
