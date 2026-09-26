//! The section thread: the only owner of the parsed section, which is the cached base
//! image with each sealed batch replayed and the open batch's edits applied. Edits apply
//! here and are written in one SQLite transaction per burst; the sync thread asks it to seal
//! and, when the remote changed, to rebase the queue.

use crate::{
    ConflictKind, Resolution, Result, base, lock, merge, queue, session::Save, signed,
    worker::Signal,
};
use onestore::{
    Arena, ExGuid, Section, Transaction,
    op::{Edit, Op, OpError},
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
    /// A whole-page save of the old session API: `None` answers a save a later one
    /// continuing it absorbed.
    Save {
        space: ExGuid,
        before: Page,
        after: Page,
        author: String,
        reply: Reply<Option<Save>>,
    },
    Page {
        space: ExGuid,
        reply: Reply<Page>,
    },
    Pages {
        reply: Reply<Vec<(ExGuid, String, u32)>>,
    },
    Image {
        reply: Reply<Vec<u8>>,
    },
    /// Seals the open batch, when no sealed batch waits for publication, recording the
    /// attempt to publish it.
    Seal {
        reply: Reply<Option<Sealed>>,
    },
    /// Replays the queue on `image` (the stored remote image, or the base, when `None`).
    Rebase {
        image: Option<Vec<u8>>,
        resolve: Option<Resolve>,
        reply: Reply<Rebased>,
    },
    /// Rereads the queue after the sync thread replaced it.
    Reopen {
        reply: Reply<()>,
    },
}

/// A sealed batch: the transaction publishing it, or none when its edits changed nothing.
pub(crate) struct Sealed {
    pub batch: i64,
    pub transaction: Option<Transaction>,
}

/// A conflict's resolution: the batch it holds and every later edit drop their ops on
/// `space`; `Mine` first rewrites the rebased page to the local one (or to `page`).
pub(crate) struct Resolve {
    pub first: u64,
    pub space: ExGuid,
    pub keep: Resolution,
    pub page: Option<Page>,
}

pub(crate) enum Rebased {
    /// The queue now applies to the image, which is the base; these pages changed.
    Applied { changed: Vec<ExGuid> },
    /// The batch holding edit `id` no longer applies; nothing changed but the record.
    Conflict {
        id: u64,
        space: ExGuid,
        kind: ConflictKind,
    },
}

impl Request {
    fn fail(self, message: &str) {
        let error = || io::Error::other(message.to_owned()).into();
        match self {
            Self::Apply { reply, .. } => reply(Err(error())),
            Self::Save { reply, .. } => reply(Err(error())),
            Self::Page { reply, .. } => reply(Err(error())),
            Self::Pages { reply } => reply(Err(error())),
            Self::Image { reply } => reply(Err(error())),
            Self::Seal { reply } => reply(Err(error())),
            Self::Rebase { reply, .. } => reply(Err(error())),
            Self::Reopen { reply } => reply(Err(error())),
        }
    }
}

type Worker = Arc<Mutex<Weak<Signal>>>;

/// Starts the section thread once the queue opens.
pub(crate) fn spawn(
    connection: Arc<Mutex<Connection>>,
    worker: Worker,
) -> Result<(mpsc::Sender<Request>, thread::JoinHandle<()>, ExGuid)> {
    let (sender, receiver) = mpsc::channel();
    let (ready, opened) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("onestore-section".into())
        .spawn(move || run(&connection, &worker, &receiver, ready))?;
    match opened.recv() {
        Ok(Ok(root)) => Ok((sender, thread, root)),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err(io::Error::other("The section thread panicked").into())
        }
    }
}

enum Next {
    Stop,
    Reopen,
}

