//! Whether a network that lost a node or link fell apart, found by racing
//! one search per former neighbour and stopping once at most one is left.

use std::collections::VecDeque;

use super::{Layer, Network, Port};
use crate::ids::{NO_NODE, NetId, NodeId};

/// Union-find over the searches: two that met are one piece.
struct Pieces {
    parent: Vec<usize>,
}

impl Pieces {
    fn new(k: usize) -> Self {
        Pieces {
            parent: (0..k).collect(),
        }
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn join(&mut self, i: usize, j: usize) {
        let (ri, rj) = (self.find(i), self.find(j));
        if ri != rj {
            self.parent[ri] = rj;
        }
    }
}

/// The searches from each seed: what each has seen, what it has left.
struct Race {
    base: u32,
    pieces: Pieces,
    queues: Vec<VecDeque<NodeId>>,
    seen: Vec<Vec<NodeId>>,
}

/// One piece of the split: its root search, whether a search in it is
/// still open, and its node count.
struct Piece {
    root: usize,
    open: bool,
    size: u32,
}

impl Layer {
    /// After `net` lost a node or link, find out whether `seeds` (the
    /// former neighbours, all still in `net`) are still connected.
    ///
    /// One BFS per seed, interleaved one node per search per round, with a
    /// union-find over the searches (two that meet are the same piece). A
    /// piece whose frontier runs dry is a complete component. As soon as at
    /// most one piece is still open, every closed piece becomes its own
    /// network and the open one keeps `net` without being walked to the
    /// end — so the cost is proportional to the pieces that split off.
    pub fn split_check(&mut self, net: NetId, seeds: &[NodeId]) -> Vec<(NetId, u32)> {
        if seeds.len() <= 1 {
            return Vec::new();
        }
        let mut race = self.start_race(seeds);
        while race.open_pieces() > 1 {
            self.race_round(&mut race);
        }
        let pieces = race.pieces();
        // The piece that keeps `net`: the open one, else the largest.
        let Some(keep) = pieces
            .iter()
            .find(|p| p.open)
            .or_else(|| pieces.iter().max_by_key(|p| p.size))
            .map(|p| p.root)
        else {
            return Vec::new();
        };
        if pieces.len() <= 1 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for p in pieces.iter().filter(|p| p.root != keep) {
            let id = self.split_off(net, &mut race, p);
            out.push((id, p.size));
        }
        self.keep_representative(net, &mut race, keep, seeds);
        out
    }

    /// A search from each seed; a seed repeated joins the first's search.
    fn start_race(&mut self, seeds: &[NodeId]) -> Race {
        let k = seeds.len();
        let base = self.stamps(k as u32);
        let mut race = Race {
            base,
            pieces: Pieces::new(k),
            queues: vec![VecDeque::new(); k],
            seen: vec![Vec::new(); k],
        };
        for (i, &s) in seeds.iter().enumerate() {
            if let Some(j) = race.search_of(self.nodes[s.index()].mark) {
                race.pieces.join(i, j);
                continue;
            }
            self.nodes[s.index()].mark = base + i as u32;
            race.queues[i].push_back(s);
            race.seen[i].push(s);
        }
        race
    }

    /// Every open search takes one step.
    fn race_round(&mut self, race: &mut Race) {
        for i in 0..race.queues.len() {
            let Some(c) = race.queues[i].pop_front() else {
                continue;
            };
            for l in self.nodes[c.index()].links {
                if l == NO_NODE {
                    continue;
                }
                if let Some(j) = race.search_of(self.nodes[l.index()].mark) {
                    race.pieces.join(i, j);
                    continue;
                }
                self.nodes[l.index()].mark = race.base + i as u32;
                race.queues[i].push_back(l);
                race.seen[i].push(l);
            }
        }
    }

    /// Make piece `p` of `net` a network of its own, with its counts and
    /// ports.
    fn split_off(&mut self, net: NetId, race: &mut Race, p: &Piece) -> NetId {
        let mut part = Network {
            size: p.size,
            any_node: NO_NODE,
            ..Network::default()
        };
        let mut nodes = Vec::new();
        for i in 0..race.seen.len() {
            if race.pieces.find(i) == p.root {
                nodes.extend_from_slice(&race.seen[i]);
            }
        }
        for &n in &nodes {
            let node = &self.nodes[n.index()];
            part.add_type(node.att, 1);
            part.frozen += self.unloaded.contains(&node.pos.chunk()) as u32;
        }
        part.any_node = nodes.first().copied().unwrap_or(NO_NODE);
        let id = self.new_net(Network::default());
        for &n in &nodes {
            self.nodes[n.index()].net = id.index;
        }
        part.ports = self.move_ports(net, id, &part);
        if let Some(new) = self.net_mut(id) {
            *new = Network {
                version: new.version,
                ..part
            };
        }
        self.activate(id, 0);
        id
    }

    /// Take `part`'s counts off `net` and hand over the ports whose nodes
    /// are now in `id`.
    fn move_ports(&mut self, net: NetId, id: NetId, part: &Network) -> Vec<Port> {
        let Some(old) = self.net_mut(net) else {
            return Vec::new();
        };
        old.size -= part.size;
        old.frozen -= part.frozen;
        for &(a, c) in &part.types {
            old.sub_type(a, c);
        }
        let all = std::mem::take(&mut old.ports);
        let (moved, kept): (Vec<Port>, Vec<Port>) = all
            .into_iter()
            .partition(|p| self.nodes[p.node.index()].net == id.index);
        if let Some(old) = self.net_mut(net) {
            old.ports = kept;
        }
        moved
    }

    /// If `net`'s representative left with a piece, any kept seed will do.
    fn keep_representative(&mut self, net: NetId, race: &mut Race, keep: usize, seeds: &[NodeId]) {
        let moved = self
            .net(net)
            .is_some_and(|n| self.nodes[n.any_node.index()].net != net.index);
        if !moved {
            return;
        }
        let Some(seed) = (0..seeds.len())
            .find(|&i| race.pieces.find(i) == keep)
            .map(|i| seeds[i])
        else {
            return;
        };
        if let Some(n) = self.net_mut(net) {
            n.any_node = seed;
        }
    }
}

impl Race {
    /// The search a node's mark says reached it, if one of this race's.
    fn search_of(&self, mark: u32) -> Option<usize> {
        let k = self.queues.len() as u32;
        (mark >= self.base && mark < self.base + k).then(|| (mark - self.base) as usize)
    }

    /// How many pieces still have an open search.
    fn open_pieces(&mut self) -> usize {
        let mut roots: Vec<usize> = Vec::with_capacity(self.queues.len());
        for i in 0..self.queues.len() {
            if !self.queues[i].is_empty() {
                let r = self.pieces.find(i);
                if !roots.contains(&r) {
                    roots.push(r);
                }
            }
        }
        roots.len()
    }

    /// The searches grouped by piece.
    fn pieces(&mut self) -> Vec<Piece> {
        let mut pieces: Vec<Piece> = Vec::new();
        for i in 0..self.queues.len() {
            let root = self.pieces.find(i);
            let open = !self.queues[i].is_empty();
            let size = self.seen[i].len() as u32;
            match pieces.iter_mut().find(|p| p.root == root) {
                Some(p) => {
                    p.open |= open;
                    p.size += size;
                }
                None => pieces.push(Piece { root, open, size }),
            }
        }
        pieces
    }
}
