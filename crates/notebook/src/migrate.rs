//! Converts a schema-14 cache, whose queue holds whole-page edits over a working image,
//! to this schema's op queue. The cache is exported first; each queued page is lowered to
//! ops against the page the conversion has so far, and every converted page must equal
//! the page in the old working image, or nothing changes.

use crate::{Result, base, queue, schema};
use onestore::{
    Arena, ExGuid, PageCreation, PageEdit, RevisionIndex, Section, Store,
    document::Document,
    op::{Edit, Op, SectionOp},
    page::Page,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{collections::BTreeMap, io, path::Path};

pub(crate) const VERSION: u32 = 14;

/// Schema 14's queued intents, as it serialized them.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
enum Operation {
    Page(PageIntent),
    CreatePage(PageCreation),
    Pages(PageEdits),
    DeletePages(Vec<ExGuid>),
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PageIntent {
    #[allow(dead_code)]
    before: Page,
    after: Page,
    author: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PageEdits {
    edits: Vec<PageEdit>,
    #[allow(dead_code)]
    observed: Vec<(ExGuid, u32)>,
}

fn invalid(message: String) -> crate::Error {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

/// Schema 14 stored the working image as its length and the runs where it differs from
/// the base.
fn working(mut base: Vec<u8>, patch: &[u8]) -> Result<Vec<u8>> {
    let damaged = || invalid("Damaged working image".into());
    let Some((length, mut runs)) = patch.split_first_chunk::<8>() else {
        return if patch.is_empty() {
            Ok(base)
        } else {
            Err(damaged())
        };
    };
    let length = usize::try_from(u64::from_le_bytes(*length))
        .ok()
        .filter(|length| *length <= base.len() + patch.len())
        .ok_or_else(damaged)?;
    base.resize(length, 0);
    while let Some((header, rest)) = runs.split_first_chunk::<16>() {
        let offset = usize::try_from(u64::from_le_bytes(header[..8].try_into().unwrap()))
            .map_err(|_| damaged())?;
        let size = usize::try_from(u64::from_le_bytes(header[8..].try_into().unwrap()))
            .map_err(|_| damaged())?;
        let (bytes, rest) = rest.split_at_checked(size).ok_or_else(damaged)?;
        base.get_mut(offset..)
            .and_then(|target| target.get_mut(..size))
            .ok_or_else(damaged)?
            .copy_from_slice(bytes);
        runs = rest;
    }
    if runs.is_empty() {
        Ok(base)
    } else {
        Err(damaged())
    }
}

struct Converted {
    id: i64,
    author: String,
    edit: Edit,
    /// Schema 14's publication evidence, when this edit was attempted.
    attempted: Option<String>,
    conflict: Option<(i64, ExGuid)>,
}

pub(crate) fn migrate(connection: &mut Connection, path: &Path) -> Result<()> {
    let mut archive = path.as_os_str().to_owned();
    archive.push(".v14-recovery");
    let archive = std::path::PathBuf::from(archive);
    crate::recovery::export(connection, &archive, true)?;
    let failed = |error: crate::Error| {
        invalid(format!(
            "The schema-14 cache could not be converted ({error}); it is unchanged and archived at {}",
            archive.display()
        ))
    };
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    convert(&transaction).map_err(failed)?;
    transaction.commit()?;
    Ok(())
}

fn convert(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let (base, patch): (Vec<u8>, Vec<u8>) =
        transaction.query_row("SELECT base, working FROM replica WHERE id=1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    let expected = working(base.clone(), &patch)?;
    let attempt: Option<(i64, String)> = transaction
        .query_row("SELECT edit_id, revisions FROM attempt", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let conflicts: BTreeMap<i64, i64> = transaction
        .prepare("SELECT edit_id, kind FROM conflicts")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let queued: Vec<(i64, String, String)> = transaction
        .prepare("SELECT id, space, operation FROM edits ORDER BY id")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let sequence: i64 = transaction.query_row(
        "SELECT max(coalesce((SELECT seq FROM sqlite_sequence WHERE name='edits'), 0),
                        coalesce((SELECT max(edit_id) FROM receipts), 0),
                        coalesce((SELECT max(edit_id) FROM archived), 0))",
        [],
        |row| row.get(0),
    )?;

    let arena = Arena::default();
    let mut section = Section::open(&arena, base.clone())?;
    let at = crate::now();
    let mut converted = Vec::new();
    let mut sealed = None;
    let mut edited = BTreeMap::new();
    let (mut created, mut deleted) = (Vec::new(), Vec::new());
    for (id, space, operation) in queued {
        let space: ExGuid = space.parse()?;
        let operation: Operation = serde_json::from_str(&operation)
            .map_err(|error| invalid(format!("Queued edit {id} is unreadable: {error}")))?;
        let (author, ops) = match operation {
            Operation::Page(intent) => {
                let current = section.page(space)?;
                let ops = onestore::op::lower_page(&current, &intent.after)?;
                edited.insert(space, id);
                (
                    intent.author,
                    ops.into_iter().map(|op| Op::Page { space, op }).collect(),
                )
            }
            Operation::CreatePage(creation) => {
                created.push(creation.space());
                (
                    String::new(),
                    vec![Op::Section(SectionOp::Create(creation))],
                )
            }
            Operation::Pages(batch) => (
                String::new(),
                vec![Op::Section(SectionOp::Pages(batch.edits))],
            ),
            Operation::DeletePages(pages) => {
                deleted.extend(&pages);
                (String::new(), vec![Op::Section(SectionOp::Delete(pages))])
            }
        };
        let edit = Edit { at, ops };
        section.apply(&author, &edit)?;
        let attempted = attempt
            .as_ref()
            .filter(|(edit, _)| *edit == id)
            .map(|(_, revisions)| revisions.clone());
        if attempted.is_some() {
            if !converted.is_empty() {
                return Err(invalid(
                    "Only the oldest queued edit can hold an attempt".into(),
                ));
            }
            sealed = Some(section.seal()?);
        }
        // A conflicting page-list edit is the section's conflict, whichever page it names.
        let conflicted = match edit.ops.first() {
            Some(Op::Section(_)) => section.root(),
            _ => space,
        };
        converted.push(Converted {
            id,
            author,
            edit,
            attempted,
            conflict: conflicts.get(&id).map(|kind| (*kind, conflicted)),
        });
    }
    let listed = |image: &[u8]| -> Result<Vec<ExGuid>> {
        let store = Store::parse(image)?;
        let index = RevisionIndex::parse(&store)?;
        Ok(Document::parse(&index)?
            .pages()?
            .into_iter()
            .map(|(space, _)| space)
            .collect())
    };
    let based = listed(&base)?;
    let listing: Vec<ExGuid> = section
        .pages()?
        .into_iter()
        .map(|(space, ..)| space)
        .collect();
    let cached = listed(&expected)?;
    // A page the remote added or removed while edits waited is in one list and not the
    // other; one the queue created or deleted must agree.
    let agree = cached
        .iter()
        .filter(|space| !listing.contains(space))
        .all(|space| !based.contains(space) && !created.contains(space))
        && listing
            .iter()
            .filter(|space| !cached.contains(space))
            .all(|space| based.contains(space) && !deleted.contains(space));
    if !agree {
        return Err(invalid(
            "The converted page list differs from the cached one".into(),
        ));
    }
    verify(&mut section, &expected, &edited)?;

    transaction.execute_batch(
        "DROP TABLE replica; DROP TABLE attempt; DROP TABLE conflicts; DROP TABLE edits;",
    )?;
    transaction.execute_batch(schema::QUEUE)?;
    base::write(transaction, base::Image::Base, &base)?;
    let mut batch = None;
    for edit in converted {
        let current = match (batch, &edit.attempted) {
            (Some(current), None) => current,
            _ => {
                transaction.execute("INSERT INTO batches DEFAULT VALUES", [])?;
                transaction.last_insert_rowid()
            }
        };
        if let Some(evidence) = &edit.attempted {
            transaction.execute(
                "UPDATE batches SET sealed=?1, revisions=?2, attempted=1 WHERE id=?3",
                params![
                    serde_json::to_string(sealed.as_ref().unwrap()).map_err(io::Error::other)?,
                    evidence,
                    current
                ],
            )?;
            batch = None;
        } else {
            batch = Some(current);
        }
        if let Some((kind, space)) = edit.conflict {
            transaction.execute(
                "UPDATE batches SET conflict=?1, space=?2 WHERE id=?3",
                params![kind, space.to_string(), current],
            )?;
        }
        queue::insert(
            transaction,
            Some(crate::unsigned(edit.id)?),
            current,
            &edit.author,
            &edit.edit,
        )?;
    }
    transaction.execute("DELETE FROM sqlite_sequence WHERE name='edits'", [])?;
    transaction.execute(
        "INSERT INTO sqlite_sequence(name, seq) VALUES ('edits', ?1)",
        [sequence],
    )?;
    transaction.pragma_update(None, "user_version", schema::VERSION)?;
    Ok(())
}

/// Requires each page a queued page edit changed to read as the old working image has it.
fn verify(
    section: &mut Section<'_>,
    expected: &[u8],
    edited: &BTreeMap<ExGuid, i64>,
) -> Result<()> {
    let store = Store::parse(expected)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    for (space, id) in edited {
        let converted = section.page(*space).ok();
        let stored = Page::from_space(&document, *space).ok();
        if converted != stored {
            return Err(invalid(format!(
                "Queued edit {id} converts to a page that differs from the cached page"
            )));
        }
    }
    Ok(())
}
