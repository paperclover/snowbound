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
    pub working_bytes: u64,
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
        if version != SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Archive schema version {version} is not the supported version {SCHEMA_VERSION}"
                ),
            )
            .into());
        }
        validate_images(&connection)?;
        for edit in pending(&connection)? {
            sync::status(&connection, edit.id)?;
        }
        receipts(&connection)?;
        Ok(Self { connection })
    }

    pub fn summary(&self) -> Result<RecoverySummary> {
        summary(&self.connection)
    }

    pub fn snapshot(&self) -> Result<Vec<u8>> {
        crate::images::working(&self.connection)
    }

    pub fn remote_snapshot(&self) -> Result<Vec<u8>> {
        crate::images::base(&self.connection)
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
        let connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        summary(&connection)
    }

    /// Exports a consistent archive to a new local path without changing the live queue.
    /// Archives contain notebook content and cannot be opened as writable replicas.
    /// An error after publication can leave a complete archive at the destination.
    pub fn export_recovery(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
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
            let source = self
                .connection
                .lock()
                .map_err(|_| io::Error::other("Cache owner panicked"))?;
            let backup = Backup::new(&source, &mut destination)?;
            if backup.step(-1)? != StepResult::Done {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Recovery export could not acquire the database snapshot",
                )
                .into());
            }
        }
        destination.pragma_update(None, "application_id", RECOVERY_ID)?;
        destination.close().map_err(|(_, error)| error)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(path)
            .map_err(|error| error.error)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
}

fn summary(connection: &Connection) -> Result<RecoverySummary> {
    let (cached_assets, cached_asset_bytes) = connection.query_row(
        "SELECT count(*),coalesce(sum(length(data)),0) FROM assets",
        [],
        |row| Ok((unsigned(row, 0)?, unsigned(row, 1)?)),
    )?;
    let working_bytes = crate::images::working(connection)?.len() as u64;
    Ok(connection.query_row(
        "SELECT (SELECT count(*) FROM edits), (SELECT count(*) FROM conflicts),
                (SELECT count(*) FROM attempt), (SELECT count(*) FROM receipts),
                length(base) FROM replica WHERE id=1",
        [],
        |row| {
            Ok(RecoverySummary {
                queued_edits: unsigned(row, 0)?,
                conflicts: unsigned(row, 1)?,
                uncertain_edits: unsigned(row, 2)?,
                published_receipts: unsigned(row, 3)?,
                working_bytes,
                remote_bytes: unsigned(row, 4)?,
                cached_assets,
                cached_asset_bytes,
            })
        },
    )?)
}

fn receipts(connection: &Connection) -> Result<BTreeMap<u64, ExGuid>> {
    let mut statement =
        connection.prepare("SELECT edit_id, revision FROM receipts ORDER BY edit_id")?;
    let mut rows = statement.query([])?;
    let mut receipts = BTreeMap::new();
    while let Some(row) = rows.next()? {
        receipts.insert(unsigned(row, 0)?, row.get::<_, String>(1)?.parse()?);
    }
    Ok(receipts)
}

fn unsigned(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(column)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
