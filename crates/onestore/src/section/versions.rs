//! Page versions, the earlier states of a page OneNote 2010 keeps (MS-ONE 2.1.17, 2.1.18).
//! A version is an earlier revision of the page's own object space, current under a context
//! of its own. The page's version history, the space's revision current under `HISTORY`,
//! holds a proxy for each (`jcidVersionProxy`), newest first. `corpus/page-versions` records
//! what OneNote 2010 writes.

use super::{Frozen, Open, Section, SpaceState, changed, one_page};
use crate::{
    Error, ExGuid, FileType, Object, ObjectData, ResolvedRevision, RevisionIndex, Store,
    active::{ActivePage, manifest_pages},
    create::{author_properties, current_timestamps, properties},
    document::{Element, Kind, Revision},
    op::{Failure, OpError},
    page::Page,
    pages::set_references,
    write::{LiveRevision, PropertyObject, chain_depth, compact, declared},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

type Result<T> = std::result::Result<T, Error>;

/// The context of a page's version history, {7111497F-1B6B-4209-9491-C98B04CF4C5A},1.
pub(crate) const HISTORY: ExGuid = ExGuid {
    guid: [
        0x7f, 0x49, 0x11, 0x71, 0x6b, 0x1b, 0x09, 0x42, 0x94, 0x91, 0xc9, 0x8b, 0x04, 0xcf, 0x4c,
        0x5a,
    ],
    n: 1,
};

/// A version's context is its revision's identity with these bits flipped, and its proxy's
/// identity the context's with `PROXY` flipped, as OneNote 2010 derives both.
const CONTEXT: [u8; 16] = [
    0x7c, 0xfe, 0x52, 0x9d, 0xbc, 0x8f, 0x95, 0x44, 0x93, 0x00, 0x12, 0x9b, 0x57, 0x97, 0xa9, 0xea,
];
const PROXY: [u8; 16] = [
    0xca, 0x0b, 0x44, 0x88, 0x8c, 0x11, 0x81, 0x0e, 0x32, 0xbf, 0x83, 0xdb, 0x8c, 0xb8, 0xb6, 0xb5,
];

fn flipped(mut id: ExGuid, salt: [u8; 16]) -> ExGuid {
    for (byte, salt) in id.guid.iter_mut().zip(salt) {
        *byte ^= salt;
    }
    id
}

/// An earlier state of a page, as the page list names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageVersion {
    /// The context its revision is current under, which names it.
    pub context: ExGuid,
    /// FILETIME of its last change (`LastModifiedTimeStamp`).
    pub modified: Option<u64>,
    /// Who last changed it (`AuthorMostRecent`).
    pub author: Option<String>,
}

fn element<'o>(object: &Object<'o>) -> Result<Element<'o>> {
    Element::parse_with(object, FileType::Section, &mut |_| {
        Err(Error {
            offset: 0,
            message: "Version metadata holds no payload",
        })
    })
}

/// When `revision`'s page was last changed and by whom, as its revision metadata says.
fn last_change(revision: &ResolvedRevision<'_>) -> Result<(Option<u64>, Option<ExGuid>)> {
    let Some(metadata) = revision
        .roots
        .get(&4)
        .and_then(|id| revision.objects.get(id))
    else {
        return Ok((None, None));
    };
    let metadata = element(metadata)?;
    let Kind::RevisionMetadata { modified_filetime } = metadata.kind else {
        return Ok((None, None));
    };
    Ok((modified_filetime, metadata.latest_author))
}

impl<'a> Section<'a> {
    /// The versions of each page that has them, in section order, each page's newest first as
    /// OneNote 2010 lists them; read from the version histories alone.
    pub fn versions(&mut self) -> Result<Vec<(ExGuid, Vec<PageVersion>)>> {
        let mut listed = Vec::new();
        for (page, ..) in self.pages()? {
            let versions: Vec<PageVersion> = self
                .listed(page)?
                .into_iter()
                .map(|(_, version)| version)
                .collect();
            if !versions.is_empty() {
                listed.push((page, versions));
            }
        }
        Ok(listed)
    }

