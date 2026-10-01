//! What the page and the interface share: painting layers of glyphs, icons, paths,
//! images, rounded rectangles and pen strokes with wgpu, Direct3D 11 or OpenGL, each layer
//! with its own transform and clip, and editing text within a laid-out line.

pub mod edit;
#[cfg(feature = "render")]
mod render;

#[cfg(feature = "render")]
pub use render::*;

/// sRGB's encoding of a linear channel.
fn encode(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// The linear light of an sRGB-encoded channel.
fn linear(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}
