//! Networks growing and shrinking: a settled region joins or founds a
//! network, two networks merge on a link, a node's removal or a cut link
//! splits one (see [`Layer::split_check`]).

use super::{Layer, NO_NET, Network, Settled, Split};
use crate::ids::{Face, NO_NODE, NetId, NodeId};

impl Layer {
    /// Give a freshly inserted, internally linked region its network: a
    /// new one, or the largest network it touches, absorbing the others.
    pub fn settle(&mut self, region: &[NodeId]) -> Settled {
        if region.is_empty() {
            return Settled::default();
        }
        let touched = self.touched_nets(region);
        let mut out = Settled::default();
        let target = match touched
            .iter()
            .max_by_key(|(i, _)| self.nets[*i as usize].net.as_ref().map_or(0, |n| n.size))
        {
            Some(&(i, _)) => self.net_id(i),
            None => {
                out.created = true;
                self.new_net(Network::default())
            }
        };
        for &(i, start) in &touched {
            if i == target.index {
                continue;
            }
            let from = self.net_id(i);
            let size = self.net(from).map_or(0, |n| n.size);
            self.absorb(target, from, start);
            out.absorbed.push((from, size));
        }
        self.join(target, region);
        self.touch(target);
        out.net = Some(target);
        out
    }

    /// The distinct networks `region` borders, with one node of each.
    fn touched_nets(&self, region: &[NodeId]) -> Vec<(u32, NodeId)> {
        let mut touched: Vec<(u32, NodeId)> = Vec::new();
        for &n in region {
            for l in self.nodes[n.index()].links {
                if l == NO_NODE {
                    continue;
                }
                let ni = self.nodes[l.index()].net;
                if ni != NO_NET && !touched.iter().any(|t| t.0 == ni) {
                    touched.push((ni, l));
                }
            }
        }
        touched
    }

    /// Put every node of `region` into `target`, with its counts.
    fn join(&mut self, target: NetId, region: &[NodeId]) {
        let mut added = Network::default();
        for &n in region {
            let node = &mut self.nodes[n.index()];
            node.net = target.index;
            added.size += 1;
            added.frozen += self.unloaded.contains(&node.pos.chunk()) as u32;
            added.add_type(node.att, 1);
        }
        let Some(net) = self.net_mut(target) else {
            return;
        };
        net.size += added.size;
        net.frozen += added.frozen;
        for (a, c) in added.types {
            net.add_type(a, c);
        }
        if net.any_node == NO_NODE {
            net.any_node = region[0];
        }
    }

    /// Relabel every node of `from` (reached from `start`) into `into` and
    /// move its ports, counts and flags.
    fn absorb(&mut self, into: NetId, from: NetId, start: NodeId) {
        let mut queue = std::mem::take(&mut self.scratch);
        queue.clear();
        queue.push(start);
        self.nodes[start.index()].net = into.index;
        while let Some(c) = queue.pop() {
            for l in self.nodes[c.index()].links {
                if l != NO_NODE && self.nodes[l.index()].net == from.index {
                    self.nodes[l.index()].net = into.index;
                    queue.push(l);
                }
            }
        }
        self.scratch = queue;
        let Some(old) = self.nets[from.index as usize].net.take() else {
            return;
        };
        self.free_net_slot(from);
        let Some(net) = self.net_mut(into) else {
            return;
        };
        net.size += old.size;
        net.frozen += old.frozen;
        for (a, c) in old.types {
            net.add_type(a, c);
        }
        net.ports.extend(old.ports);
    }

    fn free_net_slot(&mut self, id: NetId) {
        let slot = &mut self.nets[id.index as usize];
        slot.generation = slot.generation.wrapping_add(1);
        self.free_nets.push(id.index);
    }

    /// Link two settled nodes; merges their networks if they differ.
    /// Returns `(into, from, from's size)` when a merge happened.
    pub fn link_settled(
        &mut self,
        a: NodeId,
        face: Face,
        b: NodeId,
    ) -> Option<(NetId, NetId, u32)> {
        self.link_raw(a, face, b);
        let (na, nb) = (self.net_of(a)?, self.net_of(b)?);
        if na == nb {
            self.touch(na);
            return None;
        }
        let (sa, sb) = (self.net(na)?.size, self.net(nb)?.size);
        let (into, from, start) = if sa >= sb { (na, nb, b) } else { (nb, na, a) };
        let size = sa.min(sb);
        self.absorb(into, from, start);
        self.touch(into);
        Some((into, from, size))
    }

    /// Remove a node: drop its ports, unlink it, and split its network if
    /// that disconnected it.
    pub fn remove_node(&mut self, node: NodeId) -> Split {
        let net = self.net_of(node);
        if let Some(id) = net {
            self.drop_node_ports(id, node);
        }
        let seeds = self.cut_links(node);
        let pos = self.nodes[node.index()].pos;
        self.index.remove(&pos);
        if let Some(v) = self.chunks.get_mut(&pos.chunk()) {
            v.retain(|&n| n != node);
            if v.is_empty() {
                self.chunks.remove(&pos.chunk());
            }
        }
        let att = self.nodes[node.index()].att;
        self.nodes[node.index()].net = NO_NET;
        self.free_nodes.push(node.0);
        let frozen = self.unloaded.contains(&pos.chunk()) as u32;
        let Some((id, n)) = net.and_then(|id| Some((id, self.net_mut(id)?))) else {
            return Split::default();
        };
        n.size -= 1;
        n.frozen -= frozen;
        n.sub_type(att, 1);
        if n.size == 0 {
            self.free_net(id);
            return Split {
                net: Some(id),
                emptied: true,
                parts: Vec::new(),
            };
        }
        if n.any_node == node
            && let Some(&seed) = seeds.first()
        {
            n.any_node = seed;
        }
        let parts = self.split_check(id, &seeds);
        self.touch(id);
        Split {
            net: Some(id),
            emptied: false,
            parts,
        }
    }

    /// Drop the ports `node` has in network `id`.
    fn drop_node_ports(&mut self, id: NetId, node: NodeId) {
        let Some(n) = self.net_mut(id) else {
            return;
        };
        let mut gone = Vec::new();
        n.ports.retain(|p| {
            let keep = p.node != node;
            if !keep {
                gone.push(p.ep);
            }
            keep
        });
        for ep in gone {
            self.drop_port_count(ep);
        }
    }

    /// Cut every link and side of `node`; its former neighbours, once each.
    fn cut_links(&mut self, node: NodeId) -> Vec<NodeId> {
        let mut seeds: Vec<NodeId> = Vec::with_capacity(Face::ALL.len());
        for f in Face::ALL {
            let l = self.nodes[node.index()].links[f as usize];
            if l != NO_NODE {
                self.nodes[l.index()].links[f.opposite() as usize] = NO_NODE;
                self.nodes[node.index()].links[f as usize] = NO_NODE;
                if !seeds.contains(&l) {
                    seeds.push(l);
                }
            }
            self.sides.remove(&(node, f));
        }
        seeds
    }

    /// Cut the link on `a`'s `face` (side set to off, color change).
    pub fn unlink(&mut self, a: NodeId, face: Face) -> Split {
        let b = self.nodes[a.index()].links[face as usize];
        if b == NO_NODE {
            return Split::default();
        }
        self.nodes[a.index()].links[face as usize] = NO_NODE;
        self.nodes[b.index()].links[face.opposite() as usize] = NO_NODE;
        let Some(id) = self.net_of(a) else {
            return Split::default();
        };
        let parts = self.split_check(id, &[a, b]);
        self.touch(id);
        Split {
            net: Some(id),
            emptied: false,
            parts,
        }
    }
}
