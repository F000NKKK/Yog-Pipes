//! Integer ids, positions, faces, hashing and interning — the vocabulary
//! every hot path uses instead of strings.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hash, Hasher};

// ── Hashing ──────────────────────────────────────────────────────────────────

/// The Fx hash (rustc's): one multiply per word. Keys here are small
/// integers and positions, never attacker-chosen strings, so SipHash's
/// DoS resistance buys nothing and costs ~4× per lookup.
#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(buf));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = HashMap<K, V, FxBuild>;
pub type FxHashSet<K> = HashSet<K, FxBuild>;

// ── Typed ids ────────────────────────────────────────────────────────────────

macro_rules! id {
    ($(#[$m:meta])* $name:ident($t:ty)) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub $t);
        impl $name {
            #[inline]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id!(
    /// An interned dimension id (`minecraft:overworld`).
    DimId(u16)
);
id!(
    /// A registered resource kind (`ns:fe`).
    KindId(u16)
);
id!(
    /// An interned resource (kind + id + data).
    ResId(u32)
);
id!(
    /// A registered capability; also the index of its layer.
    CapId(u16)
);
id!(
    /// A block id known to the engine (it carries capabilities and/or
    /// endpoint types).
    BlockTypeId(u32)
);
id!(
    /// A registered endpoint type (by block or by position).
    EpTypeId(u32)
);
id!(
    /// A node of one layer: an index into that layer's node slab.
    NodeId(u32)
);
id!(
    /// A live endpoint instance of one layer.
    EpId(u32)
);
id!(
    /// A filter registered with the engine.
    FilterId(u32)
);

/// "No node" in an adjacency slot.
pub const NO_NODE: NodeId = NodeId(u32::MAX);

/// A network of one layer: slab index plus generation, so an id cached by a
/// model or a route never aliases a later network that reused the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NetId {
    pub index: u32,
    pub gen: u32,
}

// ── Positions and faces ──────────────────────────────────────────────────────

/// A block position in one dimension. 16 bytes, `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pos {
    pub dim: DimId,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos {
    pub const fn new(dim: DimId, x: i32, y: i32, z: i32) -> Self {
        Pos { dim, x, y, z }
    }

    #[inline]
    pub fn offset(self, face: Face) -> Pos {
        let (dx, dy, dz) = face.delta();
        Pos {
            dim: self.dim,
            x: self.x + dx,
            y: self.y + dy,
            z: self.z + dz,
        }
    }

    /// The 16×16 column this position is in.
    #[inline]
    pub fn chunk(self) -> ChunkKey {
        ChunkKey {
            dim: self.dim,
            x: self.x >> 4,
            z: self.z >> 4,
        }
    }
}

/// A chunk column, for load tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkKey {
    pub dim: DimId,
    pub x: i32,
    pub z: i32,
}

/// One of the six block faces. The discriminant is the index into a node's
/// `links` array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Face {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::Down,
        Face::Up,
        Face::North,
        Face::South,
        Face::West,
        Face::East,
    ];

    #[inline]
    pub fn delta(self) -> (i32, i32, i32) {
        match self {
            Face::Down => (0, -1, 0),
            Face::Up => (0, 1, 0),
            Face::North => (0, 0, -1),
            Face::South => (0, 0, 1),
            Face::West => (-1, 0, 0),
            Face::East => (1, 0, 0),
        }
    }

    #[inline]
    pub fn opposite(self) -> Face {
        Face::ALL[self as usize ^ 1]
    }

    #[inline]
    pub fn from_index(i: usize) -> Face {
        Face::ALL[i]
    }

    pub fn name(self) -> &'static str {
        match self {
            Face::Down => "down",
            Face::Up => "up",
            Face::North => "north",
            Face::South => "south",
            Face::West => "west",
            Face::East => "east",
        }
    }

    pub fn parse(s: &str) -> Option<Face> {
        Face::ALL.into_iter().find(|f| f.name() == s)
    }
}

// ── Interning ────────────────────────────────────────────────────────────────

/// Maps values to dense `u32` indices and back.
#[derive(Debug, Clone)]
pub struct Interner<T: Hash + Eq + Clone> {
    map: FxHashMap<T, u32>,
    list: Vec<T>,
}

impl<T: Hash + Eq + Clone> Default for Interner<T> {
    fn default() -> Self {
        Interner {
            map: FxHashMap::default(),
            list: Vec::new(),
        }
    }
}

impl<T: Hash + Eq + Clone> Interner<T> {
    pub fn intern(&mut self, value: T) -> u32 {
        if let Some(&i) = self.map.get(&value) {
            return i;
        }
        let i = self.list.len() as u32;
        self.list.push(value.clone());
        self.map.insert(value, i);
        i
    }

    pub fn get<Q>(&self, value: &Q) -> Option<u32>
    where
        T: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get(value).copied()
    }

    pub fn resolve(&self, i: u32) -> &T {
        &self.list[i as usize]
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// Namespace a bare dimension id (`"overworld"` → `"minecraft:overworld"`),
/// matching the ids the loader passes in events, so callers can use either.
pub fn normalize_dim(dim: &str) -> String {
    if dim.contains(':') {
        dim.to_string()
    } else {
        format!("minecraft:{dim}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_are_symmetric() {
        let p = Pos::new(DimId(0), 3, -4, 5);
        for f in Face::ALL {
            assert_eq!(p.offset(f).offset(f.opposite()), p);
            assert_eq!(Face::parse(f.name()), Some(f));
        }
    }

    #[test]
    fn interner_round_trips() {
        let mut i: Interner<String> = Interner::default();
        let a = i.intern("a".into());
        let b = i.intern("b".into());
        assert_eq!(i.intern("a".into()), a);
        assert_ne!(a, b);
        assert_eq!(i.resolve(b), "b");
        assert_eq!(i.get("a"), Some(a));
    }

    #[test]
    fn chunk_of_negative_coordinates() {
        let p = Pos::new(DimId(0), -1, 0, -16);
        assert_eq!((p.chunk().x, p.chunk().z), (-1, -1));
        assert_eq!(normalize_dim("overworld"), "minecraft:overworld");
        assert_eq!(normalize_dim("mymod:void"), "mymod:void");
    }
}
