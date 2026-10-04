//! The browser's file system: std's calls over files held in memory, which the host loads
//! before anything opens (`restore`) and writes out as they change, by the byte ranges that
//! changed (`changes`). SQLite reaches the same files through `sqlite`, its default VFS here,
//! so a replica is a file like any other. One thread: no file is ever locked.

use onestore::{CommitError, CommitIo, CommitState, Stamp, Transaction};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
    ops::Range,
    path::{Component, Path, PathBuf},
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod sqlite;
pub(crate) use sqlite::install as install_sqlite;

/// A file's bytes and when they last changed, in milliseconds since 1970.
struct Data {
    bytes: Vec<u8>,
    modified: f64,
    /// Where the file is, which a rename moves; none once it is removed.
    path: Option<PathBuf>,
    /// The ranges written since the host last wrote the file out, or none for all of it.
    unwritten: Option<Vec<Range<usize>>>,
}

/// More ranges than this write the whole file out instead.
const RANGES: usize = 64;

impl Data {
    fn flush(&mut self) {
        if let Some(path) = self.path.clone() {
            with(|files| {
                if files.changed.remove(&path) {
                    files.durable.push((path, self.change()));
                }
            });
        }
    }

    fn change(&mut self) -> Change {
        let length = self.bytes.len();
        let ranges = self
            .unwritten
            .replace(Vec::new())
            .unwrap_or_else(|| std::iter::once(0..length).collect());
        Change::File {
            length: length as u64,
            ranges: ranges
                .into_iter()
                .map(|range| range.start.min(length)..range.end.min(length))
                .filter(|range| !range.is_empty())
                .map(|range| (range.start as u64, self.bytes[range].to_vec()))
                .collect(),
        }
    }
    fn new(
        bytes: Vec<u8>,
        modified: f64,
        path: PathBuf,
        unwritten: Option<Vec<Range<usize>>>,
    ) -> Shared {
        Rc::new(RefCell::new(Self {
            bytes,
            modified,
            path: Some(path),
            unwritten,
        }))
    }

    /// Notes `range` written now, merged into a written range it meets.
    fn wrote(&mut self, range: Range<usize>) {
        self.modified = now();
        if let Some(ranges) = &mut self.unwritten {
            match ranges
                .iter_mut()
                .find(|known| known.start <= range.end && range.start <= known.end)
            {
                Some(known) => *known = known.start.min(range.start)..known.end.max(range.end),
                None => ranges.push(range),
            }
            if ranges.len() > RANGES {
                self.unwritten = None;
            }
        }
        if let Some(path) = self.path.clone() {
            with(|files| files.changed.insert(path));
        }
    }
}

type Shared = Rc<RefCell<Data>>;

enum Node {
    Directory,
    File(Shared),
}

#[derive(Default)]
struct Files {
    nodes: BTreeMap<PathBuf, Node>,
    /// Paths written, made or removed since the host last asked.
    changed: BTreeSet<PathBuf>,
    /// Flushes and removals in order, including SQLite's WAL before its database checkpoint.
    durable: Vec<(PathBuf, Change)>,
    /// Folders the host mirrors from elsewhere (`mount`).
    mounts: BTreeSet<PathBuf>,
    /// Images committed under a mount, waiting for the host to write them (`committed`).
    committed: BTreeMap<PathBuf, Vec<u8>>,
}

thread_local! {
    static FILES: RefCell<Files> = RefCell::new(Files {
        nodes: BTreeMap::from([(PathBuf::from("/"), Node::Directory)]),
        ..Files::default()
    });
}

/// Mirrors the folder at `root` from somewhere only the host reaches, as a folder on the
/// user's disk. A commit to a section there goes to the host (`committed`) and stands only
/// once the host has written it and put the file back (`restore`): until then it is uncertain,
/// as a commit whose answer was lost, so an edit counts as published only once it is on disk.
pub fn mount(root: impl AsRef<Path>) {
    let root = normal(root.as_ref());
    with(|files| files.mounts.insert(root));
}

/// The images committed under mounts since the last call, latest per file.
pub fn committed() -> Vec<(PathBuf, Vec<u8>)> {
    with(|files| std::mem::take(&mut files.committed).into_iter().collect())
}

/// What a path holds, as the host keeps it.
pub enum Saved {
    Directory,
    /// The bytes, and when they last changed in milliseconds since 1970.
    File(Vec<u8>, f64),
    /// Nothing: what was there went, with what it held.
    Gone,
}

