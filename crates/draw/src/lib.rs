//! What the page and the interface share: painting layers of glyphs, icons, paths,
//! images, rounded rectangles and pen strokes with wgpu, Direct3D 11 or OpenGL, each layer
//! with its own transform and clip, and editing text within a laid-out line.

pub mod edit;
#[cfg(feature = "render")]
mod render;

#[cfg(feature = "render")]
pub use render::*;

/// Appends the installed `families` to every script's fallback in `collection`, after the
/// system's, so a character that neither the text's fonts nor its script's fallback draws
/// comes from the first of them that has it, as the system's own text does.
pub fn fall_back_to(collection: &mut parley::fontique::Collection, families: &[String]) {
    use parley::fontique::{Script, ScriptExt};
    let extra: Vec<_> = families
        .iter()
        .filter_map(|name| collection.family_id(name))
        .collect();
    // Common, inherited and unknown characters keep runs of their own where no script's
    // text surrounds them.
    let scripts = Script::all_samples()
        .iter()
        .map(|(script, _)| *script)
        .chain([*b"Zyyy", *b"Zinh", *b"Zzzz"].map(Script::from_bytes));
    for script in scripts {
        let mut chain: Vec<_> = collection.fallback_families(script).collect();
        for family in &extra {
            if !chain.contains(family) {
                chain.push(*family);
            }
        }
        collection.set_fallbacks(script, chain.into_iter());
    }
}

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
