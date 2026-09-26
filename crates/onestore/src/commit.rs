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
            // SMB can lose exclusion when separate opens race with flock. On smbfs these are
            // share modes: a shared reader denies only writers, so OneNote's readers proceed.
            let lock = if write {
                nix::libc::O_EXLOCK
            } else {
                nix::libc::O_SHLOCK
            };
            options.custom_flags(lock | nix::libc::O_NONBLOCK);
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

/// Places a file in its notebook the way OneNote does on adoption: the header's
/// `guidAncestor` becomes the parent table of contents' file identity and `crcName` the CRC
/// of `name` (a section's file name, a group's folder name). OneNote re-identifies a file
/// whose header disagrees with its location, which orphans its TOC entry. The caller holds
/// OneNote-compatible exclusion on `io`.
pub fn place(io: &mut impl CommitIo, ancestor: [u8; 16], name: &str) -> io::Result<()> {
    let mut header = [0; 1024];
    if io.read_at(0, &mut header)? != header.len() {
        return Err(io::Error::from(ErrorKind::UnexpectedEof));
    }
    crate::Header::parse(&header)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error.message))?;
    let placement = crate::create::placement(ancestor, name);
    if io.write_at(128, &placement)? != placement.len() {
        return Err(io::Error::from(ErrorKind::WriteZero));
    }
    io.flush()
}

/// `place` under the conservative filesystem adapter's whole-file exclusion.
#[cfg(any(unix, windows))]
pub fn place_file(path: impl AsRef<Path>, ancestor: [u8; 16], name: &str) -> io::Result<()> {
    let mut io = FileIo::open(path, true)?;
    let result = place(&mut io, ancestor, name);
    let released = io.release();
    result?;
    released
}

/// Reads a snapshot excluding writers, as commits exclude everyone (macOS shares it with
/// other readers). Native writers can expose incomplete graphs to unlocked filesystem reads.
#[cfg(any(unix, windows))]
pub fn read_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    read_file_limited(path, usize::MAX)
}

/// `read_file`, rejecting a snapshot larger than the byte limit.
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

/// `confirm` under the same whole-file exclusion `commit_file` uses.
#[cfg(any(unix, windows))]
pub fn confirm_file(path: impl AsRef<Path>, base: &Stamp) -> Result<(), CommitError> {
    let mut io = FileIo::open(path, true).map_err(|error| CommitError {
        state: CommitState::NotCommitted,
        error,
    })?;
    let result = confirm(&mut io, base);
    io.finish(result)
}

