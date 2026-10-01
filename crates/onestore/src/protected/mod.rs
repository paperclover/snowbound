//! OneNote 2010's password-protected sections: an Office Agile password wrapper
//! (MS-OFFCRYPTO; SHA-1, 100,000 rounds) around an AES-128 key that encrypts every
//! property object (CBC, an IV of its own) and payload (CBC, one IV from the key data's
//! salt). Unknown wrappers stay opaque. CBC authenticates nothing; read-only hashes and
//! model checks catch structural damage, not every change to content.

mod crypto;

use crate::{
    Chunk, ExGuid, FileDataReference, ObjectData, Reference, RevisionIndex, Store,
    document::Document,
};
use bumpalo::Bump;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Error>;
type Format<T> = std::result::Result<T, crate::Error>;

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

impl Error {
    fn message(&self) -> &'static str {
        match self {
            Self::PasswordMismatch => "The password did not match the section verifier",
            Self::Unsupported => "This protection format is not supported",
            Self::Limit => "Opening the protected section exceeded its work limit",
            Self::Invalid(error) => error.message,
        }
    }
}

impl From<Error> for crate::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Invalid(error) => error,
            other => crate::Error {
                offset: 0,
                message: other.message(),
            },
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => error.fmt(f),
            other => f.write_str(other.message()),
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

/// A protected section's labelled revisions decoded for reading as a `Document`, which
/// cannot outlive it. Holds no password; dropping it clears its keys and decoded buffers,
/// while strings and exports made from the `Document` are the caller's to dispose of.
/// Edits go through `Section::unlock`.
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
    objects: BTreeMap<(ExGuid, usize), Zeroizing<Vec<u8>>>,
    files: BTreeMap<[u8; 16], Zeroizing<Vec<u8>>>,
}

impl<'a> UnlockedSection<'a> {
    /// Verifies the password and every labeled revision before exposing a view.
    /// Metadata and password input are each bounded to 64 KiB.
    pub fn open(index: &'a RevisionIndex<'a>, password: &str, limits: Limits) -> Result<Self> {
        Self::open_with(index, limits, |metadata, rounds| {
            crypto::Key::open(metadata, password, rounds)
        })
    }

    /// `open` with the key `Key::open` gave, which every space's encryption data must name.
    pub fn unlock(index: &'a RevisionIndex<'a>, key: &Key, limits: Limits) -> Result<Self> {
        Self::open_with(index, limits, |metadata, _| {
            match metadata == &key.metadata[..] {
                true => Ok(key.key.clone()),
                false => Err(Error::PasswordMismatch),
            }
        })
    }

    fn open_with(
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
        };
        let mut keys = BTreeMap::new();
        let mut file_keys = BTreeMap::new();
        let hashes = hashes(index.store)?;
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
                            if expected.is_some_and(|expected| digest(&decoded) != *expected) {
                                return Err(invalid("Decrypted read-only object hash mismatch"));
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

/// A protected section's key, opened with its password or made for a new one. Holds no
/// password; the key is cleared when its last clone drops.
#[derive(Clone)]
pub struct Key {
    key: crypto::Key,
    /// The encryption data (MS-ONESTORE 2.5.19) that opens the key.
    metadata: Arc<[u8]>,
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key")
    }
}

impl Key {
    /// Opens the key of the protected section `image` with `password`.
    pub fn open(image: &[u8], password: &str) -> Result<Self> {
        let store = Store::parse(image)?;
        let metadata = metadata(&store)?.ok_or(Error::Unsupported)?;
        Ok(Self {
            key: crypto::Key::open(metadata, password, &mut Limits::default().kdf_rounds)?,
            metadata: Arc::from(metadata),
        })
    }

    /// A fresh key for `password`, with OneNote 2010's encryption data.
    pub fn new(password: &str) -> Result<Self> {
        let (key, metadata) = crypto::Key::create(password)?;
        Ok(Self {
            key,
            metadata: Arc::from(metadata),
        })
    }

    /// The AES key the section's content is encrypted under, for keys derived from it.
    pub fn secret(&self) -> &[u8; 16] {
        self.key.value()
    }

