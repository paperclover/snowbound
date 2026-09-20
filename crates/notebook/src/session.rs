//! The application's view of a notebook: sections opened through a local replica that
//! publishes page saves to the section file in the background.

use crate::{
    ConflictKind, EditStatus, Error, PendingEdit, Remote, Replica, Result, SyncWorker, discover,
};
use onestore::{
    CommitError, ExGuid, PageCreation, PreparedEdit, RevisionIndex, Store, document::Document,
    page::Page,
};
use std::{
    collections::BTreeMap,
    io,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::Duration,
};

/// A difference between two catalog reads of a notebook, in catalog paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A section or group appeared, including one replacing a file under an existing path.
    Added(String),
    /// A section or group is no longer in the notebook.
    Removed(String),
    /// The same file now lives at another path: a rename or a move between groups.
    Moved { from: String, to: String },
    /// A folder's surviving sections and groups changed order.
    Reordered(String),
}

/// Sections and groups are followed by file identity; a group without a TOC only by path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    File([u8; 16]),
    Folder(String),
}

type Entries = (BTreeMap<Key, String>, BTreeMap<String, Vec<Key>>);

fn entries(folder: &discover::Folder) -> Entries {
    fn walk(folder: &discover::Folder, out: &mut Entries) {
        let mut order = Vec::new();
        for section in &folder.sections {
            let key = Key::File(section.file_id);
            out.0.insert(key.clone(), section.path.clone());
            order.push(key);
        }
        for group in &folder.groups {
            let key = match &group.toc {
                Some(toc) => Key::File(toc.file_id),
                None => Key::Folder(group.path.clone()),
            };
            out.0.insert(key.clone(), group.path.clone());
            order.push(key);
            walk(group, out);
        }
        out.1.insert(folder.path.clone(), order);
    }
    let mut out = Entries::default();
    walk(folder, &mut out);
    out
}

/// Where a notebook's files live: a mounted directory or an SMB share. Paths are catalog
/// paths, `/`-separated and relative to the notebook root.
pub trait Storage: Send + Sync {
    fn discover(&self, limits: discover::Limits) -> Result<discover::Folder>;
    fn exists(&self, path: &str) -> bool;
    fn read(&self, path: &str) -> Result<Vec<u8>>;
    /// Creates a file holding `bytes`; an existing file is an error.
    fn create(&self, path: &str, bytes: &[u8]) -> Result<()>;
    fn create_directory(&self, path: &str) -> Result<()>;
    /// Renames or moves a file or directory; an existing target is an error.
    fn rename(&self, from: &str, to: &str) -> Result<()>;
    /// Names a section or TOC file for its notebook, as `onestore::place`.
    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()>;
    fn commit(&self, path: &str, edit: &PreparedEdit<'_>) -> Result<()>;
    fn set_property(
        &self,
        path: &str,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
        property: u32,
        value: &[u8],
    ) -> Result<()>;
}

/// A password-protected section as `Notebook::unlock` read it.
#[cfg(feature = "protected")]
pub struct Unlocked {
    pub pages: Vec<(ExGuid, Page)>,
    /// The stored section the pages came from, which a save must still find in place.
    snapshot: Vec<u8>,
}

/// A mounted notebook directory.
struct Directory(PathBuf);

impl Directory {
    fn path(&self, relative: &str) -> PathBuf {
        if relative.is_empty() {
            self.0.clone()
        } else {
            self.0.join(relative)
        }
    }
}

impl Storage for Directory {
    fn discover(&self, limits: discover::Limits) -> Result<discover::Folder> {
        Ok(discover::discover(
            &mut discover::Local::open(&self.0)?,
            limits,
        )?)
    }

    fn exists(&self, path: &str) -> bool {
        self.path(path).exists()
    }

    fn read(&self, path: &str) -> Result<Vec<u8>> {
        Ok(onestore::read_file(self.path(path))?)
    }

    fn create(&self, path: &str, bytes: &[u8]) -> Result<()> {
        std::fs::File::create_new(self.path(path))?.write_all(bytes)?;
        Ok(())
    }

