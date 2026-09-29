//! The application's view of a notebook: sections opened through a local replica that
//! publishes their edits to the section file in the background.

pub use crate::background::Background;
use crate::{
    EditStatus, Error, PendingEdit, Remote, Replica, Resolution, Result, SyncWorker, discover,
};
use onestore::{
    CommitError, ExGuid, PageCreation, RevisionIndex, Stamp, Store, Transaction,
    document::Document,
    op::{Edit, Op, SectionOp},
    page::Page,
};
use std::{
    collections::BTreeMap,
    io,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
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
    /// Deletes a file or an empty directory.
    fn delete(&self, path: &str) -> Result<()>;
    /// Names a section or TOC file for its notebook, as `onestore::place`.
    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()>;
    /// Publishes a transaction made on the file's current image.
    fn commit(&self, path: &str, transaction: &Transaction) -> Result<()>;
}

/// A password-protected section as `Notebook::unlock` read it.
#[cfg(feature = "protected")]
pub struct Unlocked {
    pub pages: Vec<(ExGuid, Page)>,
    /// The stored section the pages came from, which an edit must still find in place.
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

    fn delete(&self, path: &str) -> Result<()> {
        let path = self.path(path);
        if path.is_dir() {
            std::fs::remove_dir(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()> {
        Ok(onestore::place_file(self.path(path), ancestor, name)?)
    }

    fn commit(&self, path: &str, transaction: &Transaction) -> Result<()> {
        Ok(transaction.commit_file(self.path(path))?)
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

    fn delete(&self, path: &str) -> Result<()> {
        Ok(self.client.delete(&self.path(path))?)
    }

    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()> {
        Ok(self.client.place(&self.path(path), ancestor, name)?)
    }

    fn commit(&self, path: &str, transaction: &Transaction) -> Result<()> {
        Ok(self
            .client
            .commit_transaction(&self.path(path), transaction)?)
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

    /// Creates a notebook in the new folder `root`, as OneNote 2010 creates one: a table of
    /// contents in the notebook colour `color` (COLORREF) and "New Section 1" holding the
    /// page `page` creates.
    pub fn create(
        root: impl AsRef<Path>,
        cache: impl AsRef<Path>,
        color: u32,
        page: &PageCreation,
    ) -> Result<Self> {
        std::fs::create_dir(root.as_ref())?;
        let mut notebook = Self::open(root, cache)?;
        notebook.edit_toc("", &[onestore::TocEdit::Color(color)])?;
        notebook.refresh()?;
        notebook.create_section("", "New Section 1", page)?;
        Ok(notebook)
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
                // One this change already created, before the catalog was read again.
                let bytes = if self.storage.exists(&path) {
                    self.storage.read(&path)?
                } else {
                    let bytes = onestore::create_table_of_contents(TOC, &[])?;
                    self.storage.create(&path, &bytes)?;
                    bytes
                };
                Ok((path, onestore::Store::parse(&bytes)?.header.file_id))
            }
        }
    }

    fn edit_toc(&self, folder: &str, edits: &[onestore::TocEdit]) -> Result<()> {
        let (toc, _) = self.toc(folder)?;
        self.commit_toc(&toc, edits)
    }

    /// Commits `edits` to the TOC at `toc`. A name it still lists for a file gone from its
    /// folder passes to the file an edit gives that name; the stale entry goes.
    fn commit_toc(&self, toc: &str, edits: &[onestore::TocEdit]) -> Result<()> {
        let unresolved = self
            .folder(split(toc).0)
            .ok()
            .and_then(|folder| folder.toc.as_ref())
            .map_or(&[][..], |toc| &toc.unresolved[..]);
        let mut superseded = Vec::new();
        for edit in edits {
            if let onestore::TocEdit::Add { filename, .. }
            | onestore::TocEdit::Rename { filename, .. } = edit
            {
                superseded.extend(
                    unresolved
                        .iter()
                        .filter(|entry| {
                            entry
                                .filename
                                .as_deref()
                                .is_some_and(|name| name.eq_ignore_ascii_case(filename))
                        })
                        .map(|entry| onestore::TocEdit::Remove {
                            identity: entry.file,
                        }),
                );
            }
            superseded.push(edit.clone());
        }
        let source = self.storage.read(toc)?;
        match onestore::edit_table_of_contents(&source, &superseded)? {
            Some(transaction) => self.storage.commit(toc, &transaction),
            None => Ok(()),
        }
    }

    /// Creates `name.one` in `folder` holding the page `page` creates, in the next of
    /// OneNote's section colours, and lists it last in the folder's TOC, as OneNote creates
    /// a section. Returns the new catalog path.
    pub fn create_section(
        &mut self,
        folder: &str,
        name: &str,
        page: &PageCreation,
    ) -> Result<String> {
        let filename = format!("{name}.one");
        let color = next_color(self.folder(folder)?);
        let (_, ancestor) = self.toc(folder)?;
        let mut bytes = onestore::create_empty_section(&filename, Some(color))?;
        let transaction = {
            let arena = onestore::Arena::default();
            let mut section = onestore::Section::open(&arena, bytes.clone())?;
            let edit = Edit {
                at: crate::now(),
                ops: vec![Op::Section(SectionOp::Create(page.clone()))],
            };
            section.apply(page.author(), &edit)?;
            section.seal()?
        };
        if let Some(transaction) = transaction {
            transaction.apply(&mut bytes)?;
        }
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
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, self.storage.read(&path)?)?;
        let edit = Edit {
            at: crate::now(),
            ops: vec![Op::Section(SectionOp::Color(color))],
        };
        section.apply("", &edit)?;
        if let Some(transaction) = section.seal()? {
            self.storage.commit(&path, &transaction)?;
        }
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

    /// Deletes a section or section group the way OneNote 2010 does. A section's file moves
    /// into the notebook's `OneNote_RecycleBin` folder, a section group with its own TOC that
    /// lists it (and that the root TOC lists). A group's sections, in its groups too, move
    /// there the same way, then its folders go. Either way its folder's TOC entry goes.
    pub fn delete(&mut self, path: &str) -> Result<()> {
        let (folder, entry) = split(path);
        let (filename, identity) = self.entry(folder, entry)?;
        if path == RECYCLE_BIN {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        if filename.to_ascii_lowercase().ends_with(".one") {
            self.bin_section(path, &filename, identity)?;
        } else {
            // Sections first, then each folder once it is empty, deepest first.
            let mut sections = Vec::new();
            let mut folders = Vec::new();
            let mut pending = vec![self.folder(path)?];
            while let Some(group) = pending.pop() {
                // Its folder could not be emptied, so nothing moves.
                if let Some(entry) = group.unavailable.first() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        entry.error.clone(),
                    )
                    .into());
                }
                sections.extend(
                    group
                        .sections
                        .iter()
                        .map(|section| (section.path.clone(), section.file_id)),
                );
                folders.push((
                    group.path.clone(),
                    group.toc.as_ref().map(|toc| toc.filename.clone()),
                ));
                pending.extend(&group.groups);
            }
            for (section, identity) in sections {
                let (_, name) = split(&section);
                self.bin_section(&section, name, identity)?;
            }
            for (group, toc) in folders.into_iter().rev() {
                if let Some(toc) = toc {
                    self.storage.delete(&catalog_path(&group, &toc))?;
                }
                self.storage.delete(&group)?;
            }
        }
        self.edit_toc(folder, &[onestore::TocEdit::Remove { identity }])?;
        self.refresh().map(drop)
    }

    /// The recycle bin's TOC path and file identity, creating the bin as OneNote does: a
    /// section group the root TOC lists. A bin the root TOC misses is listed again.
    fn bin(&self) -> Result<(String, [u8; 16])> {
        if let Some(entry) = self
            .catalog
            .unavailable
            .iter()
            .find(|entry| entry.path == RECYCLE_BIN)
        {
            return Err(
                io::Error::new(io::ErrorKind::PermissionDenied, entry.error.clone()).into(),
            );
        }
        // A bin OneNote made may name its TOC otherwise; a second TOC would hide the folder.
        let bin_toc = match self.folder(RECYCLE_BIN) {
            Ok(discover::Folder { toc: Some(toc), .. }) => catalog_path(RECYCLE_BIN, &toc.filename),
            _ => {
                let bin_toc = catalog_path(RECYCLE_BIN, TOC);
                if !self.storage.exists(&bin_toc) {
                    if !self.storage.exists(RECYCLE_BIN) {
                        self.storage.create_directory(RECYCLE_BIN)?;
                    }
                    let bytes = onestore::create_table_of_contents(TOC, &[])?;
                    self.storage.create(&bin_toc, &bytes)?;
                }
                bin_toc
            }
        };
        let identity = onestore::Store::parse(&self.storage.read(&bin_toc)?)?
            .header
            .file_id;
        let (root, root_identity) = self.toc("")?;
        // OneNote takes a TOC placed under another parent for a new one.
        if !lists(&self.storage.read(&root)?, identity)? {
            self.storage.place(&bin_toc, root_identity, RECYCLE_BIN)?;
            self.commit_toc(
                &root,
                &[onestore::TocEdit::Add {
                    filename: RECYCLE_BIN.into(),
                    identity,
                    group: true,
                }],
            )?;
        }
        Ok((bin_toc, identity))
    }

    /// Moves the section file at `path` into the recycle bin, under a name no binned
    /// section has, and lists it there; its own folder's TOC is the caller's.
    fn bin_section(&self, path: &str, filename: &str, identity: [u8; 16]) -> Result<()> {
        let (bin_toc, bin_identity) = self.bin()?;
        let mut target = filename.to_owned();
        let mut attempt = 1;
        while self.storage.exists(&catalog_path(RECYCLE_BIN, &target)) {
            attempt += 1;
            let (stem, extension) = filename.rsplit_once('.').unwrap_or((filename, ""));
            target = format!("{stem} ({attempt}).{extension}");
        }
        let binned = catalog_path(RECYCLE_BIN, &target);
        self.storage.rename(path, &binned)?;
        self.storage.place(&binned, bin_identity, &target)?;
        self.commit_toc(
            &bin_toc,
            &[onestore::TocEdit::Add {
                filename: target,
                identity,
                group: false,
            }],
        )
    }

    /// Keeps copies of `pages` in the notebook's recycle bin, as OneNote 2010 does with a
    /// page it deletes: in `OneNote_RecycleBin/OneNote_DeletedPages.one`, each at the top
    /// level with its identity, title, date and creation time. The caller then deletes the
    /// pages from their section.
    pub fn recycle_pages(&mut self, pages: &[Page], author: &str) -> Result<()> {
        const DELETED: &str = "OneNote_DeletedPages.one";
        let (bin_toc, bin_identity) = self.bin()?;
        let path = catalog_path(RECYCLE_BIN, DELETED);
        if !self.storage.exists(&path) {
            // OneNote's "Deleted Pages" section is grey.
            let bytes = onestore::create_empty_section(DELETED, Some(0x00e1e1e1))?;
            self.storage.create(&path, &bytes)?;
        }
        let mut bytes = self.storage.read(&path)?;
        // Listed and placed too when an earlier delete made the file but not its entry.
        let identity = onestore::Store::parse(&bytes)?.header.file_id;
        if !lists(&self.storage.read(&bin_toc)?, identity)? {
            self.storage.place(&path, bin_identity, DELETED)?;
            self.commit_toc(
                &bin_toc,
                &[onestore::TocEdit::Add {
                    filename: DELETED.into(),
                    identity,
                    group: false,
                }],
            )?;
            bytes = self.storage.read(&path)?;
        }
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, bytes)?;
        let mut ops = Vec::new();
        for page in pages {
            ops.push(moved(page, author)?);
        }
        section.apply(
            author,
            &Edit {
                at: crate::now(),
                ops,
            },
        )?;
        if let Some(transaction) = section.seal()? {
            self.storage.commit(&path, &transaction)?;
        }
        self.refresh().map(drop)
    }

