use std::io::{self, ErrorKind};

#[cfg(any(unix, windows))]
use std::{
    fs::File,
    path::Path,
    sync::{Mutex, MutexGuard},
};

#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;

#[cfg(any(unix, windows))]
struct FileIo {
    #[cfg(unix)]
    file: File,
    #[cfg(windows)]
    file: std::sync::Arc<File>,
    /// OneNote's coordination bytes, unlocked when dropped.
    #[cfg(windows)]
    locks: Vec<file_guard::FileGuard<std::sync::Arc<File>>>,
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
            // share modes, taken as OneNote takes them: a writer's shared lock denies only
            // other writers, and a reader takes none, as `stable` sees past a commit.
            let smb = nix::sys::statfs::statfs(path.as_ref())
                .is_ok_and(|fs| fs.filesystem_type_name() == "smbfs");
            let lock = match (write, smb) {
                (true, false) => nix::libc::O_EXLOCK,
                (false, true) => 0,
                _ => nix::libc::O_SHLOCK,
            };
            options.custom_flags(lock | nix::libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // As OneNote opens a section: a reader shares it with everyone, a writer denies
            // other writers (FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_SHARE_DELETE).
            options.share_mode(if write { 0x1 | 0x4 } else { 0x1 | 0x2 | 0x4 });
        }
        #[cfg(windows)]
        let file = std::sync::Arc::new(options.open(path).map_err(|error| {
            // ERROR_SHARING_VIOLATION: another writer has it open.
            match error.raw_os_error() {
                Some(32) => ErrorKind::WouldBlock.into(),
                _ => error,
            }
        })?);
        #[cfg(unix)]
        let file = options.open(path.as_ref())?;
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use std::os::unix::fs::MetadataExt;
            file.try_lock()?;
            // Locked after it was opened, the file may since have been superseded.
            let (open, named) = (file.metadata()?, std::fs::metadata(path)?);
            if (open.dev(), open.ino()) != (named.dev(), named.ino()) {
                return Err(ErrorKind::ResourceBusy.into());
            }
        }
        Ok(Self {
            #[cfg(windows)]
            locks: onenote_locks(&file, write)?,
            file,
            _process: process,
            unlock_on_drop: true,
        })
    }

    #[cfg(unix)]
    fn release(&mut self) -> io::Result<()> {
        // A failed unlock may have reached the server; Drop must not repeat it.
        self.unlock_on_drop = false;
        self.file.unlock()
    }

    #[cfg(windows)]
    fn release(&mut self) -> io::Result<()> {
        self.unlock_on_drop = false;
        self.locks.clear();
        Ok(())
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

#[cfg(unix)]
impl Drop for FileIo {
    fn drop(&mut self) {
        if self.unlock_on_drop {
            let _ = self.file.unlock();
        }
    }
}

/// Windows takes OneNote 2010's own locks on the file, one byte each past any data, so
/// neither app's locks bar the other's reads: the reader byte shared, and to write, the
/// writer byte exclusively, as `notebook::smb` takes them on a share.
#[cfg(windows)]
fn onenote_locks(
    file: &std::sync::Arc<File>,
    write: bool,
) -> io::Result<Vec<file_guard::FileGuard<std::sync::Arc<File>>>> {
    use file_guard::Lock;
    let reader = file_guard::try_lock(file.clone(), Lock::Shared, 0xffff_fffb, 1)?;
    let mut locks = vec![reader];
    if write {
        locks.push(file_guard::try_lock(
            file.clone(),
            Lock::Exclusive,
            0xffff_fffd,
            1,
        )?);
    }
    Ok(locks)
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

/// Reads a snapshot, excluding writers as their commits exclude it, except on an SMB mount,
/// where it takes no lock, as OneNote's readers take none (macOS shares it with other readers).
/// A read that meets a commit in progress is read again, then refused as `WouldBlock`.
#[cfg(any(unix, windows))]
pub fn read_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    read_file_limited(path, usize::MAX)
}

/// `read_file`, rejecting a snapshot larger than the byte limit.
/// A size failure returns `FileTooLarge` without a partial snapshot.
#[cfg(any(unix, windows))]
pub fn read_file_limited(path: impl AsRef<Path>, limit: usize) -> io::Result<Vec<u8>> {
    let mut io = FileIo::open(path, false)?;
    let result = stable(|offset, output| io.read_at(offset, output), limit);
    let released = io.release();
    let bytes = result?;
    released?;
    Ok(bytes)
}

/// How many times a read that meets a commit in progress is made before it is refused.
#[cfg(any(unix, windows))]
const TRIES: usize = 3;

/// The file through `read`, read again where a commit tore it: its header changed while it was
/// read, as commits write the header last, or it ends short of the header's length.
#[cfg(any(unix, windows))]
fn stable(
    mut read: impl FnMut(u64, &mut [u8]) -> io::Result<usize>,
    limit: usize,
) -> io::Result<Vec<u8>> {
    let mut block = vec![0; 1 << 16];
    for _ in 0..TRIES {
        let mut bytes = Vec::new();
        loop {
            let size = (limit.saturating_add(1) - bytes.len()).min(block.len());
            match read(bytes.len() as u64, &mut block[..size]) {
                Ok(0) => break,
                Ok(count) if count <= size => bytes.extend_from_slice(&block[..count]),
                Ok(_) => return Err(ErrorKind::InvalidData.into()),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
            if bytes.len() > limit {
                return Err(ErrorKind::FileTooLarge.into());
            }
        }
        let mut header = vec![0; bytes.len().min(1024)];
        match crate::snapshot::read_exact(&mut read, 0, &mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => continue,
            Err(error) => return Err(error),
        }
        let whole = crate::Header::parse(&bytes)
            .map_or(true, |parsed| parsed.expected_length <= bytes.len() as u64);
        if header == bytes[..header.len()] && whole {
            return Ok(bytes);
        }
    }
    Err(ErrorKind::WouldBlock.into())
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

/// Puts the file at `with` in the place of the file at `path`, provided `path` still has
/// `base`'s stamp, under the exclusion `commit_file` takes: the whole-image write OneNote 2010
/// makes when it writes a section anew.
#[cfg(any(unix, windows))]
pub fn supersede_file(
    path: impl AsRef<Path>,
    base: &Stamp,
    with: impl AsRef<Path>,
) -> Result<(), CommitError> {
    let (path, with) = (path.as_ref(), with.as_ref());
    let failed = |state| move |error| CommitError { state, error };
    // Windows renames nothing over an open file, so there the old file goes aside first and is
    // deleted once released, as OneNote's maintenance does.
    let mut aside = path.as_os_str().to_owned();
    if cfg!(windows) {
        aside.push(".old");
    }
    let aside = std::path::PathBuf::from(aside);
    let mut io = FileIo::open(path, true).map_err(failed(CommitState::NotCommitted))?;
    let result = base
        .check(&mut io)
        .and_then(|()| std::fs::rename(path, &aside))
        .map_err(failed(CommitState::NotCommitted))
        .and_then(|()| {
            std::fs::rename(with, path).map_err(|error| CommitError {
                state: match std::fs::rename(&aside, path) {
                    Ok(()) => CommitState::NotCommitted,
                    Err(_) => CommitState::Unknown,
                },
                error,
            })
        });
    io.finish(result)?;
    if aside != path {
        std::fs::remove_file(aside).map_err(failed(CommitState::Committed))?;
    }
    Ok(())
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

    /// Checks that the file `io` reads still has this stamp, reading its header and probing its
    /// length without reading the body; `ResourceBusy` when it moved on. The caller holds
    /// OneNote-compatible exclusion for whatever the check guards.
    pub fn check(&self, io: &mut impl CommitIo) -> io::Result<()> {
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
    /// Combines a following transaction into one guarded publication, retaining every revision.
    pub fn extend(&mut self, next: Transaction) -> Result<(), crate::Error> {
        let length = self.base.length + self.append.len() as u64;
        if next.base.header != self.header || next.base.length != length {
            return Err(crate::Error {
                offset: 0,
                message: "Transactions are not consecutive",
            });
        }
        if next.patches.iter().any(|(offset, bytes)| {
            *offset < 1024 || offset.saturating_add(bytes.len() as u64) > length
        }) {
            return Err(crate::Error {
                offset: 0,
                message: "A patch outside the base's data",
            });
        }
        for (offset, bytes) in next.patches {
            let earlier = (self.base.length.saturating_sub(offset) as usize).min(bytes.len());
            if earlier != 0 {
                self.patches.push((offset, bytes[..earlier].to_vec()));
            }
            if earlier < bytes.len() {
                let at = (offset + earlier as u64 - self.base.length) as usize;
                self.append[at..at + bytes.len() - earlier].copy_from_slice(&bytes[earlier..]);
            }
        }
        self.append.extend_from_slice(&next.append);
        self.header = next.header;
        Ok(())
    }

    /// The image this transaction applies to.
    pub fn base(&self) -> &Stamp {
        &self.base
    }

    /// The transaction as bytes `from_bytes` reads back, to carry it to another machine to
    /// commit: the base header and length, the new header, the appended bytes' length and
    /// the bytes, then each patch's offset, length and bytes. Integers are little-endian.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(2064 + self.append.len());
        bytes.extend_from_slice(&self.base.header);
        bytes.extend_from_slice(&self.base.length.to_le_bytes());
        bytes.extend_from_slice(&self.header);
        bytes.extend_from_slice(&(self.append.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&self.append);
        for (offset, patch) in &self.patches {
            bytes.extend_from_slice(&offset.to_le_bytes());
            bytes.extend_from_slice(&(patch.len() as u32).to_le_bytes());
            bytes.extend_from_slice(patch);
        }
        bytes
    }

    /// Reads `to_bytes`'s form, refusing a patch outside the base's data area, which no
    /// transaction writes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::Error> {
        let malformed = |message| crate::Error { offset: 0, message };
        fn take<'a>(rest: &mut &'a [u8], length: usize) -> Result<&'a [u8], crate::Error> {
            let (taken, after) = rest.split_at_checked(length).ok_or(crate::Error {
                offset: 0,
                message: "A truncated transaction",
            })?;
            *rest = after;
            Ok(taken)
        }
        let rest = &mut &bytes[..];
        let base_header: [u8; 1024] = take(rest, 1024)?.try_into().expect("1024 bytes");
        let length = u64::from_le_bytes(take(rest, 8)?.try_into().expect("8 bytes"));
        let header: [u8; 1024] = take(rest, 1024)?.try_into().expect("1024 bytes");
        let appended = u64::from_le_bytes(take(rest, 8)?.try_into().expect("8 bytes"));
        let appended = usize::try_from(appended).map_err(|_| malformed("A huge append"))?;
        let append = take(rest, appended)?.to_vec();
        let mut patches = Vec::new();
        while !rest.is_empty() {
            let offset = u64::from_le_bytes(take(rest, 8)?.try_into().expect("8 bytes"));
            let size = u32::from_le_bytes(take(rest, 4)?.try_into().expect("4 bytes"));
            let patch = take(rest, size as usize)?;
            if offset < 1024 || offset.saturating_add(u64::from(size)) > length {
                return Err(malformed("A patch outside the base's data"));
            }
            patches.push((offset, patch.to_vec()));
        }
        Ok(Self {
            base: Stamp {
                header: base_header,
                length,
            },
            append,
            patches,
            header,
        })
    }

    /// The bytes a commit writes, by offset, in the order `apply` writes them; the header,
    /// last, covers bytes 0..1024.
    pub fn writes(&self) -> impl Iterator<Item = (u64, &[u8])> {
        std::iter::once((self.base.length, self.append.as_slice()))
            .chain(
                self.patches
                    .iter()
                    .map(|(offset, bytes)| (*offset, bytes.as_slice())),
            )
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

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    /// Reads `bytes`, changing a header byte on each of the first `changes` rereads of it.
    fn reader(bytes: &[u8], mut changes: usize) -> impl FnMut(u64, &mut [u8]) -> io::Result<usize> {
        let mut bytes = bytes.to_vec();
        let mut reads = 0;
        move |offset, output| {
            if offset == 0 {
                reads += 1;
                // Each pass reads the header twice: with the body, then to check it.
                if reads % 2 == 0 && changes > 0 {
                    changes -= 1;
                    bytes[1000] ^= 1;
                }
            }
            let rest = bytes.get(offset as usize..).unwrap_or_default();
            let count = rest.len().min(output.len());
            output[..count].copy_from_slice(&rest[..count]);
            Ok(count)
        }
    }

    #[test]
    fn consecutive_transactions_publish_the_same_bytes_together() {
        let original = vec![1; 2048];
        let first = Transaction {
            base: Stamp::of(&original).unwrap(),
            append: vec![2; 32],
            patches: vec![(1024, vec![3; 8])],
            header: [4; 1024],
        };
        let mut separate = original.clone();
        first.apply(&mut separate).unwrap();
        let second = Transaction {
            base: Stamp::of(&separate).unwrap(),
            append: vec![5; 16],
            patches: vec![(1028, vec![6; 8]), (2044, vec![7; 12])],
            header: [8; 1024],
        };
        second.apply(&mut separate).unwrap();
        let mut combined = first.clone();
        assert!(combined.extend(first).is_err());
        combined.extend(second).unwrap();
        let combined = Transaction::from_bytes(&combined.to_bytes()).unwrap();
        let mut together = original;
        combined.apply(&mut together).unwrap();
        assert_eq!(together, separate);
    }

    #[test]
    fn transactions_read_back_from_bytes() {
        let transaction = Transaction {
            base: Stamp {
                header: [1; 1024],
                length: 4096,
            },
            append: vec![2; 300],
            patches: vec![(1024, vec![3; 8]), (4000, vec![4; 96])],
            header: [5; 1024],
        };
        let bytes = transaction.to_bytes();
        assert_eq!(Transaction::from_bytes(&bytes).unwrap(), transaction);
        assert!(Transaction::from_bytes(&bytes[..bytes.len() - 1]).is_err());
        let outside = Transaction {
            patches: vec![(4000, vec![4; 97])],
            ..transaction.clone()
        };
        assert!(Transaction::from_bytes(&outside.to_bytes()).is_err());
        let header = Transaction {
            patches: vec![(1000, vec![4; 8])],
            ..transaction
        };
        assert!(Transaction::from_bytes(&header.to_bytes()).is_err());
    }

    #[test]
    fn a_read_torn_by_a_commit_is_read_again_then_refused() {
        let section = crate::create_section("Torn.one", "Text", "Fixture").unwrap();
        assert_eq!(stable(reader(&section, 0), section.len()).unwrap(), section);
        let mut changed = section.clone();
        changed[1000] ^= 1;
        assert_eq!(stable(reader(&section, 1), section.len()).unwrap(), changed);
        assert_eq!(
            stable(reader(&section, TRIES), section.len())
                .unwrap_err()
                .kind(),
            ErrorKind::WouldBlock
        );
        // Storage shortened ahead of the header that publishes its new length.
        let short = &section[..section.len() - 1];
        assert_eq!(
            stable(reader(short, 0), section.len()).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        assert_eq!(
            stable(reader(&section, 0), section.len() - 1)
                .unwrap_err()
                .kind(),
            ErrorKind::FileTooLarge
        );
        // Files that are not revision stores read as they are.
        assert_eq!(
            stable(reader(b"unfinished", 0), 100).unwrap(),
            b"unfinished"
        );
    }

    /// OneNote's reads go on through Snowbound's reads and writes, and its writers wait for
    /// Snowbound's, as its opens and coordination bytes meet Snowbound's.
    #[cfg(windows)]
    #[test]
    fn windows_takes_onenote_s_opens_and_bytes() {
        use file_guard::Lock;
        use std::io::Read;
        use std::os::windows::fs::OpenOptionsExt;
        let folder = std::env::temp_dir().join(format!("onestore-locks-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("Locks.one");
        std::fs::write(&path, b"section").unwrap();
        let onenote = |write: bool| {
            File::options()
                .read(true)
                .write(write)
                .share_mode(if write { 0x5 } else { 0x7 })
                .open(&path)
                .map(std::sync::Arc::new)
        };
        let busy = |result: io::Result<file_guard::FileGuard<std::sync::Arc<File>>>| {
            result.is_err_and(|error| error.kind() == ErrorKind::WouldBlock)
        };

        let reading = FileIo::open(&path, false).unwrap();
        let writer = onenote(true).unwrap();
        let reader_byte = file_guard::try_lock(writer.clone(), Lock::Shared, 0xffff_fffb, 1);
        assert!(reader_byte.is_ok(), "OneNote writes beside a reader");
        drop((reader_byte, writer));
        assert!(!busy(file_guard::try_lock(
            onenote(false).unwrap(),
            Lock::Exclusive,
            0xffff_fffd,
            1
        )));
        drop(reading);

        let writing = FileIo::open(&path, true).unwrap();
        let mut text = String::new();
        (&*onenote(false).unwrap())
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "section", "OneNote reads through a commit");
        assert!(onenote(true).is_err(), "a second writer can't open it");
        let other = onenote(false).unwrap();
        assert!(busy(file_guard::try_lock(
            other,
            Lock::Exclusive,
            0xffff_fffd,
            1
        )));
        drop(writing);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
