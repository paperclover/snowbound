//! A section file kept parsed across edits. Edits apply to its object spaces in memory and
//! seal into one transaction appending one revision per changed space.

use crate::{
    Chunk, Error, ExGuid, FileType, ObjectData, Reference, ResolvedRevision, RevisionIndex, Stamp,
    Store, Transaction,
    active::{ActivePage, Changes, Files},
    document::Revision,
    page::Page,
    protected::{Key, Opened},
    store::StoreState,
    write::{
        Appending, Commit, LiveRevision, PropertyObject, Protection, Sealing, Written, chain_depth,
        check_transaction, declared, label_payload, node, replacement, unchanged,
    },
};
use bumpalo::Bump;
use std::collections::{BTreeMap, BTreeSet};

mod versions;
pub(crate) use versions::HISTORY;
pub use versions::PageVersion;

type Result<T> = std::result::Result<T, Error>;

/// Holds the bytes a `Section` reads: the image it opened and each transaction it sealed.
#[derive(Default)]
pub struct Arena(Bump);

/// A section file as its image and the transactions sealed on it leave it.
pub struct Section<'a> {
    arena: &'a Arena,
    /// The opened image, then the bytes each sealed transaction appended, at their offsets.
    segments: Vec<(u64, &'a [u8])>,
    /// The in-place writes of the sealed transactions, in order.
    patches: Vec<(u64, Vec<u8>)>,
    state: StoreState,
    /// Payloads the opened images embed, by identity, as they read.
    files: &'a [([u8; 16], Result<&'a [u8]>)],
    /// Payload identities the file-data store declares.
    declared: BTreeSet<[u8; 16]>,
    spaces: BTreeMap<ExGuid, Space<'a>>,
    /// Payloads applied changes embed, in order, until sealed.
    payloads: Vec<([u8; 16], &'a [u8])>,
    /// Whether an operation failed after it began changing state; the section must be
    /// reopened.
    broken: bool,
    /// The root object space, which lists the pages.
    root: ExGuid,
    /// While an edit applies, the spaces it changed as they were before it.
    undo: Option<Undo<'a>>,
    /// A password-protected section's key and what it decoded.
    unlocked: Option<Opened<'a>>,
}

#[derive(Default)]
struct Undo<'a> {
    /// `None` for a space the edit created.
    spaces: BTreeMap<ExGuid, Option<Space<'a>>>,
    payloads: usize,
}

#[derive(Clone)]
struct Space<'a> {
    /// The active revision the file stores; none before a new space's first seal.
    rid: Option<ExGuid>,
    state: SpaceState<'a>,
    /// The version a restore took the open revision from: the revision it depends on, which
    /// the seal writes even unchanged.
    restored: Option<ExGuid>,
    /// Page states restores made versions of, which the seal writes before the open revision.
    frozen: Vec<Frozen<'a>>,
    history: Option<History<'a>>,
    /// Revisions the seal labels as versions, with their contexts.
    labels: Vec<(ExGuid, ExGuid)>,
    /// The newest revision a seal appended to the space.
    newest: Option<ExGuid>,
}

impl<'a> Space<'a> {
    fn new(rid: Option<ExGuid>, state: SpaceState<'a>, history: Option<History<'a>>) -> Self {
        Self {
            rid,
            state,
            restored: None,
            frozen: Vec::new(),
            history,
            labels: Vec::new(),
            newest: None,
        }
    }
}

/// A page state a restore made a version of: a revision under the identity it chose.
#[derive(Clone)]
struct Frozen<'a> {
    rid: ExGuid,
    previous: Option<ExGuid>,
    live: LiveRevision<'a>,
    revised: Revised,
}

/// A revision `changed` committed: what it declares, and of those the objects whose bytes it
/// stores and the objects new to the space.
#[derive(Clone)]
struct Revised {
    commit: Commit,
    replaced: BTreeSet<ExGuid>,
    created: BTreeSet<ExGuid>,
}

/// A page's version history (`versions::HISTORY`), with the objects edits since the last
/// seal replaced in it.
#[derive(Clone)]
struct History<'a> {
    rid: ExGuid,
    revision: ResolvedRevision<'a>,
    depth: usize,
    pending: BTreeMap<ExGuid, PropertyObject>,
}

#[derive(Clone)]
enum SpaceState<'a> {
    /// As the opened image stores it, until first edited.
    Stored {
        revision: ResolvedRevision<'a>,
        depth: usize,
    },
    Open(Box<Open<'a>>),
}

#[derive(Clone)]
struct Open<'a> {
    /// The revision the file stores.
    file: LiveRevision<'a>,
    /// `file` with the changes applied since the last seal.
    page: ActivePage<'a>,
    /// Objects whose content, reachability or count changed since the last seal.
    pending: BTreeSet<ExGuid>,
}

impl<'a> Section<'a> {
    /// Parses and fully validates a section image.
    pub fn open(arena: &'a Arena, image: Vec<u8>) -> Result<Self> {
        Ok(Self::open_with(arena, vec![image], None, current)?)
    }

    /// `open` for a password-protected section, decoding it under `key`, which edits are
    /// sealed under too. Decoded bytes live in `arena` until it drops.
    pub fn unlock(
        arena: &'a Arena,
        image: Vec<u8>,
        key: &Key,
    ) -> std::result::Result<Self, crate::protected::Error> {
        Self::open_with(arena, vec![image], Some(key), current)
    }

    /// Opens an earlier state of a section: each object space in `revisions` at the revision
    /// given, taken from the first of `images` that stores it, and no other space; the first
    /// image is the section's. For reading, as a merge reads the state two copies of a section
    /// last shared, which may lie partly in each copy.
    pub fn open_at(
        arena: &'a Arena,
        images: Vec<Vec<u8>>,
        revisions: &BTreeMap<ExGuid, ExGuid>,
    ) -> Result<Self> {
        let section = Self::open_with(arena, images, None, |id, space| {
            revisions
                .get(id)
                .copied()
                .filter(|rid| space.revisions.contains_key(rid))
        })?;
        if !section.spaces.contains_key(&section.root) {
            return Err(Error {
                offset: 0,
                message: "The earlier state names no revision of the section's root",
            });
        }
        Ok(section)
    }