    /// Page `space` as its version `context` holds it. Reads the sealed image: O(section).
    pub fn version(&self, space: ExGuid, context: ExGuid) -> Result<Page> {
        if !self
            .listed(space)?
            .iter()
            .any(|(_, version)| version.context == context)
        {
            return Err(Error {
                offset: 0,
                message: "The page has no such version",
            });
        }
        let stored = &self.spaces[&space];
        if let Some(frozen) = stored
            .frozen
            .iter()
            .find(|frozen| flipped(frozen.rid, CONTEXT) == context)
        {
            let files = self.files();
            let pending = &self.payloads;
            let (view, _) = Revision::parse(
                space,
                &frozen.live.revision,
                FileType::Section,
                &mut |guid| match pending.iter().find(|(id, _)| *id == guid) {
                    Some((_, payload)) => Ok(*payload),
                    None => files(guid),
                },
            )?;
            return one_page(&view, &manifest_pages(&view));
        }
        let image = self.image();
        let store = Store::parse(&image)?;
        let rid = self.version_revision(&RevisionIndex::parse(&store)?, space, context)?;
        self.page_at(space, rid)
    }

    /// Page `space` as its stored revision `revision` holds it, whichever context that
    /// revision is current under, if any. Reads the sealed image: O(section).
    pub fn page_at(&self, space: ExGuid, revision: ExGuid) -> Result<Page> {
        let image = self.image();
        let store = Store::parse(&image)?;
        let resolved = RevisionIndex::parse(&store)?.resolve(space, revision)?;
        let (view, _) = Revision::parse(space, &resolved, FileType::Section, &mut |guid| {
            store.file_data(guid)
        })?;
        one_page(&view, &manifest_pages(&view))
    }