    fn create_directory(&self, path: &str) -> Result<()> {
        Ok(std::fs::create_dir(self.path(path))?)
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        Ok(std::fs::rename(self.path(from), self.path(to))?)
    }

    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()> {
        Ok(onestore::place_file(self.path(path), ancestor, name)?)
    }

    fn commit(&self, path: &str, edit: &PreparedEdit<'_>) -> Result<()> {
        Ok(edit.commit_file(self.path(path))?)
    }

    fn set_property(
        &self,
        path: &str,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
        property: u32,
        value: &[u8],
    ) -> Result<()> {
        Ok(onestore::commit_file_property(
            self.path(path),
            source,
            space,
            object,
            property,
            value,
        )?)
    }
}

/// A notebook directory on an SMB share, reached through the native-compatible client.
#[cfg(feature = "smb")]
pub struct Share {
    client: Arc<crate::smb::Client>,
    root: String,
}

#[cfg(feature = "smb")]
impl Share {
    fn path(&self, relative: &str) -> String {
        match (self.root.is_empty(), relative.is_empty()) {
            (_, true) => self.root.clone(),
            (true, false) => relative.to_owned(),
            (false, false) => format!("{}/{relative}", self.root),
        }
    }
}

#[cfg(feature = "smb")]
impl Storage for Share {
    fn discover(&self, limits: discover::Limits) -> Result<discover::Folder> {
        Ok(discover::discover(
            &mut discover::Smb::new(&self.client, &self.root)?,
            limits,
        )?)
    }

    fn exists(&self, path: &str) -> bool {
        let (folder, name) = split(path);
        self.client
            .read_dir(&self.path(folder), 100_000)
            .is_ok_and(|entries| entries.iter().any(|entry| entry.name == name))
    }

    fn read(&self, path: &str) -> Result<Vec<u8>> {
        Ok(self
            .client
            .read_storage(&self.path(path), 256 * 1024 * 1024)?)
    }

    fn create(&self, path: &str, bytes: &[u8]) -> Result<()> {
        Ok(self.client.create(&self.path(path), bytes)?)
    }

    fn create_directory(&self, path: &str) -> Result<()> {
        Ok(self.client.create_directory(&self.path(path))?)
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        Ok(self.client.rename(&self.path(from), &self.path(to))?)
    }

    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()> {
        Ok(self.client.place(&self.path(path), ancestor, name)?)
    }

    fn commit(&self, path: &str, edit: &PreparedEdit<'_>) -> Result<()> {
        Ok(self.client.commit_prepared(&self.path(path), edit)?)
    }

    fn set_property(
        &self,
        path: &str,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
        property: u32,
        value: &[u8],
    ) -> Result<()> {
        Ok(self.client.commit_property_bytes(
            &self.path(path),
            source,
            space,
            object,
            property,
            value,
        )?)
    }
}

const LIMITS: discover::Limits = discover::Limits {
    entries: 100_000,
    bytes_per_file: 256 * 1024 * 1024,
    depth: 64,
};

const TOC: &str = "Open Notebook.onetoc2";
const RECYCLE_BIN: &str = "OneNote_RecycleBin";

/// A notebook's files and the cache directory holding its section replicas.
pub struct Notebook {
    storage: Box<dyn Storage>,
    /// The mounted directory, when sections open through local replicas.
    root: Option<PathBuf>,
    cache: PathBuf,
    catalog: discover::Folder,
}

impl Notebook {
    pub fn open(root: impl AsRef<Path>, cache: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        Self::with(Box::new(Directory(root.clone())), Some(root), cache)
    }

    /// Opens the notebook at `root` on the share `client` is connected to. Sections open
    /// through `Section::resume_smb` with the catalog's paths.
    #[cfg(feature = "smb")]
    pub fn open_smb(
        client: Arc<crate::smb::Client>,
        root: &str,
        cache: impl AsRef<Path>,
    ) -> Result<Self> {
        let root = root.replace('\\', "/");
        Self::with(Box::new(Share { client, root }), None, cache)
    }

    fn with(
        storage: Box<dyn Storage>,
        root: Option<PathBuf>,
        cache: impl AsRef<Path>,
    ) -> Result<Self> {
        let cache = cache.as_ref().to_path_buf();
        std::fs::create_dir_all(&cache)?;
        let catalog = storage.discover(LIMITS)?;
        Ok(Self {
            storage,
            root,
            cache,
            catalog,
        })
    }

