use super::*;
use onestore::FileDataReference;
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

impl Replica {
    /// Reads a previously downloaded external payload without network access.
    /// Absence is distinct from an empty payload; every returned buffer passes its stored checksum.
    pub fn cached_asset(&self, filename: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let key = key(filename)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        cached(&connection, &key, limit)
    }

    /// Fetches a declared external payload and durably retains it without changing the edit queue.
    /// Different bytes for an already cached identity return `AssetChanged`, preserving the cache.
    /// Network I/O does not hold the cache mutex; a stale reference fails before local publication.
    pub fn fetch_asset(
        &self,
        source: &mut impl crate::discover::Source,
        section: &str,
        filename: &str,
        limit: usize,
    ) -> Result<Vec<u8>> {
        let key = key(filename)?;
        {
            let connection = self
                .connection
                .lock()
                .map_err(|_| io::Error::other("Cache owner panicked"))?;
            if !referenced(&connection, &key)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "The retained document images do not reference this external payload",
                )
                .into());
            }
        }
        let bytes = crate::discover::read_external_asset(source, section, filename, limit)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !referenced(&transaction, &key)? {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "The external payload reference changed during download",
            )
            .into());
        }
        let previous = match cached(&transaction, &key, bytes.len()) {
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::FileTooLarge => {
                return Err(Error::AssetChanged);
            }
            other => other?,
        };
        if let Some(previous) = previous {
            if previous != bytes {
                return Err(Error::AssetChanged);
            }
        } else {
            transaction.execute(
                "INSERT INTO assets(name,data,sha256) VALUES (?1,?2,?3)",
                params![key, &bytes, &Sha256::digest(&bytes)[..]],
            )?;
        }
        transaction.commit()?;
        Ok(bytes)
    }
}

pub(crate) fn key(filename: &str) -> Result<String> {
    format!("<file>{filename}").parse::<FileDataReference>()?;
    Ok(filename.to_ascii_lowercase())
}

fn referenced(connection: &Connection, key: &str) -> Result<bool> {
    for column in ["working", "base"] {
        let image: Vec<u8> = connection.query_row(
            &format!("SELECT {column} FROM replica WHERE id=1"),
            [],
            |row| row.get(0),
        )?;
        let store = Store::parse(&image)?;
        let index = RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        if document.spaces.values().flat_map(|space| space.revisions.values())
            .flat_map(|revision| revision.nodes.values()).any(|node| {
                matches!(&node.kind, Kind::File { reference: FileDataReference::External(name), .. } if name.eq_ignore_ascii_case(key))
            }) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn cached(connection: &Connection, key: &str, limit: usize) -> Result<Option<Vec<u8>>> {
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 5 {
        return Ok(None);
    }
    let length: Option<i64> = connection
        .query_row(
            "SELECT length(data) FROM assets WHERE name=?1",
            [key],
            |row| row.get(0),
        )
        .optional()?;
    let Some(length) = length else {
        return Ok(None);
    };
    let length =
        usize::try_from(length).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    if length > limit {
        return Err(io::Error::from(io::ErrorKind::FileTooLarge).into());
    }
    let (bytes, expected): (Vec<u8>, Vec<u8>) = connection.query_row(
        "SELECT data,sha256 FROM assets WHERE name=?1",
        [key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if bytes.len() != length || Sha256::digest(&bytes)[..] != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Cached external payload checksum mismatch",
        )
        .into());
    }
    Ok(Some(bytes))
}
