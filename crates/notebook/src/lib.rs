// One Win32 call, where std has no safe form: `session`'s hidden attribute.
#![deny(unsafe_code)]
#![doc = include_str!("../README.md")]

pub mod discover;
pub mod fs;
#[cfg(feature = "live")]
pub mod live;
#[cfg(feature = "smb")]
pub mod smb;

use fs::OpenOptions;
use onestore::{
    ExGuid,
    op::{Edit, OpError},
    page::Page,
    protected::Key,
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, mpsc},
    time::Duration,
};

mod assets;
mod background;
mod base;
pub mod location;
mod merge;
mod migrate;
pub mod package;
mod queue;
mod recovery;
mod resolve;
pub use resolve::Version;
mod schema;
pub mod session;
pub use recovery::{Recovery, RecoverySummary};
pub mod sidecar;
mod sync;
pub use sync::{EditStatus, Remote, Synced};
mod task;
mod worker;
mod working;
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
    /// The section refused an edit; the pages it names are as they were.
    #[error(transparent)]
    Rejected(#[from] OpError),
    #[error("External payload identity now refers to different bytes")]
    AssetChanged,
    /// A password-protected section's password did not match, or its format is unknown.
    #[error(transparent)]
    Protected(#[from] onestore::protected::Error),
}

impl Error {
    /// Whether the replica is open elsewhere, as in another section session of this process.
    pub fn busy(&self) -> bool {
        matches!(self, Self::Database(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::DatabaseBusy)
    }
}

type Result<T> = std::result::Result<T, Error>;

const APPLICATION_ID: u32 = 0x4f4e454f;

/// A locally durable edit; its ID remains stable across cache reopen.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingEdit {
    pub id: u64,
    pub author: String,
    pub edit: Edit,
}

/// How an uncertain attempt ends after review: `Mine` publishes it again, `Theirs` drops
/// the whole unpublished branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Mine,
    Theirs,
}

/// Owns one local cache. Share this handle between threads; a second open fails busy.
/// Edits apply on the cache's section thread, which keeps the section parsed; SQLite's
/// exclusive connection retains ownership between local transactions.
pub struct Replica {
    section: Arc<working::Thread>,
    synchronization: Mutex<()>,
    /// The section's root object space, which names the document.
    root: ExGuid,
    /// Last, so that it runs once the fields above have closed the cache.
    released: Released,
}

/// Runs once the replica is released and its cache closed, as `Background::hold` asks.
#[derive(Default)]
struct Released(Mutex<Option<Box<dyn FnOnce() + Send>>>);

impl Drop for Released {
    fn drop(&mut self) {
        if let Some(released) = self.0.get_mut().ok().and_then(Option::take) {
            released();
        }
    }
}

impl Replica {
    /// Seeds a new cache from a validated section image, refusing any existing path.
    /// An initialization error preserves the created file for inspection.
    pub fn create(path: impl AsRef<Path>, source: &[u8]) -> Result<Self> {
        Self::seed(path.as_ref(), source, None)?;
        Self::start(cache_connection(path.as_ref())?, None)
    }

    /// Opens the cache at `path`, a protected section's under `key`, first creating it from
    /// the image `source` reads where there is none. A cache another thread creates meanwhile,
    /// as the background makes an offline copy, is opened instead.
    pub fn open_or_create(
        path: impl AsRef<Path>,
        key: Option<&Key>,
        source: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let path = path.as_ref();
        if fs::metadata(path).is_err() {
            match Self::seed(path, &source()?, key) {
                Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {}
                seeded => seeded?,
            }
        }
        Self::open_with(path, key.cloned())
    }

    /// `create` without opening the cache it made, as for an offline copy.
    pub(crate) fn seed(path: &Path, source: &[u8], key: Option<&Key>) -> Result<()> {
        validate(source, key)?;
        // Built under a name of its own and linked into place whole, so that a cache that
        // exists is complete, and two threads making the same one never share a build.
        static BUILDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let build = BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut building = path.as_os_str().to_owned();
        building.push(format!(".creating-{}-{build}", fs::process_id()));
        let building = PathBuf::from(building);
        let built = Self::build(&building, source);
        let linked = built.and_then(|()| Ok(fs::hard_link(&building, path)?));
        for suffix in ["", "-wal", "-shm"] {
            let mut file = building.as_os_str().to_owned();
            file.push(suffix);
            let _ = fs::remove_file(file);
        }
        linked
    }

