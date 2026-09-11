//! The application's view of a notebook: sections opened through a local replica that
//! publishes page saves to the section file in the background.

use crate::{
    ConflictKind, EditStatus, Error, PendingEdit, Remote, Replica, Result, SyncWorker, discover,
};
use onestore::{
    CommitError, ExGuid, PreparedEdit, RevisionIndex, Store, document::Document, page::Page,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::Duration,
};

/// A notebook directory and the cache directory holding its section replicas.
pub struct Notebook {
    root: PathBuf,
    cache: PathBuf,
    catalog: discover::Folder,
}

impl Notebook {
    pub fn open(root: impl AsRef<Path>, cache: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let cache = cache.as_ref().to_path_buf();
        std::fs::create_dir_all(&cache)?;
        let catalog = discover::discover(
            &mut discover::Local::open(&root)?,
            discover::Limits {
                entries: 100_000,
                bytes_per_file: 256 * 1024 * 1024,
                depth: 64,
            },
        )?;
        Ok(Self {
            root,
            cache,
            catalog,
        })
    }

    pub fn catalog(&self) -> &discover::Folder {
        &self.catalog
    }

    /// Opens a section by its catalog path.
    pub fn section(&self, path: &str, notify: impl Fn() + Send + 'static) -> Result<Section> {
        let mut folders = vec![&self.catalog];
        while let Some(folder) = folders.pop() {
            if folder.sections.iter().any(|section| section.path == path) {
                let file = self.root.join(path).canonicalize()?;
                if !file.starts_with(&self.root) {
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
                }
                return Section::open(file, &self.cache, notify);
            }
            folders.extend(&folder.groups);
        }
        Err(io::Error::from(io::ErrorKind::NotFound).into())
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
                    status: *status,
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