    /// `open`, each object space at the revision `revision` chooses in the first image where it
    /// chooses one; a space it chooses none for is left out. Payloads are read from the image
    /// that first declares them. A protected section opens under `key`.
    fn open_with(
        arena: &'a Arena,
        images: Vec<Vec<u8>>,
        key: Option<&Key>,
        revision: impl Fn(&ExGuid, &crate::ObjectSpace<'_>) -> Option<ExGuid>,
    ) -> std::result::Result<Self, crate::protected::Error> {
        let mut opened = None;
        let mut unlocked = None;
        let mut spaces = BTreeMap::new();
        let mut files: Vec<([u8; 16], Result<&'a [u8]>)> = Vec::new();
        for image in images {
            let bytes: &'a [u8] = arena.0.alloc_slice_copy(&image);
            drop(image);
            let store = Store::parse(bytes)?;
            if let Some(offset) = store.checksum_mismatches.first() {
                return Err(Error {
                    offset: *offset,
                    message: "Cannot write a file with transaction checksum damage",
                }
                .into());
            }
            if store.header.file_type != FileType::Section {
                return Err(Error {
                    offset: 0,
                    message: "Choose a section file",
                }
                .into());
            }
            let index = RevisionIndex::parse(&store)?;
            if let (Some(key), None) = (key, &unlocked) {
                unlocked = Some(Opened::new(&store, key)?);
            }
            // The parse borrows `bytes` only as long as its store; its slices lie in `bytes`,
            // which the arena keeps.
            let bound = |resolved| {
                bind(arena, unlocked.as_ref(), resolved, |part| {
                    Ok(within(bytes, part))
                })
            };
            index.validate_with(|space, rid| bound(index.resolve(space, rid)?))?;
            for (id, space) in &index.spaces {
                let Some(rid) = revision(id, space).filter(|_| !spaces.contains_key(id)) else {
                    continue;
                };
                let revision = bound(index.resolve(*id, rid)?)?;
                let history = space
                    .labels
                    .get(&(versions::HISTORY, 1))
                    .map(|history| -> Result<History<'a>> {
                        Ok(History {
                            rid: *history,
                            revision: bound(index.resolve(*id, *history)?)?,
                            depth: chain_depth(&index, *id, *history),
                            pending: BTreeMap::new(),
                        })
                    })
                    .transpose()?;
                let state = SpaceState::Stored {
                    revision,
                    depth: chain_depth(&index, *id, rid),
                };
                spaces.insert(*id, Space::new(Some(rid), state, history));
            }
            let known = files.len();
            for node in store.lists.values().flat_map(|list| &list.nodes) {
                if node.id != 0x94 || node.freed() {
                    continue;
                }
                let (Some(guid), Some(Reference::Data(chunk))) =
                    (node.payload.first_chunk(), node.reference)
                else {
                    return Err(Error {
                        offset: node.offset,
                        message: "File-data object lacks a data reference",
                    }
                    .into());
                };
                // A payload an earlier image declares is the same bytes: it names them.
                if !files[..known].iter().any(|(declared, ..)| declared == guid) {
                    let (offset, length) = (chunk.offset as usize, chunk.length as usize);
                    let payload = bytes
                        .get(offset..offset + length)
                        .ok_or(Error {
                            offset,
                            message: "Chunk extends outside the data area",
                        })
                        .and_then(|blob| crate::files::payload(blob, offset))
                        .and_then(|payload| match &unlocked {
                            Some(unlocked) => unlocked.file(&arena.0, payload),
                            None => Ok(payload),
                        });
                    files.push((*guid, payload));
                }
            }
            if opened.is_none() {
                opened = Some((bytes, store.state()?, index.root));
            }
        }
        let (bytes, state, root) = opened.ok_or(Error {
            offset: 0,
            message: "Choose a section file",
        })?;
        files.sort_by_key(|(guid, ..)| *guid);
        Ok(Self {
            arena,
            segments: vec![(0, bytes)],
            patches: Vec::new(),
            state,
            declared: files.iter().map(|(guid, ..)| *guid).collect(),
            files: arena.0.alloc_slice_copy(&files),
            spaces,
            payloads: Vec::new(),
            broken: false,
            root,
            undo: None,
            unlocked,
        })
    }

    /// The header and length of the image this section's sealed transactions leave.
    pub fn stamp(&self) -> &Stamp {
        &self.state.stamp
    }

    /// The image this section's sealed transactions leave; for tests and seeding caches.
    pub fn image(&self) -> Vec<u8> {
        let mut image = self
            .segments
            .iter()
            .flat_map(|(_, bytes)| *bytes)
            .copied()
            .collect::<Vec<_>>();
        for (offset, bytes) in &self.patches {
            let offset = *offset as usize;
            image[offset..offset + bytes.len()].copy_from_slice(bytes);
        }
        image[..1024].copy_from_slice(&self.state.stamp.header);
        image
    }

    /// Each object space with the active revision the sealed image stores for it.
    pub fn revisions(&self) -> impl Iterator<Item = (ExGuid, ExGuid)> + '_ {
        self.spaces
            .iter()
            .filter_map(|(space, stored)| Some((*space, stored.rid?)))
    }

