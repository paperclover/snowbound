use super::*;
use onestore::{CommitError, CommitState};
use rusqlite::OptionalExtension;
use std::sync::{MutexGuard, TryLockError};

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
    TextChanged = 0,
    TargetUnavailable = 1,
    UnsupportedEdit = 2,
    FormattingChanged = 3,
    StructureChanged = 4,
    LayoutChanged = 5,
    ContentChanged = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditStatus {
    Pending,
    AwaitingConfirmation {
        revision: ExGuid,
    },
    Conflict(ConflictKind),
    /// Revision containing the confirmed effect; it can differ from a retired attempted revision.
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
            let attempted = transaction
                .query_row(
                    "SELECT revision FROM attempt WHERE edit_id=?1",
                    [i64::try_from(intent.id).map_err(io::Error::other)?],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            transaction.execute("UPDATE replica SET base=?1 WHERE id=1", [&snapshot])?;
            transaction.commit()?;
            (intent, attempted)
        };
        if let Some(revision) = attempted {
            let mut revision = revision.parse::<ExGuid>()?;
            let store = Store::parse(&snapshot)?;
            let index = RevisionIndex::parse(&store)?;
            if !index
                .spaces
                .get(&intent.space)
                .is_some_and(|space| space.revisions.contains_key(&revision))
            {
                let satisfied = match &intent.operation {
                    Operation::Format(edit) => edit
                        .prepare(&snapshot, intent.space)?
                        .is_ok_and(|prepared| prepared.as_bytes() == snapshot),
                    _ => false,
                };
                if !satisfied {
                    return Ok(Some((
                        intent.id,
                        EditStatus::AwaitingConfirmation { revision },
                    )));
                }
                revision = index.spaces[&intent.space].labels[&(ExGuid::default(), 1)];
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
            Operation::Format(edit) => edit.prepare(&snapshot, intent.space)?,
            Operation::Split(edit) => edit.prepare(&snapshot, intent.space)?,
            Operation::Join(edit) => edit.prepare(&snapshot, intent.space)?,
            Operation::Outline(edit) => edit.prepare(&snapshot, intent.space)?,
            Operation::Tree(edit) => edit.prepare(&snapshot, intent.space)?,
            Operation::Insert(insertion) => {
                PreparedEdit::insert(&snapshot, intent.space, insertion)
                    .map_err(|_| ConflictKind::UnsupportedEdit)
            }
            Operation::Text(edit) => paragraph(&snapshot, intent.space, edit.object)?
                .ok_or(ConflictKind::TargetUnavailable)
                .and_then(|text| {
                    crate::rebase::rebase(&edit.before, &text, edit.range.clone())
                        .ok_or(ConflictKind::TextChanged)
                })
                .and_then(|range| {
                    PreparedEdit::text(
                        &snapshot,
                        intent.space,
                        edit.object,
                        range,
                        &edit.replacement,
                    )
                    .map_err(|_| ConflictKind::UnsupportedEdit)
                }),
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
            let revision = index.spaces[&intent.space].labels[&(ExGuid::default(), 1)];
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
        let revision = index.spaces[&intent.space].labels[&(ExGuid::default(), 1)];
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
                "INSERT INTO attempt(id, edit_id, revision) VALUES (1, ?1, ?2)",
                params![
                    i64::try_from(intent.id).map_err(io::Error::other)?,
                    revision.to_string()
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

    /// Places the oldest text, formatting or split conflict at a reviewed remote UTF-16 range.
    /// The requested replacement/attributes, local image, intent ID and later edits are preserved.
    /// Both supplied images must match `snapshot` and `remote_snapshot`; stale review
    /// returns `Io(ResourceBusy)`. Uncertain publication attempts cannot be rebased.
    pub fn rebase_conflict(
        &self,
        id: u64,
        local: &[u8],
        remote: &[u8],
        range: Range<u32>,
    ) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            Ok(match intent.operation {
                Operation::Text(mut edit) => {
                    let prepared = PreparedEdit::text(
                        remote,
                        intent.space,
                        edit.object,
                        range.clone(),
                        &edit.replacement,
                    )?;
                    if prepared.as_bytes() == remote {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "The selected range already contains the replacement",
                        )
                        .into());
                    }
                    edit.before =
                        paragraph(remote, intent.space, edit.object)?.ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "The remote text target is unavailable",
                            )
                        })?;
                    edit.range = range;
                    Operation::Text(edit)
                }
                Operation::Format(edit) => Operation::Format(
                    FormatEdit::capture(
                        remote,
                        intent.space,
                        edit.object,
                        range,
                        &edit.attributes,
                    )?
                    .0,
                ),
                Operation::Split(edit) => {
                    if !range.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "Choose a single split boundary",
                        )
                        .into());
                    }
                    Operation::Split(
                        crate::paragraph::SplitEdit::capture(
                            remote,
                            intent.space,
                            &edit.intent.reposition(range.start),
                        )?
                        .0,
                    )
                }
                Operation::Join(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Review the paragraph join using rebase_join_conflict",
                    )
                    .into());
                }
                Operation::Outline(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Review layout changes using rebase_layout_conflict",
                    )
                    .into());
                }
                Operation::Tree(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Review the subtree edit using rebase_tree_conflict",
                    )
                    .into());
                }
                Operation::Insert(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Insertion conflicts require a placement, not a text range",
                    )
                    .into());
                }
            })
        })
    }

    /// Reviews a join against both current cache images, retaining its original text identities.
    /// The surviving text identity must remain the same for dependent edits.
    pub fn rebase_join_conflict(&self, id: u64, local: &[u8], remote: &[u8]) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            let Operation::Join(edit) = intent.operation else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Select a paragraph join conflict",
                )
                .into());
            };
            Ok(Operation::Join(edit.review(remote, intent.space)?))
        })
    }

    /// Repositions the oldest paragraph insertion conflict against reviewed cache images.
    /// Object identities and dependent edits are retained; uncertain attempts cannot be moved.
    pub fn rebase_paragraph_conflict(
        &self,
        id: u64,
        local: &[u8],
        remote: &[u8],
        parent: ExGuid,
        before: Option<ExGuid>,
    ) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            let Operation::Insert(insertion) = intent.operation else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Select a paragraph insertion conflict",
                )
                .into());
            };
            let insertion = insertion.reposition_paragraph(parent, before)?;
            PreparedEdit::insert(remote, intent.space, &insertion)?;
            Ok(Operation::Insert(insertion))
        })
    }

    /// Repositions the oldest outline insertion conflict against reviewed cache images.
    /// Object identities and dependent edits are retained; uncertain attempts cannot be moved.
    pub fn rebase_outline_conflict(
        &self,
        id: u64,
        local: &[u8],
        remote: &[u8],
        page: ExGuid,
        x: f32,
        y: f32,
    ) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            let Operation::Insert(insertion) = intent.operation else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Select an outline insertion conflict",
                )
                .into());
            };
            let insertion = insertion.reposition_outline(page, x, y)?;
            PreparedEdit::insert(remote, intent.space, &insertion)?;
            Ok(Operation::Insert(insertion))
        })
    }

    /// Reviews the original layout intent against both current cache images.
    /// Competing property values are replaced only after this explicit review.
    pub fn rebase_layout_conflict(&self, id: u64, local: &[u8], remote: &[u8]) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            let Operation::Outline(edit) = intent.operation else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Select an outline layout conflict",
                )
                .into());
            };
            Ok(Operation::Outline(
                OutlineEdit::capture(remote, intent.space, edit.object, edit.change)?.0,
            ))
        })
    }

    /// Reviews the original subtree intent against both current cache images.
    /// Replacement paragraph identities must remain unchanged for dependent edits.
    pub fn rebase_tree_conflict(&self, id: u64, local: &[u8], remote: &[u8]) -> Result<()> {
        self.resolve_conflict(id, local, remote, |intent| {
            let Operation::Tree(edit) = intent.operation else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Select a subtree move or deletion conflict",
                )
                .into());
            };
            Ok(Operation::Tree(edit.review(remote, intent.space)?))
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
    let record: Option<(Option<String>, Option<i64>)> = connection.query_row(
        "SELECT attempt.revision, conflicts.kind FROM edits LEFT JOIN attempt ON attempt.edit_id=edits.id LEFT JOIN conflicts ON conflicts.edit_id=edits.id WHERE edits.id=?1", [id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
    Ok(match record {
        None => None,
        Some((Some(revision), _)) => Some(EditStatus::AwaitingConfirmation {
            revision: revision.parse()?,
        }),
        Some((None, Some(kind))) => Some(EditStatus::Conflict(match kind {
            0 => ConflictKind::TextChanged,
            1 => ConflictKind::TargetUnavailable,
            2 => ConflictKind::UnsupportedEdit,
            3 => ConflictKind::FormattingChanged,
            4 => ConflictKind::StructureChanged,
            5 => ConflictKind::LayoutChanged,
            6 => ConflictKind::ContentChanged,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unknown cached conflict kind",
                )
                .into());
            }
        })),
        Some((None, None)) => Some(EditStatus::Pending),
    })
}
