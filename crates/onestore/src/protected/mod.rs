//! Explicit, in-memory opening of native OneNote 2010 protected sections.
//!
//! AES-128/CBC and SHA-1 Agile password wrappers are supported. Unknown wrappers
//! remain opaque through the ordinary storage/document APIs. CBC provides no
//! general ciphertext authentication; native read-only hashes and model checks
//! detect structural inconsistencies, not arbitrary changes to all content.

mod crypto;

use crate::{
    ExGuid, FileDataReference, ObjectData, Reference, RevisionIndex, Transaction,
    document::Document,
    op::{Edit, Op, OpError},
};
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

struct Decoded<'a> {
    stored: &'a [u8],
    clear: Zeroizing<Vec<u8>>,
}

/// Owns decoded buffers until dropped; returned views cannot outlive this owner.
///
/// Passwords are not retained. Dropping this owner clears its derived key and
/// decoded buffers; a protected save works on ordinary buffers, which are not cleared. Owned strings or exports made from a `Document` are separate
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
    /// By object space and stored address.
    objects: BTreeMap<(ExGuid, usize), Decoded<'a>>,
    files: BTreeMap<[u8; 16], Zeroizing<Vec<u8>>>,
    keys: BTreeMap<&'a [u8], crypto::Key>,
}

impl<'a> UnlockedSection<'a> {
    /// Verifies the password and every labeled revision before exposing a view.
    /// Metadata and password input are each bounded to 64 KiB.
    pub fn open(index: &'a RevisionIndex<'a>, password: &str, limits: Limits) -> Result<Self> {
        Self::unlock(index, limits, |metadata, rounds| {
            crypto::Key::open(metadata, password, rounds)
        })
    }