    pub(crate) fn metadata(&self) -> &[u8] {
        &self.metadata
    }
}

/// The section `image` written anew, under `new` or in the clear, as OneNote 2010 rewrites
/// a section whose password it sets, changes or removes: the file and its object spaces
/// take new identities, each space keeps its labelled revisions, every one a checkpoint,
/// and the payloads come along. A protected `image` opens under `key`.
pub fn rekey(image: &[u8], key: Option<&Key>, new: Option<&Key>) -> Result<Vec<u8>> {
    let store = Store::parse(image)?;
    if !store.checksum_mismatches.is_empty() {
        return Err(invalid(
            "Cannot write a file with transaction checksum damage",
        ));
    }
    let index = RevisionIndex::parse(&store)?;
    let opened = match (key, metadata(&store)?) {
        (Some(key), Some(_)) => Some(Opened::new(&store, key)?),
        (None, None) => None,
        (None, Some(_)) => return Err(Error::PasswordMismatch),
        (Some(_), None) => return Err(invalid("The section is not protected")),
    };
    let arena = Bump::new();
    let mut fresh = BTreeMap::new();
    for space in index.spaces.keys() {
        if let std::collections::btree_map::Entry::Vacant(entry) = fresh.entry(space.guid) {
            entry.insert(crate::write::fresh_guid()?);
        }
    }
    // OneNote gives the payloads new identities too.
    let mut files = BTreeMap::new();
    for guid in crate::write::declared_payloads(&store) {
        files.insert(guid, crate::write::fresh_guid()?);
    }
    let rename = |id: ExGuid| ExGuid {
        guid: fresh.get(&id.guid).copied().unwrap_or(id.guid),
        n: id.n,
    };
    let mut spaces = Vec::new();
    for (space, info) in &index.spaces {
        let mut labelled: Vec<(ExGuid, Vec<(ExGuid, u32)>)> = Vec::new();
        // The current revision first, as OneNote writes it.
        let mut labels: Vec<_> = info.labels.iter().collect();
        labels.sort_by_key(|(label, _)| **label != (ExGuid::default(), 1));
        for (label, rid) in labels {
            match labelled.iter_mut().find(|(known, _)| known == rid) {
                Some((_, names)) => names.push(*label),
                None => labelled.push((*rid, vec![*label])),
            }
        }
        let mut revisions = Vec::new();
        for (rid, labels) in labelled {
            let mut resolved = index.resolve(*space, rid)?;
            if let Some(opened) = &opened {
                for object in resolved.objects.values_mut() {
                    if let ObjectData::Encrypted(stored) = object.data {
                        object.data = ObjectData::Properties(opened.property(&arena, stored)?);
                    }
                }
            }
            let mut objects = BTreeMap::new();
            for id in resolved.reachable()? {
                let mut object = resolved.objects[&id].clone();
                if let (Some(FileDataReference::Internal(guid)), ObjectData::File { extension, .. }) =
                    (object.file_reference()?, object.data)
                    && let Some(renamed) = files.get(&guid)
                {
                    let reference: Vec<u8> = crate::op::content::payload_reference(*renamed)
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect();
                    object.data = ObjectData::File {
                        reference: arena.alloc_slice_copy(&reference),
                        extension,
                    };
                }
                object.global_ids = Arc::new(
                    object
                        .global_ids
                        .iter()
                        .map(|(entry, guid)| (*entry, fresh.get(guid).copied().unwrap_or(*guid)))
                        .collect(),
                );
                objects.insert(rename(id), object);
            }
            let roots = resolved
                .roots
                .iter()
                .map(|(role, id)| (*role, rename(*id)))
                .collect();
            revisions.push(crate::write::Labelled {
                labels,
                revision: crate::ResolvedRevision { roots, objects },
            });
        }
        spaces.push((rename(*space), revisions));
    }
    let mut payloads = Vec::new();
    for (guid, renamed) in &files {
        let stored = store.file_data(*guid)?;
        let clear = match &opened {
            Some(opened) => opened.file(&arena, stored)?,
            None => stored,
        };
        payloads.push((*renamed, clear));
    }
    let placement = image[128..148].try_into().unwrap();
    let skeleton = crate::create::skeleton(rename(index.root), placement)?;
    let written = crate::write::rewrite(skeleton, &spaces, &payloads, new)?;
    let arena = crate::Arena::default();
    match new {
        Some(new) => drop(crate::Section::unlock(&arena, written.clone(), new)?),
        None => drop(crate::Section::open(&arena, written.clone())?),
    }
    Ok(written)
}

/// The encryption data every revision of `store` names; none in a section without any.
fn metadata<'a>(store: &Store<'a>) -> Result<Option<&'a [u8]>> {
    let mut found = None;
    for node in store.lists.values().flat_map(|list| &list.nodes) {
        if node.id != 0x7c {
            continue;
        }
        let Some(Reference::Data(chunk)) = node.reference else {
            return Err(invalid("Missing encryption key reference"));
        };
        let data = store.encryption_key(chunk)?;
        if found.replace(data).is_some_and(|old| old != data) {
            return Err(Error::Unsupported);
        }
    }
    Ok(found)
}

/// The hashes read-only declarations give their stored bytes, by address.
fn hashes(store: &Store<'_>) -> Result<BTreeMap<usize, [u8; 16]>> {
    let mut hashes = BTreeMap::new();
    for node in store.lists.values().flat_map(|list| &list.nodes) {
        if !matches!(node.id, 0xc4 | 0xc5) {
            continue;
        }
        let Some(Reference::Data(chunk)) = node.reference else {
            return Err(invalid("Read-only object has no data reference"));
        };
        let bytes = store.chunk_data(chunk)?;
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
    Ok(hashes)
}

/// A protected read-only declaration's hash: MD5 of the clear bytes zero-padded to 8.
pub(crate) fn digest(clear: &[u8]) -> [u8; 16] {
    let mut hash = md5::Context::new();
    hash.consume(clear);
    hash.consume(&[0; 7][..(8 - clear.len() % 8) % 8]);
    hash.finalize().0
}

/// A `Section`'s key, with what it decoded under it: clear bytes by the address of the
/// stored bytes they decode, and stored bytes by the address of their clear bytes.
pub(crate) struct Opened<'a> {
    key: Key,
    /// The encryption data each revision names.
    chunk: Chunk,
    /// Read-only declarations' hashes by the address of their stored bytes.
    hashes: BTreeMap<usize, [u8; 16]>,
    clear: RefCell<BTreeMap<usize, &'a [u8]>>,
    stored: RefCell<BTreeMap<usize, &'a [u8]>>,
}