    pub fn catalog(&self) -> &discover::Folder {
        &self.catalog
    }

    /// Rereads the notebook and reports what changed since the last catalog, keyed by
    /// file identity so a renamed or moved section stays the same section. A failed read
    /// keeps the previous catalog: an unreachable notebook is not an empty one.
    pub fn refresh(&mut self) -> Result<Vec<Change>> {
        let catalog = self.storage.discover(LIMITS)?;
        let (before, before_orders) = entries(&self.catalog);
        let (after, after_orders) = entries(&catalog);
        let mut changes = Vec::new();
        for key in before_orders.values().flatten() {
            let from = &before[key];
            match after.get(key) {
                None => changes.push(Change::Removed(from.clone())),
                Some(to) if to != from => changes.push(Change::Moved {
                    from: from.clone(),
                    to: to.clone(),
                }),
                Some(_) => {}
            }
        }
        for key in after_orders.values().flatten() {
            if !before.contains_key(key) {
                changes.push(Change::Added(after[key].clone()));
            }
        }
        for (folder, order) in &after_orders {
            let Some(previous) = before_orders.get(folder) else {
                continue;
            };
            let kept = |sequence: &[Key], other: &[Key]| -> Vec<Key> {
                sequence
                    .iter()
                    .filter(|key| other.contains(key))
                    .cloned()
                    .collect()
            };
            if kept(previous, order) != kept(order, previous) {
                changes.push(Change::Reordered(folder.clone()));
            }
        }
        self.catalog = catalog;
        Ok(changes)
    }

    fn folder(&self, path: &str) -> Result<&discover::Folder> {
        let mut folders = vec![&self.catalog];
        while let Some(folder) = folders.pop() {
            if folder.path == path {
                return Ok(folder);
            }
            folders.extend(&folder.groups);
        }
        Err(io::Error::from(io::ErrorKind::NotFound).into())
    }

    /// A catalog section's path, refusing paths the catalog does not list.
    fn section_path(&self, path: &str) -> Result<&discover::Section> {
        let mut folders = vec![&self.catalog];
        while let Some(folder) = folders.pop() {
            if let Some(section) = folder.sections.iter().find(|section| section.path == path) {
                return Ok(section);
            }
            folders.extend(&folder.groups);
        }
        Err(io::Error::from(io::ErrorKind::NotFound).into())
    }

    /// A folder's TOC path and file identity, creating the TOC when the folder has none
    /// (OneNote names it `Open Notebook.onetoc2`).
    fn toc(&self, folder: &str) -> Result<(String, [u8; 16])> {
        match &self.folder(folder)?.toc {
            Some(toc) => Ok((catalog_path(folder, &toc.filename), toc.file_id)),
            None => {
                let path = catalog_path(folder, TOC);
                let bytes = onestore::create_table_of_contents(TOC, &[])?;
                self.storage.create(&path, &bytes)?;
                Ok((path, onestore::Store::parse(&bytes)?.header.file_id))
            }
        }
    }

    fn edit_toc(&self, folder: &str, edits: &[onestore::TocEdit]) -> Result<()> {
        let (toc, _) = self.toc(folder)?;
        let source = self.storage.read(&toc)?;
        self.storage
            .commit(&toc, &PreparedEdit::table_of_contents(&source, edits)?)
    }

    /// Creates `name.one` in `folder` with one empty page and lists it last in the folder's
    /// TOC, as OneNote creates a section. Returns the new catalog path.
    pub fn create_section(&mut self, folder: &str, name: &str, author: &str) -> Result<String> {
        let filename = format!("{name}.one");
        self.folder(folder)?;
        let (_, ancestor) = self.toc(folder)?;
        let bytes = onestore::create_section(&filename, "", author)?;
        let path = catalog_path(folder, &filename);
        self.storage.create(&path, &bytes)?;
        self.storage.place(&path, ancestor, &filename)?;
        let identity = onestore::Store::parse(&bytes)?.header.file_id;
        self.edit_toc(
            folder,
            &[onestore::TocEdit::Add {
                filename: filename.clone(),
                identity,
                group: false,
            }],
        )?;
        self.refresh()?;
        Ok(path)
    }

