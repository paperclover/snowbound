//! The application's view of a notebook: sections opened through a local replica that
//! publishes their edits to the section file in the background.

pub use crate::background::{Background, Known, Listener};
use crate::{
    EditStatus, Error, PendingEdit, Remote, Replica, Resolution, Result, SyncWorker, discover, fs,
};
use onestore::{
    CommitError, ExGuid, PageCreation, PageEdit, RevisionIndex, Stamp, Store, Transaction,
    document::Document,
    op::{Edit, Op, SectionOp},
    page::Page,
    protected,
};
use std::{
    collections::{BTreeMap, BTreeSet},
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
    /// Reads the notebook's catalog, reading only the files `cache` holds as listed otherwise.
    fn discover(
        &self,
        cache: &mut discover::Cache,
        limits: discover::Limits,
    ) -> Result<discover::Folder>;
    /// Where the notebook lives, which names its catalog's cache.
    fn location(&self) -> String;
    /// A folder's entries, as discovery lists them.
    fn entries(&self, folder: &str) -> io::Result<Vec<discover::Entry>>;
    fn exists(&self, path: &str) -> bool;
    /// A section's or TOC's stamp, without reading its body or coordinating with writers.
    fn stamp(&self, path: &str) -> io::Result<Stamp>;
    fn read(&self, path: &str) -> Result<Vec<u8>>;
    /// Reads a file of at most `limit` bytes as it stands, whatever it holds.
    fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>>;
    /// Creates a file holding `bytes`; an existing file is an error.
    fn create(&self, path: &str, bytes: &[u8]) -> Result<()>;
    fn create_directory(&self, path: &str) -> Result<()>;
    /// Gives a file or directory the Windows hidden attribute, where the storage keeps one.
    fn hide(&self, path: &str) -> Result<()>;
    /// Renames or moves a file or directory; an existing target is an error.
    fn rename(&self, from: &str, to: &str) -> Result<()>;
    /// Renames the notebook's own folder to `name` beside it once no other writer holds any
    /// of `files`; the location it then has.
    fn rename_root(&self, name: &str, files: &[String]) -> Result<String>;
    /// Renames a file over another, replacing it.
    fn replace(&self, from: &str, to: &str) -> Result<()>;
    /// Deletes a file or an empty directory.
    fn delete(&self, path: &str) -> Result<()>;
    /// Names a section or TOC file for its notebook, as `onestore::place`.
    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()>;
    /// Publishes a transaction made on the file's current image.
    fn commit(&self, path: &str, transaction: &Transaction) -> Result<()>;
    /// Confirms that the file still has `base`'s stamp and is durable (`onestore::confirm`).
    fn confirm(&self, path: &str, base: &Stamp) -> std::result::Result<(), CommitError>;
    /// Puts the file `with` in the place of the section or TOC at `path` under the coordination
    /// its writers take, provided `path` still has `base`'s stamp.
    fn supersede(&self, path: &str, base: &Stamp, with: &str) -> Result<()>;
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
    fn discover(
        &self,
        cache: &mut discover::Cache,
        limits: discover::Limits,
    ) -> Result<discover::Folder> {
        Ok(cache.discover(&mut discover::Local::open(&self.0)?, limits)?)
    }

    fn location(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    fn entries(&self, folder: &str) -> io::Result<Vec<discover::Entry>> {
        use discover::Source;
        discover::Local::open(&self.0)?.entries(folder, LIMITS.entries)
    }

    fn exists(&self, path: &str) -> bool {
        fs::metadata(self.path(path)).is_ok()
    }

    fn stamp(&self, path: &str) -> io::Result<Stamp> {
        FileRemote(self.path(path)).stamp()
    }

    fn read(&self, path: &str) -> Result<Vec<u8>> {
        Ok(fs::read_file(self.path(path))?)
    }

    fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        fs::File::open(self.path(path))?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge).into());
        }
        Ok(bytes)
    }

    fn create(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path(path);
        if discover::placeholder(&path).is_some_and(|placeholder| fs::metadata(placeholder).is_ok())
        {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
        }
        fs::File::create_new(path)?.write_all(bytes)?;
        Ok(())
    }

    fn create_directory(&self, path: &str) -> Result<()> {
        Ok(fs::create_dir(self.path(path))?)
    }

    /// macOS keeps the attribute as `UF_HIDDEN`, and passes it on to a share it mounted;
    /// Linux has no attribute to set, a dot folder being hidden there already.
    #[cfg_attr(windows, allow(unsafe_code))]
    fn hide(&self, path: &str) -> Result<()> {
        #[cfg(target_vendor = "apple")]
        {
            use nix::sys::stat::{FileFlag, stat};
            let path = self.path(path);
            let flags = stat(&path).map_err(io::Error::from)?.st_flags;
            let flags = FileFlag::from_bits_retain(flags);
            if !flags.contains(FileFlag::UF_HIDDEN) {
                nix::unistd::chflags(&path, flags | FileFlag::UF_HIDDEN)
                    .map_err(io::Error::from)?;
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_ATTRIBUTE_HIDDEN, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
                SetFileAttributesW,
            };
            let path: Vec<u16> = self
                .path(path)
                .as_os_str()
                .encode_wide()
                .chain([0])
                .collect();
            // SAFETY: `path` is a NUL-terminated UTF-16 string that outlives both calls.
            let attributes = unsafe { GetFileAttributesW(path.as_ptr()) };
            if attributes == INVALID_FILE_ATTRIBUTES
                || unsafe { SetFileAttributesW(path.as_ptr(), attributes | FILE_ATTRIBUTE_HIDDEN) }
                    == 0
            {
                return Err(io::Error::last_os_error().into());
            }
        }
        #[cfg(not(any(target_vendor = "apple", windows)))]
        let _ = path;
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        Ok(fs::rename(self.path(from), self.path(to))?)
    }

    /// A local file system shows no other writer's hold; one that refuses to rename a folder
    /// whose files are open says so as the rename fails.
    fn rename_root(&self, name: &str, _: &[String]) -> Result<String> {
        let to = self.0.with_file_name(name);
        // A change of case alone finds the folder itself on a case-insensitive volume.
        if fs::metadata(&to).is_ok() && fs::canonicalize(&to)? != self.0 {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
        }
        fs::rename(&self.0, &to)?;
        Ok(to.to_string_lossy().into_owned())
    }

    fn replace(&self, from: &str, to: &str) -> Result<()> {
        Ok(fs::rename(self.path(from), self.path(to))?)
    }

    fn delete(&self, path: &str) -> Result<()> {
        let path = self.path(path);
        if fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()> {
        Ok(fs::place_file(self.path(path), ancestor, name)?)
    }

    fn commit(&self, path: &str, transaction: &Transaction) -> Result<()> {
        Ok(fs::commit_file(transaction, self.path(path))?)
    }

    fn confirm(&self, path: &str, base: &Stamp) -> std::result::Result<(), CommitError> {
        fs::confirm_file(self.path(path), base)
    }

    fn supersede(&self, path: &str, base: &Stamp, with: &str) -> Result<()> {
        Ok(fs::supersede_file(self.path(path), base, self.path(with))?)
    }
}

