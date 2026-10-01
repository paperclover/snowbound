//! Edit rows. An edit is stored as its serialized ops with picture and attachment bytes
//! moved to the `payloads` table, named by SHA-256 in the order `slots` visits them.
//!
//! A password-protected section's queue keeps none of it in the clear: each edit and
//! payload is sealed with AES-256-GCM (a random nonce, the row's kind as associated data)
//! and payloads are named by HMAC-SHA256, both under keys HMAC-SHA256 derives from the
//! section's own key. Only the author's name and the payload names stay readable.

use crate::{PendingEdit, Result, signed, unsigned};
use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use hmac::{Hmac, Mac};
use onestore::{
    op::{Edit, Op, PageOp, SectionOp, TableEdit},
    page::{PageObject, PageParagraph, ParagraphContent, TableCell},
    protected::Key,
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{io, sync::Arc};
use zeroize::Zeroizing;

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

/// A key `key` derives for `purpose`.
fn derived(key: &Key, purpose: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut mac =
        <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key.secret()).expect("any key length");
    mac.update(purpose);
    Zeroizing::new(mac.finalize().into_bytes().into())
}

fn cipher(key: &Key) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(&*derived(key, b"Snowbound queue v1")).expect("a 32-byte key")
}

/// A payload's name: its SHA-256, or in a protected section's queue an HMAC.
fn name(key: Option<&Key>, bytes: &[u8]) -> String {
    match key {
        Some(key) => {
            let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(&*derived(
                key,
                b"Snowbound payload names v1",
            ))
            .expect("any key length");
            mac.update(bytes);
            hex(&mac.finalize().into_bytes())
        }
        None => hex(&Sha256::digest(bytes)),
    }
}

/// `clear` sealed under `key` as `kind`: a random nonce, then the ciphertext and tag.
fn seal(key: &Key, kind: &[u8], clear: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0; 12];
    getrandom::fill(&mut nonce).map_err(|_| io::Error::other("System random source failed"))?;
    let sealed = cipher(key)
        .encrypt(
            &nonce.into(),
            aes_gcm::aead::Payload {
                msg: clear,
                aad: kind,
            },
        )
        .map_err(|_| io::Error::other("A queued edit could not be sealed"))?;
    Ok([&nonce[..], &sealed].concat())
}

/// The inverse of `seal`.
fn open(key: &Key, kind: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let (nonce, sealed) = sealed
        .split_first_chunk::<12>()
        .ok_or_else(|| damaged("A sealed queue row is truncated"))?;
    Ok(Zeroizing::new(
        cipher(key)
            .decrypt(
                &(*nonce).into(),
                aes_gcm::aead::Payload {
                    msg: sealed,
                    aad: kind,
                },
            )
            .map_err(|_| damaged("A sealed queue row does not open under the section's key"))?,
    ))
}

/// Stores `edit` in `batch` under `id`, or the next id, its payloads once each by name;
/// returns the edit's id.
pub(crate) fn insert(
    connection: &Connection,
    key: Option<&Key>,
    id: Option<u64>,
    batch: i64,
    author: &str,
    edit: &Edit,
) -> Result<u64> {
    let (text, names) = encode(connection, key, edit)?;
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
pub(crate) fn rewrite(
    connection: &Connection,
    key: Option<&Key>,
    id: u64,
    edit: &Edit,
) -> Result<()> {
    let (text, names) = encode(connection, key, edit)?;
    connection.execute(
        "UPDATE edits SET edit=?1, payloads=?2 WHERE id=?3",
        params![text, names, signed(id)?],
    )?;
    Ok(())
}

/// The serialized edit without payload bytes and the payload names, storing the payloads;
/// sealed under `key` for a protected section.
fn encode(
    connection: &Connection,
    key: Option<&Key>,
    edit: &Edit,
) -> Result<(String, Option<String>)> {
    let mut edit = edit.clone();
    let mut names: Vec<Option<String>> = Vec::new();
    let mut failure = None;
    slots(&mut edit, |payload| {
        let name = payload.take().map(|bytes| {
            let name = name(key, &bytes);
            let stored = match key {
                Some(key) => seal(key, b"payload", &bytes),
                None => Ok(bytes.to_vec()),
            };
            let stored = stored.and_then(|stored| {
                Ok(connection.execute(
                    "INSERT INTO payloads(sha256, bytes) VALUES (unhex(?1), ?2) ON CONFLICT DO NOTHING",
                    params![&name, &stored[..]],
                )?)
            });
            if let Err(error) = stored {
                failure.get_or_insert(error);
            }
            name
        });
        names.push(name);
    });
    if let Some(error) = failure {
        return Err(error);
    }
    let names = names
        .iter()
        .any(Option::is_some)
        .then(|| serde_json::to_string(&names))
        .transpose()
        .map_err(io::Error::other)?;
    let text = Zeroizing::new(serde_json::to_string(&edit).map_err(io::Error::other)?);
    let text = match key {
        Some(key) => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(seal(key, b"edit", text.as_bytes())?)
        }
        None => text.to_string(),
    };
    Ok((text, names))
}

/// A stored edit's ops, without its payloads.
pub(crate) fn parse(key: Option<&Key>, text: &str) -> Result<Edit> {
    Ok(match key {
        Some(key) => {
            use base64::Engine;
            let sealed = base64::engine::general_purpose::STANDARD
                .decode(text)
                .map_err(|_| damaged("A sealed queued edit is damaged"))?;
            serde_json::from_slice(&open(key, b"edit", &sealed)?)
        }
        None => serde_json::from_str(text),
    }
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?)
}

fn decode(
    connection: &Connection,
    key: Option<&Key>,
    text: &str,
    names: Option<String>,
) -> Result<Edit> {
    let mut edit = parse(key, text)?;
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
        let bytes = query
            .query_row([&name], |row| row.get::<_, Vec<u8>>(0))
            .optional();
        let bytes = match (key, bytes) {
            (Some(key), Ok(Some(sealed))) => {
                open(key, b"payload", &sealed).map(|bytes| Some(bytes.to_vec()))
            }
            (_, bytes) => bytes.map_err(Into::into),
        };
        match bytes {
            Ok(Some(bytes)) if self::name(key, &bytes) == name => {
                *payload = Some(Arc::from(bytes));
            }
            Ok(_) => {
                failure.get_or_insert(damaged("A queued payload is missing or damaged"));
            }
            Err(error) => {
                failure.get_or_insert(error);
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
pub(crate) fn load(
    connection: &Connection,
    key: Option<&Key>,
    batch: Option<i64>,
) -> Result<Vec<PendingEdit>> {
    let mut query = connection.prepare_cached(
        "SELECT id, author, edit, payloads FROM edits WHERE ?1 IS NULL OR batch=?1 ORDER BY id",
    )?;
    let mut rows = query.query([batch])?;
    let mut edits = Vec::new();
    while let Some(row) = rows.next()? {
        edits.push(PendingEdit {
            id: unsigned(row.get(0)?)?,
            author: row.get(1)?,
            edit: decode(connection, key, &row.get::<_, String>(2)?, row.get(3)?)?,
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
                tags: Vec::new(),
                link: None,
                text: None,
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
        let id = insert(&connection, None, None, 1, "Author", &edit).unwrap();
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
        let loaded = load(&connection, None, None).unwrap();
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