    fn build(path: &Path, source: &[u8]) -> Result<()> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        drop(options.open(path)?);
        let mut connection = cache_connection(path)?;
        write_ahead(&connection)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        let tables: i64 =
            transaction.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
        let application: u32 =
            transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
        if application != 0 || tables != 0 {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Cache initialization found an existing database",
            )
            .into());
        }
        transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
        transaction.pragma_update(None, "user_version", schema::VERSION)?;
        schema::create(&transaction)?;
        base::write(&transaction, base::Image::Base, source)?;
        transaction.commit()?;
        Ok(connection.close().map_err(|(_, error)| error)?)
    }

    /// Reopens an existing cache and its durable pending edits without network access.
    /// A schema-14 cache is converted once, after exporting it to `<path>.v14-recovery`;
    /// a conversion that cannot reproduce every queued page leaves it untouched.
    /// Unrecognized databases and unsupported journal modes are rejected.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path.as_ref(), None)
    }

    fn open_with(path: &Path, key: Option<Key>) -> Result<Self> {
        let mut connection = cache_connection(path)?;
        let application: u32 =
            connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        if application != APPLICATION_ID {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Not a notebook cache").into());
        }
        let integrity: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "Cache integrity check failed").into(),
            );
        }
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        // A schema-14 cache converts in its rollback journal, so a failed conversion leaves
        // the file as it was.
        match version {
            migrate::VERSION => migrate::migrate(&mut connection, path)?,
            schema::PREVIOUS => schema::upgrade(&mut connection)?,
            schema::VERSION => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Cache schema version {version} is not the supported version {}",
                        schema::VERSION
                    ),
                )
                .into());
            }
        }
        write_ahead(&connection)?;
        Self::start(connection, key)
    }

    fn start(connection: Connection, key: Option<Key>) -> Result<Self> {
        let (section, root) = working::spawn(connection, key)?;
        Ok(Self {
            section,
            synchronization: Mutex::new(()),
            root,
            released: Released::default(),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        lock(&self.section.connection)
    }

    /// Hands `request` to the section thread.
    fn send(&self, request: working::Request) -> Result<()> {
        self.section.send(request)
    }

    /// Asks the section thread and waits for its answer.
    fn ask<T: Send + 'static>(
        &self,
        request: impl FnOnce(working::Reply<T>) -> working::Request,
    ) -> Result<T> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.send(request(Box::new(move |result| {
            let _ = sender.send(result);
        })))?;
        receiver
            .recv()
            .map_err(|_| io::Error::other("The section thread stopped"))?
    }

    /// Applies an edit and queues it durably for publication, returning its id once
    /// written. A refused edit returns `Rejected` and leaves every page as it was.
    pub fn apply(&self, author: &str, edit: Edit) -> Result<u64> {
        self.ask(|reply| working::Request::Apply {
            author: author.to_owned(),
            edit,
            reply,
        })
    }

    /// `apply` without waiting: `reply` runs on the section thread once the edit is
    /// durable or refused.
    pub(crate) fn submit(
        &self,
        author: &str,
        edit: Edit,
        reply: working::Reply<u64>,
    ) -> Result<()> {
        self.send(working::Request::Apply {
            author: author.to_owned(),
            edit,
            reply,
        })
    }

    /// The page in `space` as the queued edits leave it; O(page), for opening and reloading.
    pub fn page(&self, space: ExGuid) -> Result<Page> {
        self.ask(|reply| working::Request::Page { space, reply })
    }

    /// Page spaces, titles and outline levels (1 at the top) in section order.
    pub fn pages(&self) -> Result<Vec<(ExGuid, String, u32)>> {
        self.ask(|reply| working::Request::Pages { reply })
    }

    /// The conflict pages of each page that has them (`onestore::Section::conflicts`).
    pub fn conflicts(&self) -> Result<Vec<(ExGuid, Vec<onestore::ConflictPage>)>> {
        self.ask(|reply| working::Request::Conflicts { reply })
    }

    /// The versions of each page that has them (`onestore::Section::versions`).
    pub fn versions(&self) -> Result<Vec<(ExGuid, Vec<onestore::PageVersion>)>> {
        self.ask(|reply| working::Request::Versions { reply })
    }

    /// A page as one of its versions holds it; O(section).
    pub fn version(&self, space: ExGuid, version: ExGuid) -> Result<Page> {
        self.ask(|reply| working::Request::Version {
            space,
            version,
            reply,
        })
    }

    /// The section image the queued edits leave, the unsealed ones sealed as one more
    /// revision whose identities differ per call: O(section).
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        self.written()?;
        working::image(&*self.lock()?, self.section.key.as_ref())
    }

    /// Waits until the edits applied before it are written to the queue, as reads answer
    /// before an open burst of edits is.
    pub fn written(&self) -> Result<()> {
        self.ask(|reply| working::Request::Flush { reply })
    }

    /// The section file's identity, which internal links name as `section-id`.
    pub fn identity(&self) -> Result<[u8; 16]> {
        let stamp = base::base_stamp(&*self.lock()?)?;
        Ok(onestore::Header::parse(&stamp.header)?.file_id)
    }

    /// Queued edits, oldest first.
    pub fn pending(&self) -> Result<Vec<PendingEdit>> {
        pending(&*self.lock()?, self.section.key.as_ref())
    }

    /// The edits putting each page the queue changed, as the queue leaves it, into another
    /// section as a copy under fresh identities, by the author of the first edit changing it;
    /// content outside the page model stays behind.
    pub(crate) fn copies(&self) -> Result<Vec<(String, Edit)>> {
        let arena = onestore::Arena::default();
        let base = base::base(&*self.lock()?)?;
        let base = working::open(&arena, base, self.section.key.as_ref())?;
        let mut seen = std::collections::BTreeSet::from([self.root]);
        let mut copies = Vec::new();
        for queued in self.pending()? {
            for space in queue::spaces(&queued.edit, self.root) {
                if !seen.insert(space) {
                    continue;
                }
                let Ok(mut page) = self.page(space) else {
                    continue;
                };
                if base.page(space).is_ok_and(|base| base == page) {
                    continue;
                }
                page.objects
                    .retain(|object| !matches!(object, onestore::page::PageObject::Unsupported(_)));
                let creation =
                    onestore::PageCreation::new(None, Some(&page.title), &queued.author)?;
                let ops = vec![onestore::op::Op::Section(onestore::op::SectionOp::Import {
                    creation,
                    page: page.copy()?,
                })];
                copies.push((queued.author.clone(), Edit { at: now(), ops }));
            }
        }
        Ok(copies)
    }

    fn wake_sync(&self) {
        wake(&self.section.worker);
    }
}

