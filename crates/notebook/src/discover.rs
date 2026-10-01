//! Read-only notebook discovery over a caller-supplied root.

use onestore::{
    FileType, RevisionIndex, Stamp, Store,
    document::{Document, Kind},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Duration,
};

mod source;
pub use source::Local;
#[cfg(feature = "smb")]
pub use source::Smb;
pub(crate) use source::placeholder;

#[derive(Debug, Serialize)]
pub struct Section {
    pub path: String,
    /// Header.guidFile, also used by FileIdentityGuid in a parent TOC.
    pub file_id: [u8; 16],
    pub state: SectionState,
    /// Another section of the catalog holds the same file, as a copy made beside it does, and
    /// is the one its folder's TOC lists by that identity (or else has the shorter path). OneNote
    /// opens both; the copy's TOC entry, where one exists, is its own.
    pub copy: bool,
}

#[derive(Debug, Serialize)]
pub enum SectionState {
    Readable {
        /// An explicit SectionDisplayName; otherwise use the current filename without its extension.
        name: Option<String>,
        /// The tab colour as a COLORREF; OneNote assigns one when absent.
        color: Option<u32>,
        /// The root object space's GUID, the document identity that names the section's
        /// replica in a mounted notebook.
        document: [u8; 16],
    },
    Locked,
    Unreadable(onestore::Error),
}

#[derive(Debug, Serialize)]
pub struct Folder {
    pub path: String,
    pub toc: Option<Toc>,
    pub sections: Vec<Section>,
    pub groups: Vec<Folder>,
    /// The paths of `sections` and `groups` together, in the order the TOC lists them, those
    /// it doesn't last by path.
    pub order: Vec<String>,
    /// Child section files and groups that could not be read.
    pub unavailable: Vec<Unavailable>,
}

impl Folder {
    /// This folder and every group under it, in catalog order.
    pub(crate) fn folders(&self) -> impl Iterator<Item = &Folder> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let folder = stack.pop()?;
            stack.extend(folder.groups.iter().rev());
            Some(folder)
        })
    }

    /// The sections of this folder and every group under it, in catalog order.
    pub(crate) fn sections(&self) -> impl Iterator<Item = &Section> {
        self.folders().flat_map(|folder| &folder.sections)
    }
}

/// A section file or group folder denied or gone while listing; each discovery tries it again.
#[derive(Debug, Serialize)]
pub struct Unavailable {
    pub path: String,
    pub group: bool,
    pub error: String,
    pub reason: Reason,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub enum Reason {
    /// Access denied, or gone mid-listing.
    Denied,
    /// Not yet on this device (`EntryKind::Evicted`), for the host to download.
    Evicted,
    /// Mid-write through every retry.
    InUse,
    /// Not a notebook file this can read: corrupt, or too large.
    Unreadable,
    /// Another group at `of` holds the same TOC; the catalog lists that one.
    Copy { of: String },
}

/// A section file or group folder holding an identity, one of the copies discovery chooses
/// among.
struct Claim {
    path: String,
    group: bool,
    /// Its folder's TOC lists the identity under its name.
    listed: bool,
    modified: u64,
}

#[derive(Debug, Serialize)]
pub struct Toc {
    pub filename: String,
    pub file_id: [u8; 16],
    /// Stale or unavailable TOC references; these are not inferred active sections.
    pub unresolved: Vec<TocReference>,
    /// The notebook's colour as a COLORREF, which only a notebook's own TOC holds.
    pub color: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocReference {
    /// Native TOCs can retain an identity after removing its cached filename.
    pub filename: Option<String>,
    pub file: [u8; 16],
    pub order: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    File,
    Directory,
    Other,
    /// A notebook file kept elsewhere and not yet on this device, as iCloud Drive's evicted
    /// files list: a `.Name.icloud` placeholder, or on macOS 14 and later a dataless file.
    Evicted,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub listed: Listed,
}

/// A file as its folder's listing shows it: enough to tell that it changed without reading it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Listed {
    pub size: u64,
    /// When it was last written, in the source's own units.
    pub modified: u64,
}

/// Each file a discovery read, with the listing it was read under, so that the next discovery
/// reads only the files listed otherwise, as OneNote 2010 reopens a notebook it has cached.
#[derive(Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cache {
    read: BTreeMap<String, Read>,
    /// The sections the last discovery read from its source, up to `HELD` bytes, until taken.
    #[serde(skip)]
    images: BTreeMap<String, Vec<u8>>,
}

