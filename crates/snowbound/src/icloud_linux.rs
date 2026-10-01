//! iCloud Drive is Apple's; on Linux and Windows no folder is in it.

use notebook::session::{Background, Notebook, Section};
use std::path::{Path, PathBuf};

pub fn ubiquitous(_: &Path) -> bool {
    false
}

pub fn folder() -> Option<PathBuf> {
    None
}

pub fn look_up(_: impl Fn() + Send + Sync + 'static) {}

pub fn download(_: &Path) -> usize {
    0
}

pub fn drive() -> Option<PathBuf> {
    None
}

pub fn section(
    notebook: &Notebook,
    _: &Path,
    path: &str,
    key: Option<&onestore::protected::Key>,
    notify: impl Fn() + Send + 'static,
) -> Result<Section, notebook::Error> {
    match key {
        Some(key) => notebook.section_unlocked(path, key, notify),
        None => notebook.section(path, notify),
    }
}

pub fn lone_section(
    file: &Path,
    cache: &Path,
    notify: impl Fn() + Send + 'static,
) -> Result<Section, notebook::Error> {
    Section::open(file, cache, notify)
}

pub fn background(
    notebook: &Notebook,
    notify: impl Fn() + Send + 'static,
) -> Result<Background, notebook::Error> {
    notebook.background(false, true, notify)
}

pub struct Presenter;

pub fn presenter(_: &Path, _: impl Fn(Vec<String>) + Send + Sync + 'static) -> Option<Presenter> {
    None
}

pub fn on_account_change(_: impl Fn() + 'static) {}