    /// Creates a section group: a folder with its own TOC, listed last in the parent's TOC.
    pub fn create_group(&mut self, folder: &str, name: &str) -> Result<String> {
        self.folder(folder)?;
        if !component(name) {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        let (_, ancestor) = self.toc(folder)?;
        let group = catalog_path(folder, name);
        self.storage.create_directory(&group)?;
        let bytes = onestore::create_table_of_contents(TOC, &[])?;
        let path = catalog_path(&group, TOC);
        self.storage.create(&path, &bytes)?;
        self.storage.place(&path, ancestor, name)?;
        let identity = onestore::Store::parse(&bytes)?.header.file_id;
        self.edit_toc(
            folder,
            &[onestore::TocEdit::Add {
                filename: name.to_owned(),
                identity,
                group: true,
            }],
        )?;
        self.refresh()?;
        Ok(group)
    }

    /// Renames a section or section group: the file or folder and its TOC entry.
    pub fn rename(&mut self, path: &str, name: &str) -> Result<String> {
        let (folder, entry) = split(path);
        let (filename, identity) = self.entry(folder, entry)?;
        let target = if filename.to_ascii_lowercase().ends_with(".one") {
            format!("{name}.one")
        } else {
            name.to_owned()
        };
        let renamed = catalog_path(folder, &target);
        if !component(&target) || self.storage.exists(&renamed) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
        }
        self.storage
            .rename(&catalog_path(folder, &filename), &renamed)?;
        let (_, ancestor) = self.toc(folder)?;
        let placed = if target.ends_with(".one") {
            renamed.clone()
        } else {
            catalog_path(&renamed, TOC)
        };
        self.storage.place(&placed, ancestor, &target)?;
        self.edit_toc(
            folder,
            &[onestore::TocEdit::Rename {
                identity,
                filename: target,
            }],
        )?;
        self.refresh()?;
        Ok(renamed)
    }

    /// Sets a section's colour (COLORREF) in its own metadata, where OneNote keeps it.
    pub fn set_section_color(&mut self, path: &str, color: Option<u32>) -> Result<()> {
        let path = self.section_path(path)?.path.clone();
        let source = self.storage.read(&path)?;
        let store = onestore::Store::parse(&source)?;
        let index = onestore::RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        let revision = document.active(document.root)?;
        let metadata = *revision
            .roots
            .get(&2)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
        self.storage.set_property(
            &path,
            &source,
            document.root,
            metadata,
            0x14001cbe,
            &color.unwrap_or(0xffff_ffff).to_le_bytes(),
        )?;
        self.refresh()?;
        Ok(())
    }

    /// Orders a folder's sections and groups; entries left out follow in their current order.
    pub fn reorder(&mut self, folder: &str, paths: &[&str]) -> Result<()> {
        let mut identities = Vec::new();
        for path in paths {
            let (parent, entry) = split(path);
            if parent != folder {
                return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
            }
            identities.push(self.entry(folder, entry)?.1);
        }
        self.edit_toc(folder, &[onestore::TocEdit::Order(identities)])?;
        self.refresh().map(drop)
    }