fn run(
    connection: &Mutex<Connection>,
    worker: &Worker,
    requests: &mpsc::Receiver<Request>,
    ready: mpsc::SyncSender<Result<ExGuid>>,
) {
    let mut ready = Some(ready);
    let mut backlog = VecDeque::new();
    loop {
        let arena = Arena::default();
        let working = match Working::open(&arena, connection) {
            Ok(working) => working,
            Err(error) => {
                let message = error.to_string();
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error));
                    return;
                }
                for request in backlog.drain(..).chain(requests.iter()) {
                    request.fail(&message);
                }
                return;
            }
        };
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(working.section.root()));
        }
        match working.serve(connection, worker, requests, &mut backlog) {
            Next::Stop => return,
            Next::Reopen => {}
        }
    }
}

/// An edit applied to the section and not yet written, with who waits for it.
struct Accepted {
    author: String,
    edit: Edit,
    done: Done,
}

enum Done {
    Apply(Reply<u64>),
    Save(Reply<Option<Save>>),
}

impl Done {
    fn reply(self, written: Result<u64>) {
        match self {
            Self::Apply(reply) => reply(written),
            Self::Save(reply) => reply(written.map(|id| Some(Save::Queued(id)))),
        }
    }
}

struct Working<'a> {
    section: Section<'a>,
    /// The batch collecting edits; sealed batches before it are replayed.
    open: Option<i64>,
    /// Spaces the open batch's edits change.
    touched: BTreeSet<ExGuid>,
    /// The last whole-page save: its space, the page it was given, and the page as stored.
    saved: Option<(ExGuid, Page, Page)>,
}

impl<'a> Working<'a> {
    fn open(arena: &'a Arena, connection: &Mutex<Connection>) -> Result<Self> {
        let (section, open, touched) = replay(arena, &*lock(connection)?)?;
        Ok(Self {
            section,
            open,
            touched,
            saved: None,
        })
    }

    fn serve(
        mut self,
        connection: &Mutex<Connection>,
        worker: &Worker,
        requests: &mpsc::Receiver<Request>,
        backlog: &mut VecDeque<Request>,
    ) -> Next {
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
            while let Some(request) = burst.pop_front() {
                let reopen = match request {
                    Request::Apply {
                        author,
                        edit,
                        reply,
                    } => self
                        .accept(
                            author,
                            edit,
                            Done::Apply(reply),
                            &mut accepted,
                            connection,
                            worker,
                        )
                        .err()
                        .unwrap_or(false),
                    Request::Save {
                        space,
                        before,
                        mut after,
                        author,
                        reply,
                    } => {
                        // Saves that continue one another are written once.
                        let mut replies = vec![reply];
                        while let Some(Request::Save {
                            space: next,
                            before: continued,
                            author: by,
                            ..
                        }) = burst.front()
                            && (*next, continued, by) == (space, &after, &author)
                        {
                            let Some(Request::Save {
                                after: later,
                                reply,
                                ..
                            }) = burst.pop_front()
                            else {
                                unreachable!()
                            };
                            after = later;
                            replies.push(reply);
                        }
                        let last = replies.pop().unwrap();
                        for folded in replies {
                            folded(Ok(None));
                        }
                        match self.save(space, &before, &after) {
                            Ok(Some(edit)) => match self.accept(
                                author,
                                edit,
                                Done::Save(last),
                                &mut accepted,
                                connection,
                                worker,
                            ) {
                                Ok(()) => {
                                    self.saved = self
                                        .section
                                        .page(space)
                                        .ok()
                                        .map(|stored| (space, after, stored));
                                    false
                                }
                                Err(broken) => broken,
                            },
                            Ok(None) => {
                                self.saved = self
                                    .section
                                    .page(space)
                                    .ok()
                                    .map(|stored| (space, after, stored));
                                last(Ok(Some(Save::Unchanged)));
                                false
                            }
                            Err(Stale::Stale) => {
                                last(Ok(Some(Save::Stale)));
                                false
                            }
                            Err(Stale::Error(error)) => {
                                last(Err(error));
                                false
                            }
                        }
                    }
                    Request::Page { space, reply } => {
                        reply(self.section.page(space).map_err(Into::into));
                        false
                    }
                    Request::Pages { reply } => {
                        reply(self.section.pages().map_err(Into::into));
                        false
                    }
                    Request::Image { reply } => {
                        if self.flush(connection, worker, &mut accepted) {
                            reply(lock(connection).and_then(|connection| image(&connection)));
                            false
                        } else {
                            reply(Err(
                                io::Error::other("The queue could not be written").into()
                            ));
                            true
                        }
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
                    Request::Rebase {
                        image,
                        resolve,
                        reply,
                    } => {
                        if !self.flush(connection, worker, &mut accepted) {
                            reply(Err(
                                io::Error::other("The queue could not be written").into()
                            ));
                            true
                        } else {
                            let rebased = self.rebase(connection, image, resolve);
                            let reopen = !matches!(rebased, Ok(Rebased::Conflict { .. }));
                            reply(rebased);
                            reopen
                        }
                    }
                    Request::Reopen { reply } => {
                        self.flush(connection, worker, &mut accepted);
                        reply(Ok(()));
                        true
                    }
                };
                if reopen {
                    backlog.extend(burst);
                    return Next::Reopen;
                }
            }
            if !self.flush(connection, worker, &mut accepted) {
                return Next::Reopen;
            }
        }
    }

