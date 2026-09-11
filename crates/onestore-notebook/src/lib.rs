#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use onestore::{
    FileType, RevisionIndex, Store,
    document::{Document, Kind},
};
use serde::Serialize;
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
}

#[derive(Debug, Serialize)]
pub struct Toc {
    pub filename: String,
    pub file_id: [u8; 16],
    /// Stale or unavailable TOC references; these are not inferred active sections.
    pub unresolved: Vec<TocReference>,
}

#[derive(Debug, Serialize)]
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
    let mut remaining = limits.entries;
    let mut identities = BTreeMap::new();
    scan(source, "", &limits, 0, &mut remaining, &mut identities)
}

fn scan(
    source: &mut impl Source,
    path: &str,
    limits: &Limits,
    depth: usize,
    remaining: &mut usize,
    identities: &mut BTreeMap<[u8; 16], String>,
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
    };
    for entry in &listing {
        let child = join(path, &entry.name);
        if entry.kind == EntryKind::Directory {
            if entry.name.to_ascii_lowercase().ends_with("_onefiles") {
                continue;
            }
            result.groups.push(scan(
                source,
                &child,
                limits,
                depth + 1,
                remaining,
                identities,
            )?);
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
        let bytes = source
            .read(&child, limits.bytes_per_file)
            .map_err(|error| Error::Io {
                path: child.clone(),
                error,
            })?;
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
        let file_id = store.header.file_id;
        let parsed = (|| {
            let index = RevisionIndex::parse(&store)?;
            let document = Document::parse(&index)?;
            let revision = document.active(document.root)?;
            if expected == FileType::Section {
                if revision
                    .roots
                    .get(&1)
                    .and_then(|id| revision.nodes.get(id))
                    .is_some_and(|node| matches!(node.kind, Kind::Encrypted { .. }))
                {
                    result.sections.push(Section {
                        path: child.clone(),
                        file_id,
                        state: SectionState::Locked,
                    });
                    return Ok(());
                }
                index.validate_current()?;
                document.pages()?;
                let name = revision
                    .roots
                    .get(&2)
                    .and_then(|id| revision.nodes.get(id))
                    .and_then(|node| match &node.kind {
                        Kind::SectionMetadata { name, .. } => name.clone(),
                        _ => None,
                    });
                result.sections.push(Section {
                    path: child.clone(),
                    file_id,
                    state: SectionState::Readable { name },
                });
            } else {
                index.validate_current()?;
                let node = revision.roots.get(&1).and_then(|id| revision.nodes.get(id));
                let Some(Kind::Toc { entries, .. }) = node.map(|node| &node.kind) else {
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
                    if filename.as_deref().is_some_and(|name| !component(name))
                        || !seen.insert(*file)
                    {
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
                result.toc = Some(Toc {
                    filename: entry.name.clone(),
                    file_id,
                    unresolved,
                });
            }
            Ok(())
        })();
        if let Err(error) = parsed {
            if expected == FileType::Section {
                result.sections.push(Section {
                    path: child.clone(),
                    file_id,
                    state: SectionState::Unreadable(error),
                });
            } else {
                return Err(Error::Document { path: child, error });
            }
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
    if observed != listing {
        return Err(Error::Changed { path: path.into() });
    }
    Ok(result)
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