    /// Each object space with the revision this section last sealed in it, where one did, or
    /// its active revision: a seal that changes only a page's versions leaves the page's
    /// active revision as it was.
    pub fn newest(&self) -> impl Iterator<Item = (ExGuid, ExGuid)> + '_ {
        self.spaces
            .iter()
            .filter_map(|(space, stored)| Some((*space, stored.newest.or(stored.rid)?)))
    }

    /// The page an object space holds, with the changes applied since the last seal.
    pub fn page(&self, space: ExGuid) -> Result<Page> {
        let stored = self.spaces.get(&space).ok_or(Error {
            offset: 0,
            message: "Object space has no active default revision",
        })?;
        match &stored.state {
            SpaceState::Open(open) => one_page(&open.page.view, &open.page.pages),
            SpaceState::Stored { revision, .. } => {
                let files = self.files();
                let (view, _) =
                    Revision::parse(space, revision, FileType::Section, &mut |guid| files(guid))?;
                one_page(&view, &crate::active::manifest_pages(&view))
            }
        }
    }

    /// The conflict pages of each page that has them, in section order, each page's in the
    /// order OneNote 2010 lists them under it: the last stored first. Read from the pages'
    /// manifests and the conflict pages' metadata alone.
    pub fn conflicts(&mut self) -> Result<Vec<(ExGuid, Vec<crate::ConflictPage>)>> {
        let listed: Vec<ExGuid> = self.pages()?.into_iter().map(|(space, ..)| space).collect();
        let element = |object: &crate::Object<'a>| {
            crate::document::Element::parse_with(object, FileType::Section, &mut |_| {
                Err(Error {
                    offset: 0,
                    message: "Page metadata holds no payload",
                })
            })
        };
        let mut conflicts = Vec::new();
        for page in listed {
            let revision = self.revision(page)?;
            let Some(manifest) = revision
                .roots
                .get(&1)
                .and_then(|id| revision.objects.get(id))
            else {
                continue;
            };
            let mut pages = Vec::new();
            for space in element(manifest)?.spaces.into_iter().rev() {
                let Some(metadata) = self.revision(space).ok().and_then(|revision| {
                    revision
                        .roots
                        .get(&2)
                        .and_then(|id| revision.objects.get(id))
                }) else {
                    continue;
                };
                let metadata = element(metadata)?;
                let crate::document::Kind::ConflictMetadata { title, author, .. } = metadata.kind
                else {
                    continue;
                };
                let created = metadata.extra[0]
                    .iter()
                    .find_map(|field| match field.value {
                        crate::document::FieldValue::Bytes(bytes) if field.id == 0x18001c65 => {
                            bytes.try_into().ok().map(u64::from_le_bytes)
                        }
                        _ => None,
                    });
                let mut objects = Vec::new();
                for (id, object) in &self.revision(space)?.objects {
                    let ObjectData::Properties(bytes) = object.data else {
                        continue;
                    };
                    if crate::PropertySets::parse(bytes)?.sets[0]
                        .iter()
                        .any(|property| property.id == 0x88001d96)
                    {
                        objects.push(*id);
                    }
                }
                pages.push(crate::ConflictPage {
                    space,
                    title: title.unwrap_or_default(),
                    user: author.unwrap_or_default(),
                    created,
                    objects,
                });
            }
            if !pages.is_empty() {
                conflicts.push((page, pages));
            }
        }
        Ok(conflicts)
    }

    /// The page series holding each listed page. Moving a page gives it a series of its
    /// own, so a page in another series than before was moved.
    pub fn series(&mut self) -> Result<BTreeMap<ExGuid, ExGuid>> {
        let view = &self.active(self.root)?.view;
        let section = view
            .roots
            .get(&1)
            .and_then(|id| view.nodes.get(id))
            .ok_or(Error {
                offset: 0,
                message: "Section root is unavailable",
            })?;
        Ok(section
            .children
            .iter()
            .filter_map(|series| Some((*series, view.nodes.get(series)?)))
            .flat_map(|(series, node)| node.spaces.iter().map(move |page| (*page, series)))
            .collect())
    }

    /// Page spaces in section order with the title and outline level (1 at the top) the
    /// page list shows, read from each page's metadata alone.
    pub fn pages(&mut self) -> Result<Vec<(ExGuid, String, u32)>> {
        let root = self.root;
        let view = &self.active(root)?.view;
        let section = view
            .roots
            .get(&1)
            .and_then(|id| view.nodes.get(id))
            .ok_or(Error {
                offset: 0,
                message: "Section root is unavailable",
            })?;
        let spaces: Vec<ExGuid> = section
            .children
            .iter()
            .filter_map(|series| view.nodes.get(series))
            .flat_map(|series| series.spaces.clone())
            .collect();
        let element = |object: &crate::Object<'a>| {
            crate::document::Element::parse_with(object, FileType::Section, &mut |_| {
                Err(Error {
                    offset: 0,
                    message: "Page metadata holds no payload",
                })
            })
        };
        let mut pages = Vec::new();
        for space in spaces {
            let revision = self.revision(space)?;
            let (mut title, mut level) = (None, 1);
            if let Some(metadata) = revision
                .roots
                .get(&2)
                .and_then(|id| revision.objects.get(id))
                && let crate::document::Kind::Metadata {
                    title: stored,
                    level: stored_level,
                } = element(metadata)?.kind
            {
                title = stored;
                level = stored_level.unwrap_or(1);
            }
            if title.is_none() {
                let manifest = revision
                    .roots
                    .get(&1)
                    .and_then(|id| revision.objects.get(id));
                if let Some(manifest) = manifest
                    && let Some(page) = element(manifest)?
                        .content
                        .first()
                        .and_then(|id| revision.objects.get(id))
                    && let crate::document::Kind::Page {
                        alternate_title, ..
                    } = element(page)?.kind
                {
                    title = alternate_title;
                }
            }
            let title = crate::edit::without_fields(&title.unwrap_or_default());
            pages.push((space, title, level));
        }
        Ok(pages)
    }

    /// Reads a payload of the opened image by identity.
    fn files(&self) -> Files<'a> {
        let files = self.files;
        std::rc::Rc::new(move |guid| {
            let start = files.partition_point(|(id, _)| *id < guid);
            let mut declared = files[start..].iter().take_while(|(id, _)| *id == guid);
            match (declared.next(), declared.next()) {
                (None, _) => Err(Error {
                    offset: 0,
                    message: "File-data object is not declared",
                }),
                (Some((_, payload)), None) => *payload,
                _ => Err(Error {
                    offset: 0,
                    message: "Duplicate file-data identity",
                }),
            }
        })
    }

    fn usable(&self) -> Result<()> {
        if self.broken {
            return Err(Error {
                offset: 0,
                message: "A failed edit left the section to be reopened",
            });
        }
        Ok(())
    }

    /// The editable state of an object space, opened on first use.
    pub(crate) fn active(&mut self, space: ExGuid) -> Result<&ActivePage<'a>> {
        Ok(&self.editable(space)?.page)
    }

    fn editable(&mut self, id: ExGuid) -> Result<&mut Open<'a>> {
        let files = self.files();
        let space = self.spaces.get_mut(&id).ok_or(Error {
            offset: 0,
            message: "Object space has no active default revision",
        })?;
        if let SpaceState::Stored { revision, depth } = &space.state {
            let file = LiveRevision::new(revision.clone(), *depth)?;
            let page = ActivePage::open(id, file.clone(), FileType::Section, files)?;
            space.state = SpaceState::Open(Box::new(Open {
                file,
                page,
                pending: BTreeSet::new(),
            }));
        }
        let SpaceState::Open(open) = &mut space.state else {
            unreachable!()
        };
        Ok(open)
    }

    /// Keeps a space as it is for the running edit to return to, the first time it changes it.
    pub(crate) fn remember(&mut self, space: ExGuid) {
        if let Some(undo) = &mut self.undo
            && !undo.spaces.contains_key(&space)
        {
            undo.spaces.insert(space, self.spaces.get(&space).cloned());
        }
    }

    /// Stores a writer's objects and payloads in a space until the next seal; false when
    /// it stores nothing. An error leaves the section to be reopened.
    pub(crate) fn apply_changes(
        &mut self,
        space: ExGuid,
        payloads: &[([u8; 16], &[u8])],
        changes: Changes,
    ) -> Result<bool> {
        self.usable()?;
        self.remember(space);
        let arena = &self.arena.0;
        self.broken = true;
        let open = self.editable(space)?;
        let embedded = open.page.payloads.len();
        let Some(touched) = open.page.store(arena, payloads, changes)? else {
            self.broken = false;
            return Ok(false);
        };
        open.pending.extend(touched);
        let embedded = open.page.payloads[embedded..].to_vec();
        self.payloads.extend(embedded);
        self.broken = false;
        Ok(true)
    }

    /// Creates an object space holding `changes` under `roots` until the next seal, which
    /// declares it. An error after validation leaves the section to be reopened.
    pub(crate) fn create_space(
        &mut self,
        space: ExGuid,
        roots: BTreeMap<u32, ExGuid>,
        changes: Changes,
    ) -> Result<()> {
        self.usable()?;
        if space.guid == [0; 16] || self.spaces.contains_key(&space) {
            return Err(Error {
                offset: 0,
                message: "Choose a new object-space identity in a section file",
            });
        }
        if roots.is_empty() || changes.is_empty() {
            return Err(Error {
                offset: 0,
                message: "New object space needs roots and objects",
            });
        }
        let file = LiveRevision::new(
            ResolvedRevision {
                roots,
                objects: BTreeMap::new(),
            },
            0,
        )?;
        let mut work = file.clone();
        let changes = work.prepare(changes, true)?;
        let mut objects = Vec::new();
        for (id, change) in changes {
            let bytes = self.arena.0.alloc_slice_copy(&change.bytes);
            objects.push((id, declared(change.jcid, bytes, change.global_ids)?));
        }
        if let Some(undo) = &mut self.undo {
            undo.spaces.entry(space).or_insert(None);
        }
        self.broken = true;
        let pending = work.commit(objects)?.touched;
        let all: Vec<ExGuid> = work.revision.objects.keys().copied().collect();
        work.settle(all)?;
        let page = ActivePage::open(space, work, FileType::Section, self.files())?;
        let state = SpaceState::Open(Box::new(Open {
            file,
            page,
            pending,
        }));
        self.spaces.insert(space, Space::new(None, state, None));
        self.broken = false;
        Ok(())
    }

    /// The root object space, which lists the pages.
    pub fn root(&self) -> ExGuid {
        self.root
    }

    /// The stored revision of a space, as edits since the last seal leave it.
    pub(crate) fn revision(&self, space: ExGuid) -> Result<&ResolvedRevision<'a>> {
        match self.spaces.get(&space).map(|space| &space.state) {
            Some(SpaceState::Stored { revision, .. }) => Ok(revision),
            Some(SpaceState::Open(open)) => Ok(&open.page.live.revision),
            None => Err(Error {
                offset: 0,
                message: "Object space has no active default revision",
            }),
        }
    }

    /// Runs `f`, returning the section to its state before it when `f` fails; `undo`
    /// saves each space before `f` first changes it, which costs a copy of that space.
    pub(crate) fn atomically<T, E>(
        &mut self,
        undo: bool,
        f: impl FnOnce(&mut Self) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        if undo {
            self.undo = Some(Undo {
                spaces: BTreeMap::new(),
                payloads: self.payloads.len(),
            });
        }
        let result = f(self);
        if let Some(undo) = self.undo.take()
            && result.is_err()
        {
            for (id, space) in undo.spaces {
                match space {
                    Some(space) => self.spaces.insert(id, space),
                    None => self.spaces.remove(&id),
                };
            }
            self.payloads.truncate(undo.payloads);
            self.broken = false;
        }
        result
    }

    /// Whether a failed operation left the section to be reopened.
    pub(crate) fn broken(&self) -> bool {
        self.broken
    }

    /// Appends one revision per space changed since the last seal, and the payloads they
    /// embed, as one transaction on `stamp`; none when nothing changed. An error leaves the
    /// section to be reopened.
    pub fn seal(&mut self) -> Result<Option<Transaction>> {
        self.seal_as(&BTreeMap::new())
    }

    /// `seal`, each space in `names` taking the revision identity named there, one the space
    /// does not store, instead of a fresh one: a merge names what it wrote after what it
    /// merged, so that the next merge knows it holds that.
    pub fn seal_as(&mut self, names: &BTreeMap<ExGuid, ExGuid>) -> Result<Option<Transaction>> {
        self.usable()?;
        if names.iter().any(|(space, name)| {
            name.guid == [0; 16]
                || self
                    .spaces
                    .get(space)
                    .is_some_and(|stored| stored.rid == Some(*name))
        }) {
            return Err(Error {
                offset: 0,
                message: "Name each revision anew",
            });
        }
        self.broken = true;
        let arena = &self.arena.0;
        let protection = self
            .unlocked
            .as_ref()
            .map(|unlocked| unlocked as &dyn Protection);
        let mut appending = Appending::new(self.state.clone());
        let mut sealed = Vec::new();
        for (id, space) in &mut self.spaces {
            // A space's revisions and labels go in one fragment: page states restores made
            // versions of, the open revision, the version history, then the labels.
            let mut manifest = Vec::new();
            for frozen in &space.frozen {
                appending.manifest(
                    &Sealing {
                        space: *id,
                        previous: frozen.previous,
                        new_space: false,
                        label: (ExGuid::default(), 1),
                        rid: Some(frozen.rid),
                        live: &frozen.live,
                        commit: &frozen.revised.commit,
                        replaced: &frozen.revised.replaced,
                        created: &frozen.revised.created,
                    },
                    &self.segments,
                    protection,
                    &mut manifest,
                )?;
            }
            let mut page = None;
            if let SpaceState::Open(open) = &mut space.state
                && (!open.pending.is_empty() || space.restored.is_some())
            {
                let file = &mut open.file;
                let restored = space.restored.is_some();
                let written = match changed(arena, &open.page.live, &open.pending, file, restored)?
                {
                    Some(Revised {
                        commit,
                        replaced,
                        created,
                    }) => {
                        let (rid, stored) = appending.manifest(
                            &Sealing {
                                space: *id,
                                previous: space.restored.or(space.rid),
                                new_space: space.rid.is_none(),
                                label: (ExGuid::default(), 1),
                                rid: names.get(id).copied(),
                                live: file,
                                commit: &commit,
                                replaced: &replaced,
                                created: &created,
                            },
                            &self.segments,
                            protection,
                            &mut manifest,
                        )?;
                        if commit.checkpoint {
                            let all: Vec<ExGuid> = file.revision.objects.keys().copied().collect();
                            file.settle(all)?;
                        } else {
                            file.settle(commit.changed.iter().copied())?;
                        }
                        Some((rid, stored, commit))
                    }
                    None => None,
                };
                page = Some(written);
            }
            let mut history = None;
            if let Some(stored) = &space.history
                && !stored.pending.is_empty()
            {
                let mut live = LiveRevision::new(stored.revision.clone(), stored.depth)?;
                let replacements = live.prepare(stored.pending.clone(), true)?;
                if !replacements.is_empty() {
                    let replaced: BTreeSet<ExGuid> = replacements.keys().copied().collect();
                    let created = replaced
                        .iter()
                        .filter(|id| !live.revision.objects.contains_key(id))
                        .copied()
                        .collect();
                    let mut objects = Vec::new();
                    for (object_id, change) in replacements {
                        let bytes = arena.alloc_slice_copy(&change.bytes);
                        objects.push((object_id, declared(change.jcid, bytes, change.global_ids)?));
                    }
                    let commit = live.commit(objects)?;
                    let (rid, chunks) = appending.manifest(
                        &Sealing {
                            space: *id,
                            previous: Some(stored.rid),
                            new_space: false,
                            label: (versions::HISTORY, 1),
                            rid: None,
                            live: &live,
                            commit: &commit,
                            replaced: &replaced,
                            created: &created,
                        },
                        &self.segments,
                        protection,
                        &mut manifest,
                    )?;
                    if commit.checkpoint {
                        let all: Vec<ExGuid> = live.revision.objects.keys().copied().collect();
                        live.settle(all)?;
                    } else {
                        live.settle(commit.changed.iter().copied())?;
                    }
                    history = Some((rid, chunks, live, commit));
                }
            }
            for (rid, context) in &space.labels {
                manifest.push(node(0x5d, None, &label_payload(*rid, *context))?);
            }
            if !manifest.is_empty() {
                appending.close(*id, space.rid.is_none(), &manifest)?;
            }
            if page.is_some() || !manifest.is_empty() {
                sealed.push((*id, page, history));
            }
        }
        let payloads: Vec<_> = self
            .payloads
            .iter()
            .filter(|(guid, _)| !self.declared.contains(guid))
            .copied()
            .collect();
        appending.payloads(&payloads, protection)?;
        #[cfg_attr(not(test), expect(unused_mut))]
        let mut transaction = appending.finish()?;
        #[cfg(test)]
        if let (Some(tamper), Some((transaction, _))) = (tests::TAMPER.get(), &mut transaction) {
            tamper(transaction);
        }
        if let Some((transaction, state)) = &transaction {
            let mut written = Vec::new();
            let mut labels = Vec::new();
            for (id, page, history) in &sealed {
                let space = &self.spaces[id];
                for frozen in &space.frozen {
                    written.push(Written {
                        space: *id,
                        rid: frozen.rid,
                        previous: frozen.previous,
                        label: (ExGuid::default(), 1),
                        live: &frozen.live,
                        commit: &frozen.revised.commit,
                    });
                }
                if let (Some(Some((rid, _, commit))), SpaceState::Open(open)) = (page, &space.state)
                {
                    written.push(Written {
                        space: *id,
                        rid: *rid,
                        previous: space.restored.or(space.rid),
                        label: (ExGuid::default(), 1),
                        live: &open.file,
                        commit,
                    });
                }
                if let (Some((rid, _, live, commit)), Some(stored)) = (history, &space.history) {
                    written.push(Written {
                        space: *id,
                        rid: *rid,
                        previous: Some(stored.rid),
                        label: (versions::HISTORY, 1),
                        live,
                        commit,
                    });
                }
                labels.extend(
                    space
                        .labels
                        .iter()
                        .map(|(rid, context)| (*id, *rid, *context)),
                );
            }
            check_transaction(
                &self.state,
                state,
                transaction,
                &self.segments,
                &written,
                &labels,
                &payloads,
                protection,
            )?;
            let base = transaction.base.length;
            let segment: &'a [u8] = arena.alloc_slice_copy(&transaction.append);
            self.segments.push((base, segment));
            self.patches.extend(transaction.patches.iter().cloned());
            self.state = state.clone();
            self.declared.extend(payloads.iter().map(|(guid, _)| *guid));
            // An object now reads from where it is stored, or a protected one's clear bytes
            // from where they are.
            let unlocked = &self.unlocked;
            let rebind = |revision: &mut ResolvedRevision<'a>, stored: &[(ExGuid, Chunk)]| {
                for (object_id, chunk) in stored {
                    let start = (chunk.offset - base) as usize;
                    let bytes = &segment[start..start + chunk.length as usize];
                    let data = &mut revision.objects.get_mut(object_id).unwrap().data;
                    match (unlocked, *data) {
                        (Some(unlocked), ObjectData::Properties(clear)) => {
                            unlocked.sealed(clear, bytes)
                        }
                        _ => *data = ObjectData::Properties(bytes),
                    }
                }
            };
            for (id, page, history) in &mut sealed {
                let space = self.spaces.get_mut(id).unwrap();
                space.frozen.clear();
                space.labels.clear();
                if let (Some(Some((rid, stored, _))), SpaceState::Open(open)) =
                    (&*page, &mut space.state)
                {
                    space.rid = Some(*rid);
                    space.restored = None;
                    space.newest = Some(*rid);
                    rebind(&mut open.file.revision, stored);
                }
                if let Some((rid, stored, live, _)) = history.take() {
                    let mut revision = live.revision;
                    rebind(&mut revision, &stored);
                    space.history = Some(History {
                        rid,
                        revision,
                        depth: live.depth,
                        pending: BTreeMap::new(),
                    });
                    space.newest = Some(rid);
                }
            }
        }
        for (id, page, _) in sealed {
            let Some(revision) = page else {
                continue;
            };
            let SpaceState::Open(open) = &mut self.spaces.get_mut(&id).unwrap().state else {
                unreachable!()
            };
            let mut compare = std::mem::take(&mut open.pending);
            if let Some((_, _, commit)) = revision {
                if commit.checkpoint {
                    compare.extend(open.file.revision.objects.keys());
                } else {
                    compare.extend(commit.touched);
                }
            }
            open.page.adopt(&open.file, compare)?;
        }
        self.payloads.clear();
        self.broken = false;
        Ok(transaction.map(|(transaction, _)| transaction))
    }

    /// Applies a transaction sealed on this section's stamp, as reopening the image it
    /// leaves would; nothing may be applied since the last seal.
    pub fn replay(&mut self, transaction: &Transaction) -> Result<()> {
        self.usable()?;
        if !self.payloads.is_empty()
            || self.spaces.values().any(
                |space| matches!(&space.state, SpaceState::Open(open) if !open.pending.is_empty()),
            )
        {
            return Err(Error {
                offset: 0,
                message: "Seal applied changes before replaying a transaction",
            });
        }
        let mut image = self.image();
        transaction.apply(&mut image)?;
        let key = self
            .unlocked
            .as_ref()
            .map(|unlocked| unlocked.key().clone());
        *self = Self::open_with(self.arena, vec![image], key.as_ref(), current)?;
        Ok(())
    }
}

