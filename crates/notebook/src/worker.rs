use super::*;
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

pub(super) struct Signal {
    pub(crate) stopped: AtomicBool,
    pub(crate) offline: AtomicBool,
    /// FILETIME of the last step or poll that reached the remote; 0 before one has.
    synced: AtomicU64,
    /// Asked for by `wake`: steps run while offline until one leaves nothing to do.
    pub(crate) requested: AtomicBool,
    /// A watch on the file's folder wakes the worker when the file changes, so an idle worker
    /// waits for that instead of checking the file on its own (`Background::hold`).
    pub(crate) watched: AtomicBool,
    sender: SyncSender<()>,
}

impl Signal {
    pub(crate) fn new() -> (Arc<Self>, mpsc::Receiver<()>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        let signal = Self {
            stopped: AtomicBool::new(false),
            offline: AtomicBool::new(false),
            synced: AtomicU64::new(0),
            requested: AtomicBool::new(false),
            watched: AtomicBool::new(false),
            sender,
        };
        (Arc::new(signal), receiver)
    }

    pub(super) fn wake(&self) {
        // One retained notification covers edits that arrive during network I/O.
        let _ = self.sender.try_send(());
    }
}

/// Owns automatic reconciliation. Dropping requests cancellation without blocking.
/// The in-flight sync step finishes before ownership is released; `stop` waits for it.
pub struct SyncWorker {
    signal: Arc<Signal>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl SyncWorker {
    /// Requests a retry, for example after a network reachability change. Working
    /// offline, it synchronizes once, as OneNote's Sync Now does.
    /// A pending contention backoff finishes before processing the notification.
    pub fn wake(&self) {
        self.signal.requested.store(true, Ordering::Release);
        self.signal.wake();
    }

    /// When a step or poll last reached the remote.
    pub fn synced(&self) -> Option<u64> {
        Some(self.signal.synced.load(Ordering::Acquire)).filter(|time| *time != 0)
    }

    /// Working offline, the worker neither connects nor steps: local edits stay queued
    /// until `wake`, or until working online again.
    pub fn set_offline(&self, offline: bool) {
        self.signal.offline.store(offline, Ordering::Release);
        self.signal.wake();
    }

    /// Cancels future steps and waits for the current step and callback to finish.
    /// A stopped worker leaves pending edits and uncertain attempts in the cache.
    /// Call outside the worker's own callback, which cannot join its calling thread.
    pub fn stop(mut self) -> Result<()> {
        self.signal.stopped.store(true, Ordering::Release);
        self.signal.wake();
        self.thread
            .take()
            .expect("Worker owns its thread")
            .join()
            .map_err(|_| io::Error::other("Synchronization worker panicked"))?
    }
}

impl Drop for SyncWorker {
    fn drop(&mut self) {
        self.signal.stopped.store(true, Ordering::Release);
        self.signal.wake();
    }
}

impl Replica {
    /// Starts one worker, reconnecting through `connect` after transport failures.
    /// Local edits wake it; `interval` controls idle polling, unless a watch wakes it
    /// (`Background::hold`), and transport retries. While
    /// nothing is queued, or the queue waits on a remote that has not changed since, a
    /// remote whose `stamp` holds is not read again.
    /// Contended operations returning `NotCommitted` also back off by up to one second.
    /// `observe` runs on the worker after each attempt, including connection errors.
    /// Cache/document errors stop the worker; inspect them through `observe` or `stop`.
    /// Remote calls and callbacks must be bounded for `stop` to have bounded latency.
    pub fn start_sync<R, F, O>(
        self: &Arc<Self>,
        interval: Duration,
        mut connect: F,
        mut observe: O,
    ) -> io::Result<SyncWorker>
    where
        R: Remote + 'static,
        F: FnMut() -> io::Result<R> + Send + 'static,
        O: FnMut(&Result<Synced>) + Send + 'static,
    {
        if interval.is_zero() || Instant::now().checked_add(interval).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Synchronization interval must be positive and representable",
            ));
        }
        let mut owner = self
            .worker
            .lock()
            .map_err(|_| io::Error::other("Synchronization worker registration panicked"))?;
        if owner.upgrade().is_some() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let (signal, receiver) = Signal::new();
        let replica = Arc::clone(self);
        let worker_signal = Arc::clone(&signal);
        let thread = thread::Builder::new()
            .name("onestore-sync".into())
            .spawn(move || {
                let mut remote: Option<R> = None;
                let jitter = RandomState::new();
                let mut contention = 0_u32;
                // The first step, and the first after a failure, always runs, so `observe`
                // hears that the remote is reachable and what state the queue is in.
                let mut reported = false;
                // With nothing to do: a watched worker waits for the watch, any other looks
                // again after `interval`.
                let rest = || {
                    if worker_signal.watched.load(Ordering::Acquire) {
                        let _ = receiver.recv();
                    } else {
                        let _ = receiver.recv_timeout(interval);
                    }
                };
                while !worker_signal.stopped.load(Ordering::Acquire) {
                    if worker_signal.offline.load(Ordering::Acquire)
                        && !worker_signal.requested.load(Ordering::Acquire)
                    {
                        remote = None;
                        reported = false;
                        let _ = receiver.recv();
                        continue;
                    }
                    let result = match remote.as_mut() {
                        Some(remote) => {
                            if reported && replica.settled(remote).unwrap_or(false) {
                                worker_signal.requested.store(false, Ordering::Release);
                                worker_signal.synced.store(crate::now(), Ordering::Release);
                                rest();
                                continue;
                            }
                            let result = replica.sync_once(remote);
                            reported = result.is_ok();
                            result
                        }
                        None => match connect() {
                            Ok(connected) => {
                                remote = Some(connected);
                                continue;
                            }
                            Err(error) => Err(Error::RemoteIo(error)),
                        },
                    };
                    if result.is_ok() {
                        worker_signal.synced.store(crate::now(), Ordering::Release);
                    }
                    observe(&result);
                    let idle = matches!(result, Ok(Synced { edit: None, .. }));
                    match result {
                        Ok(Synced {
                            edit: Some((_, EditStatus::Published { .. })),
                            ..
                        }) => {
                            contention = 0;
                            continue;
                        }
                        Ok(Synced { edit: None, .. }) => contention = 0,
                        Ok(_) => {}
                        Err(Error::Remote(onestore::CommitError {
                            state: onestore::CommitState::NotCommitted,
                            ref error,
                        })) if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::ResourceBusy
                        ) =>
                        {
                            contention = contention.saturating_add(1);
                            let ceiling = (50_u64 << contention.min(5)).min(1000);
                            let until = Instant::now()
                                + Duration::from_millis(jitter.hash_one(contention) % ceiling);
                            // Local wakes must not keep competing writers in the same retry phase.
                            while !worker_signal.stopped.load(Ordering::Acquire) {
                                let Some(remaining) = until.checked_duration_since(Instant::now())
                                else {
                                    break;
                                };
                                let _ = receiver.recv_timeout(remaining);
                            }
                            continue;
                        }
                        Err(Error::RemoteIo(ref error))
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::ResourceBusy
                            ) => {}
                        Err(Error::RemoteIo(_) | Error::Remote(_)) => remote = None,
                        Err(Error::Io(ref error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                        Err(error) => return Err(error),
                    }
                    worker_signal.requested.store(false, Ordering::Release);
                    if idle {
                        rest();
                    } else if !worker_signal.stopped.load(Ordering::Acquire) {
                        let _ = receiver.recv_timeout(interval);
                    }
                }
                Ok(())
            })?;
        *owner = Arc::downgrade(&signal);
        Ok(SyncWorker {
            signal,
            thread: Some(thread),
        })
    }
}
