//! What a pipe carries.

use yog_api::yog_export;

/// What a pipe carries. Mods can add custom kinds via the string-based
/// [`PipeKind::Custom`] variant.
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

pub fn display_name(kind: &PipeKind) -> &'static str {
    match kind {
        PipeKind::Item => "Item",
        PipeKind::Fluid => "Fluid",
        PipeKind::Signal => "Signal",
        PipeKind::Energy => "Energy",
        PipeKind::Custom(_) => "Custom",
    }
}