/// Puts what the host kept at `path` back, as it was before the page last closed or as it
/// now stands where it is kept.
pub fn restore(path: impl AsRef<Path>, saved: Saved) {
    let path = normal(path.as_ref());
    FILES.with_borrow_mut(|files| {
        let node = match (saved, files.nodes.get(&path)) {
            (Saved::Gone, _) => {
                let gone: Vec<PathBuf> = files
                    .nodes
                    .range(path.clone()..)
                    .take_while(|(each, _)| each.starts_with(&path))
                    .map(|(each, _)| each.clone())
                    .collect();
                for each in gone {
                    if let Some(Node::File(data)) = files.nodes.remove(&each) {
                        data.borrow_mut().path = None;
                    }
                }
                return;
            }
            (Saved::Directory, _) => Node::Directory,
            (Saved::File(bytes, modified), Some(Node::File(data))) => {
                let mut held = data.borrow_mut();
                held.bytes = bytes;
                held.modified = modified;
                held.unwritten = Some(Vec::new());
                return;
            }
            (Saved::File(bytes, modified), _) => {
                Node::File(Data::new(bytes, modified, path.clone(), Some(Vec::new())))
            }
        };
        files.nodes.insert(path, node);
    });
}

/// How a path changed, as the host writes it out.
pub enum Change {
    Removed,
    Directory,
    /// The file's length, and the ranges that changed with their bytes.
    File {
        length: u64,
        ranges: Vec<(u64, Vec<u8>)>,
    },
}

/// Whether any path changed since `changes` was last called.
pub fn changed() -> bool {
    FILES.with_borrow(|files| {
        !files.durable.is_empty() || !files.changed.is_empty() || !files.committed.is_empty()
    })
}

/// Ordered file flushes, followed by changes not yet flushed.
pub fn changes() -> Vec<(PathBuf, Change)> {
    FILES.with_borrow_mut(|files| {
        let mut changes = std::mem::take(&mut files.durable);
        changes.extend(std::mem::take(&mut files.changed).into_iter().map(|path| {
            let change = match files.nodes.get(&path) {
                None => Change::Removed,
                Some(Node::Directory) => Change::Directory,
                Some(Node::File(data)) => data.borrow_mut().change(),
            };
            (path, change)
        }));
        changes
    })
}

fn now() -> f64 {
    js_sys::Date::now()
}

/// `path` absolute, without `.` or `..`.
fn normal(path: &Path) -> PathBuf {
    let mut normal = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::Normal(name) => normal.push(name),
            Component::ParentDir => {
                normal.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    normal
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(ErrorKind::NotFound, format!("{} not found", path.display()))
}

impl Files {
    fn file(&self, path: &Path) -> io::Result<Shared> {
        match self.nodes.get(path) {
            Some(Node::File(data)) => Ok(Rc::clone(data)),
            Some(Node::Directory) => Err(ErrorKind::IsADirectory.into()),
            None => Err(not_found(path)),
        }
    }

    fn parent_exists(&self, path: &Path) -> io::Result<()> {
        match path.parent().map(|parent| self.nodes.get(parent)) {
            None | Some(Some(Node::Directory)) => Ok(()),
            Some(Some(Node::File(_))) => Err(ErrorKind::NotADirectory.into()),
            Some(None) => Err(not_found(path.parent().unwrap_or(path))),
        }
    }

    /// A new empty file at `path`, whose folder exists.
    fn create(&mut self, path: PathBuf) -> io::Result<Shared> {
        self.parent_exists(&path)?;
        if self.nodes.contains_key(&path) {
            return Err(ErrorKind::AlreadyExists.into());
        }
        let data = Data::new(Vec::new(), now(), path.clone(), None);
        self.nodes
            .insert(path.clone(), Node::File(Rc::clone(&data)));
        self.changed.insert(path);
        Ok(data)
    }

    fn children(&self, folder: &Path) -> impl Iterator<Item = (&PathBuf, &Node)> {
        self.nodes
            .range(folder.to_path_buf()..)
            .skip(1)
            .take_while(move |(path, _)| path.starts_with(folder))
            .filter(move |(path, _)| path.parent() == Some(folder))
    }
}

fn with<T>(act: impl FnOnce(&mut Files) -> T) -> T {
    FILES.with_borrow_mut(act)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType(bool);

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.0
    }

    pub fn is_file(&self) -> bool {
        !self.0
    }

    pub fn is_symlink(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct Metadata {
    directory: bool,
    length: u64,
    modified: f64,
}

impl Metadata {
    pub fn is_dir(&self) -> bool {
        self.directory
    }

    pub fn is_file(&self) -> bool {
        !self.directory
    }

    pub fn file_type(&self) -> FileType {
        FileType(self.directory)
    }

    #[allow(clippy::len_without_is_empty, reason = "std's Metadata has none")]
    pub fn len(&self) -> u64 {
        self.length
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(UNIX_EPOCH + Duration::from_secs_f64(self.modified.max(0.0) / 1e3))
    }
}

fn metadata_of(node: &Node) -> Metadata {
    match node {
        Node::Directory => Metadata {
            directory: true,
            length: 0,
            modified: 0.0,
        },
        Node::File(data) => {
            let data = data.borrow();
            Metadata {
                directory: false,
                length: data.bytes.len() as u64,
                modified: data.modified,
            }
        }
    }
}

pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let path = normal(path.as_ref());
    with(|files| {
        files
            .nodes
            .get(&path)
            .map(metadata_of)
            .ok_or_else(|| not_found(&path))
    })
}

pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    metadata(path)
}

