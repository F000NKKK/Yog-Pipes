//! What a pipe carries.

use yog_api::yog_export;

/// What a pipe carries — a label for other mods, never used for routing.
/// Mods can add custom kinds via the string-based [`PipeKind::Custom`]
/// variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[yog_export]
pub enum PipeKind {
    Item,
    Fluid,
    Signal,
    Energy,
    /// Any custom kind — identified by a string id (e.g. `"mymod:mana"`).
    Custom(String),
}
