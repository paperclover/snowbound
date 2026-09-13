use super::*;
use rusqlite::Transaction;

const CONFLICTS: &str = "CREATE TABLE conflicts (
    edit_id INTEGER PRIMARY KEY REFERENCES edits(id) ON DELETE CASCADE,
    kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 3)
) STRICT;";

const ASSETS: &str = "CREATE TABLE assets (
    name TEXT PRIMARY KEY NOT NULL,
    data BLOB NOT NULL,
    sha256 BLOB NOT NULL CHECK(length(sha256)=32)
) STRICT;";

pub(crate) fn create(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE edits (
            id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),
            space TEXT NOT NULL,
            operation TEXT NOT NULL
        ) STRICT;
        CREATE TABLE attempt (
            id INTEGER PRIMARY KEY CHECK(id=1),
            edit_id INTEGER NOT NULL UNIQUE REFERENCES edits(id) ON DELETE CASCADE,
            revisions TEXT NOT NULL
        ) STRICT;
        CREATE TABLE receipts (
            edit_id INTEGER PRIMARY KEY CHECK(edit_id>0),
            revision TEXT NOT NULL
        ) STRICT;
        CREATE TABLE archived (
            edit_id INTEGER PRIMARY KEY CHECK(edit_id>0),
            archive TEXT NOT NULL
        ) STRICT;",
    )?;
    transaction.execute_batch(CONFLICTS)?;
    transaction.execute_batch(ASSETS)?;
    Ok(())
}
