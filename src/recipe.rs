//! Crafting recipes as plain data — any recipe kind the underlying
//! `yog-registry` supports, any pattern/ingredients, any output count, for
//! any pipe block. A pipe can carry zero, one, or several recipes (e.g. a
//! shaped recipe AND a furnace recipe for the same item) — nothing here
//! assumes crafting-table-only, single-recipe pipes.

use yog_api::yog_export;

/// One crafting recipe for a pipe's item, as plain data. Add a new variant
/// here whenever `yog-registry` grows a new recipe kind — this is the only
/// place that needs to know about it.
#[derive(Debug, Clone)]
#[yog_export]
pub enum RecipeDef {
    /// A crafting-table shaped recipe: rows top to bottom (e.g. `["RRR"]`
    /// for a 1×3 grid) plus a symbol → ingredient mapping.
    Shaped {
        rows: Vec<String>,
        keys: Vec<(char, String)>,
        result_count: u32,
    },
    /// A crafting-table shapeless recipe: any arrangement of the given
    /// ingredients.
    Shapeless {
        ingredients: Vec<String>,
        result_count: u32,
    },
    /// A furnace/smelting recipe.
    Furnace {
        input: String,
        result_count: u32,
        experience: f32,
        cook_time: u32,
    },
}

/// Register `recipe` against `block_id`'s crafting output.
pub fn register(registry: &mut yog_api::Registry, block_id: &str, recipe: &RecipeDef) {
    match recipe {
        RecipeDef::Shaped {
            rows,
            keys,
            result_count,
        } => {
            let mut r = yog_api::ShapedRecipe::new(
                format!("{block_id}_craft_shaped"),
                block_id,
                *result_count,
            );
            for row in rows {
                r = r.row(row.clone());
            }
            for (symbol, ingredient) in keys {
                r = r.key(*symbol, ingredient.clone());
            }
            registry.add_shaped_recipe(r);
        }
        RecipeDef::Shapeless {
            ingredients,
            result_count,
        } => {
            let mut r = yog_api::ShapelessRecipe::new(
                format!("{block_id}_craft_shapeless"),
                block_id,
                *result_count,
            );
            for ingredient in ingredients {
                r = r.ingredient(ingredient.clone());
            }
            registry.add_shapeless_recipe(r);
        }
        RecipeDef::Furnace {
            input,
            result_count,
            experience,
            cook_time,
        } => {
            let mut r = yog_api::FurnaceRecipe::new(
                format!("{block_id}_smelt"),
                input.clone(),
                block_id,
                *result_count,
            );
            r.experience = *experience;
            r.cook_time = *cook_time;
            registry.add_furnace_recipe(r);
        }
    }
}
