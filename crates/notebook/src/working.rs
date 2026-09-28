//! The section thread: the only owner of the parsed section, which is the cached base
//! image with each sealed batch replayed and the open batch's edits applied. Edits apply
//! here and are written in one SQLite transaction per burst; the sync thread asks it to seal
//! and, when the remote changed, to rebase the queue.

use crate::{Result, base, lock, merge, queue, worker::Signal};
use onestore::{
    Arena, ExGuid, Section, Transaction,
    op::{Edit, OpError},
    page::Page,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    sync::{Arc, Mutex, Weak, mpsc},
    thread,
};

pub(crate) type Reply<T> = Box<dyn FnOnce(Result<T>) + Send>;

pub(crate) enum Request {
    Apply {
        author: String,
        edit: Edit,
        reply: Reply<u64>,
    },
    Page {
        space: ExGuid,
        reply: Reply<Page>,
    },
    Pages {
        reply: Reply<Vec<(ExGuid, String, u32)>>,
    },
    Conflicts {
        reply: Reply<Vec<(ExGuid, Vec<onestore::ConflictPage>)>>,
    },
    Versions {
        reply: Reply<Vec<(ExGuid, Vec<onestore::PageVersion>)>>,
    },
    Version {
        space: ExGuid,
        version: ExGuid,
        reply: Reply<Page>,
    },
    /// Answers once the edits before it are written.
    Flush {
        reply: Reply<()>,
    },
    /// Seals the open batch, when no sealed batch waits for publication, recording the
    /// attempt to publish it.
    Seal {
        reply: Reply<Option<Sealed>>,
    },
    /// Replays the queue on `image` (the stored remote image, or the base, when `None`);
    /// answers with the pages the remote changed.
    Rebase {
        image: Option<Vec<u8>>,
        reply: Reply<Vec<ExGuid>>,
    },
    /// Rereads the queue after the sync thread replaced it.
    Reopen {
        reply: Reply<()>,
    },
    /// From the thread that rebuilt the section: hand it the requests.
    Handover(mpsc::SyncSender<Takeover>),
    /// From the thread that rebuilt nothing: carry on with the section as it is.
    Resume,
}

/// The request channel and the requests held while the section was rebuilt.
type Takeover = (mpsc::Receiver<Request>, VecDeque<Request>);

/// A sealed batch: the transaction publishing it, or none when its edits changed nothing.
pub(crate) struct Sealed {
    pub batch: i64,
    pub transaction: Option<Transaction>,
}

impl Request {
    fn fail(self, message: &str) {
        let error = || io::Error::other(message.to_owned()).into();
        match self {
            Self::Apply { reply, .. } => reply(Err(error())),
            Self::Page { reply, .. } => reply(Err(error())),
            Self::Pages { reply } => reply(Err(error())),
            Self::Conflicts { reply } => reply(Err(error())),
            Self::Versions { reply } => reply(Err(error())),
            Self::Version { reply, .. } => reply(Err(error())),
            Self::Flush { reply } => reply(Err(error())),
            Self::Seal { reply } => reply(Err(error())),
            Self::Rebase { reply, .. } => reply(Err(error())),
            Self::Reopen { reply } => reply(Err(error())),
            Self::Handover(_) | Self::Resume => {}
        }
    }
}

type Worker = Arc<Mutex<Weak<Signal>>>;

/// The section thread as the replica sees it. Rereading the section after the queue was
/// replaced (a rebase, a released attempt) happens on a new thread while the current one
/// keeps answering page reads from the section as it was; the new thread then takes the
/// requests over, so reads never wait for a rebuild.
pub(crate) struct Thread {
    connection: Arc<Mutex<Connection>>,
    worker: Worker,
    /// Taken when the replica drops, which ends every section thread.
    sender: Mutex<Option<mpsc::Sender<Request>>>,
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
}

impl Thread {
    pub(crate) fn send(&self, request: Request) -> Result<()> {
        self.sender
            .lock()
            .ok()
            .and_then(|sender| sender.as_ref()?.send(request).ok())
            .ok_or_else(|| io::Error::other("The section thread stopped").into())
    }

