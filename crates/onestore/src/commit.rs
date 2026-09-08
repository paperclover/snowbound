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
    read_file_limited(path, usize::MAX)
}

/// Reads under whole-file exclusion, rejecting a snapshot larger than the byte limit.
/// A size failure returns `FileTooLarge` without a partial snapshot.
#[cfg(any(unix, windows))]
pub fn read_file_limited(path: impl AsRef<Path>, limit: usize) -> io::Result<Vec<u8>> {
    let count = u64::try_from(limit)
        .map_err(|_| ErrorKind::InvalidInput)?
        .saturating_add(1);
    let mut io = FileIo::open(path, false)?;
    let mut bytes = Vec::new();
    let result = (&mut io.file).take(count).read_to_end(&mut bytes);
    let released = io.release();
    result?;
    released?;
    if bytes.len() > limit {
        return Err(ErrorKind::FileTooLarge.into());
    }
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
    PreparedEdit::text(source, space, object, range, replacement)
        .map_err(|error| CommitError {
            state: CommitState::NotCommitted,
            error: io::Error::new(ErrorKind::InvalidData, error),
        })?
        .commit(io)
}

/// An immutable writer-generated transition tied to its original snapshot.
/// Persist intended revision identities from `as_bytes` before publishing an offline edit.
/// Missing identities after native maintenance do not prove an edit was never published.
pub struct PreparedEdit<'a> {
    source: &'a [u8],
    written: Vec<u8>,
}

impl<'a> PreparedEdit<'a> {
    /// Joins adjacent ordinary paragraphs with native left-tag and text-identity semantics.
    pub fn join(
        source: &'a [u8],
        space: ExGuid,
        join: &crate::ParagraphJoin,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: join.apply(source, space)?,
        })
    }

    /// Splits a paragraph and updates its children, lists, tags and title metadata atomically.
    /// Fields and associated run metadata are rejected before I/O.
    pub fn split(
        source: &'a [u8],
        space: ExGuid,
        split: &crate::ParagraphSplit,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: split.apply(source, space)?,
        })
    }

    /// Prepares an insertion and its dependent metadata in one revision, without I/O.
    pub fn insert(
        source: &'a [u8],
        space: ExGuid,
        insertion: &crate::Insertion,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: insertion.apply(source, space)?,
        })
    }

    /// Validates and prepares a text edit without I/O, with `replace_text` semantics.
    pub fn text(
        source: &'a [u8],
        space: ExGuid,
        object: ExGuid,
        range: std::ops::Range<u32>,
        replacement: &str,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: crate::replace_text(source, space, object, range, replacement)?,
        })
    }

    /// Changes character formatting over a UTF-16 range, preserving unselected runs and styles.
    /// A zero-length range sets the insertion style only when the paragraph is empty.
    /// Fields, associated run objects and boundaries splitting preserved run data are rejected.
    pub fn format(
        source: &'a [u8],
        space: ExGuid,
        object: ExGuid,
        range: std::ops::Range<u32>,
        attributes: &[crate::TextAttribute],
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: crate::formatting::format_text(source, space, object, range, attributes)?,
        })
    }

    /// The exact complete image this edit will publish; identities do not regenerate on commit.
    /// Do not overwrite a live notebook with this image; use `commit` under exclusion.
    pub fn as_bytes(&self) -> &[u8] {
        &self.written
    }

    /// Publishes these prepared bytes after comparing the entire original snapshot.
    /// The caller must retain OneNote-compatible exclusion through the returned outcome.
    pub fn commit(&self, io: &mut impl CommitIo) -> Result<(), CommitError> {
        commit_bytes(io, self.source, &self.written)
    }

    /// Publishes these exact bytes through the conservative whole-file filesystem adapter.
    /// A changed source returns ResourceBusy; an uncertain outcome must be reconciled before replay.
    #[cfg(any(unix, windows))]
    pub fn commit_file(&self, path: impl AsRef<Path>) -> Result<(), CommitError> {
        let mut io = FileIo::open(path, true).map_err(|error| CommitError {
            state: CommitState::NotCommitted,
            error,
        })?;
        let result = self.commit(&mut io);
        io.finish(result)
    }
}

/// Compares and flushes a snapshot, then refreshes its header version metadata.
/// No revision is added; reread before using the snapshot for another physical commit.
/// The caller must hold OneNote-compatible exclusion and independently establish which
/// intents the snapshot contains. A successful read alone is not a durable acknowledgement.
pub fn confirm_snapshot(io: &mut impl CommitIo, source: &[u8]) -> Result<(), CommitError> {
    let mut state = CommitState::NotCommitted;
    let result = (|| -> io::Result<()> {
        let header = crate::Header::parse(source).map_err(io::Error::other)?;
        let generation = header
            .generation
            .checked_add(1)
            .ok_or(ErrorKind::InvalidData)?;
        let mut version = [0; 40];
        version[..16].copy_from_slice(&crate::write::fresh_guid().map_err(io::Error::other)?);
        version[16..24].copy_from_slice(&generation.to_le_bytes());
        version[24..].copy_from_slice(&crate::write::fresh_guid().map_err(io::Error::other)?);
        compare_snapshot(io, source)?;
        state = CommitState::Unknown;
        io.flush()?;
        write_all(io, 212, &version)?;
        io.flush()
    })();
    result.map_err(|error| CommitError { state, error })
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
/// Unknown outcomes require rereading; Committed errors affect lock release.
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

fn compare_snapshot(io: &mut impl CommitIo, source: &[u8]) -> io::Result<()> {
    let capacity = source.len().clamp(1, 1024 * 1024);
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(capacity)
        .map_err(io::Error::other)?;
    buffer.resize(capacity, 0);
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
    Ok(())
}

pub(crate) fn commit_bytes(
    io: &mut impl CommitIo,
    source: &[u8],
    written: &[u8],
) -> Result<(), CommitError> {
    let mut state = CommitState::NotCommitted;
    let result = (|| -> io::Result<()> {
        compare_snapshot(io, source)?;
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
        write_all(io, 100, &written[100..212])?;
        write_all(io, 252, &written[252..1024])?;
        io.flush()?;
        let highest = (96..100).rfind(|at| source[*at] != written[*at]).unwrap();
        state = CommitState::Unknown;
        write_all(io, highest, &written[highest..highest + 1])?;
        io.flush()?;
        if highest > 96 {
            write_all(io, 96, &written[96..highest])?;
            io.flush()?;
        }
        // Native readers cache the version GUID without rechecking the transaction count.
        write_all(io, 212, &written[212..252])?;
        io.flush()?;
        Ok(())
    })();
    result.map_err(|error| CommitError { state, error })
}