/// The most bytes of images a cache holds for `Cache::take`.
const HELD: usize = 64 << 20;
/// How many more times a read that meets a commit in progress is tried, and how far apart.
const RETRIES: usize = 3;
const RETRY: Duration = Duration::from_millis(250);

/// A notebook file as discovery read it.
#[derive(Clone, Serialize, Deserialize)]
struct Read {
    listed: Listed,
    /// The file's stamp: its header in hex, and its length.
    header: String,
    length: u64,
    held: Held,
}

/// What discovery takes from a file.
#[derive(Clone, Serialize, Deserialize)]
enum Held {
    Section {
        name: Option<String>,
        color: Option<u32>,
        document: [u8; 16],
    },
    Locked,
    Toc {
        unresolved: Vec<TocReference>,
        color: Option<u32>,
    },
}

impl Cache {
    /// `discover`, reading only the files listed otherwise than when this cache last read them.
    /// A failed discovery leaves the cache as it was.
    pub fn discover(&mut self, source: &mut impl Source, limits: Limits) -> Result<Folder, Error> {
        let mut remaining = limits.entries;
        let mut claims = BTreeMap::new();
        let mut found = Cache::default();
        let mut folder = scan(
            source,
            "",
            &limits,
            0,
            &mut remaining,
            &mut claims,
            (&self.read, &mut found),
        )?;
        set_aside(&mut folder, &claims);
        *self = found;
        Ok(folder)
    }

    /// How the file at `path` was listed when last read, and its stamp then.
    pub fn found(&self, path: &str) -> Option<(Listed, Stamp)> {
        let read = self.read.get(path)?;
        Some((read.listed, read.stamp()?))
    }

    /// The readable sections the last discovery read from its source, by path, once, so that
    /// what reads them next need not read them again.
    pub fn take(&mut self) -> BTreeMap<String, Vec<u8>> {
        std::mem::take(&mut self.images)
    }
}

impl Read {
    fn stamp(&self) -> Option<Stamp> {
        let header: Vec<u8> = (0..self.header.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(self.header.get(at..at + 2)?, 16).ok())
            .collect::<Option<_>>()?;
        Some(Stamp {
            header: header.try_into().ok()?,
            length: self.length,
        })
    }

    fn file_id(&self) -> Option<[u8; 16]> {
        Some(onestore::Header::parse(&self.stamp()?.header).ok()?.file_id)
    }
}

/// Rooted file access. Paths are relative, UTF-8, and use `/` between components.
pub trait Source {
    /// Return the complete immediate directory or an error, never a truncated success.
    fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<Entry>>;
    /// Return a consistent file snapshot, rejecting images larger than the byte limit.
    fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>>;
    /// Reads an external payload with the same completeness and size guarantees.
    fn read_asset(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.read(path, limit)
    }
    /// The file at `path` as a local copy of the file whose identity was `known` holds it,
    /// where the copy holds it as it stands now, so that it need not be read.
    fn copy(&mut self, _path: &str, _known: [u8; 16]) -> Option<Vec<u8>> {
        None
    }
}

/// Reads an external file-data reference from the section's sibling `_onefiles` folder.
/// `NotFound` in `Error::Io` is distinct from a successfully read zero-byte payload.
pub fn read_external_asset(
    source: &mut impl Source,
    section: &str,
    filename: &str,
    limit: usize,
) -> Result<Vec<u8>, Error> {
    let (stem, extension) = section.rsplit_once('.').ok_or_else(|| Error::Entry {
        path: section.into(),
    })?;
    if !extension.eq_ignore_ascii_case("one")
        || !section.split('/').all(component)
        || stem.ends_with('/')
        || stem.is_empty()
    {
        return Err(Error::Entry {
            path: section.into(),
        });
    }
    format!("<file>{filename}")
        .parse::<onestore::FileDataReference>()
        .map_err(|_| Error::Entry {
            path: filename.into(),
        })?;
    let path = format!("{stem}_onefiles/{filename}");
    let bytes = source.read_asset(&path, limit).map_err(|error| Error::Io {
        path: path.clone(),
        error,
    })?;
    if bytes.len() > limit {
        return Err(Error::Limit { path });
    }
    Ok(bytes)
}

