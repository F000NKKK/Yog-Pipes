//! `External` — a model implemented outside this crate (§5.7). The core
//! keeps one opaque state blob per network and hands it to a [`Driver`]
//! (in the mod: an interop bridge to another mod's `#[yog_export]` fns).
//! Conservation inside an external model is that model's responsibility;
//! endpoint I/O it does through [`Cx`] is still checked and clamped.

use crate::ids::{NetId, NodeId, ResId};
use crate::layer::Layer;
use crate::model::{Activity, Cx, Model, NetMap, Record};
use crate::registry::Registry;

/// What an external model implements. Only `tick` is required.
pub trait Driver: Send {
    fn tick(&mut self, cx: &mut Cx, net: NetId, state: &mut Vec<u8>) -> Activity;
    /// Combine `from` into `into` (default: keep `into`, drop `from`).
    fn merge(&mut self, _into: &mut Vec<u8>, _from: Vec<u8>) {}
    /// State of each part split off (default: empty), `sizes` in node
    /// counts; `from` may be changed in place.
    fn split(&mut self, _from: &mut Vec<u8>, sizes: &[u32]) -> Vec<Vec<u8>> {
        vec![Vec::new(); sizes.len()]
    }
    /// A node is about to go: adjust state (spill its share).
    fn node_removed(&mut self, _cx: &mut Cx, _node: NodeId, _state: &mut Vec<u8>) {}
}

pub struct External {
    driver: Box<dyn Driver>,
    params: &'static [&'static str],
    states: NetMap<Vec<u8>>,
}

impl External {
    /// `params` are the attach parameter names the external model reads
    /// (leaked once per registration — capabilities live for the session).
    pub fn new(driver: Box<dyn Driver>, params: Vec<String>) -> Self {
        let params: Vec<&'static str> = params.into_iter().map(|p| &*Box::leak(p.into_boxed_str())).collect();
        External {
            driver,
            params: Box::leak(params.into_boxed_slice()),
            states: NetMap::default(),
        }
    }
}

impl Model for External {
    fn params(&self) -> &'static [&'static str] {
        self.params
    }

    fn node_removed(&mut self, cx: &mut Cx, node: NodeId, net: NetId) {
        let mut s = std::mem::take(self.states.entry(net));
        self.driver.node_removed(cx, node, &mut s);
        *self.states.entry(net) = s;
    }

    fn merged(&mut self, _cx: &mut Cx, into: NetId, from: NetId) {
        if let Some(f) = self.states.take(from) {
            let mut s = std::mem::take(self.states.entry(into));
            self.driver.merge(&mut s, f);
            *self.states.entry(into) = s;
        }
    }

    fn split(&mut self, _cx: &mut Cx, from: NetId, parts: &[(NetId, u32)]) {
        let mut s = std::mem::take(self.states.entry(from));
        let sizes: Vec<u32> = parts.iter().map(|p| p.1).collect();
        let out = self.driver.split(&mut s, &sizes);
        *self.states.entry(from) = s;
        for (&(p, _), st) in parts.iter().zip(out) {
            *self.states.entry(p) = st;
        }
    }

    fn net_removed(&mut self, _cx: &mut Cx, net: NetId) {
        self.states.take(net);
    }

    fn tick(&mut self, cx: &mut Cx, net: NetId) -> Activity {
        let mut s = std::mem::take(self.states.entry(net));
        let a = self.driver.tick(cx, net, &mut s);
        *self.states.entry(net) = s;
        a
    }

    fn save(&self, layer: &Layer, _reg: &Registry, out: &mut Vec<Record>) {
        for (net, s) in self.states.iter() {
            if let Some(n) = layer.net(net)
                && !s.is_empty()
            {
                out.push(Record {
                    pos: layer.node(n.any_node).pos,
                    bytes: s.clone(),
                });
            }
        }
    }

    fn restore(&mut self, _cx: &mut Cx, _node: NodeId, net: NetId, bytes: &[u8]) -> bool {
        let mut s = std::mem::take(self.states.entry(net));
        if s.is_empty() {
            s = bytes.to_vec();
        } else {
            self.driver.merge(&mut s, bytes.to_vec());
        }
        *self.states.entry(net) = s;
        true
    }

    fn totals(&self, _layer: &Layer, _net: NetId, _out: &mut Vec<(ResId, u64)>) {}
}
