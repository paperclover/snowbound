#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use onestore::{
    ExGuid, Insertion, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{fs::OpenOptions, io, ops::Range, path::Path, sync::Mutex, time::Duration};

mod assets;
mod formatting;
mod rebase;
mod recovery;
mod schema;
pub use formatting::FormatEdit;
pub use recovery::{Recovery, RecoverySummary};
mod sync;
pub use sync::{ConflictKind, EditStatus, Remote};
mod worker;
pub use worker::SyncWorker;
#[cfg(feature = "smb")]
mod smb;
#[cfg(feature = "smb")]
pub use smb::SmbRemote;

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
    Notebook(#[from] onestore_notebook::Error),
    #[error("External payload identity now refers to different bytes")]
    AssetChanged,
}

type Result<T> = std::result::Result<T, Error>;

const APPLICATION_ID: u32 = 0x4f4e454f;
const SCHEMA_VERSION: u32 = 5;

/// Text and its observed precondition, retained across cache reopen and rebasing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    pub object: ExGuid,
    pub before: String,
    pub range: Range<u32>,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Text(TextEdit),
    Insert(Insertion),
    Format(FormatEdit),
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
            if application != APPLICATION_ID || !(1..=SCHEMA_VERSION).contains(&version) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unrecognized cache or unsupported schema version",
                )
                .into());
            }
            validate_images(&transaction)?;
            if version < SCHEMA_VERSION {
                schema::migrate(&transaction, version)?;
                transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            pending(&transaction)?;
        }
        transaction.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
            synchronization: Mutex::new(()),
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

    /// Atomically records an intent and its resulting local image; returns its durable ID.
    /// Unchanged text returns `None`. A stale image returns `Io(ResourceBusy)`.
    /// On synchronization, replacement text inherits the remote style at the rebased start.
    /// After a database error, reopen and inspect the cache before retrying the edit.
    pub fn edit_text(
        &self,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
        range: Range<u32>,
        replacement: &str,
    ) -> Result<Option<u64>> {
        let edit = PreparedEdit::text(source, space, object, range.clone(), replacement)?;
        let before = paragraph(source, space, object)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Prepared edit has no text target",
            )
        })?;
        self.record(
            source,
            space,
            Operation::Text(TextEdit {
                object,
                before,
                range,
                replacement: replacement.to_owned(),
            }),
            &edit,
        )
    }

    /// Durably queues a validated insertion with its stable object identities.
    /// Uses the same snapshot and local-acknowledgement contract as `edit_text`.
    pub fn insert(
        &self,
        source: &[u8],
        space: ExGuid,
        insertion: &Insertion,
    ) -> Result<Option<u64>> {
        let edit = PreparedEdit::insert(source, space, insertion)?;
        self.record(source, space, Operation::Insert(insertion.clone()), &edit)
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

fn paragraph(source: &[u8], space: ExGuid, object: ExGuid) -> Result<Option<String>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let node = document
        .spaces
        .get(&space)
        .and_then(|space| {
            space
                .contexts
                .get(&ExGuid::default())
                .and_then(|revision| space.revisions.get(revision))
        })
        .and_then(|revision| revision.nodes.get(&object));
    Ok(match node.map(|node| &node.kind) {
        Some(Kind::RichText { text, .. }) => Some(text.clone()),
        _ => None,
    })
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
        edits.push(PendingEdit {
            id: u64::try_from(row.get::<_, i64>(0)?).map_err(io::Error::other)?,
            space: row.get::<_, String>(1)?.parse()?,
            operation: serde_json::from_str(&row.get::<_, String>(2)?)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        });
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
