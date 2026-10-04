use super::*;
use std::{future::Future, pin::Pin};

pub(super) enum Outcome {
    Read(Vec<u8>),
    Stamp(Box<Stamp>),
    Published,
}

pub(super) enum Pending {
    Running(Pin<Box<dyn Future<Output = std::result::Result<Outcome, CommitError>>>>),
    Ready(std::result::Result<Outcome, CommitError>),
}

fn uncommitted(error: io::Error) -> CommitError {
    CommitError {
        state: CommitState::NotCommitted,
        error,
    }
}

impl HostedRemote {
    fn operation(
        &mut self,
        work: impl Future<Output = std::result::Result<Outcome, CommitError>> + 'static,
    ) -> std::result::Result<Outcome, CommitError> {
        match self.pending.take() {
            Some(Pending::Ready(result)) => result,
            pending => {
                self.pending = Some(pending.unwrap_or_else(|| Pending::Running(Box::pin(work))));
                Err(uncommitted(io::ErrorKind::WouldBlock.into()))
            }
        }
    }

    fn publication(
        &mut self,
        result: std::result::Result<Outcome, CommitError>,
    ) -> std::result::Result<(), CommitError> {
        match result {
            Ok(Outcome::Published) => {
                self.seen = None;
                Ok(())
            }
            Ok(_) => Err(uncommitted(io::ErrorKind::InvalidInput.into())),
            Err(error) => {
                if error.error.kind() != io::ErrorKind::WouldBlock
                    && error.state == CommitState::NotCommitted
                {
                    self.rejected = true;
                    self.guest.inner.current.lock().unwrap().remove(&self.path);
                }
                Err(error)
            }
        }
    }
}

impl crate::Remote for HostedRemote {
    fn pending(&mut self) -> Option<Pin<Box<dyn Future<Output = ()> + '_>>> {
        let pending = self.pending.as_mut()?;
        Some(Box::pin(async move {
            if let Pending::Running(work) = pending {
                *pending = Pending::Ready(work.await);
            }
        }))
    }

    fn read(&mut self) -> io::Result<Vec<u8>> {
        let (guest, path, seen) = (self.guest.clone(), self.path.clone(), self.seen.clone());
        let result = self
            .operation(async move {
                let image = match seen.as_ref().and_then(|stamp| guest.held(&path, stamp)) {
                    Some(image) => image,
                    None => guest
                        .read_async(kind::READ, &path, LIMIT)
                        .await
                        .map_err(uncommitted)?,
                };
                Ok(Outcome::Read(image))
            })
            .map_err(|error| error.error)?;
        match result {
            Outcome::Read(image) => {
                self.rejected = false;
                Ok(image)
            }
            _ => Err(io::ErrorKind::InvalidInput.into()),
        }
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        let (guest, path) = (self.guest.clone(), self.path.clone());
        let result = self
            .operation(async move {
                guest
                    .stamp_async(&path)
                    .await
                    .map(|stamp| Outcome::Stamp(Box::new(stamp)))
                    .map_err(uncommitted)
            })
            .map_err(|error| error.error)?;
        match result {
            Outcome::Stamp(stamp) => {
                self.seen = Some((*stamp).clone());
                Ok(*stamp)
            }
            _ => Err(io::ErrorKind::InvalidInput.into()),
        }
    }

    fn accepts_edits(&self) -> bool {
        !self.rejected
            && self
                .guest
                .host()
                .is_some_and(|host| host.ops == Some(1) && host.kinds.contains(&kind::EDITS))
    }

    fn publish_edits(
        &mut self,
        transaction: &Transaction,
        edits: &[crate::PendingEdit],
        revisions: &BTreeMap<onestore::ExGuid, onestore::ExGuid>,
    ) -> std::result::Result<(), CommitError> {
        let (guest, path, transaction, edits, revisions) = (
            self.guest.clone(),
            self.path.clone(),
            transaction.clone(),
            edits.to_vec(),
            revisions.clone(),
        );
        let result = self.operation(async move {
            guest
                .edits_async(&path, &transaction, &edits, &revisions)
                .await?;
            Ok(Outcome::Published)
        });
        self.publication(result)
    }

    fn publish(&mut self, transaction: &Transaction) -> std::result::Result<(), CommitError> {
        let (guest, path, transaction) =
            (self.guest.clone(), self.path.clone(), transaction.clone());
        let result = self.operation(async move {
            guest.commit_async(&path, &transaction).await?;
            guest.inner.published(&path, &transaction);
            Ok(Outcome::Published)
        });
        self.publication(result)
    }

    fn confirm(&mut self, base: &Stamp) -> std::result::Result<(), CommitError> {
        let (guest, path, base) = (self.guest.clone(), self.path.clone(), base.clone());
        let result = self.operation(async move {
            guest.confirm_async(&path, &base).await?;
            Ok(Outcome::Published)
        });
        self.publication(result)
    }
}

struct Cached<'a>(&'a Hosted, Listings);

