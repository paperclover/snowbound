//! Section images kept as 16 KiB chunk rows, so a publication rewrites the chunks its
//! transaction touches instead of the image: `base`, the image the queued edits apply to,
//! and `remote`, the last observed remote image while it is not the base.

use crate::Result;
use onestore::{Stamp, Transaction};
use rusqlite::{Connection, OptionalExtension, params};
use std::io;

const CHUNK: usize = 16 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum Image {
    Base,
    Remote,
}

impl Image {
    fn table(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Remote => "remote",
        }
    }
}

fn damaged() -> crate::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Damaged cached image").into()
}

/// The whole image, or `None` when none is stored.
pub(crate) fn read(connection: &Connection, image: Image) -> Result<Option<Vec<u8>>> {
    let mut query = connection.prepare_cached(&format!(
        "SELECT chunk, bytes FROM {} ORDER BY chunk",
        image.table()
    ))?;
    let mut rows = query.query([])?;
    let mut bytes = Vec::new();
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let chunk: i64 = row.get(0)?;
        let part = row.get_ref(1)?.as_blob().map_err(|_| damaged())?;
        if chunk != count || bytes.len() % CHUNK != 0 || part.is_empty() || part.len() > CHUNK {
            return Err(damaged());
        }
        bytes.extend_from_slice(part);
        count += 1;
    }
    Ok((count > 0).then_some(bytes))
}

/// The stored image's header and length, reading its first and last chunks.
pub(crate) fn stamp(connection: &Connection, image: Image) -> Result<Option<Stamp>> {
    let table = image.table();
    let Some((header, last)): Option<(Vec<u8>, i64)> = connection
        .query_row(
            &format!("SELECT substr(bytes, 1, 1024), (SELECT max(chunk) FROM {table}) FROM {table} WHERE chunk=0"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let tail: i64 = connection.query_row(
        &format!("SELECT length(bytes) FROM {table} WHERE chunk=?1"),
        [last],
        |row| row.get(0),
    )?;
    Ok(Some(Stamp {
        header: header.try_into().map_err(|_| damaged())?,
        length: u64::try_from(last).map_err(|_| damaged())? * CHUNK as u64
            + u64::try_from(tail).map_err(|_| damaged())?,
    }))
}

/// Stores `bytes` as the image, rewriting only the chunks that differ.
pub(crate) fn write(connection: &Connection, image: Image, bytes: &[u8]) -> Result<()> {
    let table = image.table();
    let count = bytes.len().div_ceil(CHUNK);
    connection.execute(
        &format!("DELETE FROM {table} WHERE chunk>=?1"),
        [count as i64],
    )?;
    let mut stored =
        connection.prepare_cached(&format!("SELECT bytes FROM {table} WHERE chunk=?1"))?;
    let mut replace = connection.prepare_cached(&format!(
        "INSERT INTO {table}(chunk, bytes) VALUES (?1, ?2) ON CONFLICT(chunk) DO UPDATE SET bytes=excluded.bytes"
    ))?;
    for (index, part) in bytes.chunks(CHUNK).enumerate() {
        let current: Option<Vec<u8>> = stored
            .query_row([index as i64], |row| row.get(0))
            .optional()?;
        if current.as_deref() != Some(part) {
            replace.execute(params![index as i64, part])?;
        }
    }
    Ok(())
}

pub(crate) fn clear(connection: &Connection, image: Image) -> Result<()> {
    connection.execute(&format!("DELETE FROM {}", image.table()), [])?;
    Ok(())
}

/// Commits `transaction` to the base image the way it commits to the file, rewriting only
/// the chunks it writes.
pub(crate) fn publish(connection: &Connection, transaction: &Transaction) -> Result<()> {
    if stamp(connection, Image::Base)?.as_ref() != Some(transaction.base()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The published transaction is not on the cached base image",
        )
        .into());
    }
    let mut stored = connection.prepare_cached("SELECT bytes FROM base WHERE chunk=?1")?;
    let mut replace = connection.prepare_cached(
        "INSERT INTO base(chunk, bytes) VALUES (?1, ?2) ON CONFLICT(chunk) DO UPDATE SET bytes=excluded.bytes",
    )?;
    let mut chunks: std::collections::BTreeMap<usize, Vec<u8>> = Default::default();
    for (offset, bytes) in transaction.writes() {
        let mut offset = usize::try_from(offset).map_err(|_| damaged())?;
        let mut bytes = bytes;
        while !bytes.is_empty() {
            let index = offset / CHUNK;
            let chunk = match chunks.entry(index) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                    stored
                        .query_row([index as i64], |row| row.get(0))
                        .optional()?
                        .unwrap_or_default(),
                ),
            };
            let at = offset % CHUNK;
            let take = bytes.len().min(CHUNK - at);
            if chunk.len() < at {
                return Err(damaged());
            }
            if chunk.len() < at + take {
                chunk.resize(at + take, 0);
            }
            chunk[at..at + take].copy_from_slice(&bytes[..take]);
            offset += take;
            bytes = &bytes[take..];
        }
    }
    for (index, bytes) in chunks {
        replace.execute(params![index as i64, bytes])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::schema::QUEUE).unwrap();
        connection
    }

    /// Types `text` at the start of the page's first body paragraph.
    pub(crate) fn typed(section: &mut onestore::Section<'_>, space: onestore::ExGuid, text: &str) {
        use onestore::op::{Edit, Op, PageOp};
        let page = section.page(space).unwrap();
        let target = page
            .objects
            .iter()
            .find_map(|object| match object {
                onestore::page::PageObject::Outline(outline) => outline
                    .paragraphs
                    .iter()
                    .find_map(|p| p.text().map(|t| t.id)),
                _ => None,
            })
            .unwrap();
        section
            .apply(
                "Author",
                &Edit {
                    at: 133_000_000_000_000_000,
                    ops: vec![Op::Page {
                        space,
                        op: PageOp::Text {
                            text: target,
                            range: 0..0,
                            with: text.into(),
                        },
                    }],
                },
            )
            .unwrap();
    }

    #[test]
    fn a_published_transaction_rewrites_only_its_chunks_and_matches_the_file() {
        let source =
            include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one").to_vec();
        assert!(source.len() > 4 * CHUNK);
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, source.clone()).unwrap();
        let space = section.pages().unwrap()[0].0;
        let connection = connection();
        write(&connection, Image::Base, &source).unwrap();
        assert_eq!(read(&connection, Image::Base).unwrap().unwrap(), source);
        assert_eq!(
            stamp(&connection, Image::Base).unwrap().unwrap(),
            Stamp::of(&source).unwrap()
        );
        let mut image = source.clone();
        for round in 0..3 {
            typed(&mut section, space, &format!("{round}"));
            let transaction = section.seal().unwrap().unwrap();
            publish(&connection, &transaction).unwrap();
            transaction.apply(&mut image).unwrap();
            assert_eq!(read(&connection, Image::Base).unwrap().unwrap(), image);
            assert_eq!(
                stamp(&connection, Image::Base).unwrap().unwrap(),
                Stamp::of(&image).unwrap()
            );
            assert!(publish(&connection, &transaction).is_err());
        }
        let shorter = &image[..CHUNK + 5];
        write(&connection, Image::Base, shorter).unwrap();
        assert_eq!(read(&connection, Image::Base).unwrap().unwrap(), shorter);
        clear(&connection, Image::Remote).unwrap();
        assert!(read(&connection, Image::Remote).unwrap().is_none());
    }
}
