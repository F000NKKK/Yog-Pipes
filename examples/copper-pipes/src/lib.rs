//! A minimal mod built on Yog Pipes. The pipe block and its recipe are
//! content (`content/`); this only tells Yog Pipes that it is a pipe.

use yog_api::{warn, Mod, Registry};
use yog_pipes::{attach, register_capability, AttachDef, CapabilityDef, Distribution, Mechanic};

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
        if let Err(e) = register_capability(carried).and_then(|()| attach(pipe)) {
            warn!("[copper-pipes] {e}");
        }
    }
}

yog_api::content!();

yog_api::export_mod!(CopperPipes);
