//! The file system notebooks and their replicas live in: std's natively. In the browser,
//! which gives a module none, it is `web`'s, which keeps SQLite's files beside the notebooks'.

#[cfg(not(target_arch = "wasm32"))]
pub use onestore::{confirm_file, place_file, read_file, read_file_limited, supersede_file};
#[cfg(not(target_arch = "wasm32"))]
pub use std::{fs::*, path::absolute, process::id as process_id};

/// `Transaction::commit_file` through this file system.
#[cfg(not(target_arch = "wasm32"))]
pub fn commit_file(
    transaction: &onestore::Transaction,
    path: impl AsRef<std::path::Path>,
) -> Result<(), onestore::CommitError> {
    transaction.commit_file(path)
}

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::*;