pub fn exists(path: impl AsRef<Path>) -> io::Result<bool> {
    let path = normal(path.as_ref());
    Ok(with(|files| files.nodes.contains_key(&path)))
}

/// This page's number among the processes that may share its files, as other tabs do.
pub fn process_id() -> u32 {
    use std::sync::OnceLock;
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| {
        let mut bytes = [0; 4];
        let _ = getrandom::fill(&mut bytes);
        u32::from_ne_bytes(bytes)
    })
}

/// `path` from the root, as every path here is.
pub fn absolute(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    Ok(normal(path.as_ref()))
}

pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = normal(path.as_ref());
    match with(|files| files.nodes.contains_key(&path)) {
        true => Ok(path),
        false => Err(not_found(&path)),
    }
}

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = normal(path.as_ref());
    let data = with(|files| files.file(&path))?;
    Ok(data.borrow().bytes.clone())
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
}

pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = normal(path.as_ref());
    let data = with(|files| files.file(&path).or_else(|_| files.create(path.clone())))?;
    let mut data = data.borrow_mut();
    data.bytes = contents.as_ref().to_vec();
    let length = data.bytes.len();
    data.wrote(0..length);
    Ok(())
}

pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let bytes = read(from)?;
    write(to, &bytes)?;
    Ok(bytes.len() as u64)
}

/// A copy, as nothing here writes a file under two names at once: a link is made to put a
/// finished file in place, then the first name removed.
pub fn hard_link(original: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    let bytes = read(original)?;
    let link = normal(link.as_ref());
    let data = with(|files| files.create(link))?;
    data.borrow_mut().bytes = bytes;
    Ok(())
}

pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = normal(path.as_ref());
    with(|files| {
        files.parent_exists(&path)?;
        if files.nodes.contains_key(&path) {
            return Err(ErrorKind::AlreadyExists.into());
        }
        files.nodes.insert(path.clone(), Node::Directory);
        files.changed.insert(path);
        Ok(())
    })
}

pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = normal(path.as_ref());
    for folder in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match with(|files| {
            files
                .nodes
                .get(folder)
                .map(|node| matches!(node, Node::Directory))
        }) {
            Some(true) => {}
            Some(false) => return Err(ErrorKind::NotADirectory.into()),
            None => create_dir(folder)?,
        }
    }
    Ok(())
}

pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let path = normal(path.as_ref());
    with(|files| {
        let data = files.file(&path)?;
        let mut data = data.borrow_mut();
        if files.changed.remove(&path) {
            files.durable.push((path.clone(), data.change()));
        }
        data.path = None;
        files.nodes.remove(&path);
        files.durable.push((path, Change::Removed));
        Ok(())
    })
}

pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = normal(path.as_ref());
    with(|files| {
        match files.nodes.get(&path) {
            Some(Node::Directory) => {}
            Some(Node::File(_)) => return Err(ErrorKind::NotADirectory.into()),
            None => return Err(not_found(&path)),
        }
        if files.children(&path).next().is_some() {
            return Err(ErrorKind::DirectoryNotEmpty.into());
        }
        files.nodes.remove(&path);
        files.changed.insert(path);
        Ok(())
    })
}

pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = normal(path.as_ref());
    with(|files| {
        let gone: Vec<PathBuf> = files
            .nodes
            .range(path.clone()..)
            .take_while(|(each, _)| each.starts_with(&path))
            .map(|(each, _)| each.clone())
            .collect();
        if gone.is_empty() {
            return Err(not_found(&path));
        }
        for each in gone {
            if let Some(Node::File(data)) = files.nodes.remove(&each) {
                data.borrow_mut().path = None;
            }
            files.changed.insert(each);
        }
        Ok(())
    })
}

/// Moves a file or folder, replacing a file at `to`, as a POSIX rename does.
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (from, to) = (normal(from.as_ref()), normal(to.as_ref()));
    with(|files| {
        if !files.nodes.contains_key(&from) {
            return Err(not_found(&from));
        }
        files.parent_exists(&to)?;
        if from == to {
            return Ok(());
        }
        match files.nodes.get(&to) {
            Some(Node::Directory) if files.children(&to).next().is_some() => {
                return Err(ErrorKind::DirectoryNotEmpty.into());
            }
            Some(Node::File(data)) => data.borrow_mut().path = None,
            _ => {}
        }
        let moved: Vec<PathBuf> = files
            .nodes
            .range(from.clone()..)
            .take_while(|(each, _)| each.starts_with(&from))
            .map(|(each, _)| each.clone())
            .collect();
        for each in moved {
            let Some(node) = files.nodes.remove(&each) else {
                continue;
            };
            let target = to.join(each.strip_prefix(&from).unwrap_or(Path::new("")));
            if let Node::File(data) = &node {
                let mut data = data.borrow_mut();
                data.path = Some(target.clone());
                data.unwritten = None;
            }
            files.changed.insert(each);
            files.changed.insert(target.clone());
            files.nodes.insert(target, node);
        }
        Ok(())
    })
}

pub struct DirEntry {
    path: PathBuf,
    metadata: Metadata,
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn file_name(&self) -> OsString {
        self.path.file_name().unwrap_or_default().to_owned()
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(self.metadata.file_type())
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        Ok(self.metadata.clone())
    }
}

pub struct ReadDir(std::vec::IntoIter<DirEntry>);

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(Ok)
    }
}

pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    let path = normal(path.as_ref());
    with(|files| {
        match files.nodes.get(&path) {
            Some(Node::Directory) => {}
            Some(Node::File(_)) => return Err(ErrorKind::NotADirectory.into()),
            None => return Err(not_found(&path)),
        }
        let entries: Vec<DirEntry> = files
            .children(&path)
            .map(|(child, node)| DirEntry {
                path: child.clone(),
                metadata: metadata_of(node),
            })
            .collect();
        Ok(ReadDir(entries.into_iter()))
    })
}

#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    pub fn open(&self, path: impl AsRef<Path>) -> io::Result<File> {
        let path = normal(path.as_ref());
        let data = with(|files| match files.file(&path) {
            Ok(_) if self.create_new => Err(ErrorKind::AlreadyExists.into()),
            Err(error)
                if error.kind() == ErrorKind::NotFound && (self.create || self.create_new) =>
            {
                files.create(path.clone())
            }
            found => found,
        })?;
        if self.truncate && self.write {
            let mut data = data.borrow_mut();
            data.bytes.clear();
            data.wrote(0..0);
        }
        Ok(File {
            data,
            position: 0,
            append: self.append,
        })
    }
}

/// An open file, which reads and writes the file under whatever name it has since.
pub struct File {
    data: Shared,
    position: u64,
    append: bool,
}

impl File {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        OpenOptions::new().read(true).open(path)
    }

    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    }

    pub fn create_new(path: impl AsRef<Path>) -> io::Result<Self> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        Ok(metadata_of(&Node::File(Rc::clone(&self.data))))
    }

    pub fn sync_all(&self) -> io::Result<()> {
        self.data.borrow_mut().flush();
        Ok(())
    }

    pub fn sync_data(&self) -> io::Result<()> {
        self.sync_all()
    }

    pub fn set_len(&self, size: u64) -> io::Result<()> {
        let mut data = self.data.borrow_mut();
        let old = data.bytes.len();
        data.bytes.resize(size as usize, 0);
        data.wrote(old.min(size as usize)..size as usize);
        Ok(())
    }

    fn read_from(&self, offset: u64, output: &mut [u8]) -> usize {
        let data = self.data.borrow();
        let rest = data.bytes.get(offset as usize..).unwrap_or_default();
        let count = rest.len().min(output.len());
        output[..count].copy_from_slice(&rest[..count]);
        count
    }

    fn write_to(&self, offset: u64, input: &[u8]) {
        let mut data = self.data.borrow_mut();
        let end = offset as usize + input.len();
        if data.bytes.len() < end {
            data.bytes.resize(end, 0);
        }
        data.bytes[offset as usize..end].copy_from_slice(input);
        data.wrote(offset as usize..end);
    }
}