    /// Ends the section threads and waits for them, so the cache is released.
    pub(crate) fn stop(&self) {
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
        while let Some(thread) = self
            .threads
            .lock()
            .ok()
            .and_then(|mut threads| threads.pop())
        {
            let _ = thread.join();
        }
    }

    fn start(self: &Arc<Self>, run: impl FnOnce(Arc<Self>) + Send + 'static) -> Result<()> {
        let shared = Arc::clone(self);
        let thread = thread::Builder::new()
            .name("onestore-section".into())
            .spawn(move || run(shared))?;
        self.threads
            .lock()
            .map_err(|_| io::Error::other("The section thread panicked"))?
            .push(thread);
        Ok(())
    }
}

/// Starts the section thread once the queue opens.
pub(crate) fn spawn(
    connection: Arc<Mutex<Connection>>,
    worker: Worker,
) -> Result<(Arc<Thread>, ExGuid)> {
    let (sender, requests) = mpsc::channel();
    let shared = Arc::new(Thread {
        connection,
        worker,
        sender: Mutex::new(Some(sender)),
        threads: Mutex::new(Vec::new()),
    });
    let (ready, opened) = mpsc::sync_channel(1);
    shared.start(move |shared| run(shared, requests, VecDeque::new(), Some(ready)))?;
    match opened.recv() {
        Ok(Ok(root)) => Ok((shared, root)),
        Ok(Err(error)) => {
            shared.stop();
            Err(error)
        }
        Err(_) => {
            shared.stop();
            Err(io::Error::other("The section thread panicked").into())
        }
    }
}

enum Next {
    Stop,
    Reopen,
    Handover(mpsc::SyncSender<Takeover>, VecDeque<Request>),
}

fn run(
    shared: Arc<Thread>,
    requests: mpsc::Receiver<Request>,
    mut backlog: VecDeque<Request>,
    mut ready: Option<mpsc::SyncSender<Result<ExGuid>>>,
) {
    loop {
        let arena = Arena::default();
        let working = match Working::open(&arena, &shared.connection) {
            Ok(working) => working,
            Err(error) => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error));
                } else {
                    fail(error, backlog, requests);
                }
                return;
            }
        };
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(working.section.root()));
        }
        match working.serve(&shared, &requests, &mut backlog) {
            Next::Stop => return,
            Next::Reopen => {}
            Next::Handover(to, held) => {
                let _ = to.send((requests, held));
                return;
            }
        }
    }
}

/// Answers every request with `error` until the replica drops.
fn fail(error: crate::Error, backlog: VecDeque<Request>, requests: mpsc::Receiver<Request>) {
    let message = error.to_string();
    for request in backlog.into_iter().chain(requests.iter()) {
        request.fail(&message);
    }
}

/// What a rebuilding thread does before it rereads the section.
enum Job {
    Rebase {
        image: Option<Vec<u8>>,
        reply: Reply<Vec<ExGuid>>,
    },
    Reopen {
        reply: Reply<()>,
    },
}

/// Runs `job`, rereads the section and takes the requests over from the thread that
/// started it; a failed rebase changed nothing that thread serves.
fn build(shared: Arc<Thread>, job: Job, signal: mpsc::Sender<Request>) {
    let answer: Box<dyn FnOnce() + Send> = match job {
        Job::Reopen { reply } => Box::new(move || reply(Ok(()))),
        Job::Rebase { image, reply } => match rebase(&shared.connection, image) {
            Ok(changed) => Box::new(move || reply(Ok(changed))),
            Err(error) => {
                reply(Err(error));
                let _ = signal.send(Request::Resume);
                return;
            }
        },
    };
    let arena = Arena::default();
    let opened = Working::open(&arena, &shared.connection);
    let (to, from) = mpsc::sync_channel(1);
    if signal.send(Request::Handover(to)).is_err() {
        return;
    }
    drop(signal);
    answer();
    let Ok((requests, mut backlog)) = from.recv() else {
        return;
    };
    let working = match opened {
        Ok(working) => working,
        Err(error) => return fail(error, backlog, requests),
    };
    match working.serve(&shared, &requests, &mut backlog) {
        Next::Stop => {}
        Next::Reopen => run(shared, requests, backlog, None),
        Next::Handover(to, held) => {
            let _ = to.send((requests, held));
        }
    }
}

