use super::*;
use onestore::{CommitError, CommitState};
use rusqlite::OptionalExtension;
use std::{
    collections::BTreeMap,
    sync::{MutexGuard, TryLockError, atomic::Ordering},
};

/// A single remote file with fresh reads and native-compatible guarded publication.
/// Errors retain publication state; confirmation compares, flushes, and notifies cached readers.
pub trait Remote {
    fn read(&mut self) -> io::Result<Vec<u8>>;
    fn publish(&mut self, edit: &PreparedEdit<'_>) -> std::result::Result<(), CommitError>;
    fn confirm(&mut self, snapshot: &[u8]) -> std::result::Result<(), CommitError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum ConflictKind {
    /// The edited page or a selected page no longer exists remotely.
    TargetUnavailable = 0,
    /// The remote image no longer accepts the intent's publication.
    UnsupportedEdit = 1,
    /// Page order or indentation changed remotely in a way the batch cannot absorb.
    StructureChanged = 2,
    /// The remote page changed where the local model changed too; review is required.
    ContentChanged = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditStatus {
    Pending,
    /// Retained publication attempt; this revision alone may be insufficient to confirm it.
    AwaitingConfirmation {
        revision: ExGuid,
    },
    Conflict(ConflictKind),
    /// Historical confirmation; later remote edits or restores may remove the effect.
    Published {
        revision: ExGuid,
    },
}

impl Replica {
    /// Returns a durable receipt or the persisted state of a locally acknowledged edit.
    pub fn status(&self, id: u64) -> Result<Option<EditStatus>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        status(&connection, id)
    }

    /// The last observed remote image, retained alongside the complete local working image.
    /// Observation alone does not acknowledge any pending edit's remote durability.
    pub fn remote_snapshot(&self) -> Result<Vec<u8>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        Ok(connection.query_row("SELECT base FROM replica WHERE id=1", [], |row| row.get(0))?)
    }

    /// Reconciles one pending edit, or refreshes the working image when the queue is empty.
    /// Network I/O holds synchronization ownership without holding the cache mutex.
    /// Uncertain edits are never replayed; retired formatting may confirm its complete observed effect.
    pub fn sync_once(&self, remote: &mut impl Remote) -> Result<Option<(u64, EditStatus)>> {
        let _owner = self.sync_owner()?;
        struct Step<'a>(&'a Replica);
        impl Drop for Step<'_> {
            fn drop(&mut self) {
                self.0.in_flight.store(0, Ordering::Release);
            }
        }
        let _step = Step(self);
        let snapshot = remote.read().map_err(Error::RemoteIo)?;
        let identity = validate(&snapshot)?;
        let (intent, attempted) = {
            let mut connection = self
                .connection
                .lock()
                .map_err(|_| io::Error::other("Cache owner panicked"))?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let base: Vec<u8> =
                transaction
                    .query_row("SELECT base FROM replica WHERE id=1", [], |row| row.get(0))?;
            let base_store = Store::parse(&base)?;
            if RevisionIndex::parse(&base_store)?.root != identity {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Remote snapshot belongs to another document",
                )
                .into());
            }
            let Some(intent) = pending(&transaction)?.into_iter().next() else {
                transaction.execute(
                    "UPDATE replica SET base=?1, working=?1 WHERE id=1",
                    [&snapshot],
                )?;
                transaction.commit()?;
                return Ok(None);
            };
            self.in_flight.store(
                i64::try_from(intent.id).map_err(io::Error::other)?,
                Ordering::Release,
            );
            let attempted = transaction
                .query_row(
                    "SELECT revisions FROM attempt WHERE edit_id=?1",
                    [i64::try_from(intent.id).map_err(io::Error::other)?],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            transaction.execute("UPDATE replica SET base=?1 WHERE id=1", [&snapshot])?;
            transaction.commit()?;
            (intent, attempted)
        };
        if let Some(encoded) = attempted {
            let revisions = attempted_revisions(&encoded, intent.space)?;
            let mut revision = revisions[&intent.space];
            let store = Store::parse(&snapshot)?;
            let index = RevisionIndex::parse(&store)?;
            if !revisions.iter().all(|(space, revision)| {
                index
                    .spaces
                    .get(space)
                    .is_some_and(|space| space.revisions.contains_key(revision))
            }) {
                // A retired attempt confirms only when its complete effect is observed.
                let satisfied = match &intent.operation {
                    Operation::Page(edit) if revisions.len() == 1 => {
                        page_of(&snapshot, intent.space)?.is_some_and(|page| page == edit.after)
                    }
                    _ => false,
                };
                if !satisfied {
                    return Ok(Some((
                        intent.id,
                        EditStatus::AwaitingConfirmation { revision },
                    )));
                }
                revision = index.active(intent.space)?;
            }
            if let Err(error) = remote.confirm(&snapshot) {
                if error.state == CommitState::Committed {
                    self.acknowledge(intent.id, revision, &snapshot)?;
                }
                return Err(error.into());
            }
            self.acknowledge(intent.id, revision, &snapshot)?;
            return Ok(Some((intent.id, EditStatus::Published { revision })));
        }
        let candidate = match &intent.operation {
            Operation::Page(edit) => page_of(&snapshot, intent.space)?
                .ok_or(ConflictKind::TargetUnavailable)
                .and_then(|current| {
                    let target = if current == edit.before {
                        edit.after.clone()
                    } else {
                        crate::merge::merge(&edit.before, &edit.after, &current)
                            .ok_or(ConflictKind::ContentChanged)?
                    };
                    PreparedEdit::page(&snapshot, intent.space, &target, &edit.author)
                        .map_err(|_| ConflictKind::UnsupportedEdit)
                }),
            Operation::CreatePage(page) => PreparedEdit::create_page(&snapshot, page)
                .map_err(|_| ConflictKind::StructureChanged),
            Operation::Pages(edit) => edit.prepare(&snapshot, intent.space)?,
            Operation::DeletePages(pages) => {
                let store = Store::parse(&snapshot)?;
                let index = RevisionIndex::parse(&store)?;
                if pages.iter().any(|page| {
                    index
                        .spaces
                        .get(page)
                        .is_none_or(|space| space.labels.is_empty())
                }) || Document::parse(&index)?
                    .pages()?
                    .iter()
                    .filter(|(sid, _)| pages.contains(sid))
                    .count()
                    != pages.len()
                {
                    Err(ConflictKind::TargetUnavailable)
                } else {
                    PreparedEdit::delete_pages_permanently(&snapshot, pages)
                        .map_err(|_| ConflictKind::UnsupportedEdit)
                }
            }
        };
        let prepared = match candidate {
            Ok(prepared) => prepared,
            Err(kind) => {
                let connection = self
                    .connection
                    .lock()
                    .map_err(|_| io::Error::other("Cache owner panicked"))?;
                connection.execute("INSERT INTO conflicts(edit_id, kind) VALUES (?1, ?2) ON CONFLICT(edit_id) DO UPDATE SET kind=excluded.kind", params![i64::try_from(intent.id).map_err(io::Error::other)?, kind as i64])?;
                return Ok(Some((intent.id, EditStatus::Conflict(kind))));
            }
        };
        if prepared.as_bytes() == snapshot {
            let store = Store::parse(&snapshot)?;
            let index = RevisionIndex::parse(&store)?;
            let revision = index.active(intent.space)?;
            if let Err(error) = remote.confirm(&snapshot) {
                if error.state == CommitState::Committed {
                    self.acknowledge(intent.id, revision, &snapshot)?;
                }
                return Err(error.into());
            }
            self.acknowledge(intent.id, revision, &snapshot)?;
            return Ok(Some((intent.id, EditStatus::Published { revision })));
        }
        let store = Store::parse(prepared.as_bytes())?;
        let index = RevisionIndex::parse(&store)?;
        let revision = index.active(intent.space)?;
        let before_store = Store::parse(&snapshot)?;
        let before = RevisionIndex::parse(&before_store)?;
        let mut revisions: BTreeMap<_, _> = index
            .spaces
            .iter()
            .filter_map(|(sid, space)| {
                let revision = *space.labels.get(&(ExGuid::default(), 1))?;
                (before
                    .spaces
                    .get(sid)
                    .and_then(|s| s.labels.get(&(ExGuid::default(), 1)))
                    != Some(&revision))
                .then_some((*sid, revision))
            })
            .collect();
        // The receipt's space can be unchanged in a multi-space edit.
        revisions.insert(intent.space, revision);
        {
            let mut connection = self
                .connection
                .lock()
                .map_err(|_| io::Error::other("Cache owner panicked"))?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute(
                "DELETE FROM conflicts WHERE edit_id=?1",
                [i64::try_from(intent.id).map_err(io::Error::other)?],
            )?;
            transaction.execute(
                "INSERT INTO attempt(id, edit_id, revisions) VALUES (1, ?1, ?2)",
                params![
                    i64::try_from(intent.id).map_err(io::Error::other)?,
                    serde_json::to_string(&revisions).map_err(io::Error::other)?
                ],
            )?;
            transaction.commit()?;
        }
        match remote.publish(&prepared) {
            Ok(()) => {}
            Err(error) if error.state == CommitState::NotCommitted => {
                let connection = self
                    .connection
                    .lock()
                    .map_err(|_| io::Error::other("Cache owner panicked"))?;
                connection.execute(
                    "DELETE FROM attempt WHERE edit_id=?1",
                    [i64::try_from(intent.id).map_err(io::Error::other)?],
                )?;
                return Err(error.into());
            }
            Err(error) if error.state == CommitState::Committed => {
                self.acknowledge(intent.id, revision, prepared.as_bytes())?;
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        }
        self.acknowledge(intent.id, revision, prepared.as_bytes())?;
        Ok(Some((intent.id, EditStatus::Published { revision })))
    }

    /// Repositions an unattempted page-creation conflict, retaining dependent object identities.
    pub fn rebase_page_creation_conflict(
        &self,
        id: u64,
        local: &[u8],
        remote: &[u8],
        before: Option<ExGuid>,
    ) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            let Operation::CreatePage(page) = intent.operation else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Select a page-creation conflict",
                )
                .into());
            };
            let page = page.reposition(before)?;
            PreparedEdit::create_page(remote, &page)?;
            Ok(Operation::CreatePage(page))
        })
    }

    /// Reviews a page batch against both cache images, retaining its page and series identities.
    pub fn rebase_pages_conflict(
        &self,
        id: u64,
        local: &[u8],
        remote: &[u8],
        edits: &[onestore::PageEdit],
    ) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            let Operation::Pages(batch) = intent.operation else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Select a page movement or indentation conflict",
                )
                .into());
            };
            Ok(Operation::Pages(batch.review(
                remote,
                intent.space,
                edits,
            )?))
        })
    }

    /// Replaces a conflicting page save with a model reviewed against the remote page.
    /// Both supplied images must match `snapshot` and `remote_snapshot`; the reviewed
    /// model must publish against the remote image, and its author is retained.
    pub fn review_page(&self, id: u64, local: &[u8], remote: &[u8], after: &Page) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            let Operation::Page(edit) = intent.operation else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Select a page save conflict",
                )
                .into());
            };
            let before = page_of(remote, intent.space)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "The page is no longer in the remote section",
                )
            })?;
            PreparedEdit::page(remote, intent.space, after, &edit.author)?;
            Ok(Operation::Page(PageIntent {
                before,
                after: after.clone(),
                author: edit.author,
            }))
        })
    }

    fn resolve_conflict(
        &self,
        id: u64,
        local: &[u8],
        remote: &[u8],
        update: impl FnOnce(PendingEdit) -> Result<Operation>,
    ) -> Result<()> {
        let owner = self.sync_owner()?;
        let intent = self
            .pending()?
            .into_iter()
            .next()
            .filter(|intent| intent.id == id)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Only the oldest conflict can be rebased",
                )
            })?;
        let operation = update(intent)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (base, working): (Vec<u8>, Vec<u8>) =
            transaction.query_row("SELECT base, working FROM replica WHERE id=1", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        if working != local || base != remote {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "The reviewed cache images changed",
            )
            .into());
        }
        let id = i64::try_from(id).map_err(io::Error::other)?;
        let eligible: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM conflicts WHERE edit_id=?1) AND NOT EXISTS(SELECT 1 FROM attempt)",
            [id], |row| row.get(0),
        )?;
        if !eligible {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The edit is not an unattempted conflict",
            )
            .into());
        }
        transaction.execute(
            "UPDATE edits SET operation=?1 WHERE id=?2",
            params![
                serde_json::to_string(&operation).map_err(io::Error::other)?,
                id
            ],
        )?;
        transaction.execute("DELETE FROM conflicts WHERE edit_id=?1", [id])?;
        transaction.commit()?;
        drop(connection);
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

    fn acknowledge(&self, id: u64, revision: ExGuid, snapshot: &[u8]) -> Result<()> {
        let id = i64::try_from(id).map_err(io::Error::other)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| io::Error::other("Cache owner panicked"))?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO receipts(edit_id, revision) VALUES (?1, ?2)",
            params![id, revision.to_string()],
        )?;
        transaction.execute("DELETE FROM edits WHERE id=?1", [id])?;
        transaction.execute("UPDATE replica SET base=?1, working=CASE WHEN EXISTS(SELECT 1 FROM edits) THEN working ELSE ?1 END WHERE id=1", [snapshot])?;
        transaction.commit()?;
        Ok(())
    }
}

