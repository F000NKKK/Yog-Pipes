//! A minimal mod built on Yog Pipes. The pipe block itself is declared in
//! `content/blocks/copper_pipe.yblock`; this only tells Yog Pipes that it is
//! a pipe and adds its recipe.

use yog_api::{warn, Mod, Registry, ShapedRecipe};
use yog_pipes::{register_pipe, PipeDef, PipeKind};

use crate::content::Blocks;

pub struct CopperPipes;

impl Mod for CopperPipes {
    fn register(registry: &mut Registry) {
        let pipe = Blocks::copper_pipe();
        if let Err(e) = register_pipe(PipeDef {
            block_id: pipe.id,
            kind: PipeKind::Item,
            link_groups: pipe.connect_groups,
            properties: [("speed".into(), 2.0)].into(),
        }) {
            warn!("[copper-pipes] {e}");
        }

        registry.add_shaped_recipe(
            ShapedRecipe::new("copper-pipes:copper_pipe", Blocks::COPPER_PIPE, 8)
                .row("CCC")
                .key('C', "minecraft:copper_ingot"),
        );
    }
}

yog_api::content!();

yog_api::export_mod!(CopperPipes);
