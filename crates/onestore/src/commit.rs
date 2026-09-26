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
    /// Applies page moves in slice order and publishes final order and indentation atomically.
    /// Each page occurs once; all pages to move must be explicit, including selected subpages.
    pub fn pages(source: &'a [u8], edits: &[crate::PageEdit]) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: crate::pages::edit_pages(source, edits, &[])?,
        })
    }

    /// Applies table-of-contents edits (sections and section groups: add, rename, colour,
    /// order, remove) as one revision of a `.onetoc2` file.
    pub fn table_of_contents(
        source: &'a [u8],
        edits: &[crate::TocEdit],
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: crate::toc::edit_table_of_contents(source, edits)?,
        })
    }

    /// Permanently removes explicitly selected pages from the section in one transaction.
    /// Subpages must be selected explicitly; a surviving first subpage becomes top-level.
    /// Creates no recycle-bin copies. Prior revisions remain stored; this is not secure erasure.
    pub fn delete_pages_permanently(
        source: &'a [u8],
        pages: &[crate::ExGuid],
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: crate::pages::edit_pages(source, &[], pages)?,
        })
    }

    /// Publishes an edited page model as one revision per changed space.
    /// The model must come from this snapshot; new paragraphs and outlines carry the identities
    /// the model assigned, and content outside the model is untouched.
    pub fn page(
        source: &'a [u8],
        space: ExGuid,
        page: &crate::page::Page,
        author: &str,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: crate::page::write::write_page(source, space, page, author)?,
        })
    }

    /// `page` for a password-protected section: the revision is stored under the section's
    /// key. The model must come from this snapshot unlocked with `password`.
    #[cfg(feature = "protected")]
    pub fn page_protected(
        source: &'a [u8],
        password: &str,
        space: ExGuid,
        page: &crate::page::Page,
        author: &str,
    ) -> Result<Self, crate::protected::Error> {
        Ok(Self {
            source,
            written: crate::protected::write_page(source, password, space, page, author)?,
        })
    }

    /// Creates a page and its section entry in one transaction, retaining the intent's identities.
    pub fn create_page(source: &'a [u8], page: &crate::PageCreation) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: page.apply(source)?,
        })
    }

    /// Moves or removes a subtree and normalizes its containers in one revision.
    pub fn tree(
        source: &'a [u8],
        space: ExGuid,
        edit: &crate::TreeEdit,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: edit.apply(source, space)?,
        })
    }

    /// Changes outline geometry or a paragraph's saved expansion state, preserving content.
    pub fn outline(
        source: &'a [u8],
        space: ExGuid,
        object: ExGuid,
        edit: crate::OutlineEdit,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            source,
            written: edit.apply(source, space, object)?,
        })
    }

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

    /// The bytes publishing this edit writes, against its snapshot's stamp.
    pub fn transaction(&self) -> Transaction {
        Transaction::between(self.source, &self.written)
    }

    /// `Transaction::commit` of this edit.
    pub fn commit(&self, io: &mut impl CommitIo) -> Result<(), CommitError> {
        self.transaction().commit(io)
    }

    /// `Transaction::commit_file` of this edit.
    #[cfg(any(unix, windows))]
    pub fn commit_file(&self, path: impl AsRef<Path>) -> Result<(), CommitError> {
        self.transaction().commit_file(path)
    }
}

/// `confirm_snapshot` under the same whole-file exclusion `commit_file` uses.
#[cfg(any(unix, windows))]
pub fn confirm_file_snapshot(path: impl AsRef<Path>, source: &[u8]) -> Result<(), CommitError> {
    let mut io = FileIo::open(path, true).map_err(|error| CommitError {
        state: CommitState::NotCommitted,
        error,
    })?;
    let result = confirm_snapshot(&mut io, source);
    io.finish(result)
}

/// Checks and flushes a snapshot's stamp, then refreshes its header version metadata.
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
        Stamp::of(source).map_err(io::Error::other)?.check(io)?;
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
    Transaction::between(source, &written).commit(io)
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
    /// The transaction turning `source`, a parsed image, into `written`, which extends it.
    pub(crate) fn between(source: &[u8], written: &[u8]) -> Self {
        let differs = |at: &usize| source[*at] != written[*at];
        let mut patches = Vec::new();
        let mut offset = 1024;
        while let Some(start) = (offset..source.len()).find(differs) {
            let mut end = start + 1;
            // Equal gaps shorter than a write request's overhead join the patch.
            while let Some(next) = (end..source.len().min(end + 64)).find(differs) {
                end = next + 1;
            }
            patches.push((start as u64, written[start..end].to_vec()));
            offset = end;
        }
        Self {
            base: Stamp {
                header: *source.first_chunk().unwrap(),
                length: source.len() as u64,
            },
            append: written[source.len()..].to_vec(),
            patches,
            header: *written.first_chunk().unwrap(),
        }
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