    /// Applies an edit, keeping it to be written with the burst. A refused edit is answered
    /// here; `Err(true)` when it failed part way and the section must be reread.
    fn accept(
        &mut self,
        author: String,
        edit: Edit,
        done: Done,
        accepted: &mut Vec<Accepted>,
        connection: &Mutex<Connection>,
        worker: &Worker,
    ) -> std::result::Result<(), bool> {
        match self.section.apply(&author, &edit) {
            Ok(()) => {
                self.touched
                    .extend(queue::spaces(&edit, self.section.root()));
                accepted.push(Accepted { author, edit, done });
                Ok(())
            }
            Err(error) => {
                let broken = matches!(error, OpError::Failed(_));
                if broken {
                    self.flush(connection, worker, accepted);
                }
                done.reply(Err(error.into()));
                Err(broken)
            }
        }
    }

    /// The edit that takes the stored page from `before` to `after`. A save continuing
    /// the previous one finds the page as that one stored it, which may read back
    /// normalized.
    fn save(
        &mut self,
        space: ExGuid,
        before: &Page,
        after: &Page,
    ) -> std::result::Result<Option<Edit>, Stale> {
        let current = self
            .section
            .page(space)
            .map_err(|error| Stale::Error(error.into()))?;
        let expected = match &self.saved {
            Some((saved, given, stored)) if *saved == space && given == before => stored,
            _ => before,
        };
        if current != *expected {
            return Err(Stale::Stale);
        }
        let ops = onestore::op::lower_page(&current, after)
            .map_err(|error| Stale::Error(OpError::Unsupported(error.message).into()))?;
        Ok((!ops.is_empty()).then(|| Edit {
            at: crate::now(),
            ops: ops.into_iter().map(|op| Op::Page { space, op }).collect(),
        }))
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
                    edit.done.reply(Ok(id));
                }
                crate::wake(worker);
            }
            Err(error) => {
                let message = error.to_string();
                for edit in accepted.drain(..) {
                    edit.done
                        .reply(Err(io::Error::other(message.clone()).into()));
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
                "SELECT EXISTS(SELECT 1 FROM batches WHERE sealed IS NOT NULL OR conflict IS NOT NULL)",
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
            .revisions()
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

    /// Replays every queued edit on `image`, keeping it as the base when all apply.
    fn rebase(
        &mut self,
        connection: &Mutex<Connection>,
        image: Option<Vec<u8>>,
        resolve: Option<Resolve>,
    ) -> Result<Rebased> {
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
                None => base::read(&connection, base::Image::Remote)?
                    .unwrap_or_else(|| base_image.clone()),
            };
            (base_image, image, queue::load(&connection, None)?)
        };
        // The page the local edits leave, which `Mine` rewrites the remote page to; the
        // root space lists pages and holds none.
        let target = match &resolve {
            Some(Resolve {
                keep: Resolution::Mine,
                page: None,
                space,
                ..
            }) if *space != self.section.root() => Some(self.section.page(*space)?),
            Some(Resolve { page, .. }) => page.clone(),
            None => None,
        };
        let old_arena = Arena::default();
        let mut old = Section::open(&old_arena, base_image.clone())?;
        let before: BTreeMap<ExGuid, ExGuid> = old.revisions().collect();
        let mut after: BTreeMap<ExGuid, ExGuid>;
        // Pages the remote already holds as the local edits leave them: their ops are done.
        let mut converged = BTreeSet::new();
        let outcome = loop {
            let arena = Arena::default();
            let mut new = Section::open(&arena, image.clone())?;
            if old.root() != new.root() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Remote snapshot belongs to another document",
                )
                .into());
            }
            after = new.revisions().collect();
            let outcome = merge::rebase(
                &mut old,
                &mut new,
                &edits,
                &converged,
                resolve.as_ref().map(|resolve| merge::Resolving {
                    first: resolve.first,
                    space: resolve.space,
                    mine: resolve.keep == Resolution::Mine,
                    target: target.as_ref(),
                }),
            )?;
            if let merge::Outcome::Conflict { space, .. } = &outcome
                && !converged.contains(space)
                && let Ok(local) = self.section.page(*space)
                && Section::open(&Arena::default(), image.clone())?
                    .page(*space)
                    .is_ok_and(|remote| remote == local)
            {
                converged.insert(*space);
                continue;
            }
            break outcome;
        };
        let mut connection = lock(connection)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rebased = match outcome {
            merge::Outcome::Conflict { id, space, kind } => {
                transaction.execute("UPDATE batches SET conflict=NULL, space=NULL", [])?;
                transaction.execute(
                    "UPDATE batches SET conflict=?1, space=?2 WHERE id=(SELECT batch FROM edits WHERE id=?3)",
                    params![kind as i64, space.to_string(), signed(id)?],
                )?;
                if image == base_image {
                    base::clear(&transaction, base::Image::Remote)?;
                } else {
                    base::write(&transaction, base::Image::Remote, &image)?;
                }
                // A conflict is reported under its batch's newest edit.
                let id = crate::unsigned(transaction.query_row(
                    "SELECT max(id) FROM edits WHERE batch=(SELECT batch FROM edits WHERE id=?1)",
                    [signed(id)?],
                    |row| row.get(0),
                )?)?;
                Rebased::Conflict { id, space, kind }
            }
            merge::Outcome::Applied(rewritten) => {
                base::write(&transaction, base::Image::Base, &image)?;
                base::clear(&transaction, base::Image::Remote)?;
                transaction.execute("INSERT INTO batches DEFAULT VALUES", [])?;
                let batch = transaction.last_insert_rowid();
                transaction.execute("UPDATE edits SET batch=?1", [batch])?;
                transaction.execute("DELETE FROM batches WHERE id<>?1", [batch])?;
                for (id, edit) in rewritten {
                    queue::rewrite(&transaction, id, &edit)?;
                }
                if transaction
                    .query_row("SELECT count(*) FROM edits", [], |row| row.get::<_, i64>(0))?
                    == 0
                {
                    transaction.execute("DELETE FROM batches", [])?;
                }
                queue::collect(&transaction)?;
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
                changed.extend(resolve.as_ref().map(|resolve| resolve.space));
                changed.extend(converged);
                Rebased::Applied {
                    changed: changed.into_iter().collect(),
                }
            }
        };
        transaction.commit()?;
        Ok(rebased)
    }
}

enum Stale {
    Stale,
    Error(crate::Error),
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
