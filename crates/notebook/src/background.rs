//! The sections of an open notebook that no session holds, kept in sync as OneNote 2010
//! keeps every section of an open notebook: queued edits publish and other clients' changes
//! are noticed without the section being open.

use crate::{
    EditStatus, Error, Remote, Replica, Result,
    session::{SyncStatus, reached},
    worker::Signal,
};
use onestore::Stamp;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::Ordering},
    thread,
    time::Duration,
};

/// Polls each watched section file's stamp every interval. A section whose replica has
/// edits waiting, or whose file moved past the replica's base, has its replica opened for
/// the synchronization steps that publish or rebase it, then closed again; any other costs
/// one stamp read. A replica a session holds is left to that session's worker. Dropping
/// requests cancellation without waiting for the step in flight.
pub struct Background {
    signal: Arc<Signal>,
    watched: Arc<Mutex<Watched>>,
}

#[derive(Default)]
struct Watched {
    sections: Vec<Watch>,
    /// Sections whose file changed since `changed` was last asked.
    changed: Vec<String>,
}

struct Watch {
    path: String,
    replica: Option<PathBuf>,
    /// The file's stamp when last reached.
    stamp: Option<Stamp>,
    status: SyncStatus,
}

impl Background {
    pub(crate) fn start<R, B>(
        interval: Duration,
        mut connect: impl FnMut() -> io::Result<B> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self>
    where
        R: Remote,
        B: FnMut(&str) -> R,
    {
        let (signal, receiver) = Signal::new();
        let watched = Arc::new(Mutex::new(Watched::default()));
        let (shared, sections) = (Arc::clone(&signal), Arc::clone(&watched));
        thread::Builder::new()
            .name("onestore-background".into())
            .spawn(move || {
                let mut bound: Option<B> = None;
                while !shared.stopped.load(Ordering::Acquire) {
                    if shared.offline.load(Ordering::Acquire)
                        && !shared.requested.load(Ordering::Acquire)
                    {
                        bound = None;
                        let _ = receiver.recv();
                        continue;
                    }
                    shared.requested.store(false, Ordering::Release);
                    let round: Vec<_> = match sections.lock() {
                        Ok(watched) => watched
                            .sections
                            .iter()
                            .map(|watch| {
                                (
                                    watch.path.clone(),
                                    watch.replica.clone(),
                                    watch.stamp.clone(),
                                )
                            })
                            .collect(),
                        Err(_) => return,
                    };
                    let mut news = false;
                    for (path, replica, seen) in round {
                        if shared.stopped.load(Ordering::Acquire) {
                            return;
                        }
                        let bind = match &mut bound {
                            Some(bind) => bind,
                            None => match connect() {
                                Ok(bind) => bound.insert(bind),
                                Err(error) => {
                                    // Nothing on the share is reachable this round.
                                    let Ok(mut watched) = sections.lock() else {
                                        return;
                                    };
                                    for watch in &mut watched.sections {
                                        news |= watch.fail(
                                            &Error::RemoteIo(io::Error::new(
                                                error.kind(),
                                                error.to_string(),
                                            )),
                                            None,
                                        );
                                    }
                                    break;
                                }
                            },
                        };
                        let (queued, outcome) =
                            step(&mut bind(&path), replica.as_deref(), seen.as_ref());
                        if matches!(outcome, Err(Error::RemoteIo(_) | Error::Remote(_))) {
                            bound = None;
                        }
                        let Ok(mut watched) = sections.lock() else {
                            return;
                        };
                        let Watched { sections, changed } = &mut *watched;
                        let Some(watch) = sections.iter_mut().find(|watch| watch.path == path)
                        else {
                            continue;
                        };
                        news |= match outcome {
                            Ok((stamp, moved)) => {
                                watch.stamp = Some(stamp);
                                if moved {
                                    changed.push(path);
                                }
                                let before = summary(&watch.status);
                                if let Some(queued) = queued {
                                    watch.status = SyncStatus {
                                        synced: Some(crate::now()),
                                        error: None,
                                        queued,
                                    };
                                }
                                moved || before != summary(&watch.status)
                            }
                            Err(error) => watch.fail(&error, queued),
                        };
                    }
                    if news {
                        notify();
                    }
                    let _ = receiver.recv_timeout(interval);
                }
            })?;
        Ok(Self { signal, watched })
    }

    /// Keeps the sections of a notebook on a share in sync while they are not open;
    /// `connect` runs again after a transport failure. Paths are relative to `root`.
    #[cfg(feature = "smb")]
    pub fn smb(
        root: &str,
        limit: usize,
        interval: Duration,
        mut connect: impl FnMut() -> io::Result<crate::smb::Client> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let root = root.replace('\\', "/");
        Self::start(
            interval,
            move || {
                let client = Arc::new(connect()?);
                let root = root.clone();
                Ok(move |path: &str| {
                    let file = match root.as_str() {
                        "" => path.to_owned(),
                        root => format!("{root}/{path}"),
                    };
                    crate::SmbRemote::new(Arc::clone(&client), file, limit)
                })
            },
            notify,
        )
    }

    /// Watches these sections, by catalog path and replica (`Notebook::replicas`), from
    /// the next round, which starts now.
    pub fn watch(&self, sections: Vec<(String, Option<PathBuf>)>) {
        if let Ok(mut watched) = self.watched.lock() {
            let mut previous = std::mem::take(&mut watched.sections);
            watched.sections = sections
                .into_iter()
                .map(
                    |(path, replica)| match previous.iter().position(|watch| watch.path == path) {
                        Some(index) => Watch {
                            replica,
                            ..previous.swap_remove(index)
                        },
                        None => Watch {
                            path,
                            replica,
                            stamp: None,
                            status: SyncStatus {
                                synced: None,
                                error: None,
                                queued: 0,
                            },
                        },
                    },
                )
                .collect();
        }
        self.signal.wake();
    }

    /// Each watched section's status as its last round left it, in watch order. A section a
    /// session holds keeps the status it had before.
    pub fn status(&self) -> Vec<(String, SyncStatus)> {
        self.watched.lock().map_or_else(
            |_| Vec::new(),
            |watched| {
                watched
                    .sections
                    .iter()
                    .map(|watch| {
                        let status = &watch.status;
                        (
                            watch.path.clone(),
                            SyncStatus {
                                synced: status.synced,
                                error: status
                                    .error
                                    .as_ref()
                                    .map(|error| io::Error::new(error.kind(), error.to_string())),
                                queued: status.queued,
                            },
                        )
                    })
                    .collect()
            },
        )
    }

    /// Sections whose file changed since the last call, by catalog path.
    pub fn changed(&self) -> Vec<String> {
        self.watched
            .lock()
            .map(|mut watched| std::mem::take(&mut watched.changed))
            .unwrap_or_default()
    }

    /// Runs a round now, working offline included (Sync Now).
    pub fn wake(&self) {
        self.signal.requested.store(true, Ordering::Release);
        self.signal.wake();
    }

    /// Working offline, no round runs until `wake`, or until working online again.
    pub fn set_offline(&self, offline: bool) {
        self.signal.offline.store(offline, Ordering::Release);
        self.signal.wake();
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.signal.stopped.store(true, Ordering::Release);
        self.signal.wake();
    }
}

impl Watch {
    /// Records a failed step and what it found waiting, answering whether the status shown
    /// changes.
    fn fail(&mut self, error: &Error, queued: Option<u64>) -> bool {
        let before = summary(&self.status);
        self.status.error = Some(reached(error));
        self.status.queued = queued.unwrap_or(self.status.queued);
        before != summary(&self.status)
    }
}

/// What the host shows of a status.
fn summary(status: &SyncStatus) -> (bool, Option<io::ErrorKind>, u64) {
    (
        status.synced.is_some(),
        status.error.as_ref().map(io::Error::kind),
        status.queued,
    )
}

/// One section's step: how many of its edits wait (`None` while unknown, as while a session
/// holds its replica), then the file's stamp now and whether it changed since `seen`.
fn step<R: Remote>(
    remote: &mut R,
    replica: Option<&Path>,
    seen: Option<&Stamp>,
) -> (Option<u64>, Result<(Stamp, bool)>) {
    let stamp = match remote.stamp() {
        Ok(stamp) => stamp,
        Err(error) => return (None, Err(Error::RemoteIo(error))),
    };
    let moved = seen.is_some_and(|seen| *seen != stamp);
    let Some(replica) = replica.filter(|replica| replica.exists()) else {
        return (Some(0), Ok((stamp, moved)));
    };
    match crate::peek(replica) {
        Ok((base, 0)) if base == stamp => return (Some(0), Ok((stamp, moved))),
        Err(error) if error.busy() => return (None, Ok((stamp, false))),
        _ => {}
    }
    let replica = match Replica::open(replica) {
        Ok(replica) => replica,
        Err(error) if error.busy() => return (None, Ok((stamp, false))),
        Err(error) => return (None, Err(error)),
    };
    let synced = (|| {
        let mut changed = moved;
        loop {
            let synced = replica.sync_once(remote)?;
            changed |= !synced.changed.is_empty();
            if !matches!(synced.edit, Some((_, EditStatus::Published { .. }))) {
                return Ok((remote.stamp().map_err(Error::RemoteIo)?, changed));
            }
        }
    })();
    let queued = replica
        .recovery_summary()
        .ok()
        .map(|summary| summary.queued_edits);
    (queued, synced)
}
