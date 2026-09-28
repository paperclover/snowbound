use super::*;
use crate::working::{Request, Sealed};
use onestore::{CommitError, CommitState, RevisionIndex, Stamp, Store, Transaction};
use rusqlite::OptionalExtension;
use std::{
    collections::BTreeMap,
    sync::{MutexGuard, TryLockError},
};

/// A single remote file with fresh reads and native-compatible guarded publication.
/// Errors retain publication state; confirmation checks the stamp, flushes, and notifies
/// cached readers.
pub trait Remote {
    fn read(&mut self) -> io::Result<Vec<u8>>;
    /// The file's stamp without reading its body or coordinating with writers. While it is
    /// the last observed image's, synchronization neither reads nor revalidates the file.
    fn stamp(&mut self) -> io::Result<Stamp>;
    fn publish(&mut self, transaction: &Transaction) -> std::result::Result<(), CommitError>;
    /// Confirms that the file still has `base`'s stamp and is durable (`onestore::confirm`).
    fn confirm(&mut self, base: &Stamp) -> std::result::Result<(), CommitError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditStatus {
    Pending,
    /// Retained publication attempt; this revision alone may be insufficient to confirm it.
    AwaitingConfirmation {
        revision: ExGuid,
    },
    /// Historical confirmation; later remote edits or restores may remove the effect.
    Published {
        revision: ExGuid,
    },
    /// Retired unpublished by a reviewed release; the archive at this path holds the
    /// edit, its attempt evidence and both images.
    Archived {
        archive: String,
    },
}

/// What one synchronization step did: the state its batch reached, named by the batch's
/// newest edit, and the pages a remote change replaced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    pub edit: Option<(u64, EditStatus)>,
    pub changed: Vec<ExGuid>,
}

/// The queue as a synchronization step starts it.
struct State {
    base: Stamp,
    /// The observed remote image's stamp while it is not the base.
    remote: Option<Stamp>,
    /// The first batch waiting on a remote answer to an attempt.
    blocked: Option<Blocked>,
    queued: bool,
}

struct Blocked {
    batch: i64,
    revisions: Option<BTreeMap<ExGuid, ExGuid>>,
}

fn state(connection: &Connection) -> Result<State> {
    let base = base::stamp(connection, base::Image::Base)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "The cache has no base image"))?;
    let blocked = connection
        .query_row(
            "SELECT id, revisions FROM batches WHERE attempted=1 ORDER BY id LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .map(|(batch, revisions)| {
            Ok::<_, Error>(Blocked {
                batch,
                revisions: revisions
                    .map(|revisions| decode_revisions(&revisions))
                    .transpose()?,
            })
        })
        .transpose()?;
    Ok(State {
        base,
        remote: base::stamp(connection, base::Image::Remote)?,
        blocked,
        queued: connection
            .query_row("SELECT EXISTS(SELECT 1 FROM batches)", [], |row| row.get(0))?,
    })
}

fn decode_revisions(encoded: &str) -> Result<BTreeMap<ExGuid, ExGuid>> {
    let revisions: BTreeMap<ExGuid, ExGuid> = serde_json::from_str(encoded)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if revisions
        .iter()
        .any(|(space, revision)| space.guid == [0; 16] || revision.guid == [0; 16])
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Cached publication evidence is incomplete",
        )
        .into());
    }
    Ok(revisions)
}

/// The revision a batch's seal stored for the first page an edit changes.
fn revision_of(edit: &onestore::op::Edit, revisions: &BTreeMap<ExGuid, ExGuid>) -> Result<ExGuid> {
    edit.ops
        .iter()
        .find_map(|op| match op {
            onestore::op::Op::Page { space, .. } => revisions.get(space),
            onestore::op::Op::Section(_) => None,
        })
        .or_else(|| revisions.values().next())
        .copied()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Cached publication evidence is incomplete",
            )
            .into()
        })
}

/// The newest edit of a batch.
fn newest(connection: &Connection, batch: i64) -> Result<u64> {
    unsigned(
        connection.query_row("SELECT max(id) FROM edits WHERE batch=?1", [batch], |row| {
            row.get(0)
        })?,
    )
}

impl Replica {
    /// Returns a durable receipt or the persisted state of a locally acknowledged edit.
    pub fn status(&self, id: u64) -> Result<Option<EditStatus>> {
        status(&*self.lock()?, id)
    }

