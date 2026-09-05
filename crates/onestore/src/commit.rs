use crate::{ExGuid, replace_property_bytes};
use std::io::{self, ErrorKind};

#[cfg(any(unix, windows))]
use std::{
    fs::File,
    io::Read,
    path::Path,
    sync::{Mutex, MutexGuard},
};

#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;

#[cfg(any(unix, windows))]
struct FileIo {
    file: File,
    _process: MutexGuard<'static, ()>,
    unlock_on_drop: bool,
}

#[cfg(any(unix, windows))]
impl FileIo {
    fn open(path: impl AsRef<Path>, write: bool) -> io::Result<Self> {
        static PROCESS: Mutex<()> = Mutex::new(());
        let process = PROCESS
            .lock()
            .map_err(|_| io::Error::other("A file operation panicked in this process"))?;
        let mut options = File::options();
        options.read(true).write(write);
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // SMB can lose exclusion when separate opens race with flock.
            options.custom_flags(nix::libc::O_EXLOCK | nix::libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        #[cfg(not(target_os = "macos"))]
        file.try_lock()?;
        Ok(Self {
            file,
            _process: process,
            unlock_on_drop: true,
        })
    }

    fn release(&mut self) -> io::Result<()> {
        // A failed unlock may have reached the server; Drop must not repeat it.
        self.unlock_on_drop = false;
        self.file.unlock()
    }

    fn finish(mut self, result: Result<(), CommitError>) -> Result<(), CommitError> {
        let released = self.release();
        result?;
        released.map_err(|error| CommitError {
            state: CommitState::Committed,
            error,
        })
    }
}

#[cfg(any(unix, windows))]
impl Drop for FileIo {
    fn drop(&mut self) {
        if self.unlock_on_drop {
            let _ = self.file.unlock();
        }
    }
}

/// Reads a snapshot under the same whole-file exclusion used for commits.
/// Native writers can expose incomplete graphs to unlocked filesystem reads.
#[cfg(any(unix, windows))]
pub fn read_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut io = FileIo::open(path, false)?;
    let mut bytes = Vec::new();
    let result = io.file.read_to_end(&mut bytes);
    let released = io.release();
    result?;
    released?;
    Ok(bytes)
}

#[cfg(any(unix, windows))]
impl CommitIo for FileIo {
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        return self.file.read_at(bytes, offset);
        #[cfg(windows)]
        return self.file.seek_read(bytes, offset);
    }

    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(unix)]
        return self.file.write_at(bytes, offset);
        #[cfg(windows)]
        return self.file.seek_write(bytes, offset);
    }

    fn flush(&mut self) -> io::Result<()> {
        crate::flush::flush(&self.file)
    }
}

/// Commits under an exclusive whole-file lock; contention returns WouldBlock.
/// Calls are serialized within this process because macOS SMB locks are reentrant.
/// Durability relies on the filesystem and server honoring their flush contract.
/// A changed snapshot returns ResourceBusy without publishing the edit.
#[cfg(any(unix, windows))]
pub fn commit_file_property(
    path: impl AsRef<Path>,
    source: &[u8],
    space: ExGuid,
    object: ExGuid,
    property: u32,
    value: &[u8],
) -> Result<(), CommitError> {
    let mut io = FileIo::open(path, true).map_err(|error| CommitError {
        state: CommitState::NotCommitted,
        error,
    })?;
    let result = commit_property_bytes(&mut io, source, space, object, property, value);
    io.finish(result)
}

/// Commits a text edit with the same exclusion and snapshot check as scalar edits.
#[cfg(any(unix, windows))]
pub fn commit_file_text(
    path: impl AsRef<Path>,
    source: &[u8],
    space: ExGuid,
    object: ExGuid,
    range: std::ops::Range<u32>,
    replacement: &str,
) -> Result<(), CommitError> {
    let mut io = FileIo::open(path, true).map_err(|error| CommitError {
        state: CommitState::NotCommitted,
        error,
    })?;
    let result = commit_text(&mut io, source, space, object, range, replacement);
    io.finish(result)
}