/// Commits to `file` the objects `pending` names that `page` changed, with what the revision
/// declares, replaces and creates; none when that changes nothing, unless `force`.
fn changed<'a>(
    arena: &'a Bump,
    page: &LiveRevision<'a>,
    pending: &BTreeSet<ExGuid>,
    file: &mut LiveRevision<'a>,
    force: bool,
) -> Result<Option<Revised>> {
    let mut replacements = BTreeMap::new();
    for object_id in pending {
        if !page.is_reachable(*object_id) {
            continue;
        }
        let object = &page.revision.objects[object_id];
        if let Some(stored) = file.revision.objects.get(object_id)
            && unchanged(stored, object)?
        {
            continue;
        }
        replacements.insert(*object_id, replacement(*object_id, object)?);
    }
    let replacements = file.prepare(replacements, true)?;
    if replacements.is_empty() && !force {
        return Ok(None);
    }
    let replaced: BTreeSet<ExGuid> = replacements.keys().copied().collect();
    let created = replaced
        .iter()
        .filter(|id| !file.revision.objects.contains_key(id))
        .copied()
        .collect();
    let mut objects = Vec::new();
    for (object_id, change) in replacements {
        // The page holds these bytes already unless preparing remapped them.
        let bytes = match page.revision.objects.get(&object_id).map(|o| o.data) {
            Some(ObjectData::Properties(bytes)) if bytes == change.bytes => bytes,
            _ => arena.alloc_slice_copy(&change.bytes),
        };
        objects.push((object_id, declared(change.jcid, bytes, change.global_ids)?));
    }
    Ok(Some(Revised {
        commit: file.commit(objects)?,
        replaced,
        created,
    }))
}