/// A notebook directory on an SMB share, reached through the native-compatible client.
#[cfg(feature = "smb")]
pub struct Share {
    client: Arc<crate::smb::Client>,
    root: String,
    /// Where the sections' replicas are.
    copies: PathBuf,
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
    fn discover(
        &self,
        cache: &mut discover::Cache,
        limits: discover::Limits,
    ) -> Result<discover::Folder> {
        let mut source = discover::Smb::new(&self.client, &self.root)?.copies(self.copies.clone());
        Ok(cache.discover(&mut source, limits)?)
    }

    fn location(&self) -> String {
        self.client.location(&self.root)
    }

    fn entries(&self, folder: &str) -> io::Result<Vec<discover::Entry>> {
        use discover::Source;
        discover::Smb::new(&self.client, &self.root)?.entries(folder, LIMITS.entries)
    }

    fn stamp(&self, path: &str) -> io::Result<Stamp> {
        self.client.stamp(&self.path(path))
    }

    fn exists(&self, path: &str) -> bool {
        let (folder, name) = split(path);
        self.client
            .read_dir(&self.path(folder), LIMITS.entries)
            .is_ok_and(|entries| entries.iter().any(|entry| entry.name == name))
    }

    fn read(&self, path: &str) -> Result<Vec<u8>> {
        Ok(self
            .client
            .read_storage(&self.path(path), 256 * 1024 * 1024)?)
    }

    fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>> {
        Ok(self.client.read_asset(&self.path(path), limit)?)
    }

    fn create(&self, path: &str, bytes: &[u8]) -> Result<()> {
        Ok(self.client.create(&self.path(path), bytes)?)
    }

    fn create_directory(&self, path: &str) -> Result<()> {
        Ok(self.client.create_directory(&self.path(path))?)
    }

    fn hide(&self, path: &str) -> Result<()> {
        Ok(self.client.hide(&self.path(path))?)
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        Ok(self.client.rename(&self.path(from), &self.path(to))?)
    }