    /// Deletes a section the way OneNote does: the file moves into the notebook's
    /// `OneNote_RecycleBin` folder, a section group with its own TOC that lists it (and that
    /// the root TOC lists), and its own folder's TOC entry goes.
    pub fn delete(&mut self, path: &str) -> Result<()> {
        let (folder, entry) = split(path);
        let (filename, identity) = self.entry(folder, entry)?;
        if !filename.to_ascii_lowercase().ends_with(".one") {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        let bin_toc = catalog_path(RECYCLE_BIN, TOC);
        if !self.storage.exists(&bin_toc) {
            if !self.storage.exists(RECYCLE_BIN) {
                self.storage.create_directory(RECYCLE_BIN)?;
            }
            let bytes = onestore::create_table_of_contents(TOC, &[])?;
            self.storage.create(&bin_toc, &bytes)?;
            self.storage.place(&bin_toc, self.toc("")?.1, RECYCLE_BIN)?;
            let bin_identity = onestore::Store::parse(&bytes)?.header.file_id;
            self.edit_toc(
                "",
                &[onestore::TocEdit::Add {
                    filename: RECYCLE_BIN.into(),
                    identity: bin_identity,
                    group: true,
                }],
            )?;
        }
        let mut target = filename.clone();
        let mut attempt = 1;
        while self.storage.exists(&catalog_path(RECYCLE_BIN, &target)) {
            attempt += 1;
            let (stem, extension) = filename.rsplit_once('.').unwrap_or((&filename, ""));
            target = format!("{stem} ({attempt}).{extension}");
        }
        let binned = catalog_path(RECYCLE_BIN, &target);
        self.storage
            .rename(&catalog_path(folder, &filename), &binned)?;
        let source = self.storage.read(&bin_toc)?;
        self.storage.place(
            &binned,
            onestore::Store::parse(&source)?.header.file_id,
            &target,
        )?;
        self.storage.commit(
            &bin_toc,
            &PreparedEdit::table_of_contents(
                &source,
                &[onestore::TocEdit::Add {
                    filename: target,
                    identity,
                    group: false,
                }],
            )?,
        )?;
        self.edit_toc(folder, &[onestore::TocEdit::Remove { identity }])?;
        self.refresh().map(drop)
    }

    /// The stored filename and TOC identity of a folder's section or group.
    fn entry(&self, folder: &str, name: &str) -> Result<(String, [u8; 16])> {
        let parent = self.folder(folder)?;
        if let Some(section) = parent
            .sections
            .iter()
            .find(|section| split(&section.path).1 == name)
        {
            return Ok((name.to_owned(), section.file_id));
        }
        if let Some(group) = parent
            .groups
            .iter()
            .find(|group| split(&group.path).1 == name)
        {
            let toc = group
                .toc
                .as_ref()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            return Ok((name.to_owned(), toc.file_id));
        }
        Err(io::Error::from(io::ErrorKind::NotFound).into())
    }

    /// The section path and page space a stored internal link opens, found by identity:
    /// in the linked section first, then in every other readable section, so a link
    /// follows its page across sections. `None` for other URLs and unknown targets.
    pub fn find_page(&self, url: &str) -> Result<Option<(String, Option<ExGuid>)>> {
        let Some(link) = onestore::page::link::parse_internal_link(url) else {
            return Ok(None);
        };
        let mut sections = Vec::new();
        let mut folders = vec![&self.catalog];
        while let Some(folder) = folders.pop() {
            sections.extend(folder.sections.iter().filter(|section| {
                matches!(section.state, discover::SectionState::Readable { .. })
            }));
            folders.extend(&folder.groups);
        }
        sections.sort_by_key(|section| section.file_id != link.section);
        let Some(page) = link.page else {
            return Ok(sections
                .first()
                .filter(|section| section.file_id == link.section)
                .map(|section| (section.path.clone(), None)));
        };
        for section in sections {
            let bytes = self.storage.read(&section.path)?;
            let store = Store::parse(&bytes)?;
            let index = RevisionIndex::parse(&store)?;
            let document = Document::parse(&index)?;
            for (space, _) in document.pages()? {
                if Page::identity_of(document.active(space)?) == Some(page) {
                    return Ok(Some((section.path.clone(), Some(space))));
                }
            }
        }
        Ok(None)
    }

    /// The pages of a password-protected section: nothing is cached, and the decoded
    /// buffers go when the pages have been built.
    #[cfg(feature = "protected")]
    pub fn unlock(&self, path: &str, password: &str) -> Result<Unlocked> {
        let path = self.section_path(path)?.path.clone();
        let snapshot = self.storage.read(&path)?;
        let pages = {
            let store = Store::parse(&snapshot)?;
            let index = RevisionIndex::parse(&store)?;
            let unlocked = onestore::protected::UnlockedSection::open(
                &index,
                password,
                onestore::protected::Limits::default(),
            )?;
            let document = unlocked.document()?;
            document
                .pages()?
                .into_iter()
                .map(|(space, _)| Ok((space, Page::from_space(&document, space)?)))
                .collect::<Result<_>>()?
        };
        Ok(Unlocked { pages, snapshot })
    }

    /// Saves an edited page of `unlocked` under the section's key, straight to the file:
    /// nothing of a protected section is cached or queued. A section written since
    /// `unlocked` was read fails the save; unlock again for its pages.
    #[cfg(feature = "protected")]
    pub fn save_unlocked(
        &self,
        path: &str,
        password: &str,
        unlocked: &mut Unlocked,
        space: ExGuid,
        page: &Page,
        author: &str,
    ) -> Result<()> {
        let path = self.section_path(path)?.path.clone();
        let edit = onestore::PreparedEdit::page_protected(
            &unlocked.snapshot,
            password,
            space,
            page,
            author,
        )?;
        self.storage.commit(&path, &edit)?;
        let written = edit.as_bytes().to_vec();
        unlocked.snapshot = written;
        Ok(())
    }

    /// Opens a section of a mounted notebook by its catalog path.
    pub fn section(&self, path: &str, notify: impl Fn() + Send + 'static) -> Result<Section> {
        let path = self.section_path(path)?.path.clone();
        let Some(root) = &self.root else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Sections on a share open through Section::resume_smb",
            )
            .into());
        };
        // A section replaced by a link out of the notebook is not the catalog's section.
        let file = root.join(path).canonicalize()?;
        if !file.starts_with(root) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
        }
        Section::open(file, &self.cache, notify)
    }
}