/// `resolved` with every slice moved into bytes that live as long as the section by `bytes`,
/// a protected section's objects decoded under `unlocked`.
fn bind<'a>(
    arena: &'a Arena,
    unlocked: Option<&Opened<'a>>,
    resolved: ResolvedRevision<'_>,
    bytes: impl Fn(&[u8]) -> Result<&'a [u8]>,
) -> Result<ResolvedRevision<'a>> {
    let mut objects = BTreeMap::new();
    for (id, object) in resolved.objects {
        let data = match (object.data, unlocked) {
            (ObjectData::Properties(data), _) => ObjectData::Properties(bytes(data)?),
            (ObjectData::Encrypted(data), Some(unlocked)) => {
                ObjectData::Properties(unlocked.property(&arena.0, bytes(data)?)?)
            }
            (ObjectData::Encrypted(data), None) => ObjectData::Encrypted(bytes(data)?),
            (
                ObjectData::File {
                    reference,
                    extension,
                },
                _,
            ) => ObjectData::File {
                reference: bytes(reference)?,
                extension: bytes(extension)?,
            },
        };
        let object = crate::Object {
            jcid: object.jcid,
            reference_count: object.reference_count,
            data,
            global_ids: object.global_ids,
        };
        objects.insert(id, object);
    }
    Ok(ResolvedRevision {
        roots: resolved.roots,
        objects,
    })
}