    /// Moves a section or section group into the folder at catalog path `folder`, last, as
    /// OneNote moves one dragged onto a group: the file or folder moves and each TOC's
    /// entry follows it. Returns its new catalog path.
    pub fn move_entry(&mut self, path: &str, folder: &str) -> Result<String> {
        let (from, entry) = split(path);
        let (filename, identity) = self.entry(from, entry)?;
        self.folder(folder)?;
        let group = !filename.to_ascii_lowercase().ends_with(".one");
        let target = catalog_path(folder, &filename);
        if from == folder
            || path == RECYCLE_BIN
            || group && (folder == path || folder.starts_with(&format!("{path}/")))
            || self.storage.exists(&target)
        {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        let (_, ancestor) = self.toc(folder)?;
        self.storage.rename(path, &target)?;
        let placed = if group {
            catalog_path(&target, TOC)
        } else {
            target.clone()
        };
        self.storage.place(&placed, ancestor, &filename)?;
        self.edit_toc(
            folder,
            &[onestore::TocEdit::Add {
                filename,
                identity,
                group,
            }],
        )?;
        self.edit_toc(from, &[onestore::TocEdit::Remove { identity }])?;
        self.refresh()?;
        Ok(target)
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

    /// The section at catalog `path` as its file stores it, without the edits a replica
    /// may hold (`stored_pages` reads it).
    pub fn read_section(&self, path: &str) -> Result<Vec<u8>> {
        self.storage.read(&self.section_path(path)?.path)
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

    /// Applies `edit` to pages of `unlocked` and stores it under the section's key, straight
    /// to the file: nothing of a protected section is cached or queued. A section written
    /// since `unlocked` was read refuses the edit; unlock again for its pages.
    #[cfg(feature = "protected")]
    pub fn apply_unlocked(
        &self,
        path: &str,
        password: &str,
        unlocked: &mut Unlocked,
        author: &str,
        edit: &Edit,
    ) -> Result<()> {
        let path = self.section_path(path)?.path.clone();
        let transaction = {
            let store = Store::parse(&unlocked.snapshot)?;
            let index = RevisionIndex::parse(&store)?;
            onestore::protected::UnlockedSection::open(
                &index,
                password,
                onestore::protected::Limits::default(),
            )?
            .apply(author, edit)?
        };
        self.storage.commit(&path, &transaction)?;
        transaction.apply(&mut unlocked.snapshot)?;
        Ok(())
    }

    /// Where the replica of the section at catalog `path` lives: named by the section's
    /// document identity in a mounted notebook (`Section::open`), by its file identity under
    /// `smb` on a share. It exists once the section has been opened.
    pub fn replica_path(&self, path: &str) -> Result<PathBuf> {
        let section = self.section_path(path)?;
        Ok(match &self.root {
            Some(_) => {
                let image = self.storage.read(&section.path)?;
                let root = RevisionIndex::parse(&Store::parse(&image)?)?.root;
                replica_file(&self.cache, &root.guid)
            }
            None => replica_file(&self.cache.join("smb"), &section.file_id),
        })
    }

    /// Every readable section's catalog path and replica, for `Background::watch`.
    pub fn replicas(&self) -> Vec<(String, Option<PathBuf>)> {
        let mut sections = Vec::new();
        let mut folders = vec![&self.catalog];
        while let Some(folder) = folders.pop() {
            for section in &folder.sections {
                if matches!(section.state, discover::SectionState::Readable { .. }) {
                    let replica = self.replica_path(&section.path).ok();
                    sections.push((section.path.clone(), replica));
                }
            }
            folders.extend(folder.groups.iter().rev());
        }
        sections
    }

    /// Keeps the sections of a mounted notebook in sync while they are not open
    /// (`Background`), polling each file every `interval` once watched.
    pub fn background(
        &self,
        interval: Duration,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Background> {
        let Some(root) = self.root.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Sections on a share sync through Background::smb",
            )
            .into());
        };
        Background::start(
            interval,
            move || {
                let root = root.clone();
                Ok(move |path: &str| FileRemote(root.join(path)))
            },
            notify,
        )
    }

    /// Opens a section of a mounted notebook by its catalog path.
    pub fn section(&self, path: &str, notify: impl Fn() + Send + 'static) -> Result<Section> {
        self.section_with(path, |file| Ok(FileRemote(file.to_owned())), notify)
    }

    /// `section`, reading and publishing through the remote `connect` makes for its file
    /// (`Section::open_with`).
    pub fn section_with<R: Remote + 'static>(
        &self,
        path: &str,
        connect: impl FnMut(&Path) -> io::Result<R> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Section> {
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
        Section::open_with(file, &self.cache, connect, notify)
    }
}

/// A page as a section file stores it.
pub struct StoredPage {
    pub space: ExGuid,
    pub page: Page,
    /// The page's `LastModifiedTime`, Time32 seconds since 1980.
    pub modified: Option<u32>,
}

/// The pages of the section file `image` holds, in section order; pages the model cannot
/// build are left out.
pub fn stored_pages(image: &[u8]) -> Result<Vec<StoredPage>> {
    let store = Store::parse(image)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    Ok(document
        .pages()?
        .into_iter()
        .filter_map(|(space, id)| {
            let revision = document.active(space).ok()?;
            Some(StoredPage {
                space,
                page: Page::from_revision(revision, id).ok()?,
                modified: revision.nodes.get(&id).and_then(|node| node.modified),
            })
        })
        .collect())
}

/// The colours OneNote 2010 gives a folder's first sixteen new sections, COLORREF, in the
/// order it gives them (`corpus/notebook-management/native/section-colors`: sixteen sections
/// made with New Section in a new notebook).
const SECTION_COLORS: [u32; 16] = [
    0x00e4a88a, 0x0078b0f6, 0x00bba4d5, 0x00d2bb9b, 0x00b79cab, 0x0099d1e8, 0x006ff9f5, 0x0092e7ad,
    0x00cabc4d, 0x007575ba, 0x00aa9595, 0x00e4a88a, 0x0069d8ff, 0x0097c9b7, 0x009795ee, 0x00de9eb4,
];

/// The colour OneNote gives a new section in `folder`, by how many sections it holds.
fn next_color(folder: &discover::Folder) -> u32 {
    SECTION_COLORS[folder.sections.len() % SECTION_COLORS.len()]
}

/// The op putting `page` into another section as OneNote moves a page there, into the
/// recycle bin or another section: last, under fresh object identities, keeping its page
/// identity, title, date and creation time.
pub fn moved(page: &Page, author: &str) -> Result<Op> {
    let mut creation = PageCreation::new(None, Some(&page.title), author)?;
    if let (Some(identity), Some(created)) = (page.identity, page.created) {
        creation = creation.keeping(identity, created)?;
    }
    if let Some([date, time]) = page.date_text() {
        creation = creation.dated(&date, &time)?;
    }
    Ok(Op::Section(SectionOp::Import {
        creation,
        page: page.copy()?,
    }))
}

fn component(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\0']) && name != "." && name != ".."
}

/// Whether the TOC `image` holds has an entry for the file identity `file`.
fn lists(image: &[u8], file: [u8; 16]) -> Result<bool> {
    let store = Store::parse(image)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let revision = document.active(document.root)?;
    let entries = revision
        .roots
        .get(&1)
        .and_then(|id| revision.nodes.get(id))
        .map_or(&[][..], |node| match &node.kind {
            onestore::document::Kind::Toc { entries, .. } => &entries[..],
            _ => &[],
        });
    Ok(entries.iter().any(|id| {
        matches!(
            revision.nodes.get(id).map(|node| &node.kind),
            Some(onestore::document::Kind::Toc { identity: Some(identity), .. }) if *identity == file
        )
    }))
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

/// The replica in `cache` of the section `identity` names.
fn replica_file(cache: &Path, identity: &[u8; 16]) -> PathBuf {
    let name: String = identity.iter().map(|byte| format!("{byte:02x}")).collect();
    cache.join(format!("{name}.sqlite"))
}

/// Why a synchronization step did not reach the section file, as the host shows it.
pub(crate) fn reached(error: &Error) -> io::Error {
    match error {
        Error::RemoteIo(error) => io::Error::new(error.kind(), error.to_string()),
        Error::Remote(error) => io::Error::new(error.error.kind(), error.to_string()),
        error => io::Error::other(error.to_string()),
    }
}

/// What happened to the section since the last poll.
#[derive(Debug)]
pub enum Event {
    /// A remote change reached these pages; reload them where they are open.
    Changed(Vec<ExGuid>),
    /// The section refused an edit `apply` handed it; the pages it names are as they were
    /// before it, so an editor showing them should reload them.
    Rejected { spaces: Vec<ExGuid>, error: String },
    /// A publication attempt finished with this durable state.
    Attempt { id: u64, status: EditStatus },
    /// The section file could not be reached; the replica keeps its state.
    Unreachable(io::Error),
    /// The replica itself failed; the worker has stopped.
    Failed(String),
}

type Notify = Arc<dyn Fn() + Send + Sync>;

/// How a section's synchronization stands, for the host to show.
#[derive(Debug)]
pub struct SyncStatus {
    /// FILETIME of the last synchronization step that reached the section file.
    pub synced: Option<u64>,
    /// Why the last step failed, until one reaches the section file again.
    pub error: Option<io::Error>,
    /// Edits the section file does not hold yet, uncertain attempts included.
    pub queued: u64,
}

/// The last attempt's outcome; `None` before the first.
type Observed = Option<Option<io::Error>>;

/// A section file with its replica and background publication. `Send + Sync`: edits
/// arrive from any thread without waiting, and `notify` wakes the host when events wait.
pub struct Section {
    file: PathBuf,
    replica: Arc<Replica>,
    worker: Option<SyncWorker>,
    events: Mutex<Receiver<Event>>,
    sender: Sender<Event>,
    notify: Notify,
    observed: Arc<Mutex<Observed>>,
}

impl Section {
    /// Opens the section file through a replica in `cache`, creating the replica from the
    /// file on first use and converting an older one. `notify` runs on a background thread
    /// whenever an event is available.
    pub fn open(
        file: impl AsRef<Path>,
        cache: impl AsRef<Path>,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        Self::open_with(file, cache, |file| Ok(FileRemote(file.to_owned())), notify)
    }

    /// `open`, reading and publishing the file through the remote `connect` makes for it,
    /// as a host that coordinates file access with other processes (iOS's file providers)
    /// needs.
    pub fn open_with<R: Remote + 'static>(
        file: impl AsRef<Path>,
        cache: impl AsRef<Path>,
        mut connect: impl FnMut(&Path) -> io::Result<R> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let file = file.as_ref().canonicalize()?;
        let source = connect(&file)?.read()?;
        let store = Store::parse(&source)?;
        let identity = RevisionIndex::parse(&store)?.root;
        std::fs::create_dir_all(&cache)?;
        let cache = replica_file(cache.as_ref(), &identity.guid);
        let replica = if cache.exists() {
            Replica::open(&cache)?
        } else {
            Replica::create(&cache, &source)?
        };
        let remote = file.clone();
        Self::start(file, replica, move || connect(&remote), notify)
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
        let notify = Mutex::new(notify);
        let notify: Notify = Arc::new(move || {
            if let Ok(notify) = notify.lock() {
                notify();
            }
        });
        let observed: Arc<Mutex<Observed>> = Arc::new(Mutex::new(None));
        let worker = {
            let (sender, notify) = (sender.clone(), Arc::clone(&notify));
            let observed = Arc::clone(&observed);
            replica.start_sync(Duration::from_secs(2), connect, move |result| {
                let error = result.as_ref().err().map(reached);
                // Reaching the file as the last attempt did is no news to the host.
                let mut changed = false;
                if let Ok(mut observed) = observed.lock() {
                    changed = observed
                        .as_ref()
                        .is_none_or(|last| last.is_some() != error.is_some());
                    *observed = Some(error);
                }
                let mut events = Vec::new();
                match result {
                    Ok(synced) => {
                        if !synced.changed.is_empty() {
                            events.push(Event::Changed(synced.changed.clone()));
                        }
                        if let Some((id, status)) = &synced.edit {
                            events.push(Event::Attempt {
                                id: *id,
                                status: status.clone(),
                            });
                        }
                    }
                    Err(Error::RemoteIo(error)) => {
                        events.push(Event::Unreachable(match error.raw_os_error() {
                            Some(code) => io::Error::from_raw_os_error(code),
                            None => io::Error::new(error.kind(), error.to_string()),
                        }))
                    }
                    Err(Error::Remote(error)) => events.push(Event::Unreachable(io::Error::new(
                        error.error.kind(),
                        error.to_string(),
                    ))),
                    Err(error) => events.push(Event::Failed(error.to_string())),
                }
                if (changed || !events.is_empty())
                    && events.into_iter().all(|event| sender.send(event).is_ok())
                {
                    notify();
                }
            })?
        };
        Ok(Self {
            file,
            replica,
            worker: Some(worker),
            events: Mutex::new(events),
            sender,
            notify,
            observed,
        })
    }

    /// The absolute local path, or share-relative path for an SMB session.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The section file identity, which internal links name as `section-id`
    /// (`onestore::page::link::internal_link`).
    pub fn identity(&self) -> Result<[u8; 16]> {
        self.replica.identity()
    }

    /// Applies an edit without waiting: it becomes durable on the section thread, which
    /// reports a refusal as `Event::Rejected`. Edits apply in the order they arrive.
    pub fn apply(&self, author: &str, edit: Edit) -> Result<()> {
        let (sender, notify) = (self.sender.clone(), Arc::clone(&self.notify));
        let root = self.replica.root;
        let spaces = crate::queue::spaces(&edit, root);
        self.replica.submit(
            author,
            edit,
            Box::new(move |result| {
                let event = match result {
                    Ok(_) => return,
                    Err(Error::Rejected(error)) => Event::Rejected {
                        spaces,
                        error: error.to_string(),
                    },
                    Err(error) => Event::Failed(error.to_string()),
                };
                if sender.send(event).is_ok() {
                    notify();
                }
            }),
        )
    }

    /// Page spaces, titles and outline levels (1 at the top) in section order, as the
    /// local edits leave them.
    pub fn pages(&self) -> Result<Vec<(ExGuid, String, u32)>> {
        self.replica.pages()
    }

    /// The page to show or edit; O(page), for opening and reloading.
    pub fn page(&self, space: ExGuid) -> Result<Page> {
        self.replica.page(space)
    }

    /// Copies a page (usually from another section) to the end of this section under
    /// fresh identities, queued like the user's own edits. Returns the new page's space.
    /// Content outside the model refuses to copy.
    pub fn import_page(&self, page: &Page, author: &str) -> Result<ExGuid> {
        let creation = PageCreation::new(None, Some(&page.title), author)?;
        let space = creation.space();
        self.replica.apply(
            author,
            Edit {
                at: crate::now(),
                ops: vec![Op::Section(SectionOp::Import {
                    creation,
                    page: page.copy()?,
                })],
            },
        )?;
        Ok(space)
    }

    /// Removes pages or conflict pages permanently, queued like the user's own edits (a
    /// move across sections is `import_page` there, then this here).
    pub fn delete_pages(&self, pages: &[ExGuid]) -> Result<u64> {
        self.replica.apply(
            "",
            Edit {
                at: crate::now(),
                ops: vec![Op::Section(SectionOp::Delete(pages.to_vec()))],
            },
        )
    }

    pub fn status(&self, id: u64) -> Result<Option<EditStatus>> {
        self.replica.status(id)
    }

    pub fn pending(&self) -> Result<Vec<PendingEdit>> {
        self.replica.pending()
    }

    /// The conflict pages of each page that has them, as the local edits leave them
    /// (`onestore::Section::conflicts`); `page` reads one, `delete_pages` removes it.
    pub fn conflicts(&self) -> Result<Vec<(ExGuid, Vec<onestore::ConflictPage>)>> {
        self.replica.conflicts()
    }

    /// The versions of each page that has them, newest first, as the local edits leave them
    /// (`onestore::Section::versions`); `version` reads one.
    pub fn versions(&self) -> Result<Vec<(ExGuid, Vec<onestore::PageVersion>)>> {
        self.replica.versions()
    }

    /// Page `space` as its version `version` holds it; O(section).
    pub fn version(&self, space: ExGuid, version: ExGuid) -> Result<Page> {
        self.replica.version(space, version)
    }

    /// Makes a page's version its current state, the page as it stood becoming the newest
    /// version, queued like the user's own edits.
    pub fn restore_version(&self, space: ExGuid, version: ExGuid, author: &str) -> Result<u64> {
        self.replica.apply(
            author,
            Edit {
                at: crate::now(),
                ops: vec![Op::Section(SectionOp::restore(space, version)?)],
            },
        )
    }

    /// Deletes versions of pages, queued like the user's own edits.
    pub fn delete_versions(&self, versions: &[(ExGuid, Vec<ExGuid>)]) -> Result<u64> {
        self.replica.apply(
            "",
            Edit {
                at: crate::now(),
                ops: versions
                    .iter()
                    .map(|(page, versions)| {
                        Op::Section(SectionOp::DeleteVersions {
                            page: *page,
                            versions: versions.clone(),
                        })
                    })
                    .collect(),
            },
        )
    }

    /// Retires an uncertain attempt after review, exporting the queue to `archive` first:
    /// `Mine` publishes the local edits again, `Theirs` abandons them. Neither claims the
    /// attempt was acknowledged.
    pub fn release(
        &self,
        id: u64,
        archive: impl AsRef<Path>,
        resolution: Resolution,
    ) -> Result<()> {
        self.replica.release(id, archive.as_ref(), resolution)
    }

    /// Captures the queue, its images and its states in a read-only archive.
    pub fn export_recovery(&self, path: impl AsRef<Path>) -> Result<()> {
        self.replica.export_recovery(path)
    }

    /// Events since the last poll, oldest first.
    pub fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .map(|events| events.try_iter().collect())
            .unwrap_or_default()
    }