pub(crate) fn status(connection: &Connection, id: u64) -> Result<Option<EditStatus>> {
    let id = i64::try_from(id).map_err(io::Error::other)?;
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
    let record: Option<(Option<String>, Option<i64>, String)> = connection.query_row(
        "SELECT attempt.revisions, conflicts.kind, edits.space FROM edits LEFT JOIN attempt ON attempt.edit_id=edits.id LEFT JOIN conflicts ON conflicts.edit_id=edits.id WHERE edits.id=?1", [id], |row| Ok((row.get(0)?,row.get(1)?, row.get(2)?))).optional()?;
    Ok(match record {
        None => None,
        Some((Some(revision), _, space)) => Some(EditStatus::AwaitingConfirmation {
            revision: {
                let space = space.parse()?;
                attempted_revisions(&revision, space)?[&space]
            },
        }),
        Some((None, Some(kind), _)) => Some(EditStatus::Conflict(match kind {
            0 => ConflictKind::TargetUnavailable,
            1 => ConflictKind::UnsupportedEdit,
            2 => ConflictKind::StructureChanged,
            3 => ConflictKind::ContentChanged,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unknown cached conflict kind",
                )
                .into());
            }
        })),
        Some((None, None, _)) => Some(EditStatus::Pending),
    })
}

fn attempted_revisions(encoded: &str, space: ExGuid) -> Result<BTreeMap<ExGuid, ExGuid>> {
    let revisions: BTreeMap<ExGuid, ExGuid> = serde_json::from_str(encoded)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if !revisions.contains_key(&space)
        || revisions
            .iter()
            .any(|(sid, rid)| sid.guid == [0; 16] || rid.guid == [0; 16])
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Cached publication evidence is incomplete",
        )
        .into());
    }
    Ok(revisions)
}
