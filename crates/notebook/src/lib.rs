#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub mod discover;
#[cfg(feature = "smb")]
pub mod smb;

use onestore::{
    ExGuid, PageCreation, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
    page::Page,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::{
    fs::OpenOptions,
    io,
    path::Path,
    sync::{Mutex, atomic::AtomicI64},
    time::Duration,
};

mod assets;
mod merge;
mod pages;
mod rebase;
mod recovery;
mod schema;
pub mod session;
pub use pages::PageEdits;
pub use recovery::{Recovery, RecoverySummary};
mod sync;
pub use sync::{ConflictKind, EditStatus, Remote};
mod worker;
#[cfg(feature = "smb")]
pub use smb::SmbRemote;
pub use worker::SyncWorker;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Document(#[from] onestore::Error),
    #[error(transparent)]
    Remote(#[from] onestore::CommitError),
    #[error(transparent)]
    RemoteIo(io::Error),
    #[error(transparent)]
    Discovery(#[from] discover::Error),
    #[error("External payload identity now refers to different bytes")]
    AssetChanged,
}

type Result<T> = std::result::Result<T, Error>;

const APPLICATION_ID: u32 = 0x4f4e454f;
const SCHEMA_VERSION: u32 = 12;

/// An edited page model together with the stored model it was edited from.
/// `before` is the precondition reconciliation checks against the remote page.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageIntent {
    pub before: Page,
    pub after: Page,
    pub author: String,
}

impl PageIntent {
    /// Describes the intent as one paragraph's text replacement, when that is all it changes:
    /// the text object, its text before, the replaced UTF-16 range and the replacement.
    pub fn text_change(&self) -> Option<(ExGuid, String, std::ops::Range<u32>, String)> {
        fn texts(page: &Page) -> Vec<(ExGuid, &onestore::page::Paragraph)> {
            let mut out = Vec::new();
            for object in &page.objects {
                let outlines: Vec<&onestore::page::Outline> = match object {
                    onestore::page::PageObject::Outline(outline) => vec![outline],
                    onestore::page::PageObject::Title(title) => title.outlines.iter().collect(),
                    _ => Vec::new(),
                };
                for outline in outlines {
                    for paragraph in &outline.paragraphs {
                        if let Some(text) = paragraph.text() {
                            out.push((text.id, &text.text));
                        }
                    }
                }
            }
            out
        }
        let (before, after) = (texts(&self.before), texts(&self.after));
        if before.len() != after.len() {
            return None;
        }
        let mut changed = None;
        for ((id, x), (other, y)) in before.iter().zip(&after) {
            if id != other {
                return None;
            }
            if x.text() != y.text() {
                if changed.is_some() {
                    return None;
                }
                changed = Some((*id, *x, *y));
            }
        }
        let (id, x, y) = changed?;
        let (b, o) = (x.text(), y.text());
        let prefix = b
            .char_indices()
            .zip(o.chars())
            .take_while(|((_, c), d)| c == d)
            .map(|((i, c), _)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        let suffix = b[prefix..]
            .chars()
            .rev()
            .zip(o[prefix..].chars().rev())
            .take_while(|(c, d)| c == d)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let start = x.utf16_offset(prefix).ok()?;
        let end = x.utf16_offset(b.len() - suffix).ok()?;
        Some((
            id,
            b.to_owned(),
            start..end,
            o[prefix..o.len() - suffix].to_owned(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    /// An edited page model; body content is edited only through this intent.
    Page(PageIntent),
    CreatePage(PageCreation),
    Pages(PageEdits),
    /// Permanent removal of explicitly selected page spaces from the section.
    DeletePages(Vec<ExGuid>),
}

/// A locally acknowledged intent; its ID remains stable across cache reopen.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingEdit {
    pub id: u64,
    pub space: ExGuid,
    pub operation: Operation,
}

/// Owns one local cache. Share this handle between threads; a second open fails busy.
/// SQLite's exclusive connection retains ownership between local transactions.
pub struct Replica {
    connection: Mutex<Connection>,
    synchronization: Mutex<()>,
    /// The intent a synchronization step selected for publication, or zero.
    in_flight: AtomicI64,
    worker: Mutex<std::sync::Weak<worker::Signal>>,
}

impl Replica {
    /// Seeds a new cache from a validated notebook image, refusing any existing path.
    /// An initialization error preserves the created file for inspection.
    pub fn create(path: impl AsRef<Path>, source: &[u8]) -> Result<Self> {
        validate(source)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        drop(options.open(path.as_ref())?);
        Self::connect(path.as_ref(), Some(source))
    }

    /// Reopens an existing cache and its durable pending edits without network access.
    /// Unrecognized databases and unsupported journal modes are rejected without conversion.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::connect(path.as_ref(), None)
    }

    fn connect(path: &Path, source: Option<&[u8]>) -> Result<Self> {
        let mut connection = cache_connection(path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        let application: u32 =
            transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: u32 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if let Some(source) = source {
            let tables: i64 =
                transaction
                    .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
            if application != 0 || version != 0 || tables != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Cache initialization found an existing database",
                )
                .into());
            }
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.execute_batch(
                "
                CREATE TABLE replica (
                    id INTEGER PRIMARY KEY CHECK(id=1),
                    base BLOB NOT NULL,
                    working BLOB NOT NULL
                ) STRICT;
            ",
            )?;
            schema::create(&transaction)?;
            transaction.execute("INSERT INTO replica VALUES (1, ?1, ?1)", [source])?;
        } else {
            if application != APPLICATION_ID {
                return Err(
                    io::Error::new(io::ErrorKind::InvalidData, "Not a notebook cache").into(),
                );
            }
            if version != SCHEMA_VERSION {
                // Older caches are refused rather than migrated until the application is
                // usable end to end; the queue must be drained by the build that wrote it.
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Cache schema version {version} is not the supported version {SCHEMA_VERSION}"),
                )
                .into());
            }
            validate_images(&transaction)?;
            pending(&transaction)?;
        }
        transaction.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
            synchronization: Mutex::new(()),
            in_flight: AtomicI64::new(0),
            worker: Mutex::new(std::sync::Weak::new()),
        })
    }

    /// Returns the latest complete locally committed image, including pending edits.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        Ok(
            connection.query_row("SELECT working FROM replica WHERE id=1", [], |row| {
                row.get(0)
            })?,
        )
    }

    pub fn pending(&self) -> Result<Vec<PendingEdit>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        pending(&connection)
    }

    /// Durably queues an edited page model using the supplied local snapshot.
    /// While the newest pending edit is an unattempted save of the same page, a new save
    /// replaces its result instead of queueing another publication, as OneNote does
    /// within its own save interval. An unchanged model returns `None`.
    pub fn save(
        &self,
        source: &[u8],
        space: ExGuid,
        after: &Page,
        author: &str,
    ) -> Result<Option<u64>> {
        let prepared = PreparedEdit::page(source, space, after, author)?;
        if prepared.as_bytes() == source {
            return Ok(None);
        }
        let before = page_of(source, space)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "The saved page is not in the local image",
            )
        })?;
        {
            let mut connection = self
                .connection
                .lock()
                .map_err(|_| io::Error::other("Cache owner panicked"))?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: Vec<u8> =
                transaction.query_row("SELECT working FROM replica WHERE id=1", [], |row| {
                    row.get(0)
                })?;
            if current != source {
                return Err(io::Error::new(
                    io::ErrorKind::ResourceBusy,
                    "The local snapshot changed before this edit",
                )
                .into());
            }
            let newest: Option<(i64, String, String)> = transaction
                .query_row(
                    "SELECT id, space, operation FROM edits WHERE id=(SELECT max(id) FROM edits)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            // The intent a step is publishing has no attempt row yet; rewriting it would lose
            // this save under the published bytes.
            if let Some((id, sid, operation)) = newest
                && id != self.in_flight.load(std::sync::atomic::Ordering::Acquire)
                && sid == space.to_string()
                && let Ok(Operation::Page(mut head)) = serde_json::from_str::<Operation>(&operation)
            {
                let attempted: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM attempt WHERE edit_id=?1) OR EXISTS(SELECT 1 FROM conflicts WHERE edit_id=?1)",
                    [id],
                    |row| row.get(0),
                )?;
                if !attempted {
                    head.after = after.clone();
                    transaction.execute(
                        "UPDATE edits SET operation=?1 WHERE id=?2",
                        params![
                            serde_json::to_string(&Operation::Page(head))
                                .map_err(io::Error::other)?,
                            id
                        ],
                    )?;
                    transaction.execute(
                        "UPDATE replica SET working=?1 WHERE id=1",
                        [prepared.as_bytes()],
                    )?;
                    transaction.commit()?;
                    drop(connection);
                    self.wake_sync();
                    return Ok(Some(u64::try_from(id).map_err(io::Error::other)?));
                }
            }
        }
        self.record(
            source,
            space,
            Operation::Page(PageIntent {
                before,
                after: after.clone(),
                author: author.to_owned(),
            }),
            &prepared,
        )
    }

    /// Queues a new page and its section entry with stable identities for dependent edits.
    pub fn create_page(&self, source: &[u8], page: &PageCreation) -> Result<Option<u64>> {
        let edit = PreparedEdit::create_page(source, page)?;
        self.record(
            source,
            page.space(),
            Operation::CreatePage(page.clone()),
            &edit,
        )
    }

    /// Queues the permanent removal of explicitly selected pages; the batch is republished
    /// only while every selected page still exists remotely.
    pub fn delete_pages(&self, source: &[u8], pages: &[ExGuid]) -> Result<Option<u64>> {
        let edit = PreparedEdit::delete_pages_permanently(source, pages)?;
        let space = validate(source)?;
        self.record(source, space, Operation::DeletePages(pages.to_vec()), &edit)
    }

    fn record(
        &self,
        source: &[u8],
        space: ExGuid,
        operation: Operation,
        edit: &PreparedEdit<'_>,
    ) -> Result<Option<u64>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Vec<u8> =
            transaction.query_row("SELECT working FROM replica WHERE id=1", [], |row| {
                row.get(0)
            })?;
        if current != source {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "The local snapshot changed before this edit",
            )
            .into());
        }
        if edit.as_bytes() == source {
            return Ok(None);
        }
        transaction.execute(
            "INSERT INTO edits(space, operation) VALUES (?1, ?2)",
            params![
                space.to_string(),
                serde_json::to_string(&operation).map_err(io::Error::other)?
            ],
        )?;
        let id = u64::try_from(transaction.last_insert_rowid()).map_err(io::Error::other)?;
        transaction.execute(
            "UPDATE replica SET working=?1 WHERE id=1",
            [edit.as_bytes()],
        )?;
        transaction.commit()?;
        drop(connection);
        self.wake_sync();
        Ok(Some(id))
    }

    fn wake_sync(&self) {
        if let Ok(worker) = self.worker.lock()
            && let Some(worker) = worker.upgrade()
        {
            worker.wake();
        }
    }
}

