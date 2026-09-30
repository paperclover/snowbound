//! Read-only notebook discovery over a caller-supplied root.

use onestore::{
    FileType, RevisionIndex, Stamp, Store,
    document::{Document, Kind},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

mod source;
pub use source::Local;
#[cfg(feature = "smb")]
pub use source::Smb;

#[derive(Debug, Serialize)]
pub struct Section {
    pub path: String,
    /// Header.guidFile, also used by FileIdentityGuid in a parent TOC.
    pub file_id: [u8; 16],
    pub state: SectionState,
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
    /// Child section files and groups that could not be read.
    pub unavailable: Vec<Unavailable>,
}

/// A section file or group folder denied or gone while listing; each discovery tries it again.
#[derive(Debug, Serialize)]
pub struct Unavailable {
    pub path: String,
    pub group: bool,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct Toc {
    pub filename: String,
    pub file_id: [u8; 16],
    /// Stale or unavailable TOC references; these are not inferred active sections.
    pub unresolved: Vec<TocReference>,
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
pub struct Cache(BTreeMap<String, Read>);

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
    Toc(Vec<TocReference>),
}

impl Cache {
    /// `discover`, reading only the files listed otherwise than when this cache last read them.
    /// A failed discovery leaves the cache as it was.
    pub fn discover(&mut self, source: &mut impl Source, limits: Limits) -> Result<Folder, Error> {
        let mut remaining = limits.entries;
        let mut identities = BTreeMap::new();
        let mut read = BTreeMap::new();
        let folder = scan(
            source,
            "",
            &limits,
            0,
            &mut remaining,
            &mut identities,
            (&self.0, &mut read),
        )?;
        self.0 = read;
        Ok(folder)
    }

    /// How the file at `path` was listed when last read, and its stamp then.
    pub fn found(&self, path: &str) -> Option<(Listed, Stamp)> {
        let read = self.0.get(path)?;
        Some((read.listed, read.stamp()?))
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
    #[error("File identity occurs at both {first} and {second}")]
    DuplicateIdentity {
        file: [u8; 16],
        first: String,
        second: String,
    },
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
    identities: &mut BTreeMap<[u8; 16], String>,
    (cached, read): (&BTreeMap<String, Read>, &mut BTreeMap<String, Read>),
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
                identities,
                (cached, read),
            ) {
                Ok(group) => result.groups.push(group),
                Err(error @ Error::Io { .. }) if unavailable(&error) => {
                    result.unavailable.push(Unavailable {
                        path: child,
                        group: true,
                        error: error.to_string(),
                    });
                }
                Err(error) => return Err(error),
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
        if entry.kind != EntryKind::File
            || (expected == FileType::TableOfContents && result.toc.is_some())
        {
            return Err(Error::Entry { path: child });
        }
        let reused = cached
            .get(&child)
            .filter(|known| known.listed == entry.listed)
            .and_then(|known| Some((known.file_id()?, known)));
        let (file_id, held) = match reused {
            Some((file_id, known)) => {
                read.insert(child.clone(), known.clone());
                (file_id, Ok(known.held.clone()))
            }
            None => {
                let copied = cached
                    .get(&child)
                    .and_then(Read::file_id)
                    .and_then(|known| source.copy(&child, known));
                let bytes =
                    match copied.map_or_else(|| source.read(&child, limits.bytes_per_file), Ok) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            let error = Error::Io {
                                path: child.clone(),
                                error,
                            };
                            if expected == FileType::Section && unavailable(&error) {
                                result.unavailable.push(Unavailable {
                                    path: child,
                                    group: false,
                                    error: error.to_string(),
                                });
                                continue;
                            }
                            return Err(error);
                        }
                    };
                if bytes.len() > limits.bytes_per_file {
                    return Err(Error::Limit { path: child });
                }
                let store = Store::parse(&bytes).map_err(|error| Error::Document {
                    path: child.clone(),
                    error,
                })?;
                if store.header.file_type != expected || !store.checksum_mismatches.is_empty() {
                    return Err(Error::Document {
                        path: child,
                        error: onestore::Error {
                            offset: 0,
                            message: "Unexpected file type or checksum mismatch",
                        },
                    });
                }
                let held = held(&store, expected);
                if let Ok(held) = &held {
                    read.insert(
                        child.clone(),
                        Read {
                            listed: entry.listed,
                            header: bytes[..1024].iter().map(|b| format!("{b:02x}")).collect(),
                            length: bytes.len() as u64,
                            held: held.clone(),
                        },
                    );
                }
                (store.header.file_id, held)
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
            }),
            Ok(Held::Locked) => result.sections.push(Section {
                path: child.clone(),
                file_id,
                state: SectionState::Locked,
            }),
            Ok(Held::Toc(unresolved)) => {
                result.toc = Some(Toc {
                    filename: entry.name.clone(),
                    file_id,
                    unresolved,
                })
            }
            Err(error) if expected == FileType::Section => result.sections.push(Section {
                path: child.clone(),
                file_id,
                state: SectionState::Unreadable(error),
            }),
            Err(error) => return Err(Error::Document { path: child, error }),
        }
        if let Some(first) = identities.insert(file_id, child.clone()) {
            return Err(Error::DuplicateIdentity {
                file: file_id,
                first,
                second: child,
            });
        }
    }
    let order: BTreeMap<_, _> = result
        .toc
        .iter()
        .flat_map(|toc| &toc.unresolved)
        .map(|entry| (entry.file, entry.order))
        .collect();
    result.sections.sort_by(|a, b| {
        (order.get(&a.file_id).copied().unwrap_or(u32::MAX), &a.path)
            .cmp(&(order.get(&b.file_id).copied().unwrap_or(u32::MAX), &b.path))
    });
    result.groups.sort_by(|a, b| {
        (
            a.toc
                .as_ref()
                .and_then(|toc| order.get(&toc.file_id).copied())
                .unwrap_or(u32::MAX),
            &a.path,
        )
            .cmp(&(
                b.toc
                    .as_ref()
                    .and_then(|toc| order.get(&toc.file_id).copied())
                    .unwrap_or(u32::MAX),
                &b.path,
            ))
    });
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
        .entries(path, listing.len())
        .map_err(|error| Error::Io {
            path: path.into(),
            error,
        })?;
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
        if root(1).is_some_and(|node| matches!(node.kind, Kind::Encrypted { .. })) {
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
    let Some(Kind::Toc { entries, .. }) = root(1).map(|node| &node.kind) else {
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
    Ok(Held::Toc(unresolved))
}

/// Access denied or a file gone mid-listing; a lost connection stays fatal.
fn unavailable(error: &Error) -> bool {
    matches!(error, Error::Io { error, .. } if matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
    ))
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
