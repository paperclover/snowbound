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

/// Reads a regular file within `limit`, including when it grows during the read.
pub fn read_limited(path: impl AsRef<std::path::Path>, limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    if metadata.len() > limit as u64 {
        return Err(std::io::ErrorKind::FileTooLarge.into());
    }
    let mut bytes = Vec::new();
    file.take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::ErrorKind::FileTooLarge.into());
    }
    Ok(bytes)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn durable() -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn bounded_reads_refuse_an_eight_gib_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        write(file.path(), b"hello").unwrap();
        assert_eq!(read_limited(file.path(), 5).unwrap(), b"hello");
        assert_eq!(
            read_limited(file.path(), 4).unwrap_err().kind(),
            std::io::ErrorKind::FileTooLarge
        );
        file.as_file().set_len(8 << 30).unwrap();
        assert_eq!(
            read_limited(file.path(), crate::MAX_FILE_BYTES)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::FileTooLarge
        );
        let folder = tempfile::tempdir().unwrap();
        assert!(read_limited(folder.path(), crate::MAX_FILE_BYTES).is_err());
    }
}