    /// Requests a synchronization attempt now, working offline included (Sync Now).
    pub fn wake(&self) {
        if let Some(worker) = &self.worker {
            worker.wake();
        }
    }

    /// Stops or resumes synchronizing: working offline, edits queue until `wake` or until
    /// working online again (OneNote's Work Offline).
    pub fn set_offline(&self, offline: bool) {
        if let Some(worker) = &self.worker {
            worker.set_offline(offline);
        }
    }

    /// When the section file was last reached, why it could not be since, and what waits
    /// for it. `notify` runs when it is first reached and when an error comes or goes.
    pub fn sync_status(&self) -> Result<SyncStatus> {
        let queued = self.replica.recovery_summary()?.queued_edits;
        let observed = self
            .observed
            .lock()
            .map_err(|_| io::Error::other("Synchronization observer panicked"))?;
        Ok(SyncStatus {
            synced: self.worker.as_ref().and_then(SyncWorker::synced),
            error: observed
                .as_ref()
                .and_then(Option::as_ref)
                .map(|error| io::Error::new(error.kind(), error.to_string())),
            queued,
        })
    }

    /// The replica, which other threads may read pages from while the section is open.
    pub fn replica(&self) -> &Arc<Replica> {
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

    /// Read without the whole-file lock, which would block OneNote's readers: a change
    /// detector, never a snapshot to edit.
    fn stamp(&mut self) -> io::Result<Stamp> {
        use std::io::Read;
        let mut file = std::fs::File::open(&self.0)?;
        let mut header = [0; 1024];
        file.read_exact(&mut header)?;
        let length = file.metadata()?.len();
        Ok(Stamp { header, length })
    }

    fn publish(&mut self, transaction: &Transaction) -> std::result::Result<(), CommitError> {
        transaction.commit_file(&self.0)
    }

    fn confirm(&mut self, base: &Stamp) -> std::result::Result<(), CommitError> {
        onestore::confirm_file(&self.0, base)
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        drop(self.worker.take());
    }
}