    fn rename_root(&self, name: &str, files: &[String]) -> Result<String> {
        if self.root.is_empty() {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        for file in files {
            self.client.unheld(&self.path(file))?;
        }
        let (parent, _) = split(&self.root);
        let taken = self
            .client
            .read_dir(parent, LIMITS.entries)?
            .iter()
            .any(|entry| {
                entry.name.eq_ignore_ascii_case(name) && entry.name != split(&self.root).1
            });
        if taken {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
        }
        let to = catalog_path(parent, name);
        self.client.rename(&self.root, &to)?;
        Ok(self.client.location(&to))
    }

    fn replace(&self, from: &str, to: &str) -> Result<()> {
        Ok(self.client.replace(&self.path(from), &self.path(to))?)
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

    fn confirm(&self, path: &str, base: &Stamp) -> std::result::Result<(), CommitError> {
        self.client.confirm(&self.path(path), base)
    }

    fn supersede(&self, path: &str, base: &Stamp, with: &str) -> Result<()> {
        Ok(self
            .client
            .supersede(&self.path(path), base, &self.path(with))?)
    }
}

pub(crate) const LIMITS: discover::Limits = discover::Limits {
    entries: 100_000,
    bytes_per_file: 256 * 1024 * 1024,
    depth: 64,
};

const TOC: &str = "Open Notebook.onetoc2";
const RECYCLE_BIN: &str = "OneNote_RecycleBin";
/// How long OneNote 2010 keeps what its recycle bin holds, its `DaysToKeepRecycledItems`.
pub const RECYCLE_DAYS: u32 = 60;

/// A notebook's files and the cache directory holding its section replicas.
pub struct Notebook {
    storage: Box<dyn Storage>,
    /// The mounted directory, when sections open through local replicas.
    root: Option<PathBuf>,
    cache: PathBuf,
    catalog: discover::Folder,
    /// What reading the catalog took from each file, kept at `listing` in the cache.
    read: discover::Cache,
    listing: PathBuf,
}

impl Notebook {
    /// The colour OneNote 2010 gives each new notebook beside its default one, COLORREF
    /// (`corpus/notebook-management/native/new-notebook`).
    pub const NEW_COLOR: u32 = 0x00aeba91;

    pub fn open(root: impl AsRef<Path>, cache: impl AsRef<Path>) -> Result<Self> {
        let root = fs::canonicalize(root)?;
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
        fs::create_dir(root.as_ref())?;
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
        let copies = crate::location::folder(cache.as_ref(), &client.location(&root));
        Self::with(
            Box::new(Share {
                client,
                root,
                copies,
            }),
            None,
            cache,
        )
    }

    /// Opens the notebook a Live Share host serves to `guest`. Sections open through
    /// `Section::resume_hosted` with the catalog's paths; while the host can't be reached,
    /// the notebook opens as its folders were last listed.
    #[cfg(feature = "live")]
    pub fn open_hosted(
        guest: Arc<crate::live::share::Guest>,
        cache: impl AsRef<Path>,
    ) -> Result<Self> {
        let listed = listing(cache.as_ref(), &guest.location()).with_extension("entries.json");
        Self::with(
            Box::new(crate::live::share::Hosted::new(guest, listed)),
            None,
            cache,
        )
    }

    /// The storage the notebook's files are in, for a Live Share host to serve.
    pub fn into_storage(self) -> Box<dyn Storage> {
        self.storage
    }

    fn with(
        storage: Box<dyn Storage>,
        root: Option<PathBuf>,
        cache: impl AsRef<Path>,
    ) -> Result<Self> {
        let cache = cache.as_ref().to_path_buf();
        let listing = listing(&cache, &storage.location());
        fs::create_dir_all(listing.parent().unwrap_or(&cache))?;
        let mut read = fs::read(&listing)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let catalog = storage.discover(&mut read, LIMITS)?;
        let location = storage.location();
        let mut claims: BTreeMap<String, Vec<([u8; 16], &str)>> = BTreeMap::new();
        for section in catalog.sections() {
            let identity = match (&root, &section.state) {
                (Some(_), discover::SectionState::Readable { document, .. }) => *document,
                (None, discover::SectionState::Readable { .. }) => section.file_id,
                _ => continue,
            };
            claims
                .entry(replica_location(&location, section))
                .or_default()
                .push((identity, &section.path));
        }
        for (at, sections) in &claims {
            let identities: Vec<[u8; 16]> =
                sections.iter().map(|(identity, _)| *identity).collect();
            crate::location::claim(&cache, at, &identities, |identity| {
                let (_, path) = sections.iter().find(|(held, _)| held == identity)?;
                Stamp::of(&storage.read(path).ok()?).ok()
            })?;
        }
        let notebook = Self {
            storage,
            root,
            cache,
            catalog,
            read,
            listing,
        };
        notebook.keep_listing();
        notebook.forget_superseded();
        Ok(notebook)
    }

    /// Keeps what the catalog read for the next time the notebook opens; failing costs only
    /// reading the files again.
    fn keep_listing(&self) {
        let _ = (|| -> Result<()> {
            let folder = self.listing.parent().unwrap_or(Path::new("."));
            let mut file = tempfile::NamedTempFile::new_in(folder)?;
            serde_json::to_writer(&mut file, &self.read).map_err(io::Error::from)?;
            file.persist(&self.listing).map_err(|error| error.error)?;
            Ok(())
        })();
    }

    pub fn catalog(&self) -> &discover::Folder {
        &self.catalog
    }

    /// The tags the notebook draws with Snowbound's art (`crate::sidecar`); none where it
    /// maps none.
    pub fn tag_art(&self) -> Result<Vec<crate::sidecar::TagMapping>> {
        crate::sidecar::mappings(&*self.storage)
    }

    /// The secret of the notebook's live presence room, made where it has none.
    pub fn presence_room(&self) -> Result<[u8; 16]> {
        crate::sidecar::room(&*self.storage)
    }

    /// The picture a mapping names, once its bytes match its name.
    pub fn tag_art_file(&self, art: &str) -> Result<Vec<u8>> {
        crate::sidecar::art(&*self.storage, art)
    }

    /// Maps tag `name` with symbol `shape` to picture `bytes`, a PNG or SVG as `extension`
    /// says, making the notebook's hidden `.snowbound` folder where it has none. Returns the
    /// notebook's mappings as they then stand.
    pub fn map_tag_art(
        &self,
        name: &str,
        shape: u16,
        bytes: &[u8],
        extension: &str,
    ) -> Result<Vec<crate::sidecar::TagMapping>> {
        crate::sidecar::map(&*self.storage, name, shape, bytes, extension)
    }

    /// The notebook's style themes and which theme each scope takes (`sidecar::themes`).
    pub fn themes(&self) -> Result<crate::sidecar::themes::Themes> {
        crate::sidecar::themes::read(&*self.storage)
    }

    /// Merges `change` into the notebook's themes, making its hidden `.snowbound` folder
    /// where it has none; returns the themes as they then stand.
    pub fn save_themes(
        &self,
        change: crate::sidecar::themes::Themes,
    ) -> Result<crate::sidecar::themes::Themes> {
        crate::sidecar::themes::write(&*self.storage, change)
    }

    /// Rereads the notebook and reports what changed since the last catalog, keyed by
    /// file identity so a renamed or moved section stays the same section. A failed read
    /// keeps the previous catalog: an unreachable notebook is not an empty one.
    pub fn refresh(&mut self) -> Result<Vec<Change>> {
        let catalog = self.storage.discover(&mut self.read, LIMITS)?;
        self.keep_listing();
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
        self.forget_superseded();
        Ok(changes)
    }

    fn folder(&self, path: &str) -> Result<&discover::Folder> {
        self.catalog
            .folders()
            .find(|folder| folder.path == path)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound).into())
    }

