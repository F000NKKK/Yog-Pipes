//! State machine for pipe transfers.
//!
//! Each tick, the state machine processes one transfer operation:
//! 1. **Extract** — pull items/energy/signal from a source node
//! 2. **Route**   — find shortest path through the graph to a destination
//! 3. **Insert**  — push items/energy into the destination

use std::sync::{LazyLock, Mutex};

use crate::graph::NodeKey;
use crate::PipeKind;

/// Transfer state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    /// Waiting for work.
    Idle,
    /// Pulling from a source inventory.
    Extracting { from: NodeKey, slot: usize },
    /// Finding path to destination.
    Routing { from: NodeKey, to: NodeKey, path: Vec<NodeKey> },
    /// Pushing into a destination.
    Inserting { to: NodeKey, slot: usize, kind: PipeKind, amount: u64 },
}

/// Global transfer state.
pub static TRANSFER: LazyLock<Mutex<TransferState>> = LazyLock::new(|| Mutex::new(TransferState::Idle));

/// Tick counter.
pub static TICK: LazyLock<Mutex<u64>> = LazyLock::new(|| Mutex::new(0));

/// Run one step of the state machine.
pub fn transfer_tick(_srv: &dyn yog_api::Server) {
    let mut tick = TICK.lock().unwrap();
    *tick += 1;

    let mut state = TRANSFER.lock().unwrap();
    match &*state {
        TransferState::Idle => {
            // TODO: scan graph for source nodes with non-empty inventories/buffers
        }
        TransferState::Extracting { from: _, slot: _ } => {
            // TODO: extract from source
            *state = TransferState::Idle;
        }
        TransferState::Routing { from: _, to: _, path: _ } => {
            // Path already computed; proceed to insert
            *state = TransferState::Idle;
        }
        TransferState::Inserting { to: _, slot: _, kind: _, amount: _ } => {
            // TODO: insert into destination
            *state = TransferState::Idle;
        }
    }
}

/// Schedule a transfer from `from` to `to` for the given amount.
pub fn schedule(from: NodeKey, to: NodeKey, amount: u64, kind: PipeKind) {
    let path = crate::graph::find_path(&from, &to);
    let mut state = TRANSFER.lock().unwrap();
    match path {
        Some(p) => *state = TransferState::Routing { from, to, path: p },
        None => {
            // No path — insert directly if adjacent
            *state = TransferState::Inserting { to, slot: 0, kind, amount };
        }
    }
}
