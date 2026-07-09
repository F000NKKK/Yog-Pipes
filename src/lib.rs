//! Yog Pipes — item, fluid, and energy transport for Yog mods.
//!
//! Inspired by Forge's pipe/transport systems (BuildCraft, Thermal Dynamics,
//! Mekanism). Provides connectable pipe blocks that transfer items, fluids,
//! and redstone-level "energy" between adjacent inventories.
//!
//! ## Architecture
//!
//! - **Item Pipe** — transports item stacks between inventories.
//!   Speed tiered (stone → iron → gold → diamond → netherite).
//! - **Fluid Pipe** — placeholder for future fluid API.
//! - **Energy Pipe** — placeholder for future energy/redstone API.
//!
//! Each pipe block connects to neighbors automatically (like `YogConnectingBlock`)
//! and forms a network. The pipe network runs a transfer tick every N game ticks
//! (configurable per tier), pulling from source inventories and pushing to
//! destination inventories.

mod pipe;

use yog_api::{info, Mod, Registry};

pub struct YogPipes;

impl Mod for YogPipes {
    fn register(registry: &mut Registry) {
        info!("[yog-pipes] initializing pipe transport system...");

        pipe::register(registry);

        info!("[yog-pipes] ready.");
    }
}

yog_api::export_mod!(YogPipes);