    /// Page `space`'s versions, newest first, with their proxies.
    fn listed(&self, space: ExGuid) -> Result<Vec<(ExGuid, PageVersion)>> {
        let Some(history) = self
            .spaces
            .get(&space)
            .and_then(|space| space.history.as_ref())
        else {
            return Ok(Vec::new());
        };
        let lookup = |id: &ExGuid| -> Result<Option<Object<'_>>> {
            match history.pending.get(id) {
                Some(object) => {
                    declared(object.jcid, &object.bytes, Arc::clone(&object.global_ids)).map(Some)
                }
                None => Ok(history.revision.objects.get(id).cloned()),
            }
        };
        let Some(root) = history.revision.roots.get(&1) else {
            return Ok(Vec::new());
        };
        let Some(root) = lookup(root)? else {
            return Ok(Vec::new());
        };
        let mut versions = Vec::new();
        for proxy in element(&root)?.children {
            let Some(object) = lookup(&proxy)? else {
                return Err(Error {
                    offset: 0,
                    message: "A version proxy is unavailable",
                });
            };
            let node = element(&object)?;
            let Kind::VersionProxy {
                context,
                modified_filetime,
            } = node.kind
            else {
                return Err(Error {
                    offset: 0,
                    message: "A version history lists another kind of object",
                });
            };
            let author = match node
                .latest_author
                .as_ref()
                .map(lookup)
                .transpose()?
                .flatten()
            {
                Some(author) => match element(&author)?.kind {
                    Kind::Author { name } => name,
                    _ => None,
                },
                None => None,
            };
            versions.push((
                proxy,
                PageVersion {
                    context,
                    modified: modified_filetime,
                    author,
                },
            ));
        }
        Ok(versions)
    }

    /// The revision version `context` of `space` is, in `index` of the sealed image.
    fn version_revision(
        &self,
        index: &RevisionIndex<'_>,
        space: ExGuid,
        context: ExGuid,
    ) -> Result<ExGuid> {
        let labelled = self.spaces[&space]
            .labels
            .iter()
            .find(|(_, labelled)| *labelled == context)
            .map(|(rid, _)| *rid);
        labelled
            .or_else(|| index.spaces.get(&space)?.labels.get(&(context, 1)).copied())
            .ok_or(Error {
                offset: 0,
                message: "A version's revision is not stored",
            })
    }

    /// Version `context` of `space` as a revision of the section's own bytes, with its
    /// identity and chain depth. Reads the sealed image: O(section).
    fn historic(
        &self,
        space: ExGuid,
        context: ExGuid,
    ) -> Result<(ExGuid, ResolvedRevision<'a>, usize)> {
        if self.spaces[&space]
            .frozen
            .iter()
            .any(|frozen| flipped(frozen.rid, CONTEXT) == context)
        {
            return Err(Error {
                offset: 0,
                message: "This version is still saving. Restore it in a moment.",
            });
        }
        let image = self.image();
        let store = Store::parse(&image)?;
        let index = RevisionIndex::parse(&store)?;
        let rid = self.version_revision(&index, space, context)?;
        let resolved = index.resolve(space, rid)?;
        // Object bytes lie in the section's segments, which outlive this image.
        let resident = |part: &[u8]| -> Result<&'a [u8]> {
            let offset = (part.as_ptr().addr() - image.as_ptr().addr()) as u64;
            self.segments
                .iter()
                .rfind(|(start, _)| *start <= offset)
                .and_then(|(start, segment)| {
                    let at = (offset - start) as usize;
                    segment.get(at..at + part.len())
                })
                .filter(|bytes| *bytes == part)
                .ok_or(Error {
                    offset: offset as usize,
                    message: "A version's bytes lie outside the section",
                })
        };
        let mut objects = BTreeMap::new();
        for (id, object) in resolved.objects {
            let data = match object.data {
                ObjectData::Properties(data) => ObjectData::Properties(resident(data)?),
                ObjectData::Encrypted(data) => ObjectData::Encrypted(resident(data)?),
                ObjectData::File {
                    reference,
                    extension,
                } => ObjectData::File {
                    reference: resident(reference)?,
                    extension: resident(extension)?,
                },
            };
            objects.insert(
                id,
                Object {
                    jcid: object.jcid,
                    reference_count: object.reference_count,
                    data,
                    global_ids: object.global_ids,
                },
            );
        }
        let revision = ResolvedRevision {
            roots: resolved.roots,
            objects,
        };
        Ok((rid, revision, chain_depth(&index, space, rid)))
    }

    /// Restores version `context` of page `space` as OneNote 2010's Restore Version does: the
    /// page as it stands becomes the newest version, and the page's next revision builds on
    /// the version's, its revision metadata naming `author` now. New objects take identities
    /// from `guid`.
    pub(crate) fn restore(
        &mut self,
        author: &str,
        space: ExGuid,
        context: ExGuid,
        guid: [u8; 16],
    ) -> std::result::Result<(), Failure> {
        if !self
            .listed(space)?
            .iter()
            .any(|(_, version)| version.context == context)
        {
            return Err(OpError::TargetUnavailable(context).into());
        }
        let (from, revision, depth) = self.historic(space, context)?;
        let (now, filetime) = current_timestamps()?;
        let named = |n| ExGuid { guid, n };
        self.remember(space);
        let files = self.files();
        let arena = &self.arena.0;
        self.editable(space)?;
        let stored = self.spaces.get_mut(&space).unwrap();
        let SpaceState::Open(open) = &mut stored.state else {
            unreachable!()
        };
        // The page as it stands: the stored revision, or its unsaved state as one of its own.
        let (current, modified, who) = if open.pending.is_empty() && stored.restored.is_none() {
            let rid = stored.rid.ok_or(Error {
                offset: 0,
                message: "A page never saved has no versions",
            })?;
            let (modified, who) = last_change(&open.file.revision)?;
            let who = who
                .and_then(|id| open.file.revision.objects.get(&id))
                .cloned();
            (rid, modified, who)
        } else {
            let mut live = open.file.clone();
            let revised = changed(arena, &open.page.live, &open.pending, &mut live, true)?
                .expect("a forced revision is written");
            let rid = named(1);
            stored.frozen.push(Frozen {
                rid,
                previous: stored.restored.or(stored.rid),
                live,
                revised,
            });
            (rid, None, None)
        };
        let label = flipped(current, CONTEXT);
        stored.labels.push((current, label));
        // Its proxy, newest in the history, names when and by whom it was last changed.
        let proxy = flipped(label, PROXY);
        let proxy_author = named(3);
        let author_object = match &who {
            Some(object) => PropertyObject::from_object(object)?,
            None => PropertyObject {
                jcid: 0x120001,
                bytes: properties(&author_properties(author))?,
                global_ids: Arc::new(BTreeMap::new()),
            },
        };
        let author_object = PropertyObject {
            global_ids: Arc::new(BTreeMap::from([(0, proxy_author.guid)])),
            ..author_object
        };
        let table = BTreeMap::from([(0, proxy.guid), (1, proxy_author.guid), (2, label.guid)]);
        let proxy_object = PropertyObject {
            jcid: 0x6003d,
            bytes: properties(&[
                (0x14001d09, now.to_le_bytes().to_vec()),
                (
                    0x18001d77,
                    modified.unwrap_or(filetime).to_le_bytes().to_vec(),
                ),
                (0x20001d79, compact(proxy_author, &table)?.to_vec()),
                (0x3400347b, compact(label, &table)?.to_vec()),
            ])?,
            global_ids: Arc::new(table),
        };
        let listed: Vec<ExGuid> = self
            .listed(space)?
            .into_iter()
            .map(|(proxy, _)| proxy)
            .collect();
        let stored = self.spaces.get_mut(&space).unwrap();
        let history = stored.history.as_mut().ok_or(Error {
            offset: 0,
            message: "The page has no version history",
        })?;
        let root = *history.revision.roots.get(&1).ok_or(Error {
            offset: 0,
            message: "The version history has no content",
        })?;
        let mut root_object = match history.pending.get(&root) {
            Some(object) => object.clone(),
            None => PropertyObject::from_object(&history.revision.objects[&root])?,
        };
        let children: Vec<ExGuid> = std::iter::once(proxy).chain(listed).collect();
        set_references(&mut root_object, 0x24001c20, &children)?;
        history.pending.extend([
            (root, root_object),
            (proxy, proxy_object),
            (proxy_author, author_object),
        ]);
        // The page's next revision builds on the version's.
        let file = LiveRevision::new(revision, depth)?;
        let page = ActivePage::open(space, file.clone(), FileType::Section, files)?;
        stored.state = SpaceState::Open(Box::new(Open {
            file,
            page,
            pending: BTreeSet::new(),
        }));
        stored.restored = Some(from);
        // Its revision metadata names who restored it, and when.
        let active = self.active(space)?;
        let revision = &active.live.revision;
        let mut changes = BTreeMap::new();
        if let Some(id) = revision.roots.get(&4)
            && revision.objects[id].jcid == 0x20044
        {
            let restorer = named(2);
            let mut metadata = PropertyObject::from_object(&revision.objects[id])?;
            let reference = metadata.reference(restorer)?;
            metadata.set(&[
                (0x18001d77, &filetime.to_le_bytes()),
                (0x20001d79, &reference),
            ])?;
            changes.insert(*id, metadata);
            changes.insert(
                restorer,
                PropertyObject {
                    jcid: 0x120001,
                    bytes: properties(&author_properties(author))?,
                    global_ids: Arc::new(BTreeMap::from([(0, restorer.guid)])),
                },
            );
        }
        self.apply_changes(space, &[], changes)?;
        Ok(())
    }

    /// Deletes versions `contexts` of page `space` as OneNote 2010's Delete Version does: its
    /// history stops listing them; their revisions stay stored. True when none is left.
    pub(crate) fn unlist(
        &mut self,
        space: ExGuid,
        contexts: &[ExGuid],
    ) -> std::result::Result<bool, Failure> {
        let listed = self.listed(space)?;
        if let Some(context) = contexts.iter().find(|context| {
            !listed
                .iter()
                .any(|(_, version)| version.context == **context)
        }) {
            return Err(OpError::TargetUnavailable(*context).into());
        }
        let kept: Vec<ExGuid> = listed
            .iter()
            .filter(|(_, version)| !contexts.contains(&version.context))
            .map(|(proxy, _)| *proxy)
            .collect();
        self.remember(space);
        let history = self
            .spaces
            .get_mut(&space)
            .and_then(|space| space.history.as_mut())
            .ok_or(Error {
                offset: 0,
                message: "The page has no version history",
            })?;
        let root = *history.revision.roots.get(&1).ok_or(Error {
            offset: 0,
            message: "The version history has no content",
        })?;
        let mut object = match history.pending.get(&root) {
            Some(object) => object.clone(),
            None => PropertyObject::from_object(&history.revision.objects[&root])?,
        };
        if kept.is_empty() {
            object.remove(&[0x24001c20])?;
        } else {
            set_references(&mut object, 0x24001c20, &kept)?;
        }
        history.pending.insert(root, object);
        Ok(kept.is_empty())
    }
}
