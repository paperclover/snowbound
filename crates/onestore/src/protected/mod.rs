//! Explicit, in-memory opening of native OneNote 2010 protected sections.
//!
//! AES-128/CBC and SHA-1 Agile password wrappers are supported. Unknown wrappers
//! remain opaque through the ordinary storage/document APIs. CBC provides no
//! general ciphertext authentication; native read-only hashes and model checks
//! detect structural inconsistencies, not arbitrary changes to all content.

mod crypto;

use crate::{ExGuid, FileDataReference, ObjectData, Reference, RevisionIndex, document::Document};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    PasswordMismatch,
    Unsupported,
    Limit,
    Invalid(crate::Error),
}

impl From<crate::Error> for Error {
    fn from(value: crate::Error) -> Self {
        Self::Invalid(value)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PasswordMismatch => {
                f.write_str("The password did not match the section verifier")
            }
            Self::Unsupported => f.write_str("This protection format is not supported"),
            Self::Limit => f.write_str("Opening the protected section exceeded its work limit"),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            _ => None,
        }
    }
}

fn invalid(message: &'static str) -> Error {
    Error::Invalid(crate::Error { offset: 0, message })
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Total password iterations across distinct encryption metadata containers.
    pub kdf_rounds: u64,
    /// Total ciphertext/reference bytes materialized; temporary copies can double this.
    pub decoded_bytes: usize,
    /// Sum of object counts in distinct labeled revisions.
    pub object_visits: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            kdf_rounds: 1_000_000,
            decoded_bytes: 256 * 1024 * 1024,
            object_visits: 1_000_000,
        }
    }
}

/// Owns decoded buffers until dropped; returned views cannot outlive this owner.
///
/// Passwords and derived keys are not retained. Dropping this owner clears its
/// decoded buffers. Owned strings or exports made from a `Document` are separate
/// caller-owned copies and must be disposed of by the caller when locking.
/// Opening never rewrites the source image or changes its protection state.
///
/// ```compile_fail
/// # use onestore::{RevisionIndex, protected::{Limits, UnlockedSection}};
/// fn outlive<'a>(index: &'a RevisionIndex<'a>, password: &str) {
///     let unlocked = UnlockedSection::open(index, password, Limits::default()).unwrap();
///     let document = unlocked.document().unwrap();
///     drop(unlocked);
///     document.pages().unwrap();
/// }
/// ```
pub struct UnlockedSection<'a> {
    index: &'a RevisionIndex<'a>,
    objects: BTreeMap<(ExGuid, usize), Zeroizing<Vec<u8>>>,
    files: BTreeMap<[u8; 16], Zeroizing<Vec<u8>>>,
}