/// An edit applied to the section and not yet written, with who waits for it.
struct Accepted {
    author: String,
    edit: Edit,
    reply: Reply<u64>,
}

struct Working<'a> {
    section: Section<'a>,
    /// The batch collecting edits; sealed batches before it are replayed.
    open: Option<i64>,
    /// Spaces the open batch's edits change.
    touched: BTreeSet<ExGuid>,
}

impl<'a> Working<'a> {
    fn open(arena: &'a Arena, connection: &Mutex<Connection>) -> Result<Self> {
        let (section, open, touched) = replay(arena, &*lock(connection)?)?;
        Ok(Self {
            section,
            open,
            touched,
        })
    }

    fn serve(
        mut self,
        shared: &Arc<Thread>,
        requests: &mpsc::Receiver<Request>,
        backlog: &mut VecDeque<Request>,
    ) -> Next {
        let (connection, worker) = (&*shared.connection, &shared.worker);
        // While another thread rebuilds the section, reads answer from this one and every
        // other request waits for the rebuilt section.
        let mut held: Option<VecDeque<Request>> = None;
        loop {
            let first = match backlog.pop_front() {
                Some(request) => request,
                None => match requests.recv() {
                    Ok(request) => request,
                    Err(_) => return Next::Stop,
                },
            };
            let mut burst: VecDeque<Request> = VecDeque::from([first]);
            burst.extend(backlog.drain(..));
            burst.extend(requests.try_iter());
            let mut accepted = Vec::new();
            // Requests that write the queue beyond the burst's edits run after its reads.
            let mut later = VecDeque::new();
            while let Some(request) = burst.pop_front().or_else(|| later.pop_front()) {
                if let Some(waiting) = &mut held {
                    match request {
                        Request::Page { space, reply } => {
                            reply(self.section.page(space).map_err(Into::into))
                        }
                        Request::Pages { reply } => reply(self.section.pages().map_err(Into::into)),
                        Request::Conflicts { reply } => {
                            reply(self.section.conflicts().map_err(Into::into))
                        }
                        Request::Versions { reply } => {
                            reply(self.section.versions().map_err(Into::into))
                        }
                        Request::Version {
                            space,
                            version,
                            reply,
                        } => reply(self.section.version(space, version).map_err(Into::into)),
                        Request::Handover(to) => {
                            let mut waiting = held.take().unwrap_or_default();
                            waiting.extend(burst.drain(..).chain(later.drain(..)));
                            return Next::Handover(to, waiting);
                        }
                        Request::Resume => {
                            let waiting = held.take().unwrap_or_default();
                            burst = waiting.into_iter().chain(burst.drain(..)).collect();
                        }
                        other => waiting.push_back(other),
                    }
                    continue;
                }
                let reopen = match request {
                    Request::Apply {
                        author,
                        edit,
                        reply,
                    } => self
                        .accept(author, edit, reply, &mut accepted, connection, worker)
                        .err()
                        .unwrap_or(false),
                    Request::Page { space, reply } => {
                        reply(self.section.page(space).map_err(Into::into));
                        false
                    }
                    Request::Pages { reply } => {
                        reply(self.section.pages().map_err(Into::into));
                        false
                    }
                    Request::Conflicts { reply } => {
                        reply(self.section.conflicts().map_err(Into::into));
                        false
                    }
                    Request::Versions { reply } => {
                        reply(self.section.versions().map_err(Into::into));
                        false
                    }
                    Request::Version {
                        space,
                        version,
                        reply,
                    } => {
                        reply(self.section.version(space, version).map_err(Into::into));
                        false
                    }
                    request @ (Request::Flush { .. } | Request::Seal { .. })
                        if !burst.is_empty() =>
                    {
                        later.push_back(request);
                        false
                    }
                    Request::Flush { reply } => {
                        let written = self.flush(connection, worker, &mut accepted);
                        reply(if written {
                            Ok(())
                        } else {
                            Err(io::Error::other("The queue could not be written").into())
                        });
                        !written
                    }
                    Request::Seal { reply } => {
                        if !self.flush(connection, worker, &mut accepted) {
                            reply(Err(
                                io::Error::other("The queue could not be written").into()
                            ));
                            true
                        } else {
                            let sealed = self.seal(connection);
                            let failed = sealed.is_err();
                            reply(sealed);
                            failed
                        }
                    }
                    Request::Rebase { image, reply } => {
                        if self.flush(connection, worker, &mut accepted) {
                            held = self
                                .rebuild(shared, Job::Rebase { image, reply })
                                .then(VecDeque::new);
                            false
                        } else {
                            reply(Err(
                                io::Error::other("The queue could not be written").into()
                            ));
                            true
                        }
                    }
                    Request::Reopen { reply } => {
                        self.flush(connection, worker, &mut accepted);
                        held = self
                            .rebuild(shared, Job::Reopen { reply })
                            .then(VecDeque::new);
                        false
                    }
                    Request::Handover(_) | Request::Resume => false,
                };
                if reopen {
                    backlog.extend(burst.drain(..).chain(later.drain(..)));
                    return Next::Reopen;
                }
            }
            if !self.flush(connection, worker, &mut accepted) {
                return Next::Reopen;
            }
        }
    }