fn component(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\0']) && name != "." && name != ".."
}

fn split(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((folder, name)) => (folder, name),
        None => ("", path),
    }
}

fn catalog_path(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// What happened on the synchronization thread since the last poll.
#[derive(Debug)]
pub enum Event {
    /// The working image was refreshed from the section file with no local edit pending.
    Refreshed,
    /// A publication attempt finished with this durable state.
    Attempt { id: u64, status: EditStatus },
    /// The section file could not be reached; the replica keeps its state.
    Unreachable(io::Error),
    /// The replica itself failed; the worker has stopped.
    Failed(String),
}

/// The outcome of saving an edited page.
#[derive(Debug, PartialEq, Eq)]
pub enum Save {
    /// The model equals the stored page.
    Unchanged,
    /// The edit is durable locally under this id.
    Queued(u64),
    /// The stored page no longer matches `before`; reload it before saving again.
    Stale,
}

/// A queued edit and its durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedEdit {
    pub id: u64,
    pub space: ExGuid,
    pub status: EditStatus,
}

/// A section file with its replica and background publication.
pub struct Section {
    file: PathBuf,
    replica: Arc<Replica>,
    worker: Option<SyncWorker>,
    events: Receiver<Event>,
}

impl Section {
    /// Opens the section file through a replica in `cache`, creating the replica from the
    /// file on first use. `notify` runs on the synchronization thread whenever an event is
    /// available.
    pub fn open(
        file: impl AsRef<Path>,
        cache: impl AsRef<Path>,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let file = file.as_ref().canonicalize()?;
        let source = onestore::read_file(&file)?;
        let store = Store::parse(&source)?;
        let identity = RevisionIndex::parse(&store)?.root;
        let name: String = identity
            .guid
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        std::fs::create_dir_all(&cache)?;
        let cache = cache.as_ref().join(format!("{name}.sqlite"));
        let replica = if cache.exists() {
            Replica::open(&cache)?
        } else {
            Replica::create(&cache, &source)?
        };
        Self::resume(file, replica, notify)
    }

    /// Resumes an owned local replica without reading the publication target.
    /// A target with a different document identity is rejected by synchronization.
    pub fn resume(
        file: impl AsRef<Path>,
        replica: Replica,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let file = std::path::absolute(file)?;
        let remote = file.clone();
        Self::start(
            file,
            replica,
            move || Ok(FileRemote(remote.clone())),
            notify,
        )
    }

    /// Resumes a replica against a share-relative section path. Credentials remain in
    /// `connect`, which is called on the worker again after transport failures.
    #[cfg(feature = "smb")]
    pub fn resume_smb(
        path: String,
        replica: Replica,
        limit: usize,
        mut connect: impl FnMut() -> io::Result<crate::smb::Client> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        Self::start(
            PathBuf::from(&path),
            replica,
            move || Ok(crate::SmbRemote::new(connect()?, path.clone(), limit)),
            notify,
        )
    }