impl<'a> UnlockedSection<'a> {
    /// Verifies the password and every labeled revision before exposing a view.
    /// Metadata and password input are each bounded to 64 KiB.
    pub fn open(index: &'a RevisionIndex<'a>, password: &str, mut limits: Limits) -> Result<Self> {
        if index.store.header.file_type != crate::FileType::Section {
            return Err(Error::Unsupported);
        }
        if !index.store.checksum_mismatches.is_empty() {
            return Err(invalid("Protected document transaction checksum mismatch"));
        }
        let mut result = Self {
            index,
            objects: BTreeMap::new(),
            files: BTreeMap::new(),
        };
        let mut keys = BTreeMap::new();
        let mut file_keys = BTreeMap::new();
        let mut hashes = BTreeMap::new();
        for node in index.store.lists.values().flat_map(|list| &list.nodes) {
            if !matches!(node.id, 0xc4 | 0xc5) {
                continue;
            }
            let Some(Reference::Data(chunk)) = node.reference else {
                return Err(invalid("Read-only object has no data reference"));
            };
            let bytes = index.store.chunk_data(chunk)?;
            let expected: [u8; 16] = node
                .payload
                .last_chunk::<16>()
                .copied()
                .ok_or_else(|| invalid("Missing read-only object hash"))?;
            if hashes
                .insert(bytes.as_ptr().addr(), expected)
                .is_some_and(|old| old != expected)
            {
                return Err(invalid("Inconsistent read-only hashes for one payload"));
            }
        }
        for (space_id, space) in &index.spaces {
            let mut metadata = None;
            for revision in space.revisions.values() {
                if !revision.encrypted {
                    return Err(Error::Unsupported);
                }
                let node = revision
                    .nodes
                    .first()
                    .ok_or_else(|| invalid("Missing encryption key node"))?;
                let Some(Reference::Data(chunk)) = node.reference else {
                    return Err(invalid("Missing encryption key reference"));
                };
                let data = index.store.encryption_key(chunk)?;
                if metadata.replace(data).is_some_and(|old| old != data) {
                    return Err(invalid("Object space changes its encryption metadata"));
                }
            }
            let metadata =
                metadata.ok_or_else(|| invalid("Protected object space has no revision"))?;
            if !keys.contains_key(metadata) {
                keys.insert(
                    metadata,
                    crypto::Key::open(metadata, password, &mut limits.kdf_rounds)?,
                );
            }
            let key = &keys[metadata];
            for rid in space.labels.values().copied().collect::<BTreeSet<_>>() {
                let revision = index.resolve(*space_id, rid)?;
                limits.object_visits = limits
                    .object_visits
                    .checked_sub(revision.objects.len())
                    .ok_or(Error::Limit)?;
                for object in revision.objects.values() {
                    match object.data {
                        ObjectData::Encrypted(bytes) => {
                            let identity = (*space_id, bytes.as_ptr().addr());
                            let expected = hashes.get(&identity.1);
                            if object.jcid & 0x100000 != 0 && expected.is_none() {
                                return Err(invalid("Read-only encrypted object has no hash"));
                            }
                            if result.objects.contains_key(&identity) {
                                continue;
                            }
                            limits.decoded_bytes = limits
                                .decoded_bytes
                                .checked_sub(bytes.len())
                                .ok_or(Error::Limit)?;
                            let decoded = key.property(bytes)?;
                            if let Some(expected) = expected {
                                let mut hash = md5::Context::new();
                                hash.consume(&decoded);
                                hash.consume(&[0; 7][..(8 - decoded.len() % 8) % 8]);
                                if hash.finalize().0 != *expected {
                                    return Err(invalid(
                                        "Decrypted read-only object hash mismatch",
                                    ));
                                }
                            }
                            result.objects.insert(identity, decoded);
                        }
                        ObjectData::File { .. } => {
                            if let Some(FileDataReference::Internal(guid)) =
                                object.file_reference()?
                            {
                                if file_keys
                                    .insert(guid, metadata)
                                    .is_some_and(|old| old != metadata)
                                {
                                    return Err(invalid(
                                        "File payload uses inconsistent encryption metadata",
                                    ));
                                }
                                if result.files.contains_key(&guid) {
                                    continue;
                                }
                                let bytes = index.store.file_data(guid)?;
                                limits.decoded_bytes = limits
                                    .decoded_bytes
                                    .checked_sub(bytes.len())
                                    .ok_or(Error::Limit)?;
                                result.files.insert(guid, key.file(bytes)?);
                            }
                        }
                        ObjectData::Properties(_) => {
                            return Err(invalid("Protected revision contains clear properties"));
                        }
                    }
                }
            }
        }
        drop(keys);
        for (space, info) in &index.spaces {
            for rid in info.labels.values().copied().collect::<BTreeSet<_>>() {
                result.resolve(*space, rid)?.reachable()?;
            }
        }
        result.document()?.pages()?;
        Ok(result)
    }

    fn resolve(
        &self,
        space: ExGuid,
        rid: ExGuid,
    ) -> std::result::Result<crate::ResolvedRevision<'_>, crate::Error> {
        let mut revision = self.index.resolve(space, rid)?;
        for object in revision.objects.values_mut() {
            if let ObjectData::Encrypted(bytes) = object.data {
                let decoded =
                    self.objects
                        .get(&(space, bytes.as_ptr().addr()))
                        .ok_or(crate::Error {
                            offset: 0,
                            message: "Protected object was not decoded",
                        })?;
                object.data = ObjectData::Properties(decoded);
            }
        }
        Ok(revision)
    }

    pub fn document(&self) -> std::result::Result<Document<'_>, crate::Error> {
        Document::parse_with(
            self.index,
            |space, rid| self.resolve(space, rid),
            |id| {
                self.files
                    .get(&id)
                    .map(|bytes| bytes.as_slice())
                    .ok_or(crate::Error {
                        offset: 0,
                        message: "Protected file payload was not decoded",
                    })
            },
        )
    }
}
