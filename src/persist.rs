//! Saving pipe state with the world, so it survives a server restart.
//!
//! What is saved: every stored payload (`crate::graph`) and every handler
//! binding (`crate::handler`). The topology itself is not — it's rebuilt
//! from the live world anyway (see `graph.rs`).
//!
//! **Handler bindings are saved as names, not code.** A binding is just the
//! `(mod id, export symbol)` pair a mod passed to `bind_handler`; the
//! function behind it is resolved through the interop table on every
//! dispatch, so after a restart it points at whatever the (re)loaded mod
//! exports under that name. Bindings whose mod is gone stay inert, the same
//! way an unresolvable binding always has. Mods may still bind again from
//! `on_server_started` — a binding made in this session always wins over
//! the saved one.
//!
//! **Nothing saved is ever thrown away on load.** A payload whose pipe was
//! removed while the server was down is restored anyway and dropped only
//! when the graph next sees that position isn't a pipe — exactly as if the
//! pipe had been broken in-game. Yog-Pipes can't turn a payload back into
//! items itself: it never knows what the bytes mean.
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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use crate::graph::{lock, NodeKey};
use crate::payload::PipePayload;

const MOD_ID: &str = "yog-pipes";
/// `Storage` key holding the encoded [`Snapshot`].
const STATE_KEY: &str = "state";
/// Where a saved state this version can't decode is set aside.
const UNREADABLE_KEY: &str = "state.unreadable";
/// Bumped whenever the encoding below changes; older blobs are refused, not
/// misread.
const FORMAT_VERSION: u8 = 1;
/// Same cadence as vanilla's autosave (5 minutes).
const AUTOSAVE_TICKS: u32 = 6000;

/// Where the current world's state lives, and the bytes last written there
/// (to skip rewriting an unchanged file). `None` while no world is open or
/// its directory couldn't be found.
static TARGET: Mutex<Option<(PathBuf, Vec<u8>)>> = Mutex::new(None);
static TICKS: AtomicU32 = AtomicU32::new(0);

/// Everything persisted for one world, sorted by position so equal state
/// always encodes to equal bytes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub payloads: Vec<(NodeKey, PipePayload)>,
    pub handlers: Vec<(NodeKey, (String, String))>,
}

impl Snapshot {
    fn capture() -> Self {
        let mut payloads = crate::graph::payloads_snapshot();
        payloads.sort_by(|a, b| a.0.cmp(&b.0));
        let mut handlers = crate::handler::snapshot();
        handlers.sort_by(|a, b| a.0.cmp(&b.0));
        Snapshot { payloads, handlers }
    }
}

/// The world was loaded: find its directory and put its saved state back.
pub fn load(srv: &dyn yog_api::Server) {
    TICKS.store(0, Ordering::Relaxed);
    let Some(dir) = find_world_dir(Path::new(&srv.game_dir())) else {
        yog_api::warn!("[yog-pipes] world directory not found; pipe state won't be saved");
        *lock(&TARGET) = None;
        return;
    };
    let mut store = yog_api::Storage::open(&dir.to_string_lossy(), MOD_ID);
    let saved = store.get_bytes(STATE_KEY).map(<[u8]>::to_vec);
    let mut last = Vec::new();
    match saved.as_deref().map(decode) {
        Some(Ok(snap)) => {
            last = saved.clone().unwrap_or_default();
            let (p, h) = (snap.payloads.len(), snap.handlers.len());
            crate::graph::restore_payloads(snap.payloads);
            crate::handler::restore(snap.handlers);
            yog_api::info!("[yog-pipes] restored {p} payload(s) and {h} handler binding(s)");
        }
        // Keep the blob aside instead of overwriting it with this session's
        // state, so it can still be recovered (e.g. by a newer Yog-Pipes).
        Some(Err(e)) => {
            yog_api::warn!(
                "[yog-pipes] saved state in {} is unreadable ({e}); kept as `{UNREADABLE_KEY}`",
                store.path().display()
            );
            store.set(UNREADABLE_KEY, saved.clone().unwrap_or_default());
            store.remove(STATE_KEY);
            if let Err(e) = store.flush() {
                yog_api::error!("[yog-pipes] failed to set unreadable state aside: {e}");
            }
        }
        None => {}
    }
    *lock(&TARGET) = Some((dir, last));
}

/// Write the current state if it changed since the last write.
pub fn save() {
    let mut target = lock(&TARGET);
    let Some((dir, last)) = target.as_mut() else {
        return;
    };
    let bytes = encode(&Snapshot::capture());
    if bytes == *last {
        return;
    }
    let mut store = yog_api::Storage::open(&dir.to_string_lossy(), MOD_ID);
    store.set(STATE_KEY, bytes.clone());
    match store.flush() {
        Ok(()) => *last = bytes,
        Err(e) => yog_api::error!(
            "[yog-pipes] failed to save pipe state to {}: {e}",
            store.path().display()
        ),
    }
}

/// Called every server tick; saves every [`AUTOSAVE_TICKS`] so a crash
/// loses at most that much.
pub fn tick() {
    if TICKS.fetch_add(1, Ordering::Relaxed) + 1 >= AUTOSAVE_TICKS {
        TICKS.store(0, Ordering::Relaxed);
        save();
    }
}

/// The server is stopping: save, then stop tracking this world.
pub fn unload() {
    save();
    *lock(&TARGET) = None;
}

/// The save directory of the world currently open under `game_dir`: among
/// `<game_dir>/saves/*` (singleplayer) and `<game_dir>/*` (dedicated
/// server), the one whose `session.lock` was written last.
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

