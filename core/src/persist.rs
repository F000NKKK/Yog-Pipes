//! A tiny explicit little-endian binary codec for saved state. Resources
//! and dimensions are written by name, never by interned id, so a save
//! survives registration order changing between runs.

use crate::ids::ResId;
use crate::ids::{Face, Pos};
use crate::registry::Registry;

#[derive(Default)]
pub struct Enc {
    pub buf: Vec<u8>,
}

impl Enc {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v);
        self
    }
    pub fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
    /// Dimension by name, then coordinates.
    pub fn pos(&mut self, reg: &Registry, p: Pos) -> &mut Self {
        self.str(reg.dim_name(p.dim)).i32(p.x).i32(p.y).i32(p.z)
    }
    pub fn face(&mut self, f: Face) -> &mut Self {
        self.u8(f as u8)
    }
    /// A resource as kind name, id and data.
    pub fn res(&mut self, reg: &Registry, r: ResId) -> &mut Self {
        let key = reg.resources.get(r);
        self.str(&reg.kinds.get(key.kind).name)
            .str(&key.id)
            .bytes(&key.data)
    }
}

pub struct Dec<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Dec<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Dec { data, at: 0 }
    }
    pub fn done(&self) -> bool {
        self.at >= self.data.len()
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    pub fn str(&mut self) -> Option<&'a str> {
        std::str::from_utf8(self.bytes()?).ok()
    }
    pub fn pos(&mut self, reg: &mut Registry) -> Option<Pos> {
        let dim = reg.dim(self.str()?);
        Some(Pos::new(dim, self.i32()?, self.i32()?, self.i32()?))
    }
    pub fn face(&mut self) -> Option<Face> {
        let i = self.u8()? as usize;
        (i < 6).then(|| Face::from_index(i))
    }
    /// A resource; `None` when its kind is not registered (yet).
    pub fn res(&mut self, reg: &mut Registry) -> Option<ResId> {
        let (kind, id, data) = (self.str()?, self.str()?, self.bytes()?);
        let kind = reg.kinds.id(kind)?;
        Some(reg.resources.intern(&reg.kinds, kind, id, data))
    }
}