    /// Starts a thread that runs `job` and rereads the section; false when the replica is
    /// stopping, which answers the job's request.
    fn rebuild(&self, shared: &Arc<Thread>, job: Job) -> bool {
        let signal = shared
            .sender
            .lock()
            .ok()
            .and_then(|sender| sender.as_ref().cloned());
        let Some(signal) = signal else {
            match job {
                Job::Rebase { reply, .. } => {
                    reply(Err(io::Error::other("The section thread stopped").into()))
                }
                Job::Reopen { reply } => {
                    reply(Err(io::Error::other("The section thread stopped").into()))
                }
            }
            return false;
        };
        shared
            .start(move |shared| build(shared, job, signal))
            .is_ok()
    }

    /// Applies an edit, keeping it to be written with the burst. A refused edit is answered
    /// here; `Err(true)` when it failed part way and the section must be reread.
    fn accept(
        &mut self,
        author: String,
        edit: Edit,
        reply: Reply<u64>,
        accepted: &mut Vec<Accepted>,
        connection: &Mutex<Connection>,
        worker: &Worker,
    ) -> std::result::Result<(), bool> {
        match self.section.apply(&author, &edit) {
            Ok(()) => {
                self.touched
                    .extend(queue::spaces(&edit, self.section.root()));
                accepted.push(Accepted {
                    author,
                    edit,
                    reply,
                });
                Ok(())
            }
            Err(error) => {
                let broken = matches!(error, OpError::Failed(_));
                if broken {
                    self.flush(connection, worker, accepted);
                }
                reply(Err(error.into()));
                Err(broken)
            }
        }
    }

    /// Writes the accepted edits in one transaction, then answers their senders; false
    /// when the write failed and the section holds edits the queue lacks.
    fn flush(
        &mut self,
        connection: &Mutex<Connection>,
        worker: &Worker,
        accepted: &mut Vec<Accepted>,
    ) -> bool {
        if accepted.is_empty() {
            return true;
        }
        let written = (|| -> Result<Vec<u64>> {
            let mut connection = lock(connection)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let batch = match self.open {
                Some(batch) => batch,
                None => {
                    transaction.execute("INSERT INTO batches DEFAULT VALUES", [])?;
                    transaction.last_insert_rowid()
                }
            };
            let mut ids = Vec::new();
            for edit in accepted.iter() {
                ids.push(queue::insert(
                    &transaction,
                    None,
                    batch,
                    &edit.author,
                    &edit.edit,
                )?);
            }
            transaction.commit()?;
            self.open = Some(batch);
            Ok(ids)
        })();
        let ok = written.is_ok();
        match written {
            Ok(ids) => {
                for (edit, id) in accepted.drain(..).zip(ids) {
                    (edit.reply)(Ok(id));
                }
                crate::wake(worker);
            }
            Err(error) => {
                let message = error.to_string();
                for edit in accepted.drain(..) {
                    (edit.reply)(Err(io::Error::other(message.clone()).into()));
                }
            }
        }
        ok
    }