fn page_of(source: &[u8], space: ExGuid) -> Result<Option<Page>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    Ok(Page::from_space(&document, space).ok())
}

fn validate(source: &[u8]) -> Result<ExGuid> {
    let store = Store::parse(source)?;
    if !store.checksum_mismatches.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Notebook transaction checksum damage",
        )
        .into());
    }
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    Document::parse(&index)?;
    Ok(index.root)
}

fn pending(connection: &Connection) -> Result<Vec<PendingEdit>> {
    let mut query = connection.prepare("SELECT id, space, operation FROM edits ORDER BY id")?;
    let mut rows = query.query([])?;
    let mut edits = Vec::new();
    while let Some(row) = rows.next()? {
        let edit = PendingEdit {
            id: u64::try_from(row.get::<_, i64>(0)?).map_err(io::Error::other)?,
            space: row.get::<_, String>(1)?.parse()?,
            operation: serde_json::from_str(&row.get::<_, String>(2)?)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        };
        if matches!(&edit.operation, Operation::CreatePage(page) if page.space() != edit.space) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Cached page identity differs from its creation intent",
            )
            .into());
        }
        edits.push(edit);
    }
    Ok(edits)
}

fn cache_connection(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(Duration::ZERO)?;
    connection.execute_batch(
        "PRAGMA locking_mode=EXCLUSIVE; PRAGMA synchronous=EXTRA; PRAGMA fullfsync=ON; PRAGMA foreign_keys=ON;",
    )?;
    for (name, expected) in [("locking_mode", "exclusive"), ("journal_mode", "delete")] {
        let actual: String = connection.pragma_query_value(None, name, |row| row.get(0))?;
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported cache locking or journal mode",
            )
            .into());
        }
    }
    for (name, expected) in [("synchronous", 3), ("fullfsync", 1), ("foreign_keys", 1)] {
        let actual: i64 = connection.pragma_query_value(None, name, |row| row.get(0))?;
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Required cache synchronization is unavailable",
            )
            .into());
        }
    }
    Ok(connection)
}

fn validate_images(connection: &Connection) -> Result<()> {
    let integrity: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(
            io::Error::new(io::ErrorKind::InvalidData, "Cache integrity check failed").into(),
        );
    }
    let (base, working): (Vec<u8>, Vec<u8>) =
        connection.query_row("SELECT base, working FROM replica WHERE id=1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    if validate(&base)? != validate(&working)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Cache images belong to different documents",
        )
        .into());
    }
    Ok(())
}