/// Checks that the file still has `base`'s stamp and flushes it, then refreshes its header
/// version metadata. No revision is added; reread before committing on `base` again.
/// The caller must hold OneNote-compatible exclusion and independently establish which
/// intents the image contains. A successful read alone is not a durable acknowledgement.
pub fn confirm(io: &mut impl CommitIo, base: &Stamp) -> Result<(), CommitError> {
    let mut state = CommitState::NotCommitted;
    let result = (|| -> io::Result<()> {
        let header = crate::Header::parse(&base.header).map_err(io::Error::other)?;
        let generation = header
            .generation
            .checked_add(1)
            .ok_or(ErrorKind::InvalidData)?;
        let mut version = [0; 40];
        version[..16].copy_from_slice(&crate::write::fresh_guid().map_err(io::Error::other)?);
        version[16..24].copy_from_slice(&generation.to_le_bytes());
        version[24..].copy_from_slice(&crate::write::fresh_guid().map_err(io::Error::other)?);
        base.check(io)?;
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

fn write_all(io: &mut impl CommitIo, mut offset: u64, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match io.write_at(offset, bytes) {
            Ok(0) => return Err(io::Error::from(ErrorKind::WriteZero)),
            Ok(count) if count <= bytes.len() => {
                offset += count as u64;
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

/// What a commit requires unchanged since its snapshot: the header, which every committed
/// transaction and every placement rewrites (MS-ONESTORE 2.3.1 `guidFileVersion`), and the
/// length appends start from. Equal stamps name the same committed image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub header: [u8; 1024],
    pub length: u64,
}

impl Stamp {
    pub fn of(image: &[u8]) -> Result<Self, crate::Error> {
        Ok(Self {
            header: image.first_chunk().copied().ok_or(crate::Error {
                offset: 0,
                message: "Truncated revision-store header",
            })?,
            length: image.len() as u64,
        })
    }

    /// Reads the header and probes the length, without reading the body.
    fn check(&self, io: &mut impl CommitIo) -> io::Result<()> {
        let mut header = [0; 1024];
        crate::snapshot::read_exact(
            &mut |offset, output| io.read_at(offset, output),
            0,
            &mut header,
        )?;
        if header != self.header {
            return Err(io::Error::new(
                ErrorKind::ResourceBusy,
                "The file header changed after the edit snapshot",
            ));
        }
        let last = self.length.checked_sub(1).ok_or(ErrorKind::InvalidInput)?;
        let mut tail = [0; 2];
        let count = loop {
            match io.read_at(last, &mut tail) {
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                result => break result?,
            }
        };
        if count != 1 {
            return Err(io::Error::new(
                ErrorKind::ResourceBusy,
                "The file length changed after the edit snapshot",
            ));
        }
        Ok(())
    }
}

/// One change to a revision store as the bytes committing it writes: `append` at the base
/// length, `patches` inside the base's data area (list tails and the transaction log), and
/// the header that publishes them.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(into = "Wire", try_from = "Wire")]
pub struct Transaction {
    pub(crate) base: Stamp,
    pub(crate) append: Vec<u8>,
    pub(crate) patches: Vec<(u64, Vec<u8>)>,
    pub(crate) header: [u8; 1024],
}

/// A transaction as serialized: headers as byte strings, since serde stops at 32-byte arrays.
#[derive(serde::Serialize, serde::Deserialize)]
struct Wire {
    base: Vec<u8>,
    length: u64,
    append: Vec<u8>,
    patches: Vec<(u64, Vec<u8>)>,
    header: Vec<u8>,
}

impl From<Transaction> for Wire {
    fn from(transaction: Transaction) -> Self {
        Self {
            base: transaction.base.header.to_vec(),
            length: transaction.base.length,
            append: transaction.append,
            patches: transaction.patches,
            header: transaction.header.to_vec(),
        }
    }
}

impl TryFrom<Wire> for Transaction {
    type Error = &'static str;

    fn try_from(wire: Wire) -> Result<Self, Self::Error> {
        let header = |bytes: Vec<u8>| {
            <[u8; 1024]>::try_from(bytes).map_err(|_| "A transaction header is 1024 bytes")
        };
        Ok(Self {
            base: Stamp {
                header: header(wire.base)?,
                length: wire.length,
            },
            append: wire.append,
            patches: wire.patches,
            header: header(wire.header)?,
        })
    }
}

impl Transaction {
    /// The image this transaction applies to.
    pub fn base(&self) -> &Stamp {
        &self.base
    }

    /// The bytes a commit writes, by offset, in the order `apply` writes them; the header,
    /// last, covers bytes 0..1024.
    pub fn writes(&self) -> impl Iterator<Item = (u64, &[u8])> {
        std::iter::once((self.base.length, self.append.as_slice()))
            .chain(self.patches.iter().map(|(offset, bytes)| (*offset, bytes.as_slice())))
            .chain(std::iter::once((0, self.header.as_slice())))
    }

    /// Writes this transaction into its base image, as a successful commit leaves the file.
    pub fn apply(&self, image: &mut Vec<u8>) -> Result<(), crate::Error> {
        if Stamp::of(image)? != self.base {
            return Err(crate::Error {
                offset: 0,
                message: "The image is not this transaction's base",
            });
        }
        image.extend_from_slice(&self.append);
        for (offset, bytes) in &self.patches {
            let offset = *offset as usize;
            image[offset..offset + bytes.len()].copy_from_slice(bytes);
        }
        image[..1024].copy_from_slice(&self.header);
        Ok(())
    }

    /// Publishes under caller-held OneNote-compatible exclusion, provided the file still has
    /// the base stamp; otherwise returns ResourceBusy without writing. Appended data and
    /// patches are flushed before the header, and the transaction count commits them
    /// (MS-ONESTORE 2.3.3). Flush must make preceding writes durable before later ones.
    pub fn commit(&self, io: &mut impl CommitIo) -> Result<(), CommitError> {
        let mut state = CommitState::NotCommitted;
        let result = (|| -> io::Result<()> {
            self.base.check(io)?;
            if self.append.is_empty() && self.patches.is_empty() && self.header == self.base.header
            {
                state = CommitState::Unknown;
                return io.flush();
            }
            write_all(io, self.base.length, &self.append)?;
            for (offset, bytes) in &self.patches {
                write_all(io, *offset, bytes)?;
            }
            io.flush()?;
            write_all(io, 100, &self.header[100..212])?;
            write_all(io, 252, &self.header[252..])?;
            io.flush()?;
            state = CommitState::Unknown;
            // At a counter carry the highest changed byte commits; the lower ones follow.
            if let Some(highest) = (96..100).rfind(|at| self.base.header[*at] != self.header[*at]) {
                write_all(io, highest as u64, &self.header[highest..highest + 1])?;
                io.flush()?;
                if highest > 96 {
                    write_all(io, 96, &self.header[96..highest])?;
                    io.flush()?;
                }
            }
            // Native readers cache the version GUID without rechecking the transaction count.
            write_all(io, 212, &self.header[212..252])?;
            io.flush()
        })();
        result.map_err(|error| CommitError { state, error })
    }

    /// `commit` under the conservative filesystem adapter's whole-file exclusion.
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
