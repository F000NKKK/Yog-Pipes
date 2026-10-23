//! Saving and loading: each capability's seeds, sides, colors, model
//! records, global state and inboxes; a save is parked until its
//! capability registers and its nodes are rediscovered.

use std::collections::VecDeque;

use super::{Engine, FORMAT, MAGIC, Parked, Probe, Slot};
use crate::ids::{Face, Pos};
use crate::indicate::Indication;
use crate::layer::Layer;
use crate::model::{Io, Record};
use crate::persist::{Dec, Enc};
use crate::registry::{Filter, FilterEntry, FilterMode, Registry, Side, SideMode};

/// Tags of a saved filter entry.
const ENTRY_RESOURCE: u8 = 0;
const ENTRY_ID: u8 = 1;
const ENTRY_KIND: u8 = 2;
const ENTRY_COLOR: u8 = 3;

/// The error of a save that ends early.
fn truncated() -> String {
    "truncated yog-pipes save".to_string()
}

/// One capability's state to save: the live slot's and the parked one's.
struct Saved<'a> {
    slot: Option<&'a Slot>,
    parked: Option<&'a Parked>,
}

impl Engine {
    pub fn save(&self) -> Vec<u8> {
        let mut e = Enc::default();
        e.buf.extend_from_slice(MAGIC);
        e.u32(FORMAT);
        let mut names: Vec<&str> = self.reg.caps.iter().map(|c| c.name.as_str()).collect();
        for k in self.parked.keys() {
            if !names.contains(&k.as_str()) {
                names.push(k);
            }
        }
        e.u32(names.len() as u32);
        for name in names {
            e.str(name);
            let saved = Saved {
                slot: self.reg.cap_id(name).map(|c| &self.slots[c.index()]),
                parked: self.parked.get(name),
            };
            self.save_seeds(&mut e, &saved);
            self.save_sides(&mut e, &saved);
            self.save_records(&mut e, &saved);
            self.save_inbox(&mut e, &saved);
        }
        e.buf
    }

    /// One node per network, plus seeds not yet rediscovered.
    fn save_seeds(&self, e: &mut Enc, saved: &Saved) {
        let mut seeds: Vec<Pos> = saved.parked.map(|p| p.seeds.clone()).unwrap_or_default();
        if let Some(s) = saved.slot {
            seeds.extend(s.layer.nets().map(|(_, n)| s.layer.node(n.any_node).pos));
        }
        e.u32(seeds.len() as u32);
        for p in seeds {
            e.pos(&self.reg, p);
        }
    }

    /// Configured sides, then colors.
    fn save_sides(&self, e: &mut Enc, saved: &Saved) {
        let reg = &self.reg;
        let mut sides: Vec<(Pos, Face, Side)> = Vec::new();
        let mut colors: Vec<(Pos, u8)> = Vec::new();
        if let Some(s) = saved.slot {
            sides.extend(s.layer.configured_sides());
            colors.extend(
                s.layer
                    .nodes()
                    .filter(|(_, n)| n.color != 0)
                    .map(|(_, n)| (n.pos, n.color)),
            );
        }
        if let Some(p) = saved.parked {
            for (pos, list) in &p.sides {
                sides.extend(list.iter().map(|&(f, s)| (*pos, f, s)));
            }
            colors.extend(p.colors.iter().map(|(&p, &c)| (p, c)));
        }
        e.u32(sides.len() as u32);
        for (p, f, s) in sides {
            e.pos(reg, p)
                .face(f)
                .u8(s.mode as u8)
                .u8(s.disabled as u8)
                .i32(s.priority)
                .u8(s.color);
            match s.filter {
                Some(id) => {
                    e.u8(1);
                    self.save_filter(e, reg.filters.get(id));
                }
                None => {
                    e.u8(0);
                }
            }
        }
        e.u32(colors.len() as u32);
        for (p, c) in colors {
            e.pos(reg, p).u8(c);
        }
    }

    /// The model's records, then its global state.
    fn save_records(&self, e: &mut Enc, saved: &Saved) {
        let reg = &self.reg;
        let mut records: Vec<Record> = Vec::new();
        if let Some(s) = saved.slot {
            s.model.save(&s.layer, reg, &mut records);
        }
        if let Some(p) = saved.parked {
            for (pos, list) in &p.records {
                records.extend(list.iter().map(|b| Record {
                    pos: *pos,
                    bytes: b.clone(),
                }));
            }
        }
        e.u32(records.len() as u32);
        for r in records {
            e.pos(reg, r.pos).bytes(&r.bytes);
        }
        let global = match (saved.slot, saved.parked.and_then(|p| p.global.as_ref())) {
            (Some(s), _) => s.model.save_global(reg),
            (None, Some(g)) => g.clone(),
            _ => Vec::new(),
        };
        e.bytes(&global);
    }

    /// Messages not yet received.
    fn save_inbox(&self, e: &mut Enc, saved: &Saved) {
        let mut inbox: Vec<(Pos, &VecDeque<Vec<u8>>)> = Vec::new();
        if let Some(s) = saved.slot {
            inbox.extend(s.inbox.iter().map(|(p, q)| (*p, q)));
        }
        if let Some(p) = saved.parked {
            inbox.extend(p.inbox.iter().map(|(p, q)| (*p, q)));
        }
        e.u32(inbox.len() as u32);
        for (p, q) in inbox {
            e.pos(&self.reg, p).u32(q.len() as u32);
            for m in q {
                e.bytes(m);
            }
        }
    }