    /// Seals the open batch unless a sealed batch still waits for publication.
    fn seal(&mut self, connection: &Mutex<Connection>) -> Result<Option<Sealed>> {
        let Some(batch) = self.open else {
            return Ok(None);
        };
        {
            let connection = lock(connection)?;
            let waiting: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM batches WHERE sealed IS NOT NULL)",
                [],
                |row| row.get(0),
            )?;
            if waiting {
                return Ok(None);
            }
        }
        let transaction = self.section.seal()?;
        // A batch whose edits change nothing is recorded against the section's root.
        let root = self.section.root();
        let revisions: BTreeMap<ExGuid, ExGuid> = self
            .section
            .newest()
            .filter(|(space, _)| {
                self.touched.contains(space) || (self.touched.is_empty() && *space == root)
            })
            .collect();
        // The sync thread publishes what it seals at once: the attempt is recorded with it.
        lock(connection)?.execute(
            "UPDATE batches SET sealed=?1, revisions=?2, attempted=?3 WHERE id=?4",
            params![
                serde_json::to_string(&transaction).map_err(io::Error::other)?,
                serde_json::to_string(&revisions).map_err(io::Error::other)?,
                transaction.is_some(),
                batch
            ],
        )?;
        self.open = None;
        self.touched.clear();
        Ok(Some(Sealed { batch, transaction }))
    }
}

/// Replays every queued edit on `image`, which becomes the base; each page whose local
/// version the remote could not take gains a conflict page holding it, queued as a new edit.
/// Returns the pages the remote changed.
fn rebase(connection: &Mutex<Connection>, image: Option<Vec<u8>>) -> Result<Vec<ExGuid>> {
    let (base_image, image, edits) = {
        let connection = lock(connection)?;
        if connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM batches WHERE attempted=1)",
            [],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "An uncertain attempt is never rebased",
            )
            .into());
        }
        let base_image = base::read(&connection, base::Image::Base)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "The cache has no base image")
        })?;
        let image = match image {
            Some(image) => image,
            None => {
                base::read(&connection, base::Image::Remote)?.unwrap_or_else(|| base_image.clone())
            }
        };
        (base_image, image, queue::load(&connection, None)?)
    };
    let old_arena = Arena::default();
    let mut old = Section::open(&old_arena, base_image)?;
    let before: BTreeMap<ExGuid, ExGuid> = old.revisions().collect();
    let remote_arena = Arena::default();
    let remote = Section::open(&remote_arena, image.clone())?;
    if old.root() != remote.root() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Remote snapshot belongs to another document",
        )
        .into());
    }
    let after: BTreeMap<ExGuid, ExGuid> = remote.revisions().collect();
    // The pages as the queue leaves them, read once a page conflicts: O(section).
    let local_arena = Arena::default();
    let mut local = None;
    // Pages the remote already holds as the local edits leave them: their ops are done.
    let mut converged = BTreeSet::new();
    let (rewritten, added) = loop {
        let arena = Arena::default();
        let mut new = Section::open(&arena, image.clone())?;
        let merged = merge::rebase(&mut old, &mut new, &edits, &converged)?;
        if !merged.conflicts.is_empty() && local.is_none() {
            local = Some(replay(&local_arena, &*lock(connection)?)?.0);
        }
        let pages: BTreeMap<ExGuid, Page> = merged
            .conflicts
            .keys()
            .filter_map(|space| Some((*space, local.as_ref()?.page(*space).ok()?)))
            .collect();
        let settled: Vec<ExGuid> = pages
            .iter()
            .filter(|(space, page)| remote.page(**space).is_ok_and(|remote| remote == **page))
            .map(|(space, _)| *space)
            .collect();
        if !settled.is_empty() {
            converged.extend(settled);
            continue;
        }
        let mut added = Vec::new();
        for (space, (author, objects)) in &merged.conflicts {
            // A page the queue went on to delete keeps no version of its own.
            let (Some(page), Some(local)) = (pages.get(space), local.as_mut()) else {
                continue;
            };
            let edit = merge::conflict_page(
                &mut new,
                local,
                &merged.moved,
                *space,
                page,
                author,
                objects,
                crate::now(),
            )?;
            added.push((author.clone(), edit));
        }
        break (merged.rewritten, added);
    };
    let mut connection = lock(connection)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    base::write(&transaction, base::Image::Base, &image)?;
    base::clear(&transaction, base::Image::Remote)?;
    transaction.execute("INSERT INTO batches DEFAULT VALUES", [])?;
    let batch = transaction.last_insert_rowid();
    transaction.execute("UPDATE edits SET batch=?1", [batch])?;
    transaction.execute("DELETE FROM batches WHERE id<>?1", [batch])?;
    for (id, edit) in rewritten {
        queue::rewrite(&transaction, id, &edit)?;
    }
    for (author, edit) in &added {
        queue::insert(&transaction, None, batch, author, edit)?;
    }
    if transaction.query_row("SELECT count(*) FROM edits", [], |row| row.get::<_, i64>(0))? == 0 {
        transaction.execute("DELETE FROM batches", [])?;
    }
    queue::collect(&transaction)?;
    transaction.commit()?;
    let mut changed: BTreeSet<ExGuid> = before
        .iter()
        .filter(|(space, rid)| after.get(space) != Some(rid))
        .map(|(space, _)| *space)
        .chain(
            after
                .keys()
                .filter(|space| !before.contains_key(space))
                .copied(),
        )
        .collect();
    changed.extend(converged);
    Ok(changed.into_iter().collect())
}

