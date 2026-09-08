use super::*;
use rusqlite::{OptionalExtension, Transaction};

const CONFLICTS: &str = "CREATE TABLE conflicts (
    edit_id INTEGER PRIMARY KEY REFERENCES edits(id) ON DELETE CASCADE,
    kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 5)
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
            revision TEXT NOT NULL
        ) STRICT;
        CREATE TABLE receipts (
            edit_id INTEGER PRIMARY KEY CHECK(edit_id>0),
            revision TEXT NOT NULL
        ) STRICT;",
    )?;
    transaction.execute_batch(CONFLICTS)?;
    transaction.execute_batch(ASSETS)?;
    Ok(())
}

pub(crate) fn migrate(transaction: &Transaction<'_>, version: u32) -> Result<()> {
    if version >= 3 {
        transaction.execute_batch("ALTER TABLE conflicts RENAME TO old_conflicts;")?;
        transaction.execute_batch(CONFLICTS)?;
        transaction.execute_batch(
            "INSERT INTO conflicts SELECT * FROM old_conflicts; DROP TABLE old_conflicts;",
        )?;
        if version < 5 {
            transaction.execute_batch(ASSETS)?;
        }
        return Ok(());
    }

    let mut query = transaction.prepare(
        "SELECT id, space, object, before_text, start, end, replacement FROM edits ORDER BY id",
    )?;
    let mut rows = query.query([])?;
    let mut edits = Vec::new();
    while let Some(row) = rows.next()? {
        edits.push(PendingEdit {
            id: u64::try_from(row.get::<_, i64>(0)?).map_err(io::Error::other)?,
            space: row.get::<_, String>(1)?.parse()?,
            operation: Operation::Text(TextEdit {
                object: row.get::<_, String>(2)?.parse()?,
                before: row.get(3)?,
                range: row.get(4)?..row.get(5)?,
                replacement: row.get(6)?,
            }),
        });
    }
    drop(rows);
    drop(query);
    let sequence: i64 = transaction
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='edits'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let (mut attempts, mut conflicts, mut receipts) = (Vec::new(), Vec::new(), Vec::new());
    if version == 2 {
        let mut query = transaction.prepare("SELECT edit_id, revision FROM attempt")?;
        attempts = query
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let mut query = transaction.prepare("SELECT edit_id, kind FROM conflicts")?;
        conflicts = query
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut query = transaction.prepare("SELECT edit_id, revision FROM receipts")?;
        receipts = query
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        transaction
            .execute_batch("DROP TABLE attempt; DROP TABLE conflicts; DROP TABLE receipts;")?;
    }
    transaction.execute_batch("DROP TABLE edits;")?;
    create(transaction)?;
    for edit in edits {
        transaction.execute(
            "INSERT INTO edits(id,space,operation) VALUES (?1,?2,?3)",
            params![
                i64::try_from(edit.id).map_err(io::Error::other)?,
                edit.space.to_string(),
                serde_json::to_string(&edit.operation).map_err(io::Error::other)?
            ],
        )?;
    }
    // A drained queue must not reuse IDs belonging to existing durable receipts.
    transaction.execute("DELETE FROM sqlite_sequence WHERE name='edits'", [])?;
    transaction.execute(
        "INSERT INTO sqlite_sequence(name,seq) VALUES ('edits',?1)",
        [sequence],
    )?;
    for (id, revision) in attempts {
        transaction.execute(
            "INSERT INTO attempt VALUES (1,?1,?2)",
            params![id, revision],
        )?;
    }
    for (id, kind) in conflicts {
        transaction.execute("INSERT INTO conflicts VALUES (?1,?2)", params![id, kind])?;
    }
    for (id, revision) in receipts {
        transaction.execute("INSERT INTO receipts VALUES (?1,?2)", params![id, revision])?;
    }
    Ok(())
}