    /// `open` with each distinct encryption metadata's key from `key`.
    fn unlock(
        index: &'a RevisionIndex<'a>,
        mut limits: Limits,
        mut key: impl FnMut(&[u8], &mut u64) -> Result<crypto::Key>,
    ) -> Result<Self> {
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
            keys: BTreeMap::new(),
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
                let Some(node) = revision.nodes.first().filter(|node| node.id == 0x7c) else {
                    continue;
                };
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
                keys.insert(metadata, key(metadata, &mut limits.kdf_rounds)?);
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
                            result.objects.insert(
                                identity,
                                Decoded {
                                    stored: bytes,
                                    clear: decoded,
                                },
                            );
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
        result.keys = keys;
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
                object.data = ObjectData::Properties(&decoded.clear);
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

impl UnlockedSection<'_> {
    /// A plaintext section holding every object space's current revision and payloads
    /// under their own identities, for writers that read ordinary sections.
    fn twin(&self) -> std::result::Result<Zeroizing<Vec<u8>>, crate::Error> {
        use crate::write::{PropertyObject, RevisionEdit, append_revisions};
        let copy = |space: ExGuid, revision: ExGuid| {
            let revision = self.resolve(space, revision)?;
            // A revision still declares objects its edits detached; a new one holds live ones.
            let live = revision.reachable()?;
            let mut objects = BTreeMap::new();
            for (id, object) in revision.objects.iter().filter(|(id, _)| live.contains(id)) {
                let copy = match object.data {
                    ObjectData::File {
                        reference,
                        extension,
                    } => {
                        let text = |bytes: &[u8]| {
                            String::from_utf16(
                                &bytes
                                    .chunks_exact(2)
                                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                                    .collect::<Vec<_>>(),
                            )
                            .map_err(|_| crate::Error {
                                offset: 0,
                                message: "Invalid UTF-16 file-data declaration",
                            })
                        };
                        let mut copy =
                            PropertyObject::file(*id, &text(reference)?, &text(extension)?)?;
                        copy.jcid = object.jcid;
                        copy.global_ids = std::sync::Arc::clone(&object.global_ids);
                        copy
                    }
                    _ => PropertyObject::from_object(object)?,
                };
                objects.insert(*id, copy);
            }
            Ok::<_, crate::Error>((revision.roots, objects))
        };
        let payloads: Vec<_> = self
            .files
            .iter()
            .map(|(id, bytes)| (*id, bytes.as_slice()))
            .collect();
        let current = (ExGuid::default(), 1);
        // The scaffold's root space takes the section's identity, then its content.
        let mut scaffold = crate::create_section("twin.one", "", "")?;
        let identity = |id: ExGuid| {
            let mut bytes = Vec::new();
            id.encode(&mut bytes);
            bytes
        };
        let store = crate::Store::parse(&scaffold)?;
        let placeholder = identity(RevisionIndex::parse(&store)?.root);
        for at in 0..=scaffold.len() - 20 {
            if scaffold[at..at + 20] == placeholder {
                scaffold[at..at + 20].copy_from_slice(&identity(self.index.root));
            }
        }
        let mut twin = Zeroizing::new(append_revisions(&scaffold, &payloads, None, |_| {
            let mut spaces = BTreeMap::new();
            for id in self.index.spaces.keys() {
                let (roots, objects) = copy(*id, self.index.active(*id)?)?;
                let edit = if *id == self.index.root {
                    RevisionEdit::Label {
                        context: current.0,
                        role: current.1,
                        roots,
                        objects,
                    }
                } else {
                    RevisionEdit::Create { roots, objects }
                };
                spaces.insert(*id, edit);
            }
            Ok(spaces)
        })?);
        // One revision per call and space: the remaining labels follow in rounds.
        for round in 0.. {
            let mut spaces = BTreeMap::new();
            for (id, space) in &self.index.spaces {
                let label = space.labels.iter().filter(|(label, _)| **label != current);
                if let Some(((context, role), revision)) = label.clone().nth(round) {
                    let (roots, objects) = copy(*id, *revision)?;
                    spaces.insert(
                        *id,
                        RevisionEdit::Label {
                            context: *context,
                            role: *role,
                            roots,
                            objects,
                        },
                    );
                }
            }
            if spaces.is_empty() {
                break;
            }
            twin = Zeroizing::new(append_revisions(&twin, &[], None, |_| Ok(spaces))?);
        }
        Ok(twin)
    }
}

impl UnlockedSection<'_> {
    /// Applies `edit` to this section as `Section::apply` applies it to an ordinary one and
    /// seals the result as one revision per changed space, stored under the section's key.
    /// Page ops only: a protected section's page list is not edited.
    pub fn apply(&self, author: &str, edit: &Edit) -> std::result::Result<Transaction, OpError> {
        let failed = |error: crate::Error| OpError::Failed(error);
        if self.keys.len() != 1 {
            return Err(OpError::Unsupported(
                "Only a section under one key takes edits",
            ));
        }
        if edit.ops.iter().any(|op| matches!(op, Op::Section(_))) {
            return Err(OpError::Unsupported(
                "A protected section's page list is not edited",
            ));
        }
        let twin = self.twin().map_err(failed)?;
        let arena = crate::Arena::default();
        let mut section = crate::Section::open(&arena, twin.to_vec()).map_err(failed)?;
        section.apply(author, edit)?;
        section.seal().map_err(failed)?;
        let applied = Zeroizing::new(section.image());
        drop(section);
        let applied_store = crate::Store::parse(&applied).map_err(failed)?;
        let applied_index = RevisionIndex::parse(&applied_store).map_err(failed)?;
        // The twin's scaffold spaces are not the section's.
        let edited = self
            .index
            .spaces
            .keys()
            .map(|space| Ok((*space, applied_index.resolve_active(*space)?)))
            .collect::<std::result::Result<Vec<_>, crate::Error>>()
            .map_err(failed)?;
        let edited: Vec<_> = edited
            .iter()
            .map(|(space, after)| (*space, after))
            .collect();
        let payloads = crate::write::declared_payloads(&applied_store)
            .into_iter()
            .map(|guid| Ok((guid, applied_store.file_data(guid)?)))
            .collect::<std::result::Result<Vec<_>, crate::Error>>()
            .map_err(failed)?;
        let transaction = crate::write::squash(self.index, &edited, &payloads, Some(self))
            .map_err(failed)?
            .ok_or(OpError::Unsupported("The edit changes nothing"))?;
        let written =
            crate::write::applied(self.index.store.data, Some(&transaction)).map_err(failed)?;
        let store = crate::Store::parse(&written).map_err(failed)?;
        let index = RevisionIndex::parse(&store).map_err(failed)?;
        UnlockedSection::unlock(&index, Limits::default(), |metadata, _| {
            self.keys
                .get(metadata)
                .cloned()
                .ok_or(Error::PasswordMismatch)
        })
        .map_err(|error| match error {
            Error::Invalid(error) => failed(error),
            _ => failed(crate::Error {
                offset: 0,
                message: "The written section does not unlock under its key",
            }),
        })?;
        Ok(transaction)
    }
}

impl UnlockedSection<'_> {
    /// The section key; `apply` admits sections with exactly one.
    fn key(&self) -> &crypto::Key {
        self.keys.values().next().expect("one section key")
    }
}

impl crate::write::Protection for UnlockedSection<'_> {
    fn resolve(
        &self,
        space: ExGuid,
        revision: ExGuid,
    ) -> std::result::Result<crate::ResolvedRevision<'_>, crate::Error> {
        self.resolve(space, revision)
    }

    fn stored(&self, clear: &[u8]) -> Option<&[u8]> {
        self.objects
            .values()
            .find(|decoded| std::ptr::eq(decoded.clear.as_ptr(), clear.as_ptr()))
            .map(|decoded| decoded.stored)
    }

    fn seal_property(&self, clear: &[u8]) -> std::result::Result<Vec<u8>, crate::Error> {
        let random = crate::Error {
            offset: 0,
            message: "System random source failed",
        };
        let mut iv = [0; 16];
        getrandom::fill(&mut iv).map_err(|_| random)?;
        self.key()
            .seal_property(clear, iv)
            .map_err(|error| match error {
                Error::Invalid(error) => error,
                _ => random,
            })
    }

    fn seal_file(&self, clear: &[u8]) -> Vec<u8> {
        self.key().seal_file(clear)
    }
}