/// Batches in order with their sealed transactions: `None` while open, `Some(None)` when
/// sealing stored nothing.
fn batches(connection: &Connection) -> Result<Vec<(i64, Option<Option<Transaction>>)>> {
    let mut query = connection.prepare("SELECT id, sealed FROM batches ORDER BY id")?;
    let mut rows = query.query([])?;
    let mut batches = Vec::new();
    while let Some(row) = rows.next()? {
        let sealed: Option<String> = row.get(1)?;
        batches.push((
            row.get(0)?,
            sealed
                .map(|sealed| serde_json::from_str(&sealed))
                .transpose()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        ));
    }
    Ok(batches)
}

/// The cached base with each sealed batch replayed and the open batch's edits applied,
/// with the open batch and the spaces its edits change.
fn replay<'a>(
    arena: &'a Arena,
    connection: &Connection,
) -> Result<(Section<'a>, Option<i64>, BTreeSet<ExGuid>)> {
    let image = base::read(connection, base::Image::Base)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "The cache has no base image"))?;
    let mut section = Section::open(arena, image)?;
    let mut open = None;
    let mut touched = BTreeSet::new();
    for (batch, sealed) in batches(connection)? {
        if open.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "A cached batch follows the open batch",
            )
            .into());
        }
        match sealed {
            Some(Some(transaction)) => section.replay(&transaction)?,
            Some(None) => {}
            None => {
                for queued in queue::load(connection, Some(batch))? {
                    section
                        .apply(&queued.author, &queued.edit)
                        .map_err(|error| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("Queued edit {} no longer applies: {error}", queued.id),
                            )
                        })?;
                    touched.extend(queue::spaces(&queued.edit, section.root()));
                }
                open = Some(batch);
            }
        }
    }
    Ok((section, open, touched))
}

/// The image the queue leaves, its unsealed edits sealed as one more revision; that
/// revision's identities are fresh on every call. O(section), for tests and recovery.
pub(crate) fn image(connection: &Connection) -> Result<Vec<u8>> {
    let arena = Arena::default();
    let (mut section, ..) = replay(&arena, connection)?;
    section.seal()?;
    Ok(section.image())
}

/// The sealed batch waiting for publication, if any.
pub(crate) fn sealed(connection: &Connection) -> Result<Option<Sealed>> {
    let row: Option<(i64, String)> = connection
        .query_row(
            "SELECT id, sealed FROM batches WHERE sealed IS NOT NULL ORDER BY id LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    row.map(|(batch, sealed)| {
        Ok(Sealed {
            batch,
            transaction: serde_json::from_str(&sealed)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        })
    })
    .transpose()
}
