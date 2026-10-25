//! A minimal mod built on Yog Pipes. The pipe block and its recipe are
//! content (`content/`); this only tells Yog Pipes that it is a pipe.

use yog_api::{Mod, Registry, warn};
use yog_pipes::{AttachDef, CapabilityDef, Distribution, Mechanic, attach, register_capability};

use crate::content::Blocks;

/// The capability the copper pipe carries.
const ITEMS: &str = "copper-pipes:items";
/// Blocks an item travels a tick.
const SPEED: f64 = 0.5;

pub struct CopperPipes;

impl Mod for CopperPipes {
    fn register(_registry: &mut Registry) {
        let pipe = Blocks::copper_pipe();
        // What the pipe carries: items travelling block by block.
        let carried = CapabilityDef {
            name: ITEMS.into(),
            kinds: Vec::new(),
            exclusive: true,
            state_prefix: "items".into(),
            indicators: Vec::new(),
            distribution: Distribution::RoundRobin,
            events: None,
            mechanic: Mechanic::InFlight,
        };
        let pipe = AttachDef {
            block: pipe.id,
            capability: ITEMS.into(),
            // The same groups its arms connect by.
            groups: pipe.connect_groups,
            params: [("speed".into(), SPEED)].into(),
        };
        // Each call answers `None` when Yog Pipes isn't loaded.
        let done = register_capability(carried).and_then(|registered| match registered {
            Ok(()) => attach(pipe),
            Err(e) => Some(Err(e)),
        });
        match done {
            Some(Ok(())) => {}
            Some(Err(e)) => warn!("[copper-pipes] {e}"),
            None => warn!("[copper-pipes] Yog Pipes isn't loaded"),
        }
    }
}

yog_api::content!();

yog_api::export_mod!(CopperPipes);