    /// A catalog section's path, refusing paths the catalog does not list.
    fn section_path(&self, path: &str) -> Result<&discover::Section> {
        self.catalog
            .sections()
            .find(|section| section.path == path)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound).into())
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
        let Entry {
            filename,
            identity,
            copy,
        } = self.entry(folder, entry)?;
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
        if !copy {
            let (_, ancestor) = self.toc(folder)?;
            let placed = if target.ends_with(".one") {
                renamed.clone()
            } else {
                catalog_path(&renamed, TOC)
            };
            self.storage.place(&placed, ancestor, &target)?;
        }
        if let Some(identity) = identity {
            self.edit_toc(
                folder,
                &[onestore::TocEdit::Rename {
                    identity,
                    filename: target,
                }],
            )?;
        }
        self.refresh()?;
        Ok(renamed)
    }

    /// Renames the notebook's folder to `name`, as OneNote 2010 finds a notebook folder renamed
    /// outside it: no file inside changes, and it opens the folder again by its new name. Refused,
    /// `WouldBlock`, while another writer holds one of its sections or tables of contents, and
    /// `AlreadyExists` where `name` is taken. The replicas and the catalog's listing follow, so
    /// edits waiting publish to the renamed folder; every replica must be closed. Returns the
    /// notebook's new location, as `crate::location` names it.
    pub fn rename_folder(mut self, name: &str) -> Result<String> {
        if !folder_name(name) {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        let from = self.storage.location();
        let files: Vec<String> = (self.catalog.folders())
            .flat_map(|folder| {
                let toc = folder.toc.as_ref().map(|toc| &toc.filename);
                (toc.map(|toc| catalog_path(&folder.path, toc)).into_iter())
                    .chain(folder.sections.iter().map(|section| section.path.clone()))
            })
            .collect();
        let to = self.storage.rename_root(name, &files)?;
        let mut moves = vec![(from.clone(), to.clone())];
        moves.extend(
            (self.catalog.sections())
                .filter(|section| section.copy)
                .map(|section| {
                    (
                        replica_location(&from, section),
                        replica_location(&to, section),
                    )
                }),
        );
        for (from, to) in moves {
            crate::location::moved(&self.cache, &from, &to)?;
        }
        let _ = fs::remove_file(&self.listing);
        self.listing = listing(&self.cache, &to);
        self.keep_listing();
        Ok(to)
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

    /// Sets the notebook's colour (COLORREF) in its root table of contents, as OneNote 2010's
    /// Notebook Properties does (`corpus/section-color`).
    pub fn set_color(&mut self, color: u32) -> Result<()> {
        self.edit_toc("", &[onestore::TocEdit::Color(color)])?;
        self.refresh().map(drop)
    }

    /// Orders a folder's sections and groups; entries left out follow in their current order.
    pub fn reorder(&mut self, folder: &str, paths: &[&str]) -> Result<()> {
        let mut identities = Vec::new();
        for path in paths {
            let (parent, entry) = split(path);
            if parent != folder {
                return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
            }
            identities.extend(self.entry(folder, entry)?.identity);
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
        let deleted = self.entry(folder, entry)?;
        if path == RECYCLE_BIN {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        if deleted.filename.to_ascii_lowercase().ends_with(".one") {
            self.bin_section(path, &deleted)?;
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
                        .map(|section| (section.path.clone(), Entry::of(group, section))),
                );
                folders.push((
                    group.path.clone(),
                    group.toc.as_ref().map(|toc| toc.filename.clone()),
                ));
                pending.extend(&group.groups);
            }
            for (section, entry) in sections {
                self.bin_section(&section, &entry)?;
            }
            for (group, toc) in folders.into_iter().rev() {
                if let Some(toc) = toc {
                    self.storage.delete(&catalog_path(&group, &toc))?;
                }
                self.storage.delete(&group)?;
            }
        }
        if let Some(identity) = deleted.identity {
            self.edit_toc(folder, &[onestore::TocEdit::Remove { identity }])?;
        }
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

    /// Moves the section file at `path`, its folder's `entry`, into the recycle bin, under a
    /// name no binned section has, and lists it there; its own folder's TOC is the caller's.
    fn bin_section(&self, path: &str, entry: &Entry) -> Result<()> {
        let filename = &entry.filename;
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
        let (false, Some(identity)) = (entry.copy, entry.identity) else {
            return Ok(());
        };
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

    /// Takes the pages `identities` names out of the recycle bin's Deleted Pages in one
    /// revision, as OneNote 2010 does when Undo brings deleted pages back. Pages not there
    /// are passed over.
    pub fn unrecycle_pages(&mut self, identities: &[[u8; 16]]) -> Result<()> {
        let path = catalog_path(RECYCLE_BIN, "OneNote_DeletedPages.one");
        if !self.storage.exists(&path) {
            return Ok(());
        }
        let bytes = self.storage.read(&path)?;
        let binned: Vec<ExGuid> = stored_pages(&bytes)?
            .into_iter()
            .filter(|stored| {
                stored
                    .page
                    .identity
                    .is_some_and(|id| identities.contains(&id))
            })
            .map(|stored| stored.space)
            .collect();
        if binned.is_empty() {
            return Ok(());
        }
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, bytes)?;
        section.apply(
            "",
            &Edit {
                at: crate::now(),
                ops: vec![Op::Section(SectionOp::Delete(binned))],
            },
        )?;
        if let Some(transaction) = section.seal()? {
            self.storage.commit(&path, &transaction)?;
        }
        self.refresh().map(drop)
    }

    /// Deletes for good what OneNote 2010 prunes from the recycle bin at `now`, Time32 seconds
    /// since 1980: each page of Deleted Pages last changed over `RECYCLE_DAYS` before, in one
    /// revision, and each binned section none of whose pages changed since
    /// (`corpus/recycle-purge`). OneNote judges by the pages' own last change, not when they
    /// were deleted, and leaves a pruned section's entry in the bin's TOC, as this does.
    /// Returns how many pages and sections went.
    pub fn purge_recycle_bin(&mut self, now: u32) -> Result<usize> {
        const DELETED: &str = "OneNote_DeletedPages.one";
        let expired = |modified: Option<u32>| {
            modified.is_some_and(|modified| now.saturating_sub(modified) > RECYCLE_DAYS * 86_400)
        };
        let Ok(bin) = self.folder(RECYCLE_BIN) else {
            return Ok(0);
        };
        let sections: Vec<String> = bin
            .sections
            .iter()
            .map(|section| section.path.clone())
            .collect();
        let mut purged = 0;
        for path in sections {
            let bytes = self.storage.read(&path)?;
            // A protected section, or one the model cannot read, stays.
            let Ok(pages) = stored_pages(&bytes) else {
                continue;
            };
            if split(&path).1.eq_ignore_ascii_case(DELETED) {
                let old: Vec<ExGuid> = pages
                    .iter()
                    .filter(|page| expired(page.modified))
                    .map(|page| page.space)
                    .collect();
                if old.is_empty() {
                    continue;
                }
                let arena = onestore::Arena::default();
                let mut section = onestore::Section::open(&arena, bytes)?;
                section.apply(
                    "",
                    &Edit {
                        at: crate::now(),
                        ops: vec![Op::Section(SectionOp::Delete(old.clone()))],
                    },
                )?;
                if let Some(transaction) = section.seal()? {
                    self.storage.commit(&path, &transaction)?;
                }
                purged += old.len();
            } else if !pages.is_empty() && pages.iter().all(|page| expired(page.modified)) {
                self.storage.delete(&path)?;
                purged += 1;
            }
        }
        if purged > 0 {
            self.refresh()?;
        }
        Ok(purged)
    }

    /// Empties the recycle bin as OneNote 2010's Empty Recycle Bin does: every page of
    /// Deleted Pages goes in one revision, and every binned section's file goes, its entry
    /// left in the bin's TOC (`corpus/recycle-bin-view`).
    pub fn empty_recycle_bin(&mut self) -> Result<()> {
        let Ok(bin) = self.folder(RECYCLE_BIN) else {
            return Ok(());
        };
        let sections: Vec<String> = (bin.sections.iter())
            .filter(|section| !section.copy)
            .map(|section| section.path.clone())
            .collect();
        for path in sections {
            if !split(&path)
                .1
                .eq_ignore_ascii_case("OneNote_DeletedPages.one")
            {
                self.storage.delete(&path)?;
                continue;
            }
            let arena = onestore::Arena::default();
            let mut section = onestore::Section::open(&arena, self.storage.read(&path)?)?;
            let pages: Vec<ExGuid> = section
                .pages()?
                .into_iter()
                .map(|(space, ..)| space)
                .collect();
            if pages.is_empty() {
                continue;
            }
            section.apply(
                "",
                &Edit {
                    at: crate::now(),
                    ops: vec![Op::Section(SectionOp::Delete(pages))],
                },
            )?;
            if let Some(transaction) = section.seal()? {
                self.storage.commit(&path, &transaction)?;
            }
        }
        self.refresh().map(drop)
    }

    /// The table of contents of the folder at catalog path `folder` as its file stores it.
    pub fn read_toc(&self, folder: &str) -> Result<Vec<u8>> {
        let toc = self.folder(folder)?.toc.as_ref();
        let toc = toc.ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        self.storage.read(&catalog_path(folder, &toc.filename))
    }

    /// Moves a section or section group into the folder at catalog path `folder`, last, as
    /// OneNote moves one dragged onto a group: the file or folder moves and each TOC's
    /// entry follows it. Returns its new catalog path.
    pub fn move_entry(&mut self, path: &str, folder: &str) -> Result<String> {
        let (from, entry) = split(path);
        let Entry {
            filename,
            identity,
            copy,
        } = self.entry(from, entry)?;
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
        if !copy {
            let placed = if group {
                catalog_path(&target, TOC)
            } else {
                target.clone()
            };
            self.storage.place(&placed, ancestor, &filename)?;
            if let Some(identity) = identity {
                self.edit_toc(
                    folder,
                    &[onestore::TocEdit::Add {
                        filename,
                        identity,
                        group,
                    }],
                )?;
            }
        }
        if let Some(identity) = identity {
            self.edit_toc(from, &[onestore::TocEdit::Remove { identity }])?;
        }
        self.refresh()?;
        Ok(target)
    }

    /// The stored filename and TOC identity of a folder's section or group.
    fn entry(&self, folder: &str, name: &str) -> Result<Entry> {
        let parent = self.folder(folder)?;
        if let Some(section) = parent
            .sections
            .iter()
            .find(|section| split(&section.path).1 == name)
        {
            return Ok(Entry::of(parent, section));
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
            return Ok(Entry {
                filename: name.to_owned(),
                identity: Some(toc.file_id),
                copy: false,
            });
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
        let mut sections: Vec<_> = self
            .catalog
            .sections()
            .filter(|section| matches!(section.state, discover::SectionState::Readable { .. }))
            .collect();
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

    /// The key of the password-protected section at catalog `path`, opened with `password`.
    /// Edits this device queued before the section was protected elsewhere, held since, are
    /// queued again under the key: each page they changed comes back as a copy, as a page
    /// another client removed does, the section's pages having taken new identities.
    pub fn unlock(&self, path: &str, password: &str) -> Result<protected::Key> {
        let section = self.section_path(path)?;
        let image = self.storage.read(&section.path)?;
        let key = protected::Key::open(&image, password)?;
        let held: Vec<PathBuf> = self
            .superseded(&[section])
            .into_iter()
            .filter(|(.., queued)| *queued > 0)
            .map(|(replica, ..)| replica)
            .collect();
        if !held.is_empty() {
            let replica =
                Replica::open_or_create(self.replica_path(path)?, Some(&key), || Ok(image))?;
            for path in held {
                // A queue sealed under an earlier password waits for that password.
                let Ok(old) = Replica::open(&path) else {
                    continue;
                };
                for (author, edit) in old.copies()? {
                    replica.apply(&author, edit)?;
                }
                drop(old);
                remove_replica(&path)?;
            }
        }
        Ok(key)
    }

    /// The replicas of the files the protected `locked` sections superseded when their
    /// passwords were set (`onestore::protected::rekey` keeps the placement a file's header
    /// names), each held so that no one opens it meanwhile, with how many edits it queues. Only
    /// replicas no readable section names are read; one in use is left.
    fn superseded(
        &self,
        locked: &[&discover::Section],
    ) -> Vec<(PathBuf, rusqlite::Connection, u64)> {
        let named: BTreeSet<PathBuf> = self
            .catalog
            .sections()
            .filter(|section| matches!(section.state, discover::SectionState::Readable { .. }))
            .filter_map(|section| self.replica_path(&section.path).ok())
            .collect();
        let placed: Vec<_> = locked
            .iter()
            .filter_map(|section| {
                let (_, stamp) = self.read.found(&section.path)?;
                Some((section.file_id, stamp.header, self.replica_folder(section)))
            })
            .collect();
        let folders: BTreeSet<&PathBuf> = placed.iter().map(|(.., folder)| folder).collect();
        let mut superseded = Vec::new();
        for folder in folders {
            let Ok(entries) = fs::read_dir(folder) else {
                continue;
            };
            for path in entries.filter_map(|entry| Some(entry.ok()?.path())) {
                if path
                    .extension()
                    .is_none_or(|extension| extension != "sqlite")
                    || named.contains(&path)
                {
                    continue;
                }
                let Ok(held) = crate::closed(&path) else {
                    continue;
                };
                let Ok((base, queued)) = crate::peek(&held) else {
                    continue;
                };
                let Ok(header) = onestore::Header::parse(&base.header) else {
                    continue;
                };
                if placed.iter().any(|(file, stamp, at)| {
                    at == folder
                        && base.header[128..148] == stamp[128..148]
                        && header.file_id != *file
                }) {
                    superseded.push((path, held, queued));
                }
            }
        }
        superseded
    }

    /// Deletes the replicas holding nothing unpublished of files a protected section
    /// superseded: their plaintext is the section's before its password.
    fn forget_superseded(&self) {
        let locked: Vec<_> = self
            .catalog
            .sections()
            .filter(|section| matches!(section.state, discover::SectionState::Locked))
            .collect();
        if locked.is_empty() {
            return;
        }
        for (replica, held, queued) in self.superseded(&locked) {
            drop(held);
            if queued == 0 {
                let _ = remove_replica(&replica);
            }
        }
    }

    /// Sets, changes or removes the password of the section at catalog `path`, as OneNote
    /// 2010 does: the section is written anew under new identities (`onestore::protected::
    /// rekey`), the file replaces the old one and its folder's TOC follows it. `key` opens
    /// it as it stands; `password` protects it anew, or `None` leaves it unprotected. Its
    /// replica, a cache of the old file, goes too, so its session must be closed and its
    /// edits published first. Returns the new key.
    pub fn set_password(
        &mut self,
        path: &str,
        key: Option<&protected::Key>,
        password: Option<&str>,
    ) -> Result<Option<protected::Key>> {
        let section = self.section_path(path)?;
        let replica = self.replica_path(path)?;
        // Held until the file is replaced, so that no session queues edits to the old file.
        let held = fs::metadata(&replica)
            .is_ok()
            .then(|| crate::closed(&replica))
            .transpose()?;
        if let Some(held) = &held
            && crate::peek(held)?.1 > 0
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Edits to the section wait to be published",
            )
            .into());
        }
        let source = self.storage.read(&section.path)?;
        let new = password.map(protected::Key::new).transpose()?;
        let image = onestore::protected::rekey(&source, key, new.as_ref())?;
        let (folder, filename) = split(&section.path);
        // A dot file, which discovery and OneNote pass over, until it replaces the section.
        let written = catalog_path(folder, &format!(".{filename}.snowbound"));
        if self.storage.exists(&written) {
            self.storage.delete(&written)?;
        }
        self.storage.create(&written, &image)?;
        if let Err(error) = self
            .storage
            .supersede(&section.path, &Stamp::of(&source)?, &written)
        {
            let _ = self.storage.delete(&written);
            return Err(error);
        }
        drop(held);
        remove_replica(&replica)?;
        let parent = self.folder(folder)?;
        if let Some(identity) = Entry::of(parent, section).identity {
            self.edit_toc(
                folder,
                &[onestore::TocEdit::Reidentify {
                    identity,
                    with: Store::parse(&image)?.header.file_id,
                }],
            )?;
        }
        self.refresh()?;
        Ok(new)
    }

    /// Where the replica of the section at catalog `path` lives, in the notebook's
    /// `location::folder`: named by the section's document identity in a mounted notebook
    /// (`Section::open`), by its file identity on a share. It exists once the section has
    /// been opened.
    pub fn replica_path(&self, path: &str) -> Result<PathBuf> {
        let section = self.section_path(path)?;
        let folder = self.replica_folder(section);
        Ok(match (&self.root, &section.state) {
            (Some(_), discover::SectionState::Readable { document, .. }) => {
                replica_file(&folder, document)
            }
            (Some(_), _) => {
                let image = self.storage.read(&section.path)?;
                let root = RevisionIndex::parse(&Store::parse(&image)?)?.root;
                replica_file(&folder, &root.guid)
            }
            (None, _) => replica_file(&folder, &section.file_id),
        })
    }

    /// The `location::folder` holding the replica of `section`.
    fn replica_folder(&self, section: &discover::Section) -> PathBuf {
        let location = replica_location(&self.storage.location(), section);
        crate::location::folder(&self.cache, &location)
    }

    /// Every readable section, for `Background::watch`, handing on the files the last
    /// discovery read so that each is read once.
    pub fn replicas(&mut self) -> Vec<Known> {
        let mut images = self.read.take();
        self.catalog
            .sections()
            .filter(|section| matches!(section.state, discover::SectionState::Readable { .. }))
            .map(|section| Known {
                path: section.path.clone(),
                replica: self.replica_path(&section.path).ok(),
                found: self.read.found(&section.path),
                image: images.remove(&section.path),
            })
            .collect()
    }

    /// Keeps the sections of a mounted notebook in sync while they are not open
    /// (`Background`): a file is checked when `Background::touched` reports it changed, and
    /// otherwise every `Background::BACKSTOP` when `watched`, as a host that watches the
    /// folder reports every change, or else every `Background::UNWATCHED`. `copies` keeps an
    /// offline copy of every section, for a folder that is not on this computer.
    pub fn background(
        &self,
        watched: bool,
        copies: bool,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Background> {
        self.background_with(watched, copies, |file| FileRemote(file.to_owned()), notify)
    }

    /// `background`, reaching each section file through the remote `remote` makes for it
    /// (`section_with`).
    pub fn background_with<R: Remote + 'static>(
        &self,
        watched: bool,
        copies: bool,
        remote: impl Fn(&Path) -> R + Clone + Send + 'static,
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
            copies,
            move |_| {
                let (folder, remote) = (root.clone(), remote.clone());
                let bind = move |path: &str| remote(&folder.join(path));
                let mut local = discover::Local::open(&root)?;
                let list = move |folder: String| {
                    use discover::Source;
                    std::future::ready(local.entries(&folder, LIMITS.entries))
                };
                Ok(((bind, list), watched))
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
        self.open_section(path, None, connect, notify)
    }

    /// `section` for a password-protected section, under the `key` `unlock` gave.
    pub fn section_unlocked(
        &self,
        path: &str,
        key: &protected::Key,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Section> {
        self.section_unlocked_with(path, key, |file| Ok(FileRemote(file.to_owned())), notify)
    }

    /// `section_with` for a password-protected section, under its `key`.
    pub fn section_unlocked_with<R: Remote + 'static>(
        &self,
        path: &str,
        key: &protected::Key,
        connect: impl FnMut(&Path) -> io::Result<R> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Section> {
        self.open_section(path, Some(key), connect, notify)
    }

    fn open_section<R: Remote + 'static>(
        &self,
        path: &str,
        key: Option<&protected::Key>,
        connect: impl FnMut(&Path) -> io::Result<R> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Section> {
        let section = self.section_path(path)?;
        let (path, replicas) = (section.path.clone(), self.replica_folder(section));
        let Some(root) = &self.root else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Sections on a share open through Section::resume_smb",
            )
            .into());
        };
        // A section replaced by a link out of the notebook is not the catalog's section.
        let file = fs::canonicalize(root.join(path))?;
        if !file.starts_with(root) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
        }
        // A replica the catalog's identity names resumes without reading the file, which its
        // worker checks next, as a share's does.
        if let discover::SectionState::Readable { document, .. } = &section.state {
            let cache = replica_file(&replicas, document);
            if fs::metadata(&cache).is_ok() {
                let (replica, remote, mut connect) =
                    (Replica::open(&cache)?, file.clone(), connect);
                return Section::start(file, replica, move || connect(&remote), notify);
            }
        }
        let replica = |identity: &[u8; 16], _: &[u8]| {
            fs::create_dir_all(&replicas)?;
            Ok(replica_file(&replicas, identity))
        };
        Section::open_in(file, key, replica, connect, notify)
    }
}