impl Guest {
    pub(super) async fn cache_entries(
        &self,
        folder: &str,
        entries: &[discover::Entry],
    ) -> io::Result<()> {
        let Some(listed) = self.inner.catalog.lock().unwrap().clone() else {
            return Ok(());
        };
        let files = listed.with_extension("files");
        let mut listings: Listings = crate::fs::read(&listed)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let before = listings.get(folder).cloned().unwrap_or_default();
        let mut current = Vec::with_capacity(entries.len());
        crate::fs::create_dir_all(files.join(folder))?;
        for entry in entries {
            if entry.name.contains(['/', '\\', '\0']) || matches!(entry.name.as_str(), "." | "..") {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let e = wire_entry(entry);
            let listed = (e.name, e.kind, e.size, e.modified);
            let path = if folder.is_empty() {
                entry.name.clone()
            } else {
                format!("{folder}/{}", entry.name)
            };
            if entry.kind == discover::EntryKind::File
                && (!before.contains(&listed) || crate::fs::metadata(files.join(&path)).is_err())
            {
                let kind = if path.ends_with(".one") || path.ends_with(".onetoc2") {
                    kind::READ
                } else {
                    kind::READ_FILE
                };
                let bytes = self.read_async(kind, &path, LIMIT).await?;
                crate::fs::write(files.join(path), bytes)?;
            }
            current.push(listed);
        }
        // Other folders may have finished listing while these files were being read.
        if let Ok(bytes) = crate::fs::read(&listed) {
            listings = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        }
        if listings.get(folder) != Some(&current) {
            listings.insert(folder.to_owned(), current);
            crate::fs::write(&listed, serde_json::to_vec(&listings)?)?;
            crate::fs::durable().await?;
            if let Some(reports) = &*self.inner.watch.lock().unwrap() {
                reports.catalog(folder);
            }
            (self.inner.events)();
        }
        Ok(())
    }
}

impl discover::Source for Cached<'_> {
    fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<discover::Entry>> {
        let entries = self.1.get(path).ok_or(io::ErrorKind::NotConnected)?;
        if entries.len() > limit {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        Ok(entries
            .iter()
            .map(|(name, kind, size, modified)| {
                entry(&WireEntry {
                    name: name.clone(),
                    kind: *kind,
                    size: *size,
                    modified: *modified,
                })
            })
            .collect())
    }
    fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        let bytes = self
            .0
            .guest
            .inner
            .image(path)
            .map(|image| image.to_vec())
            .map(Ok)
            .unwrap_or_else(|| crate::fs::read(self.0.files().join(path)))?;
        if bytes.len() > limit {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        Ok(bytes)
    }
    fn read_asset(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.read(path, limit)
    }
}

impl Hosted {
    fn files(&self) -> PathBuf {
        self.listed.with_extension("files")
    }

    /// Loads a bounded catalog and its files before the synchronous editor opens it.
    pub async fn prepare(guest: Arc<Guest>, cache: &std::path::Path) -> io::Result<()> {
        let listed =
            crate::session::listing(cache, &guest.location()).with_extension("entries.json");
        let hosted = Self::new(guest, listed);
        let deadline = Instant::now() + Duration::from_secs(20);
        let (_, waiting) = crate::task::channel();
        while hosted.guest.host().is_none() {
            if Instant::now() >= deadline {
                return Err(hosted.guest.offline());
            }
            crate::task::wait(&waiting, Some(Duration::from_millis(50))).await;
        }
        let mut folders = vec![String::new()];
        let mut count = 0;
        while let Some(folder) = folders.pop() {
            if folder.split('/').count() > 32 {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            let entries = hosted.guest.entries_async(&folder).await?;
            count += entries.len();
            if count > 10000 {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            crate::fs::create_dir_all(hosted.files().join(&folder))?;
            for entry in &entries {
                if entry.kind == discover::EntryKind::Directory {
                    folders.push(if folder.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{folder}/{}", entry.name)
                    });
                }
            }
        }
        crate::fs::durable().await
    }
}

fn desktop() -> Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Create and organize sections in desktop Snowbound.",
    )
    .into()
}

impl Storage for Hosted {
    fn discover(
        &self,
        cache: &mut discover::Cache,
        limits: discover::Limits,
    ) -> Result<discover::Folder> {
        let listings =
            serde_json::from_slice(&crate::fs::read(&self.listed)?).map_err(io::Error::other)?;
        Ok(cache.discover(&mut Cached(self, listings), limits)?)
    }
    fn location(&self) -> String {
        self.guest.location()
    }
    fn entries(&self, path: &str) -> io::Result<Vec<discover::Entry>> {
        let listings =
            serde_json::from_slice(&crate::fs::read(&self.listed)?).map_err(io::Error::other)?;
        discover::Source::entries(&mut Cached(self, listings), path, 10000)
    }
    fn exists(&self, path: &str) -> bool {
        crate::fs::metadata(self.files().join(path)).is_ok()
    }
    fn stamp(&self, path: &str) -> io::Result<Stamp> {
        Stamp::of(&self.read(path).map_err(io::Error::other)?).map_err(io::Error::other)
    }
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.read_file(path, LIMIT)
    }
    fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>> {
        Ok(discover::Source::read(
            &mut Cached(self, Listings::new()),
            path,
            limit,
        )?)
    }
    fn create(&self, _: &str, _: &[u8]) -> Result<()> {
        Err(desktop())
    }
    fn create_directory(&self, _: &str) -> Result<()> {
        Err(desktop())
    }
    fn hide(&self, _: &str) -> Result<()> {
        Err(desktop())
    }
    fn rename(&self, _: &str, _: &str) -> Result<()> {
        Err(desktop())
    }
    fn rename_root(&self, _: &str, _: &[String]) -> Result<String> {
        Err(desktop())
    }
    fn replace(&self, _: &str, _: &str) -> Result<()> {
        Err(desktop())
    }
    fn delete(&self, _: &str) -> Result<()> {
        Err(desktop())
    }
    fn place(&self, _: &str, _: [u8; 16], _: &str) -> Result<()> {
        Err(desktop())
    }
    fn commit(&self, _: &str, _: &Transaction) -> Result<()> {
        Err(desktop())
    }
    fn confirm(&self, _: &str, _: &Stamp) -> std::result::Result<(), CommitError> {
        Err(uncommitted(io::Error::new(
            io::ErrorKind::Unsupported,
            "Section management requires desktop Snowbound",
        )))
    }
    fn supersede(&self, _: &str, _: &Stamp, _: &str) -> Result<()> {
        Err(desktop())
    }
}
