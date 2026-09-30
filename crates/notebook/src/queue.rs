//! Edit rows. An edit is stored as its serialized ops with picture and attachment bytes
//! moved to the `payloads` table, named by SHA-256 in the order `slots` visits them.

use crate::{PendingEdit, Result, signed, unsigned};
use onestore::{
    op::{Edit, Op, PageOp, SectionOp, TableEdit},
    page::{PageObject, PageParagraph, ParagraphContent, TableCell},
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{io, sync::Arc};

type Payload = Option<Arc<[u8]>>;

fn paragraphs(list: &mut [PageParagraph], visit: &mut impl FnMut(&mut Payload)) {
    for paragraph in list {
        match &mut paragraph.content {
            ParagraphContent::Image(image) => visit(&mut image.bytes),
            ParagraphContent::Attachment(attachment) => {
                visit(&mut attachment.bytes);
                visit(&mut attachment.preview);
            }
            ParagraphContent::Table(table) => {
                for row in &mut table.rows {
                    cells(&mut row.cells, visit);
                }
            }
            ParagraphContent::Text(_)
            | ParagraphContent::Ink(_)
            | ParagraphContent::Unsupported(_) => {}
        }
    }
}

fn cells(list: &mut [TableCell], visit: &mut impl FnMut(&mut Payload)) {
    for cell in list {
        paragraphs(&mut cell.paragraphs, visit);
    }
}

fn object(object: &mut PageObject, visit: &mut impl FnMut(&mut Payload)) {
    match object {
        PageObject::Outline(outline) => paragraphs(&mut outline.paragraphs, visit),
        PageObject::Title(title) => {
            for outline in &mut title.outlines {
                paragraphs(&mut outline.paragraphs, visit);
            }
        }
        PageObject::Image(image) => visit(&mut image.bytes),
        PageObject::Attachment(attachment) => {
            visit(&mut attachment.bytes);
            visit(&mut attachment.preview);
        }
        PageObject::Ink(_) | PageObject::Unsupported(_) => {}
    }
}

/// Visits every payload an edit carries, in a fixed order.
fn slots(edit: &mut Edit, mut visit: impl FnMut(&mut Payload)) {
    for op in &mut edit.ops {
        match op {
            Op::Page { op, .. } => match op {
                PageOp::Insert {
                    paragraphs: list, ..
                } => paragraphs(list, &mut visit),
                PageOp::Add { object: added, .. } => object(added, &mut visit),
                PageOp::Table {
                    edit: TableEdit::Rows { rows, .. },
                    ..
                } => {
                    for row in rows {
                        cells(&mut row.cells, &mut visit);
                    }
                }
                PageOp::Table {
                    edit: TableEdit::Column { cells: list, .. },
                    ..
                } => cells(list, &mut visit),
                _ => {}
            },
            Op::Section(SectionOp::Import { page, .. } | SectionOp::Conflict { page, .. }) => {
                for added in &mut page.objects {
                    object(added, &mut visit);
                }
            }
            Op::Section(_) => {}
        }
    }
}

fn damaged(message: &'static str) -> crate::Error {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

/// Stores `edit` in `batch` under `id`, or the next id, its payloads once each by hash;
/// returns the edit's id.
pub(crate) fn insert(
    connection: &Connection,
    id: Option<u64>,
    batch: i64,
    author: &str,
    edit: &Edit,
) -> Result<u64> {
    let (text, names) = encode(connection, edit)?;
    connection.execute(
        "INSERT INTO edits(id, batch, author, edit, payloads) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id.map(signed).transpose()?, batch, author, text, names],
    )?;
    unsigned(connection.last_insert_rowid())
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Replaces a stored edit's ops.
pub(crate) fn rewrite(connection: &Connection, id: u64, edit: &Edit) -> Result<()> {
    let (text, names) = encode(connection, edit)?;
    connection.execute(
        "UPDATE edits SET edit=?1, payloads=?2 WHERE id=?3",
        params![text, names, signed(id)?],
    )?;
    Ok(())
}

/// The serialized edit without payload bytes and the payload names, storing the payloads.
fn encode(connection: &Connection, edit: &Edit) -> Result<(String, Option<String>)> {
    let mut edit = edit.clone();
    let mut names: Vec<Option<String>> = Vec::new();
    let mut failure = None;
    slots(&mut edit, |payload| {
        let name = payload.take().map(|bytes| {
            let digest = Sha256::digest(&bytes);
            if let Err(error) = connection.execute(
                "INSERT INTO payloads(sha256, bytes) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                params![&digest[..], &bytes[..]],
            ) {
                failure.get_or_insert(error);
            }
            hex(&digest)
        });
        names.push(name);
    });
    if let Some(error) = failure {
        return Err(error.into());
    }
    let names = names
        .iter()
        .any(Option::is_some)
        .then(|| serde_json::to_string(&names))
        .transpose()
        .map_err(io::Error::other)?;
    Ok((
        serde_json::to_string(&edit).map_err(io::Error::other)?,
        names,
    ))
}

fn decode(connection: &Connection, text: &str, names: Option<String>) -> Result<Edit> {
    let mut edit: Edit = serde_json::from_str(text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let Some(names) = names else {
        return Ok(edit);
    };
    let names: Vec<Option<String>> = serde_json::from_str(&names)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut names = names.into_iter();
    let mut failure: Option<crate::Error> = None;
    let mut query =
        connection.prepare_cached("SELECT bytes FROM payloads WHERE sha256=unhex(?1)")?;
    slots(&mut edit, |payload| {
        let Some(name) = names.next() else {
            failure.get_or_insert(damaged("A queued edit names fewer payloads than it holds"));
            return;
        };
        let Some(name) = name else {
            return;
        };
        match query
            .query_row([&name], |row| row.get::<_, Vec<u8>>(0))
            .optional()
        {
            Ok(Some(bytes)) if hex(&Sha256::digest(&bytes)) == name => {
                *payload = Some(Arc::from(bytes));
            }
            Ok(_) => {
                failure.get_or_insert(damaged("A queued payload is missing or damaged"));
            }
            Err(error) => {
                failure.get_or_insert(error.into());
            }
        }
    });
    if names.next().is_some() {
        failure.get_or_insert(damaged("A queued edit names more payloads than it holds"));
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(edit),
    }
}

/// Queued edits, oldest first; of one batch when `batch` is given.
pub(crate) fn load(connection: &Connection, batch: Option<i64>) -> Result<Vec<PendingEdit>> {
    let mut query = connection.prepare_cached(
        "SELECT id, author, edit, payloads FROM edits WHERE ?1 IS NULL OR batch=?1 ORDER BY id",
    )?;
    let mut rows = query.query([batch])?;
    let mut edits = Vec::new();
    while let Some(row) = rows.next()? {
        edits.push(PendingEdit {
            id: unsigned(row.get(0)?)?,
            author: row.get(1)?,
            edit: decode(connection, &row.get::<_, String>(2)?, row.get(3)?)?,
        });
    }
    Ok(edits)
}

/// Drops payloads no queued edit names.
pub(crate) fn collect(connection: &Connection) -> Result<()> {
    connection.execute(
        "DELETE FROM payloads WHERE NOT EXISTS (SELECT 1 FROM edits, json_each(edits.payloads) AS name
            WHERE edits.payloads IS NOT NULL AND name.value=lower(hex(payloads.sha256)))",
        [],
    )?;
    Ok(())
}

/// The object spaces an edit changes; section ops change the root space as well.
pub(crate) fn spaces(edit: &Edit, root: onestore::ExGuid) -> Vec<onestore::ExGuid> {
    let mut spaces = Vec::new();
    for op in &edit.ops {
        match op {
            Op::Page { space, .. } => spaces.push(*space),
            Op::Section(op) => {
                spaces.push(root);
                match op {
                    SectionOp::Create(creation) | SectionOp::Import { creation, .. } => {
                        spaces.push(creation.space());
                    }
                    SectionOp::Conflict { of, creation, .. } => {
                        spaces.extend([*of, creation.space()]);
                    }
                    SectionOp::Pages(edits) => spaces.extend(edits.iter().map(|edit| edit.space())),
                    SectionOp::Delete(pages) => spaces.extend(pages),
                    SectionOp::RestoreVersion { page, .. }
                    | SectionOp::DeleteVersions { page, .. } => spaces.push(*page),
                    SectionOp::Color(_) => {}
                }
            }
        }
    }
    spaces.sort();
    spaces.dedup();
    spaces
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::{ExGuid, page::Image};

    #[test]
    fn payloads_are_stored_once_by_hash_and_restored_in_place() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::schema::QUEUE).unwrap();
        connection
            .execute("INSERT INTO batches DEFAULT VALUES", [])
            .unwrap();
        let bytes: Arc<[u8]> = Arc::from(vec![7_u8; 100_000]);
        let image = |id: u32| {
            PageObject::Image(Image {
                id: ExGuid {
                    guid: [1; 16],
                    n: id,
                },
                layout: Default::default(),
                size: None,
                bytes: Some(Arc::clone(&bytes)),
                display: None,
                alt: None,
                background: false,
                printout: None,
            })
        };
        let space = ExGuid {
            guid: [2; 16],
            n: 1,
        };
        let edit = Edit {
            at: 1,
            ops: [image(1), image(2)]
                .into_iter()
                .map(|object| Op::Page {
                    space,
                    op: PageOp::Add {
                        object,
                        before: None,
                    },
                })
                .collect(),
        };
        let id = insert(&connection, None, 1, "Author", &edit).unwrap();
        let stored: String = connection
            .query_row("SELECT edit FROM edits WHERE id=?1", [id as i64], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(stored.len() < 1000, "{}", stored.len());
        let payloads: i64 = connection
            .query_row("SELECT count(*) FROM payloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(payloads, 1);
        let loaded = load(&connection, None).unwrap();
        assert_eq!(loaded[0].edit, edit);
        let Op::Page {
            op:
                PageOp::Add {
                    object: PageObject::Image(image),
                    ..
                },
            ..
        } = &loaded[0].edit.ops[1]
        else {
            panic!()
        };
        assert_eq!(image.bytes.as_deref(), Some(&bytes[..]));
        connection.execute("DELETE FROM edits", []).unwrap();
        collect(&connection).unwrap();
        let payloads: i64 = connection
            .query_row("SELECT count(*) FROM payloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(payloads, 0);
    }
}