/// The revision a space is current under.
fn current(_: &ExGuid, space: &crate::ObjectSpace<'_>) -> Option<ExGuid> {
    space.labels.get(&(ExGuid::default(), 1)).copied()
}

/// `part`, a slice of `image`, with the image's lifetime.
fn within<'a>(image: &'a [u8], part: &[u8]) -> &'a [u8] {
    let start = part.as_ptr().addr() - image.as_ptr().addr();
    &image[start..start + part.len()]
}

fn one_page(view: &Revision<'_>, pages: &[ExGuid]) -> Result<Page> {
    let [page] = pages else {
        return Err(Error {
            offset: 0,
            message: "Choose an object space containing one active page",
        });
    };
    Page::from_revision(view, *page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Insertion, active::tests::Writes, document::Document, write::GUIDS};

    thread_local! {
        /// Changes a transaction after it is built and before it is checked.
        pub(super) static TAMPER: std::cell::Cell<Option<fn(&mut Transaction)>> =
            const { std::cell::Cell::new(None) };
    }

    /// `f` with `fresh_guid` counting from `seed`.
    fn seeded<T>(seed: u64, f: impl FnOnce() -> T) -> T {
        let outer = GUIDS.replace(Some(seed));
        let result = f();
        GUIDS.set(outer);
        result
    }

    /// The image the image writer leaves: the `edited` revisions squashed onto the source
    /// `index` parsed.
    fn squashed(
        index: &RevisionIndex<'_>,
        edited: &[(ExGuid, &ResolvedRevision<'_>)],
        payloads: &[([u8; 16], &[u8])],
    ) -> Vec<u8> {
        let transaction = crate::write::squash(index, edited, payloads).unwrap();
        crate::write::applied(index.store.data, transaction.as_ref()).unwrap()
    }

    /// The page spaces of an image, fullest first.
    fn page_spaces(image: &[u8]) -> Vec<ExGuid> {
        let store = Store::parse(image).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut spaces: Vec<_> = document
            .pages()
            .unwrap()
            .into_iter()
            .map(|(space, _)| (document.active(space).unwrap().nodes.len(), space))
            .collect();
        spaces.sort_by(|a, b| b.cmp(a));
        spaces.into_iter().map(|(_, space)| space).collect()
    }

    /// Applies seeded batches of typed-writer changes to the pages of `source` through a
    /// `Section` and through the writer that parses the image and squashes a batch into
    /// one revision, requiring equal bytes after every seal.
    fn differential(
        source: &[u8],
        seed: u64,
        batches: usize,
        largest: usize,
        pages: usize,
    ) -> usize {
        let arena = Arena::default();
        let mut section = Section::open(&arena, source.to_vec()).unwrap();
        let spaces = page_spaces(source);
        let mut image = source.to_vec();
        let mut writes = Writes(seed);
        // New objects count from far above the identities builds draw.
        GUIDS.set(Some(1 << 62 | seed << 40));
        let (mut sealed, mut checkpoints) = (0, 0);
        for batch in 0..batches {
            // The writers stamp modification times; a fixed clock keeps the batches alike.
            let at = 134_000_000_000_000_000 + batch as u64 * 10_000_000;
            crate::create::at(at, || {
                let space = spaces[batch % spaces.len().min(pages)];
                let store = Store::parse(&image).unwrap();
                let index = RevisionIndex::parse(&store).unwrap();
                let bump = Bump::new();
                let mut legacy = ActivePage::parse(&index, space).unwrap();
                for _ in 0..1 + (writes.0 as usize >> 40) % largest {
                    let Ok(changes) = writes.next(section.active(space).unwrap()) else {
                        continue;
                    };
                    let stored = legacy.write(&bump, &[], changes.clone()).unwrap();
                    assert_eq!(
                        section.apply_changes(space, &[], changes).unwrap(),
                        stored,
                        "batch {batch}"
                    );
                }
                let guids = (batch as u64 + 1) << 32;
                let expected = seeded(guids, || {
                    squashed(&index, &[(space, &legacy.live.revision)], &legacy.payloads)
                });
                let before = section.stamp().clone();
                let chains = depths(&section);
                let transaction = seeded(guids, || section.seal().unwrap());
                checkpoints += depths(&section)
                    .iter()
                    .filter(|(id, depth)| chains.get(id).is_some_and(|before| before > depth))
                    .count();
                let mut written = image.clone();
                if let Some(transaction) = &transaction {
                    assert_eq!(transaction.base, before);
                    transaction.apply(&mut written).unwrap();
                    sealed += 1;
                }
                assert!(written == expected, "batch {batch}");
                assert_eq!(Stamp::of(&written).unwrap(), *section.stamp());
                drop(legacy);
                drop(index);
                drop(store);
                image = expected;
                if batch % 25 == 0 || batch + 1 == batches {
                    assert!(section.image() == image);
                    let store = Store::parse(&image).unwrap();
                    let index = RevisionIndex::parse(&store).unwrap();
                    index.validate_current().unwrap();
                    let document = Document::parse(&index).unwrap();
                    for space in &spaces {
                        assert_eq!(
                            section.page(*space).unwrap(),
                            Page::from_space(&document, *space).unwrap()
                        );
                    }
                }
            });
        }
        GUIDS.set(None);
        assert!(sealed > 0, "no batch of {batches} sealed");
        checkpoints
    }

    /// Chain depths of the spaces a section has opened.
    fn depths(section: &Section<'_>) -> BTreeMap<ExGuid, usize> {
        section
            .spaces
            .iter()
            .filter_map(|(id, space)| match &space.state {
                SpaceState::Open(open) => Some((*id, open.file.depth)),
                SpaceState::Stored { .. } => None,
            })
            .collect()
    }

    #[test]
    fn seals_append_the_bytes_the_image_writer_does() {
        let corpus = [
            include_bytes!(
                "../../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one"
            )
            .as_slice(),
            include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one"),
            include_bytes!("../../../corpus/outline-edit/tree/before/notebook/synthetic.one"),
            include_bytes!("../../../corpus/paragraph-edit/before/notebook/synthetic.one"),
        ];
        let shift = crate::sweep::full().unwrap_or(0);
        for (seed, source) in (shift..).zip(corpus) {
            differential(source, seed, 60, 1, 2);
            differential(source, seed + 100, 60, 5, 2);
        }
        // Seeded identities: the writes pick nodes in identity order.
        let created = seeded(7, || {
            crate::create_section("model.one", "First", "Author").unwrap()
        });
        differential(&created, 7 + shift, 60, 3, 1);
    }

    #[test]
    fn wide_batches_regroup_and_alias_as_the_image_writer_does() {
        // These seeds make a seal alias read-only objects or group tables across writes
        // differently from the writes themselves, so the pages adopt the sealed form.
        let corpus = [
            include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one")
                .as_slice(),
            include_bytes!("../../../corpus/outline-edit/tree/before/notebook/synthetic.one"),
            include_bytes!("../../../corpus/paragraph-edit/before/notebook/synthetic.one"),
        ];
        for seed in crate::sweep::seeds(4..9, 5) {
            for source in corpus {
                differential(source, seed, 30, 12, 3);
            }
        }
    }

    #[test]
    fn a_long_chain_checkpoints_as_the_image_writer_does() {
        // Past 512 revisions of one space, so the chain checkpoints once.
        let checkpoints = differential(
            include_bytes!(
                "../../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one"
            ),
            11,
            560,
            2,
            1,
        );
        assert_eq!(checkpoints, 1);
    }

    #[test]
    fn a_created_space_seals_as_the_image_writer_creates_it() {
        let source = &crate::create_section("model.one", "First", "Author").unwrap();
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        // A page without history, whose objects name no other space or context.
        let (copied, revision, reachable) = page_spaces(source)
            .into_iter()
            .map(|space| {
                let revision = index.resolve_active(space).unwrap();
                let reachable = revision.reachable().unwrap();
                (space, revision, reachable)
            })
            .find(|(_, revision, reachable)| {
                reachable.iter().all(|id| {
                    let references = revision.objects[id].references().unwrap();
                    references.object_spaces.is_empty() && references.contexts.is_empty()
                })
            })
            .unwrap();
        let objects: Changes = reachable
            .iter()
            .map(|id| (*id, replacement(*id, &revision.objects[id]).unwrap()))
            .collect();
        let space = ExGuid {
            guid: [7; 16],
            n: 0,
        };
        let expected = seeded(1, || {
            crate::write::write_revisions(source, |_| {
                Ok(BTreeMap::from([(
                    space,
                    crate::write::RevisionEdit::Create {
                        roots: revision.roots.clone(),
                        objects: objects.clone(),
                    },
                )]))
            })
            .unwrap()
        });
        let arena = Arena::default();
        let mut section = Section::open(&arena, source.to_vec()).unwrap();
        section
            .create_space(space, revision.roots.clone(), objects)
            .unwrap();
        assert_eq!(section.page(space).unwrap(), section.page(copied).unwrap());
        seeded(1, || section.seal().unwrap()).unwrap();
        assert!(section.image() == expected);
        // The space's next revision depends on the one creating it.
        let mut writes = Writes(5);
        let mut image = expected;
        let mut written = 0;
        for batch in 0..6 {
            let Ok(changes) = writes.next(section.active(space).unwrap()) else {
                continue;
            };
            written += 1;
            section.apply_changes(space, &[], changes.clone()).unwrap();
            let guids = (batch + 2) << 32;
            let store = Store::parse(&image).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let bump = Bump::new();
            // The root lists no page in this space, so no document reaches it.
            let rid = index.active(space).unwrap();
            let live = LiveRevision::new(
                index.resolve(space, rid).unwrap(),
                chain_depth(&index, space, rid),
            )
            .unwrap();
            let store = &store;
            let files: Files<'_> = std::rc::Rc::new(|guid| store.file_data(guid));
            let mut legacy = ActivePage::open(space, live, FileType::Section, files).unwrap();
            legacy.write(&bump, &[], changes).unwrap();
            let expected = seeded(guids, || {
                let revision = &legacy.live.revision;
                squashed(&index, &[(space, revision)], &[])
            });
            drop(legacy);
            drop(index);
            image = expected;
            seeded(guids, || section.seal().unwrap());
            assert!(section.image() == image, "batch {batch}");
        }
        assert!(written > 2);
        let reopened = Section::open(&arena, image).unwrap();
        assert_eq!(reopened.page(space).unwrap(), section.page(space).unwrap());
    }

    #[test]
    fn payloads_embed_once_as_the_image_writer_embeds_them() {
        let source = include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one");
        let space = page_spaces(source)[0];
        let payload = ([9; 16], b"payload bytes".as_slice());
        let arena = Arena::default();
        let mut section = Section::open(&arena, source.to_vec()).unwrap();
        let mut image = source.to_vec();
        for seed in [1_u64 << 32, 2 << 32] {
            assert!(
                section
                    .apply_changes(space, &[payload], Changes::new())
                    .unwrap()
            );
            let store = Store::parse(&image).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let legacy = ActivePage::parse(&index, space).unwrap();
            let expected = seeded(seed, || {
                let revision = &legacy.live.revision;
                squashed(&index, &[(space, revision)], &[payload])
            });
            let transaction = seeded(seed, || section.seal().unwrap());
            assert_eq!(transaction.is_some(), expected != image);
            assert!(section.image() == expected);
            drop(legacy);
            drop(index);
            drop(store);
            image = expected;
        }
        let store = Store::parse(&image).unwrap();
        assert_eq!(store.file_data(payload.0).unwrap(), payload.1);
    }

    #[test]
    fn a_seal_checks_the_bytes_it_appends() {
        fn utf16(text: &str) -> Vec<u8> {
            text.encode_utf16().flat_map(u16::to_le_bytes).collect()
        }
        fn object_bytes(transaction: &mut Transaction) {
            let text = utf16("Tampered text");
            let at = transaction
                .append
                .windows(text.len())
                .position(|w| w == text)
                .unwrap();
            transaction.append[at] ^= 1;
        }
        fn link(transaction: &mut Transaction) {
            let (_, bytes) = transaction
                .patches
                .iter_mut()
                .find(|(_, b)| b.len() == 12)
                .unwrap();
            bytes[0] ^= 8;
        }
        fn sentinel(transaction: &mut Transaction) {
            let (_, bytes) = transaction.patches.last_mut().unwrap();
            *bytes.last_mut().unwrap() ^= 1;
        }
        fn header(transaction: &mut Transaction) {
            transaction.header[96] ^= 2;
        }
        let source = include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one");
        let space = page_spaces(source)[0];
        let seal = |tamper: Option<fn(&mut Transaction)>| {
            let arena = Arena::default();
            let mut section = Section::open(&arena, source.to_vec()).unwrap();
            let page = section.active(space).unwrap();
            let changes = page
                .view
                .nodes
                .iter()
                .filter(|(id, node)| {
                    matches!(node.kind, crate::document::Kind::Outline { .. })
                        && page.parents.contains_key(id)
                })
                .find_map(|(outline, _)| {
                    let paragraph = crate::page::text::new_id().ok()?;
                    let text = crate::page::text::new_id().ok()?;
                    Insertion::paragraph(*outline, None, "Tampered text", "Author")
                        .and_then(|insertion| {
                            insertion.changes_as(page, paragraph, paragraph, text)
                        })
                        .ok()
                })
                .unwrap();
            section.apply_changes(space, &[], changes).unwrap();
            TAMPER.set(tamper);
            let sealed = section.seal();
            TAMPER.set(None);
            let error = sealed.as_ref().err().map(|error| error.message);
            assert_eq!(section.seal().is_err(), error.is_some());
            error
        };
        assert_eq!(seal(None), None);
        for tamper in [object_bytes, link, sentinel, header] {
            assert!(seal(Some(tamper)).is_some());
        }
    }

    #[test]
    fn a_replayed_transaction_leaves_the_sealing_section() {
        let source = include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one");
        let space = page_spaces(source)[0];
        let arena = Arena::default();
        let mut sealing = Section::open(&arena, source.to_vec()).unwrap();
        let mut replaying = Section::open(&arena, source.to_vec()).unwrap();
        let mut writes = Writes(3);
        for _ in 0..8 {
            for _ in 0..3 {
                if let Ok(changes) = writes.next(sealing.active(space).unwrap()) {
                    sealing.apply_changes(space, &[], changes).unwrap();
                }
            }
            let Some(transaction) = sealing.seal().unwrap() else {
                continue;
            };
            let queued: Transaction =
                serde_json::from_str(&serde_json::to_string(&transaction).unwrap()).unwrap();
            assert_eq!(queued, transaction);
            replaying.replay(&queued).unwrap();
            assert_eq!(replaying.stamp(), sealing.stamp());
            assert!(replaying.image() == sealing.image());
            assert_eq!(replaying.page(space).unwrap(), sealing.page(space).unwrap());
        }
    }
}

#[cfg(test)]
mod probe {
    use super::*;
    use crate::{document::Kind, edit::text_changes};
    use std::time::{Duration, Instant};

    fn median(mut samples: Vec<Duration>) -> Duration {
        samples.sort();
        samples[samples.len() / 2]
    }

    /// The fullest page space of `image` and a rich-text object in its middle.
    fn target(image: &[u8]) -> (ExGuid, ExGuid, usize) {
        let store = Store::parse(image).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = crate::document::Document::parse(&index).unwrap();
        let texts = |space: ExGuid| -> Vec<ExGuid> {
            document
                .active(space)
                .unwrap()
                .nodes
                .iter()
                .filter(|(_, node)| {
                    matches!(
                        node.kind,
                        Kind::RichText {
                            boilerplate: false,
                            ..
                        }
                    )
                })
                .map(|(id, _)| *id)
                .collect()
        };
        let (space, _) = document
            .pages()
            .unwrap()
            .into_iter()
            .max_by_key(|(space, _)| texts(*space).len())
            .unwrap();
        let texts = texts(space);
        (space, texts[texts.len() / 2], texts.len())
    }

    /// One-character edits of a large page through a `Section`: `SECTION_PROBE=path cargo test --release -p onestore --lib probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn keystrokes() {
        let path = std::env::var("SECTION_PROBE").unwrap_or("/tmp/probe3000.one".into());
        let image = std::fs::read(&path).unwrap();
        let (space, text, count) = target(&image);
        println!(
            "{path}: {} bytes, {count} text objects on the page",
            image.len()
        );

        let arena = Arena::default();
        let start = Instant::now();
        let mut section = Section::open(&arena, image.clone()).unwrap();
        println!("Section::open: {:?}", start.elapsed());
        let start = Instant::now();
        section.active(space).unwrap();
        println!("first edit opens the page: {:?}", start.elapsed());
        let (mut applies, mut seals, mut sizes) = (Vec::new(), Vec::new(), Vec::new());
        for keystroke in 0..200u32 {
            let start = Instant::now();
            let changes = text_changes(
                section.active(space).unwrap(),
                text,
                keystroke..keystroke,
                "x",
            )
            .unwrap();
            section.apply_changes(space, &[], changes).unwrap();
            applies.push(start.elapsed());
            let start = Instant::now();
            let transaction = section.seal().unwrap().unwrap();
            seals.push(start.elapsed());
            sizes.push((
                transaction.append.len(),
                transaction
                    .patches
                    .iter()
                    .map(|(_, bytes)| bytes.len())
                    .sum::<usize>(),
            ));
        }
        println!(
            "first seal: appended {} bytes, patched {} bytes",
            sizes[0].0, sizes[0].1
        );
        sizes.sort();
        println!(
            "Section, one keystroke: apply {:?}, seal {:?}; appended {:?} bytes, patched {:?} bytes (median)",
            median(applies),
            median(seals),
            sizes[sizes.len() / 2].0,
            sizes[sizes.len() / 2].1,
        );
        let start = Instant::now();
        let reopened = Section::open(&arena, section.image()).unwrap();
        println!("reopen after 200 seals: {:?}", start.elapsed());
        assert_eq!(reopened.page(space).unwrap(), section.page(space).unwrap());
    }
}