pub struct Limits {
    pub entries: usize,
    pub bytes_per_file: usize,
    pub depth: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cannot read {path}: {error}")]
    Io {
        path: String,
        #[source]
        error: io::Error,
    },
    #[error("Invalid notebook file {path}: {error}")]
    Document {
        path: String,
        #[source]
        error: onestore::Error,
    },
    #[error("Discovery limit exceeded at {path}")]
    Limit { path: String },
    #[error("Directory changed during discovery: {path}")]
    Changed { path: String },
    #[error("Invalid or unsupported directory entry: {path}")]
    Entry { path: String },
}

/// Discovers rooted notebook topology within caller-specified work and size limits.
/// Reserved `_onefiles` directories contain payloads, not section groups.
pub fn discover(source: &mut impl Source, limits: Limits) -> Result<Folder, Error> {
    Cache::default().discover(source, limits)
}

fn scan(
    source: &mut impl Source,
    path: &str,
    limits: &Limits,
    depth: usize,
    remaining: &mut usize,
    claims: &mut BTreeMap<[u8; 16], Vec<Claim>>,
    (cached, found): (&BTreeMap<String, Read>, &mut Cache),
) -> Result<Folder, Error> {
    if depth > limits.depth {
        return Err(Error::Limit { path: path.into() });
    }
    let mut listing = source
        .entries(path, *remaining)
        .map_err(|error| Error::Io {
            path: path.into(),
            error,
        })?;
    *remaining = remaining
        .checked_sub(listing.len())
        .ok_or_else(|| Error::Limit { path: path.into() })?;
    listing.retain(|entry| !foreign(&entry.name));
    listing.sort();
    let mut names = BTreeSet::new();
    for entry in &listing {
        if !component(&entry.name) || !names.insert(&entry.name) {
            return Err(Error::Entry {
                path: join(path, &entry.name),
            });
        }
    }
    let mut result = Folder {
        path: path.into(),
        toc: None,
        sections: Vec::new(),
        groups: Vec::new(),
        order: Vec::new(),
        unavailable: Vec::new(),
    };
    for entry in &listing {
        let child = join(path, &entry.name);
        if entry.kind == EntryKind::Directory {
            if entry.name.to_ascii_lowercase().ends_with("_onefiles") {
                continue;
            }
            // A group whose own listing or TOC cannot be read is unavailable, not fatal.
            match scan(
                source,
                &child,
                limits,
                depth + 1,
                remaining,
                claims,
                (cached, found),
            ) {
                Ok(group) => result.groups.push(group),
                Err(error) => match unavailable(&error) {
                    Some(reason) => result.unavailable.push(Unavailable {
                        path: child,
                        group: true,
                        error: error.to_string(),
                        reason,
                    }),
                    None => return Err(error),
                },
            }
            continue;
        }
        let lower = entry.name.to_ascii_lowercase();
        let expected = if lower.ends_with(".one") {
            FileType::Section
        } else if lower.ends_with(".onetoc2") {
            FileType::TableOfContents
        } else {
            continue;
        };
        let reused = cached
            .get(&child)
            .filter(|known| known.listed == entry.listed)
            .and_then(|known| Some((known.file_id()?, known)));
        // An evicted file listed as it was last read lists as then. Without an evicted TOC the
        // folder lists in path order; nothing writes a second one, as its placeholder, or the
        // dataless file itself, blocks creating it.
        if entry.kind == EntryKind::Evicted && reused.is_none() {
            if expected == FileType::Section {
                result.unavailable.push(Unavailable {
                    path: child,
                    group: false,
                    error: "Not downloaded to this device yet".into(),
                    reason: Reason::Evicted,
                });
            }
            continue;
        }
        if !matches!(entry.kind, EntryKind::File | EntryKind::Evicted)
            || (expected == FileType::TableOfContents && result.toc.is_some())
        {
            return Err(Error::Entry { path: child });
        }
        let (file_id, held) = match reused {
            Some((file_id, known)) => {
                found.read.insert(child.clone(), known.clone());
                (file_id, Ok(known.held.clone()))
            }
            None => {
                let copied = cached
                    .get(&child)
                    .and_then(Read::file_id)
                    .and_then(|known| source.copy(&child, known));
                let fetched = copied.is_none();
                // A section that cannot be read lists as unavailable; the rest still open.
                let mut list_unavailable = |error: Error| match unavailable(&error) {
                    Some(reason) if expected == FileType::Section => {
                        result.unavailable.push(Unavailable {
                            path: child.clone(),
                            group: false,
                            error: error.to_string(),
                            reason,
                        });
                        Ok(())
                    }
                    _ => Err(error),
                };
                let read = copied.map_or_else(
                    || {
                        let mut read = source.read(&child, limits.bytes_per_file);
                        // A read that meets a commit in progress succeeds once it lands.
                        for _ in 0..RETRIES {
                            if !read.as_ref().is_err_and(|error| {
                                matches!(
                                    error.kind(),
                                    io::ErrorKind::WouldBlock | io::ErrorKind::ResourceBusy
                                )
                            }) {
                                break;
                            }
                            std::thread::sleep(RETRY);
                            read = source.read(&child, limits.bytes_per_file);
                        }
                        read
                    },
                    Ok,
                );
                let bytes = match read {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        list_unavailable(Error::Io {
                            path: child.clone(),
                            error,
                        })?;
                        continue;
                    }
                };
                if bytes.len() > limits.bytes_per_file {
                    return Err(Error::Limit { path: child });
                }
                let parsed = Store::parse(&bytes).and_then(|store| {
                    if store.header.file_type != expected || !store.checksum_mismatches.is_empty() {
                        return Err(onestore::Error {
                            offset: 0,
                            message: "Unexpected file type or checksum mismatch",
                        });
                    }
                    Ok(store)
                });
                let store = match parsed {
                    Ok(store) => store,
                    Err(error) => {
                        list_unavailable(Error::Document {
                            path: child.clone(),
                            error,
                        })?;
                        continue;
                    }
                };
                let held = held(&store, expected);
                let file_id = store.header.file_id;
                if let Ok(held) = &held {
                    found.read.insert(
                        child.clone(),
                        Read {
                            listed: entry.listed,
                            header: bytes[..1024].iter().map(|b| format!("{b:02x}")).collect(),
                            length: bytes.len() as u64,
                            held: held.clone(),
                        },
                    );
                    let holding: usize = found.images.values().map(Vec::len).sum();
                    if fetched
                        && matches!(held, Held::Section { .. })
                        && holding + bytes.len() <= HELD
                    {
                        found.images.insert(child.clone(), bytes);
                    }
                }
                (file_id, held)
            }
        };
        match held {
            Ok(Held::Section {
                name,
                color,
                document,
            }) => result.sections.push(Section {
                path: child.clone(),
                file_id,
                state: SectionState::Readable {
                    name,
                    color,
                    document,
                },
                copy: false,
            }),
            Ok(Held::Locked) => result.sections.push(Section {
                path: child.clone(),
                file_id,
                state: SectionState::Locked,
                copy: false,
            }),
            Ok(Held::Toc { unresolved, color }) => {
                result.toc = Some(Toc {
                    filename: entry.name.clone(),
                    file_id,
                    unresolved,
                    color,
                })
            }
            Err(error) if expected == FileType::Section => result.sections.push(Section {
                path: child.clone(),
                file_id,
                state: SectionState::Unreadable(error),
                copy: false,
            }),
            Err(error) => return Err(Error::Document { path: child, error }),
        }
    }
    let order: BTreeMap<_, _> = result
        .toc
        .iter()
        .flat_map(|toc| &toc.unresolved)
        .map(|entry| (entry.file, entry.order))
        .collect();
    let rank = |file: Option<[u8; 16]>| file.and_then(|file| order.get(&file).copied());
    let ranked = |file, path: &String| (rank(file).unwrap_or(u32::MAX), path.clone());
    result
        .sections
        .sort_by_cached_key(|section| ranked(Some(section.file_id), &section.path));
    let group_file = |group: &Folder| group.toc.as_ref().map(|toc| toc.file_id);
    result
        .groups
        .sort_by_cached_key(|group| ranked(group_file(group), &group.path));
    let mut entries: Vec<(u32, String)> = (result.sections.iter())
        .map(|section| ranked(Some(section.file_id), &section.path))
        .chain(
            result
                .groups
                .iter()
                .map(|group| ranked(group_file(group), &group.path)),
        )
        .collect();
    entries.sort();
    result.order = entries.into_iter().map(|(_, path)| path).collect();
    let listed = |file: [u8; 16], path: &str| {
        let name = path.rsplit('/').next().unwrap_or(path);
        result
            .toc
            .iter()
            .flat_map(|toc| &toc.unresolved)
            .any(|entry| {
                entry.file == file
                    && entry
                        .filename
                        .as_deref()
                        .is_some_and(|filename| filename.eq_ignore_ascii_case(name))
            })
    };
    let modified = |path: &str| found.read.get(path).map_or(0, |read| read.listed.modified);
    for section in &result.sections {
        claims.entry(section.file_id).or_default().push(Claim {
            path: section.path.clone(),
            group: false,
            listed: listed(section.file_id, &section.path),
            modified: modified(&section.path),
        });
    }
    for group in &result.groups {
        if let Some(toc) = &group.toc {
            claims.entry(toc.file_id).or_default().push(Claim {
                path: group.path.clone(),
                group: true,
                listed: listed(toc.file_id, &group.path),
                modified: modified(&join(&group.path, &toc.filename)),
            });
        }
    }
    let present: BTreeSet<_> = result
        .sections
        .iter()
        .map(|section| section.file_id)
        .chain(
            result
                .groups
                .iter()
                .filter_map(|group| group.toc.as_ref().map(|toc| toc.file_id)),
        )
        .collect();
    if let Some(toc) = &mut result.toc {
        toc.unresolved
            .retain(|entry| !present.contains(&entry.file));
    }
    let mut observed = source
        .entries(path, limits.entries)
        .map_err(|error| Error::Io {
            path: path.into(),
            error,
        })?;
    observed.retain(|entry| !foreign(&entry.name));
    observed.sort();
    let names = |entries: &[Entry]| {
        entries
            .iter()
            .map(|entry| (entry.name.clone(), entry.kind))
            .collect::<Vec<_>>()
    };
    // A file written meanwhile lists otherwise, and is read again next time.
    if names(&observed) != names(&listing) {
        return Err(Error::Changed { path: path.into() });
    }
    Ok(result)
}