    fn start<R: Remote + 'static>(
        file: PathBuf,
        replica: Replica,
        connect: impl FnMut() -> io::Result<R> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let replica = Arc::new(replica);
        let (sender, events) = mpsc::channel();
        let worker = replica.start_sync(Duration::from_secs(2), connect, move |result| {
            let event = match result {
                Ok(None) => Event::Refreshed,
                Ok(Some((id, status))) => Event::Attempt {
                    id: *id,
                    status: status.clone(),
                },
                Err(Error::RemoteIo(error)) => Event::Unreachable(match error.raw_os_error() {
                    Some(code) => io::Error::from_raw_os_error(code),
                    None => io::Error::new(error.kind(), error.to_string()),
                }),
                Err(Error::Remote(error)) => {
                    Event::Unreachable(io::Error::new(error.error.kind(), error.to_string()))
                }
                Err(error) => Event::Failed(error.to_string()),
            };
            if sender.send(event).is_ok() {
                notify();
            }
        })?;
        Ok(Self {
            file,
            replica,
            worker: Some(worker),
            events,
        })
    }

    /// The absolute local path, or share-relative path for an SMB session.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The section file identity, which internal links name as `section-id`
    /// (`onestore::page::link::internal_link`).
    pub fn identity(&self) -> Result<[u8; 16]> {
        Ok(Store::parse(&self.replica.snapshot()?)?.header.file_id)
    }

    /// Page spaces and titles in section order, from the local working image.
    pub fn pages(&self) -> Result<Vec<(ExGuid, String)>> {
        let snapshot = self.replica.snapshot()?;
        let store = Store::parse(&snapshot)?;
        let index = RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        document
            .pages()?
            .into_iter()
            .map(|(space, _)| Ok((space, Page::from_space(&document, space)?.title)))
            .collect()
    }

    pub fn page(&self, space: ExGuid) -> Result<Page> {
        let snapshot = self.replica.snapshot()?;
        let store = Store::parse(&snapshot)?;
        let index = RevisionIndex::parse(&store)?;
        Ok(Page::from_space(&Document::parse(&index)?, space)?)
    }

    /// Copies a page (usually from another section) to the end of this section as a
    /// creation and a save queued like the user's own edits, under fresh identities.
    /// Returns the new page's space. Content outside the model refuses to copy.
    pub fn import_page(&self, page: &Page, author: &str) -> Result<ExGuid> {
        let copy = page.copy()?;
        let creation = PageCreation::new(None, Some(&page.title), author)?;
        self.replica
            .create_page(&self.replica.snapshot()?, &creation)?;
        let space = creation.space();
        loop {
            // The worker may publish the creation, and so replace the working image,
            // between reading it and saving against it.
            let source = self.replica.snapshot()?;
            let mut after = Page::from_space(
                &Document::parse(&RevisionIndex::parse(&Store::parse(&source)?)?)?,
                space,
            )?;
            after
                .objects
                .retain(|object| matches!(object, onestore::page::PageObject::Title(_)));
            after.objects.extend(
                copy.objects
                    .iter()
                    .filter(|object| !matches!(object, onestore::page::PageObject::Title(_)))
                    .cloned(),
            );
            after.definitions = copy.definitions.clone();
            match self.replica.save(&source, space, &after, author) {
                Err(Error::Io(error)) if error.kind() == io::ErrorKind::ResourceBusy => continue,
                result => result?,
            };
            break;
        }
        self.wake();
        Ok(space)
    }

    /// Removes pages permanently, queued like the user's own edits (a move across
    /// sections is `import_page` there, then this here).
    pub fn delete_pages(&self, pages: &[ExGuid]) -> Result<Option<u64>> {
        let id = self
            .replica
            .delete_pages(&self.replica.snapshot()?, pages)?;
        self.wake();
        Ok(id)
    }

    /// Saves an edited page. `before` is the model the edit started from; a stored page
    /// that differs from it means the section changed underneath the editor.
    pub fn save(&self, space: ExGuid, before: &Page, after: &Page, author: &str) -> Result<Save> {
        loop {
            let snapshot = self.replica.snapshot()?;
            let store = Store::parse(&snapshot)?;
            let index = RevisionIndex::parse(&store)?;
            if Page::from_space(&Document::parse(&index)?, space)? != *before {
                return Ok(Save::Stale);
            }
            match self.replica.save(&snapshot, space, after, author) {
                Ok(Some(id)) => return Ok(Save::Queued(id)),
                Ok(None) => return Ok(Save::Unchanged),
                Err(Error::Io(error)) if error.kind() == io::ErrorKind::ResourceBusy => {}
                Err(error) => return Err(error),
            }
        }
    }

    pub fn status(&self, id: u64) -> Result<Option<EditStatus>> {
        self.replica.status(id)
    }

    pub fn pending(&self) -> Result<Vec<PendingEdit>> {
        self.replica.pending()
    }

    /// Every queued edit with its state: pending, awaiting confirmation of a retained
    /// attempt, or a conflict awaiting review.
    pub fn queue(&self) -> Result<Vec<QueuedEdit>> {
        self.replica
            .pending()?
            .into_iter()
            .map(|edit| {
                Ok(QueuedEdit {
                    id: edit.id,
                    space: edit.space,
                    status: self.replica.status(edit.id)?.unwrap_or(EditStatus::Pending),
                })
            })
            .collect()
    }

    /// The queued edits whose publication conflicted with a remote change.
    pub fn conflicts(&self) -> Result<Vec<(QueuedEdit, ConflictKind)>> {
        Ok(self
            .queue()?
            .into_iter()
            .filter_map(|edit| match edit.status {
                EditStatus::Conflict(kind) => Some((edit, kind)),
                _ => None,
            })
            .collect())
    }

    /// The page as last observed in the section file, for reviewing a conflict.
    pub fn remote_page(&self, space: ExGuid) -> Result<Page> {
        let snapshot = self.replica.remote_snapshot()?;
        let store = Store::parse(&snapshot)?;
        let index = RevisionIndex::parse(&store)?;
        Ok(Page::from_space(&Document::parse(&index)?, space)?)
    }

    /// Resolves the oldest conflict with a page reviewed against `remote_page`; the
    /// reviewed model publishes as a whole, keeping the edit's id.
    pub fn review(&self, id: u64, after: &Page) -> Result<()> {
        let local = self.replica.snapshot()?;
        let remote = self.replica.remote_snapshot()?;
        self.replica.review_page(id, &local, &remote, after)?;
        self.wake();
        Ok(())
    }

    /// Retires an uncertain attempt after review, exporting the branch to `archive`
    /// first: `Some(page)` continues from the reviewed page against `remote_page`,
    /// `None` abandons the local branch. Neither claims the attempt was acknowledged.
    pub fn release(&self, id: u64, archive: impl AsRef<Path>, after: Option<&Page>) -> Result<()> {
        let local = self.replica.snapshot()?;
        let remote = self.replica.remote_snapshot()?;
        self.replica
            .release_attempt(id, &local, &remote, archive.as_ref(), after)?;
        self.wake();
        Ok(())
    }

    /// Captures both images, the queue and its states in a read-only archive.
    pub fn export_recovery(&self, path: impl AsRef<Path>) -> Result<()> {
        self.replica.export_recovery(path)
    }

    /// Events since the last poll, oldest first.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Requests a synchronization attempt now.
    pub fn wake(&self) {
        if let Some(worker) = &self.worker {
            worker.wake();
        }
    }

    pub fn replica(&self) -> &Replica {
        &self.replica
    }

    /// Waits for the in-flight operation and callback before releasing the replica.
    /// Dropping instead requests cancellation without waiting; the worker retains
    /// cache ownership until that operation finishes. Remote calls must be bounded.
    pub fn close(mut self) -> Result<()> {
        match self.worker.take() {
            Some(worker) => worker.stop(),
            None => Ok(()),
        }
    }
}

/// The section file itself as the publication target, under OneNote-compatible exclusion.
struct FileRemote(PathBuf);

impl Remote for FileRemote {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        onestore::read_file(&self.0)
    }

    fn publish(&mut self, edit: &PreparedEdit<'_>) -> std::result::Result<(), CommitError> {
        edit.commit_file(&self.0)
    }

    fn confirm(&mut self, snapshot: &[u8]) -> std::result::Result<(), CommitError> {
        onestore::confirm_file_snapshot(&self.0, snapshot)
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        drop(self.worker.take());
    }
}