impl Read for File {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.read_from(self.position, output);
        self.position += count as u64;
        Ok(count)
    }
}

impl Write for File {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if self.append {
            self.position = self.data.borrow().bytes.len() as u64;
        }
        self.write_to(self.position, input);
        self.position += input.len() as u64;
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for File {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let length = self.data.borrow().bytes.len() as i128;
        let target = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => length + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        self.position = u64::try_from(target).map_err(|_| ErrorKind::InvalidInput)?;
        Ok(self.position)
    }
}

impl CommitIo for File {
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> io::Result<usize> {
        Ok(self.read_from(offset, bytes))
    }

    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        self.write_to(offset, bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `onestore::read_file`: a read here is whole, as nothing commits while it runs.
pub fn read_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    read(path)
}

pub fn read_file_limited(path: impl AsRef<Path>, limit: usize) -> io::Result<Vec<u8>> {
    let bytes = read(path)?;
    if bytes.len() > limit {
        return Err(ErrorKind::FileTooLarge.into());
    }
    Ok(bytes)
}

fn writable(path: impl AsRef<Path>) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

fn not_committed(error: io::Error) -> CommitError {
    CommitError {
        state: CommitState::NotCommitted,
        error,
    }
}

fn mounted(path: &Path) -> bool {
    with(|files| files.mounts.iter().any(|root| path.starts_with(root)))
}

pub fn commit_file(transaction: &Transaction, path: impl AsRef<Path>) -> Result<(), CommitError> {
    let path = normal(path.as_ref());
    if !mounted(&path) {
        return transaction.commit(&mut writable(&path).map_err(not_committed)?);
    }
    let mut image = File {
        data: Rc::new(RefCell::new(Data {
            bytes: read(&path).map_err(not_committed)?,
            modified: now(),
            path: None,
            unwritten: None,
        })),
        position: 0,
        append: false,
    };
    transaction.commit(&mut image)?;
    let image = std::mem::take(&mut image.data.borrow_mut().bytes);
    with(|files| files.committed.insert(path, image));
    Err(CommitError {
        state: CommitState::Unknown,
        error: io::Error::new(ErrorKind::WouldBlock, "Writing the section to its folder"),
    })
}

/// A mounted file is durable as the host puts it back, and keeps the version its commit wrote.
pub fn confirm_file(path: impl AsRef<Path>, base: &Stamp) -> Result<(), CommitError> {
    let mut file = writable(&path).map_err(not_committed)?;
    if mounted(&normal(path.as_ref())) {
        return base.check(&mut file).map_err(not_committed);
    }
    onestore::confirm(&mut file, base)
}

pub fn place_file(path: impl AsRef<Path>, ancestor: [u8; 16], name: &str) -> io::Result<()> {
    onestore::place(&mut writable(path)?, ancestor, name)
}

/// Puts the file at `with` in the place of the file at `path`, provided `path` still has
/// `base`'s stamp.
pub fn supersede_file(
    path: impl AsRef<Path>,
    base: &Stamp,
    with: impl AsRef<Path>,
) -> Result<(), CommitError> {
    base.check(&mut writable(&path).map_err(not_committed)?)
        .map_err(not_committed)?;
    rename(with, path).map_err(not_committed)
}

/// Waits until the browser host has durably written every ordered flush handed to it.
pub async fn durable() -> io::Result<()> {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(catch, js_namespace = globalThis, js_name = snowboundFlushStorage)]
        fn flush_storage() -> Result<js_sys::Promise, JsValue>;
    }
    let failure = |error: JsValue| {
        io::Error::other(
            error
                .as_string()
                .unwrap_or_else(|| "Browser storage failed".into()),
        )
    };
    wasm_bindgen_futures::JsFuture::from(flush_storage().map_err(failure)?)
        .await
        .map_err(failure)?;
    Ok(())
}