/// A section or group as its folder holds it, for the structure operations.
struct Entry {
    filename: String,
    /// What its folder's TOC lists it under, if anything.
    identity: Option<[u8; 16]>,
    /// A copy of another section of the notebook (`discover::Section::copy`). Its header names
    /// the other's file, so it is never placed or listed anew: OneNote lists a copy it finds
    /// under an identity of its own, leaving the file as it is.
    copy: bool,
}

impl Entry {
    /// `section` of the folder `parent`: a copy has only the entry its folder's TOC lists
    /// under its name.
    fn of(parent: &discover::Folder, section: &discover::Section) -> Self {
        let filename = split(&section.path).1.to_owned();
        let identity = if section.copy {
            parent
                .toc
                .iter()
                .flat_map(|toc| &toc.unresolved)
                .find(|entry| {
                    entry
                        .filename
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&filename))
                })
                .map(|entry| entry.file)
        } else {
            Some(section.file_id)
        };
        Self {
            filename,
            identity,
            copy: section.copy,
        }
    }
}

/// A page as a section file stores it.
pub struct StoredPage {
    pub space: ExGuid,
    pub page: Page,
    /// The page's `LastModifiedTime`, Time32 seconds since 1980.
    pub modified: Option<u32>,
    /// Who changed it last: the `AuthorMostRecent` of its latest modified object.
    pub author: Option<String>,
}

