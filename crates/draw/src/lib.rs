//! What the page and the interface share: painting layers of glyphs, icons, paths,
//! images, rounded rectangles and pen strokes with wgpu, each layer with its own transform
//! and clip, and editing text within a laid-out line.

pub mod edit;
#[cfg(feature = "render")]
mod render;

#[cfg(feature = "render")]
pub use render::*;