/// Atomically publishes text and its dependent run boundaries under caller-held exclusion.
pub fn commit_text(
    io: &mut impl CommitIo,
    source: &[u8],
    space: ExGuid,
    object: ExGuid,
    range: std::ops::Range<u32>,
    replacement: &str,
) -> Result<(), CommitError> {
    let written =
        crate::replace_text(source, space, object, range, replacement).map_err(|error| {
            CommitError {
                state: CommitState::NotCommitted,
                error: io::Error::new(ErrorKind::InvalidData, error),
            }
        })?;
    commit_bytes(io, source, &written)
}

/// The caller must hold OneNote-compatible exclusion for the entire operation.
/// Flush must make preceding writes durable before subsequent writes can persist.
pub trait CommitIo {
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> io::Result<usize>;
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize>;
    fn flush(&mut self) -> io::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitState {
    /// Publication did not occur; preparation bytes may remain. Reread before retrying.
    NotCommitted,
    /// Publication may have persisted. Reread and reconcile the intent before retrying.
    Unknown,
    /// Publication was durably acknowledged, but cleanup failed. Do not replay the edit.
    Committed,
}

#[derive(Debug)]
pub struct CommitError {
    pub state: CommitState,
    pub error: io::Error,
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.state, self.error)
    }
}

impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

fn write_all(io: &mut impl CommitIo, mut offset: usize, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match io.write_at(offset as u64, bytes) {
            Ok(0) => return Err(io::Error::from(ErrorKind::WriteZero)),
            Ok(count) if count <= bytes.len() => {
                offset += count;
                bytes = &bytes[count..];
            }
            Ok(_) => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "Storage returned an excessive write count",
                ));
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Publishes a scalar revision after checking the locked file against its snapshot.
/// Unknown outcomes require rereading; Committed errors affect counter cleanup or lock release.
pub fn commit_property_bytes(
    io: &mut impl CommitIo,
    source: &[u8],
    space: ExGuid,
    object: ExGuid,
    property: u32,
    value: &[u8],
) -> Result<(), CommitError> {
    let written =
        replace_property_bytes(source, space, object, property, value).map_err(|error| {
            CommitError {
                state: CommitState::NotCommitted,
                error: io::Error::new(ErrorKind::InvalidData, error),
            }
        })?;
    commit_bytes(io, source, &written)
}

pub(crate) fn commit_bytes(
    io: &mut impl CommitIo,
    source: &[u8],
    written: &[u8],
) -> Result<(), CommitError> {
    let mut state = CommitState::NotCommitted;
    let result = (|| -> io::Result<()> {
        let mut buffer = [0; 65536];
        let mut offset = 0;
        while offset < source.len() {
            let size = buffer.len().min(source.len() - offset);
            let count = match io.read_at(offset as u64, &mut buffer[..size]) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                return Err(io::Error::from(ErrorKind::UnexpectedEof));
            }
            if count > size || buffer[..count] != source[offset..offset + count] {
                return Err(io::Error::new(
                    ErrorKind::ResourceBusy,
                    "The locked file differs from the edit snapshot",
                ));
            }
            offset += count;
        }
        if io.read_at(source.len() as u64, &mut buffer[..1])? != 0 {
            return Err(io::Error::new(
                ErrorKind::ResourceBusy,
                "The locked file grew after the edit snapshot",
            ));
        }
        if written == source {
            state = CommitState::Unknown;
            io.flush()?;
            return Ok(());
        }
        write_all(io, source.len(), &written[source.len()..])?;
        let mut offset = 1024;
        while offset < source.len() {
            if source[offset] == written[offset] {
                offset += 1;
                continue;
            }
            let start = offset;
            while offset < source.len() && source[offset] != written[offset] {
                offset += 1;
            }
            write_all(io, start, &written[start..offset])?;
        }
        io.flush()?;
        write_all(io, 100, &written[100..1024])?;
        io.flush()?;
        let highest = (96..100).rfind(|at| source[*at] != written[*at]).unwrap();
        state = CommitState::Unknown;
        write_all(io, highest, &written[highest..highest + 1])?;
        io.flush()?;
        state = CommitState::Committed;
        if highest > 96 {
            write_all(io, 96, &written[96..highest])?;
            io.flush()?;
        }
        Ok(())
    })();
    result.map_err(|error| CommitError { state, error })
}
