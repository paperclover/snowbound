pub mod conflict;
pub mod document;
pub mod editor;
pub mod language;
pub mod layout;
pub mod math;
pub mod outline;
pub mod reading;
pub mod search;
pub mod spelling;
pub mod template;

pub mod date;
#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(feature = "interaction")]
pub mod interaction;
#[cfg(feature = "pdf")]
pub mod print;
#[cfg(feature = "gpu")]
pub mod recording;

/// Where an object stored with `layout` lies; an unset coordinate is zero.
pub(crate) fn origin(layout: &onestore::document::Layout) -> [f32; 2] {
    [layout.x.unwrap_or(0.0), layout.y.unwrap_or(0.0)]
}

/// `[x0, y0, x1, y1]` moved by `[x, y]`.
pub(crate) fn translated([x0, y0, x1, y1]: [f32; 4], [x, y]: [f32; 2]) -> [f32; 4] {
    [x0 + x, y0 + y, x1 + x, y1 + y]
}

/// Parley's caret affinity mapped onto the page model's hidden-field affinity.
pub fn affinity(affinity: parley::Affinity) -> onestore::page::text::Affinity {
    match affinity {
        parley::Affinity::Upstream => onestore::page::text::Affinity::Upstream,
        parley::Affinity::Downstream => onestore::page::text::Affinity::Downstream,
    }
}
