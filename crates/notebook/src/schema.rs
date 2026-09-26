use super::*;
use rusqlite::{OptionalExtension, Transaction};

pub(crate) const VERSION: u32 = 16;
/// The schema whose batches still recorded review conflicts.
pub(crate) const PREVIOUS: u32 = 15;

/// Tables a schema-14 cache shares with this one.
pub(crate) const KEPT: &str = "
    CREATE TABLE receipts (
        edit_id INTEGER PRIMARY KEY CHECK(edit_id>0),
        revision TEXT NOT NULL
    ) STRICT;
    CREATE TABLE archived (
        edit_id INTEGER PRIMARY KEY CHECK(edit_id>0),
        archive TEXT NOT NULL
    ) STRICT;
    CREATE TABLE assets (
        name TEXT PRIMARY KEY NOT NULL,
        data BLOB NOT NULL,
        sha256 BLOB NOT NULL CHECK(length(sha256)=32)
    ) STRICT;";

/// The queue: the image its edits apply to, in chunks, and the edits in publication batches.
pub(crate) const QUEUE: &str = "
    CREATE TABLE base (chunk INTEGER PRIMARY KEY CHECK(chunk>=0), bytes BLOB NOT NULL) STRICT;
    CREATE TABLE remote (chunk INTEGER PRIMARY KEY CHECK(chunk>=0), bytes BLOB NOT NULL) STRICT;
    CREATE TABLE batches (
        id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),
        sealed TEXT,
        revisions TEXT,
        attempted INTEGER NOT NULL DEFAULT 0 CHECK(attempted IN (0,1)),
        CHECK((sealed IS NULL) = (revisions IS NULL)),
        CHECK(attempted=0 OR sealed IS NOT NULL)
    ) STRICT;
    CREATE TABLE edits (
        id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),
        batch INTEGER NOT NULL REFERENCES batches(id) ON DELETE CASCADE,
        author TEXT NOT NULL,
        edit TEXT NOT NULL,
        payloads TEXT
    ) STRICT;
    CREATE TABLE payloads (
        sha256 BLOB PRIMARY KEY CHECK(length(sha256)=32),
        bytes BLOB NOT NULL
    ) STRICT;";

pub(crate) fn create(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(QUEUE)?;
    transaction.execute_batch(KEPT)?;
    Ok(())
}

/// Converts a schema-15 cache: its batches lose the review conflict and its page, which
/// a merge no longer records. A conflict it held is queued work the next rebase keeps as
/// conflict pages. Foreign keys stay off while `edits` briefly references the old table.
pub(crate) fn upgrade(connection: &mut Connection) -> Result<()> {
    connection.execute_batch("PRAGMA foreign_keys=OFF")?;
    let upgraded = (|| -> Result<()> {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        let sequence: Option<i64> = transaction
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='batches'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        transaction.execute_batch(
            "CREATE TABLE upgraded (
                id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),
                sealed TEXT,
                revisions TEXT,
                attempted INTEGER NOT NULL DEFAULT 0 CHECK(attempted IN (0,1)),
                CHECK((sealed IS NULL) = (revisions IS NULL)),
                CHECK(attempted=0 OR sealed IS NOT NULL)
            ) STRICT;
            INSERT INTO upgraded(id, sealed, revisions, attempted)
                SELECT id, sealed, revisions, attempted FROM batches;
            DROP TABLE batches;
            ALTER TABLE upgraded RENAME TO batches;",
        )?;
        if let Some(sequence) = sequence {
            transaction.execute(
                "UPDATE sqlite_sequence SET seq=max(seq, ?1) WHERE name='batches'",
                [sequence],
            )?;
        }
        let dangling: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get(0),
        )?;
        if dangling {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "The schema-15 cache holds edits without their batch",
            )
            .into());
        }
        transaction.pragma_update(None, "user_version", VERSION)?;
        transaction.commit()?;
        Ok(())
    })();
    connection.execute_batch("PRAGMA foreign_keys=ON")?;
    upgraded
}
