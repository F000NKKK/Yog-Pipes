//! 3D model system — describes a pipe block's shape and textures as data,
//! so mods never need separate JSON model files.

use std::collections::HashMap;
use yog_api::yog_export;

/// Describes a block model with cubic elements and per-face textures.
///
/// This replaces the need for separate JSON model files. Mods describe
/// their pipe shape programmatically.
#[derive(Debug, Clone)]
#[yog_export]
pub struct ModelDef {
    /// Block texture (e.g. `"mymod:block/pipe_iron"`).
    /// If `None`, the framework uses a default pipe texture.
    pub texture: Option<String>,
    /// List of cubic elements that make up the model.
    /// Empty = default pipe shape (4..12 on all axes).
    pub elements: Vec<ModelElement>,
}

/// A single cubic element in a block model.
#[derive(Debug, Clone)]
#[yog_export]
pub struct ModelElement {
    /// Start position `(x, y, z)` in 16×16×16 voxel space (0..16).
    pub from: [f32; 3],
    /// End position `(x, y, z)` in 16×16×16 voxel space (0..16).
    pub to: [f32; 3],
    /// Per-face textures and UV data. Key: `"up"`, `"down"`, `"north"`,
    /// `"south"`, `"east"`, `"west"`, or `"all"` to set all faces at once.
    pub faces: HashMap<String, FaceDef>,
    /// Optional rotation around a center point.
    pub rotation: Option<ElementRotation>,
}

/// Texture and UV data for one face of a model element.
#[derive(Debug, Clone)]
#[yog_export]
pub struct FaceDef {
    /// Texture reference (e.g. `"mymod:block/pipe_iron"`).
    /// If empty, inherits from [`ModelDef::texture`].
    pub texture: String,
    /// UV coordinates `[u_min, v_min, u_max, v_max]` in 0..16 range.
    /// Empty = full face (0, 0, 16, 16).
    pub uv: Option<[f32; 4]>,
    /// Rotation of the face texture in 90-degree increments (0, 90, 180, 270).
    pub rotation: u32,
}

/// Rotation of a model element around a center point.
#[derive(Debug, Clone)]
#[yog_export]
pub struct ElementRotation {
    /// Center of rotation `(x, y, z)` in voxel space.
    pub origin: [f32; 3],
    /// Axis: `"x"`, `"y"`, or `"z"`.
    pub axis: String,
    /// Angle in degrees (positive = clockwise when looking towards origin
    /// along the positive axis direction). Typically -45, -22.5, 22.5, 45.
    pub angle: f32,
    /// Whether to rescale the faces after rotation.
    pub rescale: bool,
}

/// Resolve collision shape from model or use default pipe shape.
pub fn resolve_shape(model: &Option<ModelDef>) -> (f32, f32, f32, f32, f32, f32) {
    if let Some(ref m) = model {
        if !m.elements.is_empty() {
            // Compute bounding box from all elements
            let mut min = [16.0f32; 3];
            let mut max = [0.0f32; 3];
            for el in &m.elements {
                for i in 0..3 {
                    min[i] = min[i].min(el.from[i]);
                    max[i] = max[i].max(el.to[i]);
                }
            }
            // Scale from 0..16 to -8..8 (Minecraft block space)
            return (
                min[0] - 8.0,
                min[1] - 8.0,
                min[2] - 8.0,
                max[0] - 8.0,
                max[1] - 8.0,
                max[2] - 8.0,
            );
        }
    }
    // Default pipe shape: 4..12 on all axes → -4..4 in block space
    (4.0, 4.0, 4.0, 12.0, 12.0, 12.0)
}

/// Apply model data to a BlockDef (textures, elements, etc.)
pub fn apply(block: yog_api::BlockDef, _model: &ModelDef) -> yog_api::BlockDef {
    // Model data (textures, elements, faces) is used by the renderer layer.
    // BlockDef handles shape, strength, connect groups — model details
    // are passed to the Yog runtime separately.
    let _ = &_model.texture;
    block
}
