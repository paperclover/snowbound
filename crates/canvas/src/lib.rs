pub mod conflict;
pub mod document;
pub mod editor;
pub mod language;
pub mod layout;
pub mod math;
pub mod outline;
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

/// Parley's caret affinity mapped onto the page model's hidden-field affinity.
pub fn affinity(affinity: parley::Affinity) -> onestore::page::text::Affinity {
    match affinity {
        parley::Affinity::Upstream => onestore::page::text::Affinity::Upstream,
        parley::Affinity::Downstream => onestore::page::text::Affinity::Downstream,
    }
}