    /// The last observed remote image. Observation alone does not acknowledge any pending
    /// edit's remote durability.
    pub(crate) fn remote_snapshot(&self) -> Result<Vec<u8>> {
        let connection = self.lock()?;
        let image = match base::read(&connection, base::Image::Remote)? {
            Some(image) => Some(image),
            None => base::read(&connection, base::Image::Base)?,
        };
        Ok(image.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "The cache has no base image")
        })?)
    }

    /// Publishes the oldest unpublished batch, rebasing the queue first when the remote
    /// changed. Reads the remote image only when its stamp moved; network I/O holds
    /// synchronization ownership without holding the cache mutex. Uncertain attempts are
    /// never replayed.
    pub fn sync_once(&self, remote: &mut impl Remote) -> Result<Synced> {
        let _owner = self.sync_owner()?;
        let state = state(&*self.lock()?)?;
        let observed = remote.stamp().map_err(Error::RemoteIo)?;
        if let Some(blocked) = &state.blocked
            && observed == *state.remote.as_ref().unwrap_or(&state.base)
        {
            // Nothing new to decide: the remote is as it was when the batch blocked.
            let id = newest(&*self.lock()?, blocked.batch)?;
            return Ok(Synced {
                edit: Some((
                    id,
                    status(&*self.lock()?, id)?.unwrap_or(EditStatus::Pending),
                )),
                changed: Vec::new(),
            });
        }
        let image = match observed {
            observed if observed == state.base => None,
            _ => {
                let image = remote.read().map_err(Error::RemoteIo)?;
                // A read image is compared whole: a stamp stands for it only when read alone.
                let base = base::read(&*self.lock()?, base::Image::Base)?;
                (base.as_deref() != Some(&image[..])).then_some(image)
            }
        };
        let mut changed = Vec::new();
        if let Some(image) = image {
            if let Some(Blocked { batch, revisions }) = &state.blocked {
                let id = newest(&*self.lock()?, *batch)?;
                let revisions = revisions.clone().unwrap_or_default();
                let store = Store::parse(&image)?;
                if !store.checksum_mismatches.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Notebook transaction checksum damage",
                    )
                    .into());
                }
                let index = RevisionIndex::parse(&store)?;
                index.validate_current()?;
                if index.root != self.root {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Remote snapshot belongs to another document",
                    )
                    .into());
                }
                let observed = revisions.iter().all(|(space, revision)| {
                    index
                        .spaces
                        .get(space)
                        .is_some_and(|space| space.revisions.contains_key(revision))
                });
                let sealed = working::sealed(&*self.lock()?)?
                    .filter(|sealed| sealed.batch == *batch)
                    .and_then(|sealed| sealed.transaction);
                // Without its revisions the attempt still counts once the remote holds
                // every page it changed as it changed them; its receipts then name the
                // remote's revisions.
                let receipts = if observed {
                    None
                } else {
                    let equal = self.holds(&image, sealed.as_ref(), &revisions)?;
                    if !equal {
                        base::write(&*self.lock()?, base::Image::Remote, &image)?;
                        return Ok(Synced {
                            edit: Some((
                                id,
                                status(&*self.lock()?, id)?.unwrap_or(EditStatus::Pending),
                            )),
                            changed,
                        });
                    }
                    Some(
                        revisions
                            .keys()
                            .filter_map(|space| Some((*space, index.active(*space).ok()?)))
                            .collect(),
                    )
                };
                if let Err(error) = remote.confirm(&Stamp::of(&image)?) {
                    if error.state == CommitState::Committed {
                        self.acknowledge(*batch, sealed.as_ref(), receipts.as_ref())?;
                    }
                    return Err(error.into());
                }
                self.acknowledge(*batch, sealed.as_ref(), receipts.as_ref())?;
                let revision = self.receipt(id)?;
                changed = self.rebase(Some(image))?;
                return Ok(Synced {
                    edit: Some((id, EditStatus::Published { revision })),
                    changed,
                });
            }
            changed = self.rebase(Some(image))?;
        } else if let Some(blocked) = &state.blocked {
            if state.remote.is_some() {
                // The remote is back at the base: what blocked the queue is gone.
                base::clear(&*self.lock()?, base::Image::Remote)?;
            }
            let id = newest(&*self.lock()?, blocked.batch)?;
            return Ok(Synced {
                edit: Some((
                    id,
                    status(&*self.lock()?, id)?.unwrap_or(EditStatus::Pending),
                )),
                changed,
            });
        } else if !state.queued {
            return Ok(Synced::default());
        }
        // The cache lock is released before the section thread, which takes it, seals.
        let waiting = working::sealed(&*self.lock()?)?;
        let sealed = match waiting {
            Some(sealed) => {
                if sealed.transaction.is_some() {
                    self.lock()?
                        .execute("UPDATE batches SET attempted=1 WHERE id=?1", [sealed.batch])?;
                }
                Some(sealed)
            }
            None => self.ask(|reply| Request::Seal { reply })?,
        };
        let Some(Sealed { batch, transaction }) = sealed else {
            return Ok(Synced {
                edit: None,
                changed,
            });
        };
        let id = newest(&*self.lock()?, batch)?;
        let Some(transaction) = transaction else {
            // Edits that changed nothing are published once the remote's image is durable.
            let base = base::stamp(&*self.lock()?, base::Image::Base)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "The cache holds no base image")
            })?;
            if let Err(error) = remote.confirm(&base) {
                if error.state == CommitState::Committed {
                    self.acknowledge(batch, None, None)?;
                }
                return Err(error.into());
            }
            self.acknowledge(batch, None, None)?;
            let revision = self.receipt(id)?;
            return Ok(Synced {
                edit: Some((id, EditStatus::Published { revision })),
                changed,
            });
        };
        match remote.publish(&transaction) {
            Ok(()) => {}
            Err(error) if error.state == CommitState::NotCommitted => {
                self.lock()?
                    .execute("UPDATE batches SET attempted=0 WHERE id=?1", [batch])?;
                return Err(error.into());
            }
            Err(error) if error.state == CommitState::Committed => {
                self.acknowledge(batch, Some(&transaction), None)?;
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        }
        self.acknowledge(batch, Some(&transaction), None)?;
        let revision = self.receipt(id)?;
        Ok(Synced {
            edit: Some((id, EditStatus::Published { revision })),
            changed,
        })
    }

    /// Whether `image` holds every page in `revisions` as the base with `sealed` has it.
    fn holds(
        &self,
        image: &[u8],
        sealed: Option<&Transaction>,
        revisions: &BTreeMap<ExGuid, ExGuid>,
    ) -> Result<bool> {
        let base = base::read(&*self.lock()?, base::Image::Base)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "The cache has no base image")
        })?;
        let (local, remote) = (onestore::Arena::default(), onestore::Arena::default());
        let mut local = onestore::Section::open(&local, base)?;
        if let Some(sealed) = sealed {
            local.replay(sealed)?;
        }
        let mut remote = onestore::Section::open(&remote, image.to_vec())?;
        // A page's versions change without its page changing.
        let versions = |section: &mut onestore::Section<'_>| {
            section.versions().map(|versions| {
                versions
                    .into_iter()
                    .filter(|(page, _)| revisions.contains_key(page))
                    .collect::<Vec<_>>()
            })
        };
        Ok(revisions.keys().all(
            |space| matches!((local.page(*space), remote.page(*space)), (Ok(a), Ok(b)) if a == b),
        ) && versions(&mut local)? == versions(&mut remote)?)
    }

    /// Asks the section thread to replay the queue on `image`, returning the pages the
    /// remote changed.
    fn rebase(&self, image: Option<Vec<u8>>) -> Result<Vec<ExGuid>> {
        self.ask(|reply| Request::Rebase { image, reply })
    }

    fn receipt(&self, id: u64) -> Result<ExGuid> {
        match status(&*self.lock()?, id)? {
            Some(EditStatus::Published { revision }) => Ok(revision),
            _ => Err(io::Error::other("A published edit has no receipt").into()),
        }
    }

    /// Records a published batch: the base becomes the image it left, each of its edits
    /// gets a receipt naming the revisions its seal stored, or `revisions`, and it leaves
    /// the queue.
    fn acknowledge(
        &self,
        batch: i64,
        transaction: Option<&Transaction>,
        revisions: Option<&BTreeMap<ExGuid, ExGuid>>,
    ) -> Result<()> {
        let mut connection = self.lock()?;
        let database = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(transaction) = transaction {
            base::publish(&database, transaction)?;
        }
        let revisions = match revisions {
            Some(revisions) => revisions.clone(),
            None => decode_revisions(&database.query_row(
                "SELECT revisions FROM batches WHERE id=?1",
                [batch],
                |row| row.get::<_, String>(0),
            )?)?,
        };
        for queued in queue::load(&database, Some(batch))? {
            database.execute(
                "INSERT INTO receipts(edit_id, revision) VALUES (?1, ?2)",
                params![
                    signed(queued.id)?,
                    revision_of(&queued.edit, &revisions)?.to_string()
                ],
            )?;
        }
        database.execute("DELETE FROM batches WHERE id=?1", [batch])?;
        queue::collect(&database)?;
        database.commit()?;
        Ok(())
    }

    /// Retires the uncertain attempt of the batch holding edit `id` after review. The
    /// queue is first exported to `archive` (a new file), which is the record: no receipt
    /// is written. `Mine` publishes the batch again against the current remote; `Theirs`
    /// abandons every unpublished edit, which become `Archived`, and the local pages return
    /// to the remote's.
    pub fn release(&self, id: u64, archive: &Path, resolution: Resolution) -> Result<()> {
        let owner = self.sync_owner()?;
        let batch: Option<i64> = self
            .lock()?
            .query_row(
                "SELECT id FROM batches WHERE attempted=1 AND id=(SELECT batch FROM edits WHERE id=?1)",
                [signed(id)?],
                |row| row.get(0),
            )
            .optional()?;
        let Some(batch) = batch else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The edit has no uncertain attempt",
            )
            .into());
        };
        self.export_recovery(archive)?;
        {
            let mut connection = self.lock()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            match resolution {
                Resolution::Mine => {
                    transaction.execute("UPDATE batches SET attempted=0 WHERE id=?1", [batch])?;
                }
                Resolution::Theirs => {
                    transaction.execute(
                        "INSERT INTO archived(edit_id, archive) SELECT id, ?1 FROM edits",
                        [archive.to_string_lossy().into_owned()],
                    )?;
                    transaction.execute("DELETE FROM batches", [])?;
                    if let Some(remote) = base::read(&transaction, base::Image::Remote)? {
                        base::write(&transaction, base::Image::Base, &remote)?;
                        base::clear(&transaction, base::Image::Remote)?;
                    }
                    queue::collect(&transaction)?;
                }
            }
            transaction.commit()?;
        }
        if resolution == Resolution::Theirs {
            self.ask(|reply| Request::Reopen { reply })?;
        }
        drop(owner);
        self.wake_sync();
        Ok(())
    }

    fn sync_owner(&self) -> Result<MutexGuard<'_, ()>> {
        Ok(self
            .synchronization
            .try_lock()
            .map_err(|error| match error {
                TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
                TryLockError::Poisoned(_) => {
                    io::Error::other("Synchronization owner panicked; reopen the cache")
                }
            })?)
    }

    /// Whether nothing is queued and the remote still has the base's stamp, or the queue
    /// is blocked on a remote that has not changed since; reads neither image and does not
    /// lock the cache during remote I/O.
    pub(crate) fn settled(&self, remote: &mut impl Remote) -> Result<bool> {
        let state = state(&*self.lock()?)?;
        let expected = match &state.blocked {
            Some(_) => state.remote.unwrap_or(state.base),
            None if !state.queued => state.base,
            None => return Ok(false),
        };
        Ok(remote.stamp().map_err(Error::RemoteIo)? == expected)
    }
}

pub(crate) fn status(connection: &Connection, id: u64) -> Result<Option<EditStatus>> {
    let id = signed(id)?;
    if let Some(revision) = connection
        .query_row(
            "SELECT revision FROM receipts WHERE edit_id=?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(Some(EditStatus::Published {
            revision: revision.parse()?,
        }));
    }
    if let Some(archive) = connection
        .query_row(
            "SELECT archive FROM archived WHERE edit_id=?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(Some(EditStatus::Archived { archive }));
    }
    let record: Option<(bool, Option<String>, String)> = connection
        .query_row(
            "SELECT batches.attempted, batches.revisions, edits.edit
             FROM edits JOIN batches ON batches.id=edits.batch WHERE edits.id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(match record {
        None => None,
        Some((true, revisions, edit)) => {
            let revisions = decode_revisions(revisions.as_deref().unwrap_or("{}"))?;
            let edit: onestore::op::Edit = serde_json::from_str(&edit)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let revision = revision_of(&edit, &revisions)?;
            Some(EditStatus::AwaitingConfirmation { revision })
        }
        Some((false, ..)) => Some(EditStatus::Pending),
    })
}