    fn save_filter(&self, e: &mut Enc, f: &Filter) {
        e.u8(f.mode as u8).u32(f.entries.len() as u32);
        for entry in &f.entries {
            match entry {
                FilterEntry::Resource(r) => e.u8(ENTRY_RESOURCE).res(&self.reg, *r),
                FilterEntry::Id(id) => e.u8(ENTRY_ID).str(id),
                FilterEntry::Kind(k) => e.u8(ENTRY_KIND).str(&self.reg.kinds.get(*k).name),
                FilterEntry::Color(c) => e.u8(ENTRY_COLOR).u8(*c),
            };
        }
    }

    fn load_filter(reg: &mut Registry, d: &mut Dec) -> Option<Filter> {
        let mode = match d.u8()? {
            0 => FilterMode::Allow,
            _ => FilterMode::Deny,
        };
        let n = d.u32()?;
        let mut entries = Vec::new();
        for _ in 0..n {
            match d.u8()? {
                ENTRY_RESOURCE => {
                    if let Some(r) = d.res(reg) {
                        entries.push(FilterEntry::Resource(r));
                    }
                }
                ENTRY_ID => entries.push(FilterEntry::Id(d.str()?.into())),
                ENTRY_KIND => {
                    if let Some(k) = reg.kinds.id(d.str()?) {
                        entries.push(FilterEntry::Kind(k));
                    }
                }
                _ => entries.push(FilterEntry::Color(d.u8()?)),
            }
        }
        Some(Filter { mode, entries })
    }

    /// Park a save: state is handed to capabilities as they register and
    /// to nodes as they are rediscovered (call [`Engine::rediscover`]).
    pub fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        let Some(body) = bytes.strip_prefix(MAGIC.as_slice()) else {
            return Err("not a yog-pipes transport save".into());
        };
        let mut d = Dec::new(body);
        let version = d.u32().ok_or_else(truncated)?;
        if version != FORMAT {
            return Err(format!("unsupported save format {version}"));
        }
        let caps = d.u32().ok_or_else(truncated)?;
        for _ in 0..caps {
            let name = d.str().ok_or_else(truncated)?.to_string();
            let p = self.load_parked(&mut d).ok_or_else(truncated)?;
            self.parked.insert(name.clone(), p);
            if let Some(id) = self.reg.cap_id(&name) {
                self.unpark_global(&name, id);
            }
        }
        Ok(())
    }

    /// One capability's saved state.
    fn load_parked(&mut self, d: &mut Dec) -> Option<Parked> {
        let mut p = Parked::default();
        for _ in 0..d.u32()? {
            p.seeds.push(d.pos(&mut self.reg)?);
        }
        for _ in 0..d.u32()? {
            let pos = d.pos(&mut self.reg)?;
            let (face, side) = self.load_side(d)?;
            p.sides.entry(pos).or_default().push((face, side));
        }
        for _ in 0..d.u32()? {
            let pos = d.pos(&mut self.reg)?;
            p.colors.insert(pos, d.u8()?);
        }
        for _ in 0..d.u32()? {
            let pos = d.pos(&mut self.reg)?;
            p.records.entry(pos).or_default().push(d.bytes()?.to_vec());
        }
        let global = d.bytes()?;
        if !global.is_empty() {
            p.global = Some(global.to_vec());
        }
        for _ in 0..d.u32()? {
            let pos = d.pos(&mut self.reg)?;
            let mut q = VecDeque::new();
            for _ in 0..d.u32()? {
                q.push_back(d.bytes()?.to_vec());
            }
            p.inbox.insert(pos, q);
        }
        Some(p)
    }

    /// One saved side: its face and configuration.
    fn load_side(&mut self, d: &mut Dec) -> Option<(Face, Side)> {
        let face = d.face()?;
        let mode = SideMode::from_index(d.u8()?);
        let disabled = d.u8()? != 0;
        let priority = d.i32()?;
        let color = d.u8()?;
        let filter = match d.u8()? {
            1 => {
                let f = Self::load_filter(&mut self.reg, d)?;
                Some(self.reg.filters.add(f))
            }
            _ => None,
        };
        Some((
            face,
            Side {
                mode,
                disabled,
                priority,
                filter,
                color,
            },
        ))
    }

    /// Rediscover saved networks of registered capabilities from their
    /// seeds. Call after capabilities are registered (e.g. on the first
    /// server tick); cheap when there is nothing left to do.
    pub fn rediscover(&mut self, io: &mut dyn Io, probe: &mut dyn Probe) {
        let mut seeds = Vec::new();
        for c in &self.reg.caps {
            if let Some(p) = self.parked.get_mut(&c.name) {
                seeds.append(&mut p.seeds);
            }
        }
        for s in seeds {
            if !self.slots.iter().any(|sl| sl.layer.node_at(s).is_some()) {
                self.discover(io, probe, s);
            }
        }
    }

    /// Saved records still waiting for their node (e.g. a pipe removed
    /// while the server was down). Kept and re-saved, never invented.
    pub fn parked_records(&self) -> usize {
        self.parked
            .values()
            .map(|p| p.records.values().map(Vec::len).sum::<usize>())
            .sum()
    }

    /// Forget everything about the world (server stopping). Registrations
    /// stay.
    pub fn clear_world(&mut self) {
        for s in &mut self.slots {
            s.layer = {
                let mut l = Layer::default();
                for a in &s.layer.atts {
                    l.set_attach(None, a.clone());
                }
                l
            };
            s.ind = Indication::new(s.ind.levels.clone());
            s.inbox.clear();
            s.latest.clear();
            s.model.reset();
        }
        self.parked.clear();
        self.tick = 0;
    }
}
