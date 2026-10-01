//! The browser's notebooks change only through this page, so nothing reports their changes.

use std::path::Path;

pub struct Watch;

pub fn watch(_: &Path, _: impl Fn(Vec<String>) + Send + Sync + 'static) -> Option<Watch> {
    None
}

pub fn on_this_computer(_: &Path) -> bool {
    true
}