/// Keeps one group of each TOC identity in the catalog, the one its parent's TOC lists or else
/// the newest; the others list as unavailable copies with their sections. Of sections holding
/// one file, each lists, and all but the one its folder's TOC lists (or else the one with the
/// shortest path) are marked as copies: a choice that stands while the files keep their names.
fn set_aside(folder: &mut Folder, claims: &BTreeMap<[u8; 16], Vec<Claim>>) {
    let mut copies = BTreeMap::new();
    for claims in claims.values() {
        let mut groups: Vec<_> = claims.iter().filter(|claim| claim.group).collect();
        groups.sort_by_key(|claim| {
            (
                !claim.listed,
                std::cmp::Reverse(claim.modified),
                claim.path.len(),
                &claim.path,
            )
        });
        if let [original, rest @ ..] = &groups[..] {
            for copy in rest {
                copies.insert(copy.path.clone(), original.path.clone());
            }
        }
    }
    if !copies.is_empty() {
        demote(folder, &copies);
    }
    let mut sections = BTreeSet::new();
    for claims in claims.values() {
        let mut held: Vec<_> = claims
            .iter()
            .filter(|claim| {
                !claim.group
                    && !copies
                        .keys()
                        .any(|copy: &String| claim.path.starts_with(&format!("{copy}/")))
            })
            .collect();
        held.sort_by_key(|claim| (!claim.listed, claim.path.len(), &claim.path));
        sections.extend(held.iter().skip(1).map(|claim| claim.path.clone()));
    }
    if !sections.is_empty() {
        mark(folder, &sections);
    }
}