// ── Encoding ─────────────────────────────────────────────────────────────────
//
// A small explicit format rather than rkyv, so a later change to
// `PipePayload` can't silently reinterpret old saves: a version byte, then
// two length-prefixed lists. Integers are little-endian; strings and byte
// strings are a u32 length followed by the bytes.

pub fn encode(snap: &Snapshot) -> Vec<u8> {
    let mut out = vec![FORMAT_VERSION];
    put_u32(&mut out, snap.payloads.len());
    for (pos, payload) in &snap.payloads {
        put_pos(&mut out, pos);
        put_bytes(&mut out, &payload.data);
        put_u32(&mut out, payload.metadata.len());
        for (k, v) in &payload.metadata {
            put_bytes(&mut out, k.as_bytes());
            put_bytes(&mut out, v.as_bytes());
        }
    }
    put_u32(&mut out, snap.handlers.len());
    for (pos, (mod_id, symbol)) in &snap.handlers {
        put_pos(&mut out, pos);
        put_bytes(&mut out, mod_id.as_bytes());
        put_bytes(&mut out, symbol.as_bytes());
    }
    out
}

pub fn decode(bytes: &[u8]) -> Result<Snapshot, String> {
    let mut r = Reader(bytes);
    let version = r.take(1)?[0];
    if version != FORMAT_VERSION {
        return Err(format!("unsupported format version {version}"));
    }
    let mut snap = Snapshot::default();
    for _ in 0..r.u32()? {
        let pos = r.pos()?;
        let data = r.bytes()?.to_vec();
        let mut metadata = Vec::new();
        for _ in 0..r.u32()? {
            metadata.push((r.string()?, r.string()?));
        }
        snap.payloads.push((pos, PipePayload { data, metadata }));
    }
    for _ in 0..r.u32()? {
        let pos = r.pos()?;
        snap.handlers.push((pos, (r.string()?, r.string()?)));
    }
    if !r.0.is_empty() {
        return Err(format!("{} trailing byte(s)", r.0.len()));
    }
    Ok(snap)
}

fn put_u32(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_u32(out, b.len());
    out.extend_from_slice(b);
}

fn put_pos(out: &mut Vec<u8>, pos: &NodeKey) {
    put_bytes(out, pos.0.as_bytes());
    for c in [pos.1, pos.2, pos.3] {
        out.extend_from_slice(&c.to_le_bytes());
    }
}

/// Cursor over the encoded bytes; every read checks the length first, so
/// truncated or corrupt input is an error, never a panic.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("unexpected end of data".into());
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    fn string(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|e| e.to_string())
    }

    fn pos(&mut self) -> Result<NodeKey, String> {
        Ok((self.string()?, self.i32()?, self.i32()?, self.i32()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            payloads: vec![
                (
                    ("minecraft:overworld".into(), 0, 64, 0),
                    PipePayload {
                        data: vec![0, 1, 2, 255],
                        metadata: vec![
                            ("kind".into(), "item".into()),
                            ("count".into(), "64".into()),
                        ],
                    },
                ),
                (
                    ("mymod:void/ünïcode".into(), -30_000_000, -64, i32::MAX),
                    PipePayload::default(),
                ),
            ],
            handlers: vec![(
                ("minecraft:the_nether".into(), 4, 70, -4),
                ("my-mod".into(), "on_pipe_payload".into()),
            )],
        }
    }

    #[test]
    fn round_trips() {
        let snap = sample();
        assert_eq!(decode(&encode(&snap)), Ok(snap));
        assert_eq!(
            decode(&encode(&Snapshot::default())),
            Ok(Snapshot::default())
        );
    }

    #[test]
    fn rejects_damaged_data() {
        let bytes = encode(&sample());
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len]).is_err(), "truncated at {len}");
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        let mut versioned = bytes;
        versioned[0] = FORMAT_VERSION + 1;
        assert!(decode(&versioned).is_err());
        // A huge length prefix must fail cleanly, not try to allocate.
        assert!(decode(&[FORMAT_VERSION, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("yog-pipes-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_through_storage() {
        let dir = scratch_dir("storage");
        let bytes = encode(&sample());
        let root = dir.to_string_lossy();
        let mut store = yog_api::Storage::open(&root, MOD_ID);
        store.set(STATE_KEY, bytes.clone());
        store.flush().unwrap();
        drop(store);
        let store = yog_api::Storage::open(&root, MOD_ID);
        assert_eq!(store.get_bytes(STATE_KEY), Some(&bytes[..]));
        assert_eq!(decode(store.get_bytes(STATE_KEY).unwrap()), Ok(sample()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_most_recently_opened_world() {
        use std::time::{Duration, SystemTime};
        let game = scratch_dir("worlds");
        let lock_at = |dir: &Path, age_secs: u64| {
            std::fs::create_dir_all(dir).unwrap();
            let f = std::fs::File::create(dir.join("session.lock")).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
                .unwrap();
        };
        assert_eq!(find_world_dir(&game), None);
        lock_at(&game.join("saves/Old World"), 600);
        lock_at(&game.join("saves/New World"), 10);
        std::fs::create_dir_all(game.join("logs")).unwrap();
        assert_eq!(find_world_dir(&game), Some(game.join("saves/New World")));
        // A dedicated server keeps its world right in the game directory.
        lock_at(&game.join("world"), 0);
        assert_eq!(find_world_dir(&game), Some(game.join("world")));
        let _ = std::fs::remove_dir_all(&game);
    }
}
