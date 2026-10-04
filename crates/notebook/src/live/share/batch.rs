use super::*;
use crate::{PendingEdit, merge};
use onestore::{Arena, ExGuid, Section, op::Edit};
use std::collections::BTreeSet;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edits {
    pub edits: Vec<(String, Edit)>,
    pub revisions: BTreeMap<ExGuid, ExGuid>,
}

pub(super) struct Waiting {
    peer: [u8; 16],
    request: Request,
    reply: mpsc::Sender<Reply>,
}

fn retry(message: &str) -> Error {
    Error::Remote(CommitError {
        state: CommitState::NotCommitted,
        error: io::Error::new(io::ErrorKind::ResourceBusy, message.to_owned()),
    })
}

impl Served {
    pub(super) fn batch(self: &Arc<Self>, peer: &[u8; 16], request: Request) -> Result<Reply> {
        let path = request.path.clone();
        let mut writers = self.writers.lock().unwrap();
        let writer = writers.entry(path.clone()).or_insert_with(|| {
            let (send, receive) = mpsc::sync_channel::<Waiting>(64);
            let served = Arc::downgrade(self);
            thread::spawn(move || {
                while let Ok(first) = receive.recv() {
                    let mut waiting = vec![first];
                    if let Ok(next) = receive.recv_timeout(Duration::from_millis(5)) {
                        waiting.push(next);
                    }
                    waiting.extend(receive.try_iter().take(62));
                    let Some(served) = served.upgrade() else {
                        return;
                    };
                    served.write_batches(&path, waiting);
                }
            });
            send
        });
        let (reply, receive) = mpsc::channel();
        writer
            .try_send(Waiting {
                peer: *peer,
                request,
                reply,
            })
            .map_err(|_| retry("The section's writer is busy"))?;
        drop(writers);
        receive.recv_timeout(TIMEOUT).map_err(|_| {
            Error::Remote(CommitError {
                state: CommitState::Unknown,
                error: io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The section's writer did not answer",
                ),
            })
        })
    }

    fn write_batches(&self, path: &str, waiting: Vec<Waiting>) {
        let before = match self.image(path) {
            Ok(image) => image,
            Err(error) => {
                for batch in waiting {
                    let _ = batch.reply.send(failed(0, retry(&error.to_string())));
                }
                return;
            }
        };
        let mut image = Arc::clone(&before);
        let mut combined: Option<Transaction> = None;
        let mut accepted = Vec::new();
        for batch in waiting {
            match self.prepare_batch(path, &image, &batch) {
                Ok(Some(transaction)) => {
                    let mut next = (*image).clone();
                    let extended =
                        transaction
                            .apply(&mut next)
                            .and_then(|()| match &mut combined {
                                Some(combined) => combined.extend(transaction),
                                None => {
                                    combined = Some(transaction);
                                    Ok(())
                                }
                            });
                    match extended {
                        Ok(()) => {
                            image = Arc::new(next);
                            accepted.push(batch);
                        }
                        Err(error) => {
                            let _ = batch.reply.send(failed(0, error.into()));
                        }
                    }
                }
                Ok(None) => accepted.push(batch),
                Err(error) => {
                    let _ = batch.reply.send(failed(0, error));
                }
            }
        }
        if accepted.is_empty() {
            return;
        }
        let result = match &combined {
            Some(transaction) => self.storage.commit(path, transaction),
            None => self
                .storage
                .confirm(path, &Stamp::of(&image).expect("section stamp"))
                .map_err(Error::from),
        };
        if result.is_ok() {
            self.tell_delta(path, &before, &image, None);
            self.keep(path, Arc::clone(&image));
            self.changed(&[path.to_owned()]);
        }
        for batch in accepted {
            let reply = match &result {
                Ok(()) => Reply {
                    stamp: Some((&Stamp::of(&image).expect("section stamp")).into()),
                    ..Reply::default()
                },
                Err(Error::Remote(error)) => failed(
                    0,
                    Error::Remote(CommitError {
                        state: error.state,
                        error: io::Error::new(error.error.kind(), error.error.to_string()),
                    }),
                ),
                Err(error) => failed(0, retry(&error.to_string())),
            };
            let _ = batch.reply.send(reply);
        }
    }

    fn prepare_batch(
        &self,
        path: &str,
        image: &[u8],
        batch: &Waiting,
    ) -> Result<Option<Transaction>> {
        let stamp: Stamp = batch
            .request
            .stamp
            .as_ref()
            .ok_or_else(|| retry("No batch base"))?
            .try_into()?;
        let edits: Edits = serde_json::from_slice(&self.carried(&batch.peer, &batch.request)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if edits.revisions.is_empty()
            || edits
                .revisions
                .values()
                .any(|revision| revision.guid == [0; 16])
        {
            return Err(retry("No batch revisions"));
        }
        let store = Store::parse(image)?;
        let index = RevisionIndex::parse(&store)?;
        if edits.revisions.iter().all(|(space, revision)| {
            index
                .spaces
                .get(space)
                .is_some_and(|space| space.revisions.contains_key(revision))
        }) {
            return Ok(None);
        }
        let names = edits
            .revisions
            .iter()
            .filter(|(space, revision)| {
                !index
                    .spaces
                    .get(space)
                    .is_some_and(|space| space.revisions.contains_key(revision))
            })
            .map(|(space, revision)| (*space, *revision))
            .collect();
        let arena = Arena::default();
        let mut next = Section::open(&arena, image.to_vec())?;
        let queued: Vec<PendingEdit> = edits
            .edits
            .into_iter()
            .enumerate()
            .map(|(id, (author, edit))| PendingEdit {
                id: id as u64,
                author,
                edit,
            })
            .collect();
        if Stamp::of(image)? == stamp {
            for edit in &queued {
                next.apply(&edit.author, &edit.edit)?;
            }
        } else {
            let base = self
                .images
                .lock()
                .unwrap()
                .iter()
                .find_map(|(held, at, image)| {
                    (held == path && *at == stamp).then(|| Arc::clone(image))
                })
                .ok_or_else(|| retry("The batch's base is no longer held"))?;
            let old_arena = Arena::default();
            let mut old = Section::open(&old_arena, (*base).clone())?;
            if old.root() != next.root() {
                return Err(retry("The batch belongs to another section"));
            }
            let merged = merge::rebase(&mut old, &mut next, &queued, &BTreeSet::new())?;
            if !merged.conflicts.is_empty() {
                let local_arena = Arena::default();
                let mut local = Section::open(&local_arena, (*base).clone())?;
                for edit in &queued {
                    local.apply(&edit.author, &edit.edit)?;
                }
                for (space, (author, objects)) in &merged.conflicts {
                    let page = local.page(*space)?;
                    if next.page(*space).is_ok_and(|remote| remote == page) {
                        return Err(retry("The batch's edits already landed"));
                    }
                    merge::conflict_page(
                        &mut next,
                        &mut local,
                        &merged.moved,
                        *space,
                        &page,
                        author,
                        objects,
                        crate::now(),
                        None,
                    )?;
                }
            }
        }
        let transaction = next.seal_as(&names)?;
        if edits.revisions.iter().any(|(space, revision)| {
            !next
                .newest()
                .any(|(held, at)| held == *space && at == *revision)
        }) {
            return Err(refused(
                io::ErrorKind::Unsupported,
                "The batch needs a guest's commit",
            ));
        }
        Ok(transaction)
    }
}
