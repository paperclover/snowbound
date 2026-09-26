use super::*;
use rusqlite::Transaction;

pub(crate) const VERSION: u32 = 15;

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
        conflict INTEGER CHECK(conflict BETWEEN 0 AND 3),
        space TEXT,
        CHECK((sealed IS NULL) = (revisions IS NULL)),
        CHECK(attempted=0 OR sealed IS NOT NULL),
        CHECK((conflict IS NULL) = (space IS NULL))
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