fn demote(folder: &mut Folder, copies: &BTreeMap<String, String>) {
    folder.groups.retain(|group| {
        let Some(of) = copies.get(&group.path) else {
            return true;
        };
        let name = of.rsplit('/').next().unwrap_or(of);
        folder.unavailable.push(Unavailable {
            path: group.path.clone(),
            group: true,
            error: format!("A copy of \u{201c}{name}\u{201d}, which opens instead"),
            reason: Reason::Copy { of: of.clone() },
        });
        false
    });
    for group in &mut folder.groups {
        demote(group, copies);
    }
}

fn mark(folder: &mut Folder, copies: &BTreeSet<String>) {
    for section in &mut folder.sections {
        section.copy = copies.contains(&section.path);
    }
    for group in &mut folder.groups {
        mark(group, copies);
    }
}

/// Whether `store` holds a password-protected section.
pub(crate) fn locked(store: &Store) -> bool {
    RevisionIndex::parse(store)
        .and_then(|index| {
            let document = Document::parse(&index)?;
            Ok(encrypted(document.active(document.root)?))
        })
        .unwrap_or(false)
}

fn encrypted(revision: &onestore::document::Revision<'_>) -> bool {
    revision
        .roots
        .get(&1)
        .and_then(|id| revision.nodes.get(id))
        .is_some_and(|node| matches!(node.kind, Kind::Encrypted { .. }))
}