/// The pages of the section file `image` holds, in section order; pages the model cannot
/// build are left out.
pub fn stored_pages(image: &[u8]) -> Result<Vec<StoredPage>> {
    let store = Store::parse(image)?;
    let index = RevisionIndex::parse(&store)?;
    pages_of(&Document::parse(&index)?)
}

/// `stored_pages` of a password-protected section, under its `key`.
pub fn stored_pages_unlocked(image: &[u8], key: &protected::Key) -> Result<Vec<StoredPage>> {
    let store = Store::parse(image)?;
    let index = RevisionIndex::parse(&store)?;
    let unlocked = protected::UnlockedSection::unlock(&index, key, Default::default())?;
    pages_of(&unlocked.document()?)
}

fn pages_of(document: &Document<'_>) -> Result<Vec<StoredPage>> {
    Ok(document
        .pages()?
        .into_iter()
        .filter_map(|(space, id)| {
            let revision = document.active(space).ok()?;
            let latest = (revision.nodes.values())
                .filter(|node| node.latest_author.is_some())
                .max_by_key(|node| node.modified);
            let author = latest
                .and_then(|node| revision.nodes.get(&node.latest_author?))
                .and_then(|node| match &node.kind {
                    onestore::document::Kind::Author { name } => name.clone(),
                    _ => None,
                });
            Some(StoredPage {
                space,
                page: Page::from_revision(revision, id).ok()?,
                modified: revision.nodes.get(&id).and_then(|node| node.modified),
                author,
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

/// The edits putting the pages `moved` where `order` lists them, with their levels, in a
/// section now listing `listed`: each goes before the next page of `order` that stays put
/// and is still listed. Pages gone from the section are left out.
pub fn arrange(
    listed: &[(ExGuid, String, u32)],
    order: &[(ExGuid, u32)],
    moved: &[ExGuid],
) -> Result<Vec<PageEdit>> {
    let present = |space: &ExGuid| listed.iter().any(|(listed, ..)| listed == space);
    let mut edits = Vec::new();
    for (index, (space, level)) in order.iter().enumerate() {
        if !moved.contains(space) || !present(space) {
            continue;
        }
        let before = order[index + 1..]
            .iter()
            .map(|(space, _)| *space)
            .find(|space| !moved.contains(space) && present(space));
        edits.push(PageEdit::move_to(*space, before, *level)?);
    }
    Ok(edits)
}

/// Whether `page` holds nothing but an empty title, as the page a section left without
/// pages gains does.
pub fn blank(page: &Page) -> bool {
    page.title.trim().is_empty()
        && page
            .objects
            .iter()
            .all(|object| matches!(object, onestore::page::PageObject::Title(_)))
}

fn component(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\0']) && name != "." && name != ".."
}

/// Whether the TOC `image` holds has an entry for the file identity `file`.
pub(crate) fn lists(image: &[u8], file: [u8; 16]) -> Result<bool> {
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

/// Where the cache keeps what reading the catalog of the notebook at `location` took.
pub(crate) fn listing(cache: &Path, location: &str) -> PathBuf {
    let name: String = <sha2::Sha256 as sha2::Digest>::digest(location)[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    cache.join("listings").join(format!("{name}.json"))
}

/// Whether `name` can name a folder on every system a notebook's readers use, Windows's
/// included.
fn folder_name(name: &str) -> bool {
    component(name)
        && !name.contains(['<', '>', ':', '"', '|', '?', '*'])
        && !name.chars().any(char::is_control)
        && !name.ends_with(['.', ' '])
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

/// The location keying the replica of `section` in the notebook at `notebook`: the notebook's,
/// so that a section renamed or moved within it keeps its replica, or for a copy of another
/// section of the notebook its own file's.
fn replica_location(notebook: &str, section: &discover::Section) -> String {
    if section.copy {
        format!("{notebook}/{}", section.path)
    } else {
        notebook.to_owned()
    }
}

/// Deletes the closed replica at `replica`, with the files SQLite keeps beside it.
fn remove_replica(replica: &Path) -> io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = replica.as_os_str().to_owned();
        file.push(suffix);
        match fs::remove_file(file) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// The replica in `cache` of the section `identity` names.
pub(crate) fn replica_file(cache: &Path, identity: &[u8; 16]) -> PathBuf {
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

/// What a `SyncStatus` comes to for the reader, from the best to the worst.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncState {
    UpToDate,
    /// Not reached yet, or edits wait.
    Syncing,
    /// Another client holds the section file.
    InUse,
    /// The file or its server cannot be reached; edits wait until it can.
    NotConnected,
    /// The file cannot be written where it is stored.
    ReadOnly,
    /// The section is password protected, which Snowbound cannot open.
    Protected,
    /// The file is stably not a section Snowbound can read.
    Unreadable,
    Failed,
}

impl SyncStatus {
    pub fn state(&self) -> SyncState {
        use io::ErrorKind::*;
        match self.error.as_ref().map(io::Error::kind) {
            Some(PermissionDenied | ReadOnlyFilesystem) => SyncState::ReadOnly,
            Some(WouldBlock | ResourceBusy) => SyncState::InUse,
            Some(Unsupported) => SyncState::Protected,
            Some(InvalidData) => SyncState::Unreadable,
            Some(
                NotFound | ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected
                | TimedOut | HostUnreachable | NetworkUnreachable | NetworkDown | BrokenPipe
                | AddrNotAvailable,
            ) => SyncState::NotConnected,
            Some(_) => SyncState::Failed,
            None if self.synced.is_none() || self.queued > 0 => SyncState::Syncing,
            None => SyncState::UpToDate,
        }
    }
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
    /// Opens the lone section file through a replica in `cache`, in the file's
    /// `location::folder`, creating the replica from the file on first use and converting an
    /// older one. `notify` runs on a background thread whenever an event is available.
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
        connect: impl FnMut(&Path) -> io::Result<R> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let file = fs::canonicalize(file)?;
        let location = file.to_string_lossy().into_owned();
        let replicas = |identity: &[u8; 16], source: &[u8]| {
            let folder = crate::location::claim(cache.as_ref(), &location, &[*identity], |_| {
                Stamp::of(source).ok()
            })?;
            Ok(replica_file(&folder, identity))
        };
        Self::open_in(file, None, replicas, connect, notify)
    }

    /// Opens the canonical section `file`, a protected one under `key`, through the replica
    /// `replica` names for its document identity and image.
    fn open_in<R: Remote + 'static>(
        file: PathBuf,
        key: Option<&protected::Key>,
        replica: impl FnOnce(&[u8; 16], &[u8]) -> Result<PathBuf>,
        mut connect: impl FnMut(&Path) -> io::Result<R> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let source = connect(&file)?.read()?;
        let store = Store::parse(&source)?;
        let identity = RevisionIndex::parse(&store)?.root;
        let cache = replica(&identity.guid, &source)?;
        let replica = Replica::open_or_create(&cache, key, || Ok(source))?;
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
        let file = fs::absolute(file)?;
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

    /// Resumes a replica against a section a Live Share host serves at catalog `path`.
    #[cfg(feature = "live")]
    pub fn resume_hosted(
        path: String,
        replica: Replica,
        guest: Arc<crate::live::share::Guest>,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        Self::start(
            PathBuf::from(&path),
            replica,
            move || Ok(crate::live::share::HostedRemote::new(&guest, &path)),
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

    /// See [`Replica::written`].
    pub fn written(&self) -> Result<()> {
        self.replica.written()
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

    /// Publishes local edits once `pause` passes without another (`SyncWorker::set_pause`),
    /// as a notebook on a cloud drive does; `wake` publishes them at once.
    pub fn set_pause(&self, pause: Duration) {
        if let Some(worker) = &self.worker {
            worker.set_pause(pause);
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
        fs::read_file(&self.0)
    }

    /// Read without the whole-file lock, which would block OneNote's readers: a change
    /// detector, never a snapshot to edit.
    fn stamp(&mut self) -> io::Result<Stamp> {
        use std::io::Read;
        let mut file = fs::File::open(&self.0)?;
        let mut header = [0; 1024];
        file.read_exact(&mut header)?;
        let length = file.metadata()?.len();
        Ok(Stamp { header, length })
    }

    fn publish(&mut self, transaction: &Transaction) -> std::result::Result<(), CommitError> {
        fs::commit_file(transaction, &self.0)
    }

    fn confirm(&mut self, base: &Stamp) -> std::result::Result<(), CommitError> {
        fs::confirm_file(&self.0, base)
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        drop(self.worker.take());
    }
}

#[cfg(test)]
mod tests;