impl<'a> Opened<'a> {
    /// `key` for the section `store` holds, every object space of which it must encrypt.
    pub(crate) fn new(store: &Store<'_>, key: &Key) -> Result<Self> {
        if metadata(store)?.ok_or(Error::Unsupported)? != &key.metadata[..] {
            return Err(Error::PasswordMismatch);
        }
        let index = RevisionIndex::parse(store)?;
        if index
            .spaces
            .values()
            .flat_map(|space| space.revisions.values())
            .any(|revision| !revision.encrypted)
        {
            return Err(invalid(
                "A protected section stores a revision in the clear",
            ));
        }
        let chunk = store
            .lists
            .values()
            .flat_map(|list| &list.nodes)
            .find_map(|node| match node.reference {
                Some(Reference::Data(chunk)) if node.id == 0x7c => Some(chunk),
                _ => None,
            })
            .ok_or(Error::Unsupported)?;
        Ok(Self {
            key: key.clone(),
            chunk,
            hashes: hashes(store)?,
            clear: RefCell::default(),
            stored: RefCell::default(),
        })
    }

    /// `key` for writing a new image whose encryption data lies at `chunk`.
    pub(crate) fn sealing(key: &Key, chunk: Chunk) -> Self {
        Self {
            key: key.clone(),
            chunk,
            hashes: BTreeMap::new(),
            clear: RefCell::default(),
            stored: RefCell::default(),
        }
    }

    /// The clear bytes of a stored object, decoded once into `arena`.
    pub(crate) fn property(&self, arena: &'a Bump, stored: &'a [u8]) -> Format<&'a [u8]> {
        if let Some(clear) = self.clear.borrow().get(&stored.as_ptr().addr()) {
            return Ok(clear);
        }
        let decoded = self.key.key.property(stored).map_err(crate::Error::from)?;
        if let Some(expected) = self.hashes.get(&stored.as_ptr().addr())
            && digest(&decoded) != *expected
        {
            return Err(crate::Error {
                offset: 0,
                message: "Decrypted read-only object hash mismatch",
            });
        }
        let clear: &'a [u8] = arena.alloc_slice_copy(&decoded);
        self.sealed(clear, stored);
        Ok(clear)
    }

    /// The clear bytes of a stored payload, in `arena`.
    pub(crate) fn file(&self, arena: &'a Bump, stored: &[u8]) -> Format<&'a [u8]> {
        Ok(arena.alloc_slice_copy(&self.key.key.file(stored).map_err(crate::Error::from)?))
    }

    pub(crate) fn key(&self) -> &Key {
        &self.key
    }

    /// Records that `clear` is stored as `stored`.
    pub(crate) fn sealed(&self, clear: &'a [u8], stored: &'a [u8]) {
        self.clear
            .borrow_mut()
            .insert(stored.as_ptr().addr(), clear);
        self.stored
            .borrow_mut()
            .insert(clear.as_ptr().addr(), stored);
    }
}

impl crate::write::Protection for Opened<'_> {
    fn stored(&self, clear: &[u8]) -> Option<&[u8]> {
        self.stored.borrow().get(&clear.as_ptr().addr()).copied()
    }

    fn seal_property(&self, clear: &[u8]) -> Format<Vec<u8>> {
        seal_property(&self.key, clear)
    }

    fn seal_file(&self, clear: &[u8]) -> Format<Vec<u8>> {
        self.key.key.seal_file(clear).map_err(crate::Error::from)
    }

    fn open_property(&self, stored: &[u8]) -> Format<Zeroizing<Vec<u8>>> {
        self.key.key.property(stored).map_err(crate::Error::from)
    }

    fn open_file(&self, stored: &[u8]) -> Format<Zeroizing<Vec<u8>>> {
        self.key.key.file(stored).map_err(crate::Error::from)
    }

    fn metadata(&self) -> Chunk {
        self.chunk
    }
}

/// A property object's stored form under `key`, with a fresh IV.
fn seal_property(key: &Key, clear: &[u8]) -> Format<Vec<u8>> {
    let mut iv = [0; 16];
    getrandom::fill(&mut iv).map_err(|_| crate::Error {
        offset: 0,
        message: "System random source failed",
    })?;
    key.key.seal_property(clear, iv).map_err(crate::Error::from)
}