impl Drop for Replica {
    fn drop(&mut self) {
        self.section.stop();
    }
}

fn lock(connection: &Mutex<Connection>) -> Result<MutexGuard<'_, Connection>> {
    connection
        .lock()
        .map_err(|_| io::Error::other("Cache owner panicked").into())
}

/// Tells the worker a burst of local edits is durable.
fn edited(worker: &Mutex<std::sync::Weak<worker::Signal>>) {
    if let Ok(worker) = worker.lock()
        && let Some(worker) = worker.upgrade()
    {
        worker.edited();
    }
}

fn wake(worker: &Mutex<std::sync::Weak<worker::Signal>>) {
    if let Ok(worker) = worker.lock()
        && let Some(worker) = worker.upgrade()
    {
        worker.wake();
    }
}

/// Fully validates a section image, a protected one under `key`, returning its root object
/// space.
fn validate(source: &[u8], key: Option<&Key>) -> Result<ExGuid> {
    let arena = onestore::Arena::default();
    Ok(match key {
        Some(key) => onestore::Section::unlock(&arena, source.to_vec(), key)?.root(),
        None => onestore::Section::open(&arena, source.to_vec())?.root(),
    })
}

/// A closed cache, without opening the replica.
fn closed(path: &Path) -> Result<Connection> {
    let connection = cache_connection(path)?;
    let application: u32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if application != APPLICATION_ID || version != schema::VERSION {
        return Err(io::Error::from(io::ErrorKind::InvalidData).into());
    }
    Ok(connection)
}

/// A closed cache's base stamp and how many edits wait.
fn peek(connection: &Connection) -> Result<(onestore::Stamp, u64)> {
    let base = base::base_stamp(connection)?;
    let queued: i64 = connection.query_row("SELECT count(*) FROM edits", [], |row| row.get(0))?;
    Ok((base, unsigned(queued)?))
}

fn pending(connection: &Connection, key: Option<&Key>) -> Result<Vec<PendingEdit>> {
    queue::load(connection, key, None)
}

fn unsigned(value: i64) -> Result<u64> {
    Ok(u64::try_from(value).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?)
}

fn signed(value: u64) -> Result<i64> {
    Ok(i64::try_from(value).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?)
}

/// Opens a cache without writing to it: exclusive locking, a full sync of every commit
/// (`F_FULLFSYNC` on macOS, the WAL's syncs included) and foreign keys, each queried back.
fn cache_connection(path: &Path) -> Result<Connection> {
    #[cfg(target_arch = "wasm32")]
    fs::install_sqlite();
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(Duration::ZERO)?;
    connection.execute_batch(
        "PRAGMA locking_mode=EXCLUSIVE; PRAGMA synchronous=FULL; PRAGMA fullfsync=ON; PRAGMA foreign_keys=ON;",
    )?;
    let locking: String = connection.pragma_query_value(None, "locking_mode", |row| row.get(0))?;
    let journal: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if locking != "exclusive" || !["wal", "delete"].contains(&journal.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported cache locking or journal mode",
        )
        .into());
    }
    for (name, expected) in [("synchronous", 2), ("fullfsync", 1), ("foreign_keys", 1)] {
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

/// Switches a cache to write-ahead logging: a commit appends its pages to `<cache>-wal`
/// instead of copying the originals to a rollback journal. Under exclusive locking the WAL
/// index lives in memory, so the WAL is the only file beside the cache; it is checkpointed
/// and removed on close, and replayed on the next open after a crash.
fn write_ahead(connection: &Connection) -> Result<()> {
    let mode: String =
        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if mode != "wal" {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "The cache cannot use write-ahead logging",
        )
        .into());
    }
    Ok(())
}

/// FILETIME now, as an edit's `at`: later than any this process took before, as Windows 7's
/// clock ticks only every 15.6 ms and a later edit must still order after an earlier one.
pub(crate) fn now() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let unix = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default();
    let clock =
        (unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(unix.subsec_nanos() / 100);
    let previous = LAST
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            Some(clock.max(last + 1))
        })
        .unwrap_or_default();
    clock.max(previous + 1)
}