/// What discovery takes from the file `store` holds, a section or a TOC as `expected`.
fn held(store: &Store, expected: FileType) -> Result<Held, onestore::Error> {
    let index = RevisionIndex::parse(store)?;
    let document = Document::parse(&index)?;
    let revision = document.active(document.root)?;
    let root = |id| {
        revision
            .roots
            .get(&id)
            .and_then(|id| revision.nodes.get(id))
    };
    if expected == FileType::Section {
        if encrypted(revision) {
            return Ok(Held::Locked);
        }
        index.validate_current()?;
        document.pages()?;
        let (name, color) = root(2).map_or((None, None), |node| match &node.kind {
            Kind::SectionMetadata { name, color } => (name.clone(), *color),
            _ => (None, None),
        });
        return Ok(Held::Section {
            name,
            color: color.filter(|color| *color != 0xffff_ffff),
            document: index.root.guid,
        });
    }
    index.validate_current()?;
    let Some(Kind::Toc { entries, color, .. }) = root(1).map(|node| &node.kind) else {
        return Err(onestore::Error {
            offset: 0,
            message: "Missing notebook TOC root",
        });
    };
    let mut seen = BTreeSet::new();
    let mut unresolved = Vec::new();
    for id in entries {
        let Some(Kind::Toc {
            filename,
            identity: Some(file),
            order: Some(order),
            ..
        }) = revision.nodes.get(id).map(|node| &node.kind)
        else {
            return Err(onestore::Error {
                offset: 0,
                message: "Incomplete notebook TOC reference",
            });
        };
        if filename.as_deref().is_some_and(|name| !component(name)) || !seen.insert(*file) {
            return Err(onestore::Error {
                offset: 0,
                message: "Invalid or duplicated notebook TOC reference",
            });
        }
        unresolved.push(TocReference {
            filename: filename.clone(),
            file: *file,
            order: *order,
        });
    }
    Ok(Held::Toc {
        unresolved,
        color: *color,
    })
}

/// Why a file or group that failed lists as unavailable; `None` fails the whole discovery,
/// as a lost connection does.
fn unavailable(error: &Error) -> Option<Reason> {
    match error {
        Error::Io { error, .. } => match error.kind() {
            io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound => Some(Reason::Denied),
            io::ErrorKind::WouldBlock | io::ErrorKind::ResourceBusy => Some(Reason::InUse),
            io::ErrorKind::InvalidData | io::ErrorKind::FileTooLarge => Some(Reason::Unreadable),
            _ => None,
        },
        Error::Document { .. } => Some(Reason::Unreadable),
        _ => None,
    }
}

/// Not the notebook's: dot files, among them macOS's `.DS_Store` and AppleDouble `._`
/// companions and Snowbound's `.snowbound` folder, and Office's `~$` owner files.
fn foreign(name: &str) -> bool {
    name.starts_with('.') || name.starts_with("~$")
}

fn component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.into()
    } else {
        format!("{parent}/{name}")
    }
}
