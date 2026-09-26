use super::*;
use rusqlite::backup::{Backup, StepResult};
use std::{collections::BTreeMap, fs::File};

const RECOVERY_ID: u32 = 0x4f4e4552;

/// Counts and image sizes without notebook text, paths, authors or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RecoverySummary {
    pub queued_edits: u64,
    pub conflicts: u64,
    pub uncertain_edits: u64,
    pub published_receipts: u64,
    /// Bytes of the image the queued edits apply to.
    pub base_bytes: u64,
    /// Bytes of the observed remote image, while it is not the base.
    pub remote_bytes: u64,
    pub cached_assets: u64,
    pub cached_asset_bytes: u64,
}

/// Read-only recovery evidence; it cannot publish or acknowledge an edit.
pub struct Recovery {
    connection: Connection,
}

impl Recovery {
    /// Opens an exported archive without migration or conversion into a writable replica.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::ZERO)?;
        connection.execute_batch("BEGIN")?;
        let application: u32 =
            connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if application != RECOVERY_ID {
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "Not a recovery archive").into(),
            );
        }
        if version != schema::VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Archive schema version {version} is not the supported version {}",
                    schema::VERSION
                ),
            )
            .into());
        }
        let recovery = Self { connection };
        recovery.snapshot()?;
        for edit in pending(&recovery.connection)? {
            sync::status(&recovery.connection, edit.id)?;
        }
        receipts(&recovery.connection)?;
        Ok(recovery)
    }

    pub fn summary(&self) -> Result<RecoverySummary> {
        summary(&self.connection)
    }

    /// The image the archived queue leaves, as `Replica::snapshot` gives it.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        working::image(&self.connection)
    }

    fn base(&self) -> Result<Vec<u8>> {
        Ok(
            base::read(&self.connection, base::Image::Base)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "The archive has no base image")
            })?,
        )
    }

    /// The last observed remote image.
    pub fn remote_snapshot(&self) -> Result<Vec<u8>> {
        match base::read(&self.connection, base::Image::Remote)? {
            Some(image) => Ok(image),
            None => self.base(),
        }
    }

    pub fn pending(&self) -> Result<Vec<PendingEdit>> {
        pending(&self.connection)
    }

    pub fn status(&self, id: u64) -> Result<Option<EditStatus>> {
        sync::status(&self.connection, id)
    }

    pub fn receipts(&self) -> Result<BTreeMap<u64, ExGuid>> {
        receipts(&self.connection)
    }

    /// Reads a previously downloaded external payload without accessing its former server.
    pub fn cached_asset(&self, filename: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        assets::cached(&self.connection, &assets::key(filename)?, limit)
    }
}

impl Replica {
    /// Summarizes durable state without exposing notebook content.
    pub fn recovery_summary(&self) -> Result<RecoverySummary> {
        summary(&*self.lock()?)
    }

    /// Exports a consistent archive to a new local path without changing the live queue.
    /// Archives contain notebook content and cannot be opened as writable replicas.
    /// An error after publication can leave a complete archive at the destination.
    pub fn export_recovery(&self, path: impl AsRef<Path>) -> Result<()> {
        export(&*self.lock()?, path.as_ref(), false)
    }
}

/// Copies the cache to `path` as a recovery archive; `replace` overwrites an existing file.
pub(crate) fn export(source: &Connection, path: &Path, replace: bool) -> Result<()> {
    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Provide a new recovery archive filename",
        )
        .into());
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".onestore-recovery-")
        .tempfile_in(parent)?;
    let mut destination = cache_connection(temporary.path())?;
    {
        let backup = Backup::new(source, &mut destination)?;
        if backup.step(-1)? != StepResult::Done {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Recovery export could not acquire the database snapshot",
            )
            .into());
        }
    }
    // One self-contained file: the copied header says WAL, as the cache does.
    destination.pragma_update(None, "journal_mode", "DELETE")?;
    destination.pragma_update(None, "application_id", RECOVERY_ID)?;
    destination.close().map_err(|(_, error)| error)?;
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(path).map_err(|error| error.error)?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|error| error.error)?;
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn summary(connection: &Connection) -> Result<RecoverySummary> {
    let (cached_assets, cached_asset_bytes): (i64, i64) = connection.query_row(
        "SELECT count(*),coalesce(sum(length(data)),0) FROM assets",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let length = |image| -> Result<u64> {
        Ok(base::stamp(connection, image)?.map_or(0, |stamp| stamp.length))
    };
    let (queued_edits, conflicts, uncertain_edits, published_receipts): (i64, i64, i64, i64) =
        connection.query_row(
            "SELECT (SELECT count(*) FROM edits),
                    (SELECT count(*) FROM edits JOIN batches ON batches.id=edits.batch WHERE conflict IS NOT NULL),
                    (SELECT count(*) FROM edits JOIN batches ON batches.id=edits.batch WHERE attempted=1),
                    (SELECT count(*) FROM receipts)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    Ok(RecoverySummary {
        queued_edits: unsigned(queued_edits)?,
        conflicts: unsigned(conflicts)?,
        uncertain_edits: unsigned(uncertain_edits)?,
        published_receipts: unsigned(published_receipts)?,
        base_bytes: length(base::Image::Base)?,
        remote_bytes: length(base::Image::Remote)?,
        cached_assets: unsigned(cached_assets)?,
        cached_asset_bytes: unsigned(cached_asset_bytes)?,
    })
}

fn receipts(connection: &Connection) -> Result<BTreeMap<u64, ExGuid>> {
    let mut statement =
        connection.prepare("SELECT edit_id, revision FROM receipts ORDER BY edit_id")?;
    let mut rows = statement.query([])?;
    let mut receipts = BTreeMap::new();
    while let Some(row) = rows.next()? {
        receipts.insert(unsigned(row.get(0)?)?, row.get::<_, String>(1)?.parse()?);
    }
    Ok(receipts)
}
