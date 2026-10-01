//! The sections of an open notebook that no session holds, kept in sync as OneNote 2010
//! keeps every section of an open notebook: queued edits publish and other clients' changes
//! are noticed without the section being open.

use crate::fs;
use crate::{
    EditStatus, Error, Remote, Replica, Result,
    discover::{Entry, Listed},
    session::{Section, SyncStatus, reached},
    worker::Signal,
};
use onestore::Stamp;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use web_time::Instant;

/// How long a section that could not be reached waits to be tried again; OneNote 2010
/// retries a failing section about every 31 seconds.
const RETRY: Duration = Duration::from_secs(31);
/// Between one section's first check and the next's, so opening a notebook reads its files
/// one after another rather than in a burst.
const STAGGER: Duration = Duration::from_millis(100);
/// How long a reported change settles before its section is checked, so the writes of one
/// commit cost one check.
const SETTLE: Duration = Duration::from_secs(1);

/// Keeps each watched section in sync: a section is checked when a watch on the notebook's
/// folder reports it changed, when it could not be reached, and otherwise every interval.
/// On connecting, and when a watch reports a folder rather than a file, the folders are
/// listed and only the sections listed otherwise than before are checked, as OneNote 2010
/// reopens a notebook. A check costs one stamp read; a section whose replica has edits waiting,
/// or whose file moved past the replica's base, has its replica opened for the synchronization
/// steps that publish or rebase it, then closed again. A section without a replica gets one,
/// its offline copy, where the notebook keeps them. A section a session holds is left to that
/// session's worker, which the watch wakes. Dropping requests cancellation without waiting for
/// the step in flight.
pub struct Background(Arc<Shared>);

/// A section for `Background::watch`, as its notebook knows it.
pub struct Known {
    /// The catalog path.
    pub path: String,
    pub replica: Option<PathBuf>,
    /// How the notebook's discovery last found the file: its listing, and its stamp then.
    pub found: Option<(Listed, Stamp)>,
    /// The file as that discovery read it, if it did, which the first check takes in place
    /// of reading the file again while its stamp still matches.
    pub image: Option<Vec<u8>>,
}

/// What the background thread and whatever reports changes to it share.
pub(crate) struct Shared {
    signal: Arc<Signal>,
    watched: Mutex<Watched>,
    /// Asked for by `discard`, once the thread stops.
    discard: AtomicBool,
}

/// A connection's report of its notebook folder's changes, from a watch armed on connecting.
#[cfg_attr(not(feature = "smb"), allow(dead_code))]
pub(crate) struct Reports {
    shared: Weak<Shared>,
    connection: u64,
}

#[derive(Default)]
struct Watched {
    sections: Vec<Watch>,
    /// Sections whose file changed since `changed` was last asked.
    changed: Vec<String>,
    /// Folders a watch reported without naming the file, listed at `relist`.
    folders: BTreeSet<String>,
    relist: Option<Instant>,
    /// Counts connections, so a watch that ended with an earlier one is not this one's.
    connection: u64,
    /// The current connection's watch ended, or none was connected: changes since went
    /// unreported.
    lost: bool,
    /// The current connection's watch reports the folder's changes.
    reported: bool,
}

struct Watch {
    path: String,
    replica: Option<PathBuf>,
    /// The file's stamp when last read.
    stamp: Option<Stamp>,
    /// How the file was listed when its folder was last listed; the stamp was read since.
    listed: Option<Listed>,
    /// The folder's listing since `stamp` shows the file unchanged, so the next check need not
    /// read it.
    current: bool,
    /// A watch reported the file changed since its folder's listing began.
    reported: bool,
    status: SyncStatus,
    /// The file as discovery read it, until the next check takes it.
    image: Option<Vec<u8>>,
    /// When the section is next checked.
    due: Instant,
    /// The worker of the session that holds the section.
    held: Option<Weak<Signal>>,
}

impl Background {
    /// How often each section is checked while a watch reports its folder's changes, against
    /// a report that went missing; OneNote 2010, watching, checks nothing on a timer.
    pub const BACKSTOP: Duration = Duration::from_secs(60 * 60);
    /// How often each section is checked where nothing reports its folder's changes.
    pub const UNWATCHED: Duration = Duration::from_secs(15);

    /// Starts the thread. `connect` answers how the connection reaches each section file and
    /// lists each folder, both by catalog path, arming any watch that reports the folder's
    /// changes through `Reports`, and whether one does; it runs again after a transport
    /// failure. `copies` keeps an offline copy of every section.
    pub(crate) fn start<R, B, L>(
        copies: bool,
        mut connect: impl FnMut(Reports) -> io::Result<((B, L), bool)> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self>
    where
        R: Remote,
        B: FnMut(&str) -> R,
        L: FnMut(&str) -> io::Result<Vec<Entry>>,
    {
        let (signal, receiver) = Signal::new();
        let shared = Arc::new(Shared {
            signal,
            watched: Mutex::default(),
            discard: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&shared);
        let owner = Arc::clone(&shared);
        crate::task::spawn("onestore-background", move || async move {
            let signal = &owner.signal;
            let mut bound: Option<(B, L)> = None;
            let mut news = false;
            while !signal.stopped.load(Ordering::Acquire) {
                if signal.offline.load(Ordering::Acquire)
                    && !signal.requested.load(Ordering::Acquire)
                {
                    // Working online again lists every folder again.
                    if let Ok(mut watched) = owner.watched.lock() {
                        watched.watching(false);
                        watched.lost = true;
                    }
                    bound = None;
                    crate::task::wait(&receiver, None).await;
                    continue;
                }
                let now = Instant::now();
                let next = {
                    let Ok(mut watched) = owner.watched.lock() else {
                        return;
                    };
                    if std::mem::take(&mut watched.lost) {
                        bound = None;
                        watched.watching(false);
                        for watch in &mut watched.sections {
                            watch.due = now;
                        }
                    }
                    watched.next(now, bound.is_some())
                };
                let (path, replica, seen, current, image) = match next {
                    Next::Wait(wait) => {
                        signal.requested.store(false, Ordering::Release);
                        if std::mem::take(&mut news) {
                            notify();
                        }
                        crate::task::wait(&receiver, wait).await;
                        continue;
                    }
                    Next::Connect(connection) => {
                        let reports = Reports {
                            shared: weak.clone(),
                            connection: connection + 1,
                        };
                        let connected = connect(reports);
                        let Ok(mut watched) = owner.watched.lock() else {
                            return;
                        };
                        // Whatever ended before now ended with the last connection.
                        watched.connection += 1;
                        watched.lost = false;
                        match connected {
                            Ok((mut files, reported)) => {
                                watched.watching(reported);
                                let folders = watched.rescanning();
                                drop(watched);
                                let listings = list(&mut files.1, folders);
                                let Ok(mut watched) = owner.watched.lock() else {
                                    return;
                                };
                                watched.listed(&listings, Some(copies), Instant::now());
                                bound = Some(files);
                            }
                            Err(error) => {
                                let error = Error::RemoteIo(error);
                                let retry = now + RETRY;
                                for watch in &mut watched.sections {
                                    news |= watch.fail(&error, None);
                                    watch.due = watch.due.max(retry);
                                }
                                watched.relist = watched.relist.map(|due| due.max(retry));
                            }
                        }
                        continue;
                    }
                    Next::List(folders) => {
                        let Some((_, list_folder)) = &mut bound else {
                            continue;
                        };
                        let listings = list(list_folder, folders);
                        let Ok(mut watched) = owner.watched.lock() else {
                            return;
                        };
                        watched.listed(&listings, None, Instant::now());
                        continue;
                    }
                    Next::Check {
                        path,
                        replica,
                        seen,
                        current,
                        image,
                    } => (path, replica, seen, current, image),
                };
                let Some((bind, _)) = &mut bound else {
                    continue;
                };
                let (queued, outcome) = step(
                    &mut bind(&path),
                    replica.as_deref(),
                    seen.as_deref(),
                    current,
                    image,
                    copies,
                );
                if outcome.as_ref().is_err_and(disconnected) {
                    bound = None;
                }
                let Ok(mut watched) = owner.watched.lock() else {
                    return;
                };
                let interval = watched.interval();
                let Watched {
                    sections, changed, ..
                } = &mut *watched;
                let Some(watch) = sections.iter_mut().find(|watch| watch.path == path) else {
                    continue;
                };
                let now = Instant::now();
                watch.current = false;
                news |= match outcome {
                    Ok((stamp, moved)) => {
                        watch.stamp = Some(stamp);
                        watch.due = now + interval;
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
                    Err(error) => {
                        watch.due = now + RETRY;
                        watch.fail(&error, queued)
                    }
                };
            }
            if let Ok(mut watched) = owner.watched.lock() {
                watched.watching(false);
                if owner.discard.load(Ordering::Acquire) {
                    for replica in watched
                        .sections
                        .iter()
                        .filter_map(|watch| watch.replica.as_ref())
                    {
                        discard(replica);
                    }
                }
            }
        })?;
        Ok(Self(shared))
    }

    /// Keeps the sections of a notebook on a share in sync while they are not open, with an
    /// offline copy of each, and one watch on the notebook's folder reporting what changed,
    /// as OneNote 2010 watches it; `connect` runs again after a transport failure. Paths are
    /// relative to `root`.
    #[cfg(feature = "smb")]
    pub fn smb(
        root: &str,
        limit: usize,
        mut connect: impl FnMut() -> io::Result<crate::smb::Client> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let root = root.replace('\\', "/");
        Self::start(
            true,
            move |reports| {
                let client = Arc::new(connect()?);
                let reported = match client.watch(&root, move |changed| match changed {
                    Ok(paths) => reports.touched(&paths),
                    Err(_) => reports.lost(),
                }) {
                    Ok(()) => true,
                    Err(error) if error.kind() == io::ErrorKind::Unsupported => false,
                    Err(error) => return Err(error),
                };
                let (bound, folder) = (Arc::clone(&client), root.clone());
                let bind = move |path: &str| {
                    let file = match folder.as_str() {
                        "" => path.to_owned(),
                        root => format!("{root}/{path}"),
                    };
                    crate::SmbRemote::new(Arc::clone(&bound), file, limit)
                };
                let root = root.clone();
                let list = move |folder: &str| {
                    use crate::discover::Source;
                    crate::discover::Smb::new(&client, &root)?
                        .entries(folder, crate::session::LIMITS.entries)
                };
                Ok(((bind, list), reported))
            },
            notify,
        )
    }

    /// Watches these sections (`Notebook::replicas`); a section not watched before is first
    /// checked soon after, the next one a little later.
    pub fn watch(&self, sections: Vec<Known>) {
        if let Ok(mut watched) = self.0.watched.lock() {
            watched.watch(sections, Instant::now());
        }
        self.0.signal.wake();
    }

    /// Leaves the section at catalog `path` to `section`'s worker, which from now on the watch
    /// on the notebook's folder wakes when the file changes, instead of the worker checking
    /// it every few seconds. Once the section closes, this takes it over again.
    pub fn hold(&self, path: &str, section: &Section) {
        let replica = section.replica();
        let Some(worker) = replica
            .section
            .worker
            .lock()
            .ok()
            .and_then(|worker| worker.upgrade())
        else {
            return;
        };
        if let Ok(mut watched) = self.0.watched.lock() {
            let reported = watched.reported;
            if let Some(watch) = watched.sections.iter_mut().find(|watch| watch.path == path) {
                worker.watched.store(reported, Ordering::Release);
                watch.held = Some(Arc::downgrade(&worker));
            }
        }
        let (shared, path) = (Arc::downgrade(&self.0), path.to_owned());
        if let Ok(mut released) = replica.released.0.lock() {
            *released = Some(Box::new(move || {
                if let Some(shared) = shared.upgrade() {
                    shared.released(&path);
                }
            }));
        }
    }

    /// Checks the sections at or below these catalog paths soon, as a watch on the notebook's
    /// folder reports them changed: a section's own path checks it, a folder's lists it first
    /// and checks the sections listed otherwise. `""` names the notebook's folder.
    pub fn touched(&self, paths: &[String]) {
        self.0.touched(paths);
    }

    /// Each watched section's status as its last check left it, in watch order. A section a
    /// session holds keeps the status it had before.
    pub fn status(&self) -> Vec<(String, SyncStatus)> {
        self.0.watched.lock().map_or_else(
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
        self.0
            .watched
            .lock()
            .map(|mut watched| std::mem::take(&mut watched.changed))
            .unwrap_or_default()
    }

    /// Reads every section's stamp now, working offline included (Sync Now).
    pub fn wake(&self) {
        if let Ok(mut watched) = self.0.watched.lock() {
            let now = Instant::now();
            for watch in &mut watched.sections {
                watch.due = now;
                watch.current = false;
            }
        }
        self.0.signal.requested.store(true, Ordering::Release);
        self.0.signal.wake();
    }

    /// Stops for good, then deletes each section's replica that holds nothing unpublished, as
    /// OneNote lets go of a notebook it closes; one with edits waiting, or that a session
    /// holds, stays.
    pub fn discard(&self) {
        self.0.discard.store(true, Ordering::Release);
        self.0.signal.stopped.store(true, Ordering::Release);
        self.0.signal.wake();
    }

    /// Working offline, nothing is checked until `wake`, or until working online again, which
    /// lists every folder again.
    pub fn set_offline(&self, offline: bool) {
        self.0.signal.offline.store(offline, Ordering::Release);
        self.0.signal.wake();
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.0.signal.stopped.store(true, Ordering::Release);
        self.0.signal.wake();
    }
}

impl Shared {
    fn touched(&self, paths: &[String]) {
        if let Ok(mut watched) = self.watched.lock() {
            watched.touched(paths, Instant::now());
        }
        self.signal.wake();
    }

    /// The session holding the section at `path` let it go: it is checked now.
    fn released(&self, path: &str) {
        if let Ok(mut watched) = self.watched.lock()
            && let Some(watch) = watched.sections.iter_mut().find(|watch| watch.path == path)
        {
            watch.held = None;
            watch.current = false;
            watch.due = Instant::now();
        }
        self.signal.wake();
    }
}

#[cfg_attr(not(feature = "smb"), allow(dead_code))]
impl Reports {
    /// The sections at or below these paths changed.
    pub(crate) fn touched(&self, paths: &[String]) {
        if let Some(shared) = self.shared.upgrade() {
            shared.touched(paths);
        }
    }

    /// The watch ended, and with it, as far as anyone can tell, the connection.
    pub(crate) fn lost(&self) {
        if let Some(shared) = self.shared.upgrade() {
            if let Ok(mut watched) = shared.watched.lock()
                && watched.connection == self.connection
            {
                watched.lost = true;
            }
            shared.signal.wake();
        }
    }
}

/// What the background thread does next.
enum Next {
    Wait(Option<Duration>),
    /// Connects, as the current connection is `.0`, then lists every folder.
    Connect(u64),
    /// Lists these folders, checking the sections listed otherwise.
    List(BTreeSet<String>),
    Check {
        path: String,
        replica: Option<PathBuf>,
        seen: Option<Box<Stamp>>,
        current: bool,
        image: Option<Vec<u8>>,
    },
}

impl Watched {
    fn watch(&mut self, sections: Vec<Known>, now: Instant) {
        let mut previous = std::mem::take(&mut self.sections);
        let mut new = 0;
        self.sections = sections
            .into_iter()
            .map(
                |known| match previous.iter().position(|watch| watch.path == known.path) {
                    Some(index) => Watch {
                        replica: known.replica,
                        image: known.image,
                        ..previous.swap_remove(index)
                    },
                    None => {
                        new += 1;
                        let (listed, stamp) = known.found.unzip();
                        Watch {
                            path: known.path,
                            replica: known.replica,
                            stamp,
                            listed,
                            current: false,
                            reported: false,
                            status: SyncStatus {
                                synced: None,
                                error: None,
                                queued: 0,
                            },
                            image: known.image,
                            due: now + STAGGER * (new - 1),
                            held: None,
                        }
                    }
                },
            )
            .collect();
    }

    fn interval(&self) -> Duration {
        if self.reported {
            Background::BACKSTOP
        } else {
            Background::UNWATCHED
        }
    }

    /// Whether a watch reports the folder's changes from now on, as the held sessions' workers
    /// learn.
    fn watching(&mut self, reported: bool) {
        self.reported = reported;
        for signal in self
            .sections
            .iter()
            .filter_map(|watch| watch.held.as_ref()?.upgrade())
        {
            signal.watched.store(reported, Ordering::Release);
            signal.wake();
        }
    }

    fn touched(&mut self, paths: &[String], now: Instant) {
        for path in paths {
            let path = path.trim_matches('/');
            let section = self
                .sections
                .iter()
                .position(|watch| watch.path.eq_ignore_ascii_case(path));
            match section {
                Some(index) => {
                    let watch = &mut self.sections[index];
                    watch.due = watch.due.min(now + SETTLE);
                    watch.current = false;
                    watch.reported = true;
                }
                None if self.sections.iter().any(|watch| within(&watch.path, path)) => {
                    self.folders.insert(path.to_owned());
                    self.relist = Some(
                        self.relist
                            .map_or(now + SETTLE, |due| due.min(now + SETTLE)),
                    );
                }
                None => {}
            }
        }
    }

    /// Every folder holding a section, to list them all, as a listing begins.
    fn rescanning(&mut self) -> BTreeSet<String> {
        for watch in &mut self.sections {
            watch.reported = false;
        }
        self.parents(&[String::new()])
    }

    /// The folders holding the sections at or below `paths`.
    fn parents<'a>(&self, paths: impl IntoIterator<Item = &'a String> + Clone) -> BTreeSet<String> {
        self.sections
            .iter()
            .filter(|watch| {
                paths
                    .clone()
                    .into_iter()
                    .any(|path| within(&watch.path, path))
            })
            .map(|watch| split(&watch.path).0.to_owned())
            .collect()
    }

    /// Takes the folders' listings. With `rescan`, as on connecting (whether copies are kept),
    /// every section of a listed folder is checked, from now: first those whose file lists as
    /// when its stamp was last read, which need no reading unless for a copy, then the others
    /// one `STAGGER` after another. Otherwise only the sections listed otherwise are, as are
    /// those a watch reported meanwhile either way.
    fn listed(
        &mut self,
        listings: &BTreeMap<String, Vec<Entry>>,
        rescan: Option<bool>,
        now: Instant,
    ) {
        let mut staggered = 0;
        for watch in &mut self.sections {
            let (folder, name) = split(&watch.path);
            let Some(entries) = listings.get(folder) else {
                continue;
            };
            let listed = entries
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(name))
                .map(|entry| entry.listed);
            let unchanged = watch.stamp.is_some() && listed.is_some() && listed == watch.listed;
            watch.listed = listed;
            match rescan {
                // A report while listing may name a change the listing missed.
                Some(_) if watch.reported => watch.current = false,
                Some(copies) => {
                    watch.current = unchanged;
                    let read = !unchanged
                        || copies
                            && watch.image.is_none()
                            && watch
                                .replica
                                .as_ref()
                                .is_some_and(|replica| fs::metadata(replica).is_err());
                    watch.due = if read {
                        staggered += 1;
                        now + STAGGER * (staggered - 1)
                    } else {
                        now
                    };
                }
                None if !unchanged => {
                    watch.current = false;
                    watch.due = watch.due.min(now);
                }
                None => {}
            }
        }
    }

    /// What to do at `now`, waking the held sections that are due; `bound` while connected.
    fn next(&mut self, now: Instant, bound: bool) -> Next {
        let interval = self.interval();
        loop {
            let due = (0..self.sections.len()).min_by_key(|&index| self.sections[index].due);
            let at = due.map(|index| self.sections[index].due);
            if let Some(relist) = self.relist
                && relist <= now
                && at.is_none_or(|at| relist <= at)
            {
                if !bound {
                    return Next::Connect(self.connection);
                }
                let folders = std::mem::take(&mut self.folders);
                self.relist = None;
                return Next::List(self.parents(&folders));
            }
            let Some(index) = due.filter(|&index| self.sections[index].due <= now) else {
                let wait = [at, self.relist].into_iter().flatten().min();
                return Next::Wait(wait.map(|at| at.saturating_duration_since(now)));
            };
            let watch = &mut self.sections[index];
            if let Some(worker) = watch.held.as_ref().and_then(Weak::upgrade) {
                worker.wake();
                watch.due = now + interval;
                continue;
            }
            if !bound {
                return Next::Connect(self.connection);
            }
            return Next::Check {
                path: watch.path.clone(),
                replica: watch.replica.clone(),
                seen: watch.stamp.clone().map(Box::new),
                current: watch.current,
                image: watch.image.take(),
            };
        }
    }
}

/// Lists each of `folders` through `list`; a folder that cannot be listed lists nothing, so
/// that its sections are checked.
fn list(
    list: &mut impl FnMut(&str) -> io::Result<Vec<Entry>>,
    folders: BTreeSet<String>,
) -> BTreeMap<String, Vec<Entry>> {
    folders
        .into_iter()
        .map(|folder| {
            let entries = list(&folder).unwrap_or_default();
            (folder, entries)
        })
        .collect()
}

/// Deletes the replica at `replica` if it holds nothing unpublished and no one holds it.
fn discard(replica: &Path) {
    if matches!(
        crate::closed(replica).and_then(|held| crate::peek(&held)),
        Ok((_, 0))
    ) {
        for suffix in ["-wal", "-shm", ""] {
            let mut file = replica.as_os_str().to_owned();
            file.push(suffix);
            let _ = fs::remove_file(file);
        }
    }
}

/// Whether the catalog path `section` is `path` or lies below it, as a share compares names.
fn within(section: &str, path: &str) -> bool {
    let path = path.trim_matches('/');
    path.is_empty()
        || section.len() >= path.len()
            && section.is_char_boundary(path.len())
            && section[..path.len()].eq_ignore_ascii_case(path)
            && matches!(section.as_bytes().get(path.len()), None | Some(b'/'))
}

/// A catalog path's folder and name.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// Whether a failed step lost the connection, rather than failing for its one file.
fn disconnected(error: &Error) -> bool {
    use io::ErrorKind::*;
    let kind = match error {
        Error::RemoteIo(error) => error.kind(),
        Error::Remote(error) => error.error.kind(),
        _ => return false,
    };
    matches!(
        kind,
        NotConnected | TimedOut | ConnectionReset | ConnectionAborted | BrokenPipe
    )
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
/// holds its replica), then the file's stamp now and whether it changed since `seen`, which
/// with `current` is the stamp now, unread. With `copies`, a section without a replica gets
/// one from the file as it is now. `image`, the file as discovery read it, stands in for
/// reading it while the stamp is still its own.
fn step<R: Remote>(
    remote: &mut R,
    replica: Option<&Path>,
    seen: Option<&Stamp>,
    current: bool,
    image: Option<Vec<u8>>,
    copies: bool,
) -> (Option<u64>, Result<(Stamp, bool)>) {
    let stamp = match seen.filter(|_| current) {
        Some(seen) => seen.clone(),
        None => match remote.stamp() {
            Ok(stamp) => stamp,
            Err(error) => return (None, Err(Error::RemoteIo(error))),
        },
    };
    let remote = &mut Discovered {
        image: image.and_then(|image| Some((Stamp::of(&image).ok()?, image))),
        stamp: stamp.clone(),
        remote,
    };
    let moved = seen.is_some_and(|seen| *seen != stamp);
    let Some(replica) = replica else {
        return (Some(0), Ok((stamp, moved)));
    };
    if fs::metadata(replica).is_err() {
        if !copies {
            return (Some(0), Ok((stamp, moved)));
        }
        let copied = (|| {
            let image = remote.read().map_err(Error::RemoteIo)?;
            if let Some(folder) = replica.parent() {
                fs::create_dir_all(folder)?;
            }
            Replica::seed(replica, &image, None)?;
            Ok(Stamp::of(&image)?)
        })();
        return match copied {
            Ok(stamp) => (Some(0), Ok((stamp, moved))),
            // A session made it first.
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                (None, Ok((stamp, false)))
            }
            Err(error) => (None, Err(error)),
        };
    }
    // A version kept beside the file leaves its stamp as it was.
    let versions = match remote.versions() {
        Ok(versions) => versions,
        Err(error) => return (None, Err(Error::RemoteIo(error))),
    };
    match crate::closed(replica).and_then(|held| crate::peek(&held)) {
        Ok((base, 0)) if base == stamp && versions.is_empty() => {
            return (Some(0), Ok((stamp, moved)));
        }
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

/// A remote whose first read, while the file's stamp is still `image`'s, answers `image`.
struct Discovered<'a, R> {
    remote: &'a mut R,
    image: Option<(Stamp, Vec<u8>)>,
    /// The stamp last read.
    stamp: Stamp,
}

impl<R: Remote> Remote for Discovered<'_, R> {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        match self.image.take() {
            Some((stamp, image)) if stamp == self.stamp => Ok(image),
            _ => self.remote.read(),
        }
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        self.stamp = self.remote.stamp()?;
        Ok(self.stamp.clone())
    }

    fn publish(
        &mut self,
        transaction: &onestore::Transaction,
    ) -> std::result::Result<(), onestore::CommitError> {
        self.remote.publish(transaction)
    }

    fn confirm(&mut self, base: &Stamp) -> std::result::Result<(), onestore::CommitError> {
        self.remote.confirm(base)
    }

    fn versions(&mut self) -> io::Result<Vec<crate::Version>> {
        self.remote.versions()
    }

    fn version(&mut self, id: &str) -> io::Result<Vec<u8>> {
        self.remote.version(id)
    }

    fn retire(&mut self, id: &str, keep: bool) -> io::Result<()> {
        self.remote.retire(id, keep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::EntryKind;
    const BACKSTOP: Duration = Background::BACKSTOP;
    use onestore::{CommitError, Transaction};
    use std::{
        collections::HashMap,
        sync::mpsc::{self, Receiver},
    };

    fn path(n: usize) -> String {
        let folder = if n.is_multiple_of(2) { "" } else { "Group/" };
        format!("{folder}Section {n:03}.one")
    }

    fn listed(n: usize) -> Listed {
        Listed {
            size: 1000 + n as u64,
            modified: 7,
        }
    }

    fn stamp(n: usize) -> Stamp {
        Stamp {
            header: [n as u8; 1024],
            length: n as u64,
        }
    }

    /// `count` sections, each found by discovery as `listed` and `stamp` have it, with `warm`.
    fn sections(count: usize, warm: bool) -> Vec<Known> {
        (0..count)
            .map(|n| Known {
                path: path(n),
                replica: None,
                found: warm.then(|| (listed(n), stamp(n))),
                image: None,
            })
            .collect()
    }

    /// The notebook's folders as a listing shows its first `count` sections.
    fn listing(count: usize) -> BTreeMap<String, Vec<Entry>> {
        let mut folders: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        for n in 0..count {
            let path = path(n);
            let (folder, name) = split(&path);
            folders.entry(folder.to_owned()).or_default().push(Entry {
                name: name.to_owned(),
                kind: EntryKind::File,
                listed: listed(n),
            });
        }
        folders
    }

    /// Connects at `now`, a watch reporting, and the folders listing as `listings` has them;
    /// `meanwhile` runs while they are listed.
    fn connect(
        watched: &mut Watched,
        listings: &BTreeMap<String, Vec<Entry>>,
        now: Instant,
        meanwhile: impl FnOnce(&mut Watched),
    ) {
        watched.watching(true);
        let folders = watched.rescanning();
        meanwhile(watched);
        let listings = folders
            .into_iter()
            .map(|folder| (folder.clone(), listings[&folder].clone()))
            .collect();
        watched.listed(&listings, Some(false), now);
    }

    /// Runs the schedule as the thread would from `now` to `end`, every check succeeding and
    /// every folder listing as `listings` has it, answering each check: when, which section,
    /// and whether it read the file's stamp.
    fn run(
        watched: &mut Watched,
        mut now: Instant,
        end: Instant,
        listings: &BTreeMap<String, Vec<Entry>>,
    ) -> Vec<(Instant, String, bool)> {
        let mut checks = Vec::new();
        loop {
            match watched.next(now, true) {
                Next::Wait(Some(wait)) if now + wait <= end => now += wait,
                Next::Wait(_) => return checks,
                Next::Connect(_) => unreachable!("connected"),
                Next::List(folders) => {
                    let listings = folders
                        .into_iter()
                        .map(|folder| (folder.clone(), listings[&folder].clone()))
                        .collect();
                    watched.listed(&listings, None, now);
                }
                Next::Check { path, current, .. } => {
                    let interval = watched.interval();
                    let watch = watched
                        .sections
                        .iter_mut()
                        .find(|watch| watch.path == path)
                        .unwrap();
                    watch.current = false;
                    watch.due = now + interval;
                    checks.push((now, path, !current));
                }
            }
        }
    }

    #[test]
    fn a_new_notebook_is_read_once_staggered_then_left_alone() {
        let start = Instant::now();
        let mut watched = Watched::default();
        watched.watch(sections(200, false), start);
        connect(&mut watched, &listing(200), start, |_| {});
        let checks = run(
            &mut watched,
            start,
            start + BACKSTOP - SETTLE,
            &listing(200),
        );
        assert_eq!(checks.len(), 200, "one first check each, then none");
        assert!(checks.iter().all(|(.., read)| *read));
        for pair in checks.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= STAGGER, "never a burst");
        }
        // Then each section costs one backstop check an interval: over a day, 24 stamp reads
        // where a 15-second poll made 5,760.
        let day = run(
            &mut watched,
            start + BACKSTOP - SETTLE,
            start + Duration::from_secs(86_400) - SETTLE,
            &listing(200),
        );
        assert_eq!(day.len(), 200 * (86_400 / BACKSTOP.as_secs() as usize - 1));
    }

    #[test]
    fn reopening_reads_only_the_files_listed_otherwise() {
        let start = Instant::now();
        let mut watched = Watched::default();
        watched.watch(sections(200, true), start);
        let mut listings = listing(200);
        // Another client wrote one section while the notebook was closed.
        listings.get_mut("Group").unwrap()[3].listed.modified += 1;
        connect(&mut watched, &listings, start, |_| {});
        let checks = run(&mut watched, start, start + SETTLE, &listings);
        assert_eq!(checks.len(), 200);
        let read: Vec<_> = checks.iter().filter(|(.., read)| *read).collect();
        assert_eq!(read.len(), 1, "{read:?}");
        assert_eq!(read[0].1, "Group/Section 007.one");
        assert!(
            checks.iter().all(|(at, ..)| *at == start),
            "nothing to read, nothing to stagger"
        );
    }

    #[test]
    fn a_change_reported_while_listing_is_read_though_the_listing_missed_it() {
        let start = Instant::now();
        let mut watched = Watched::default();
        watched.watch(sections(4, true), start);
        connect(&mut watched, &listing(4), start, |watched| {
            watched.touched(&[path(2)], start);
        });
        let checks = run(&mut watched, start, start + SETTLE * 2, &listing(4));
        let read: Vec<_> = checks
            .into_iter()
            .filter(|(.., read)| *read)
            .map(|(_, path, _)| path)
            .collect();
        assert_eq!(read, vec![path(2)]);
    }

    #[test]
    fn a_reported_change_checks_exactly_the_sections_it_names() {
        let start = Instant::now();
        let mut watched = Watched::default();
        watched.watch(sections(200, true), start);
        let mut listings = listing(200);
        connect(&mut watched, &listings, start, |_| {});
        let now = start + Duration::from_secs(60);
        run(&mut watched, start, now, &listings);
        // A commit's several writes are reported apiece, and settle into one check.
        for _ in 0..3 {
            watched.touched(&["section 004.ONE".into()], now);
        }
        watched.touched(
            &["Section 004.one".into()],
            now + Duration::from_millis(300),
        );
        let checks = run(&mut watched, now, now + Duration::from_secs(60), &listings);
        assert_eq!(
            checks,
            vec![(now + SETTLE, "Section 004.one".to_owned(), true)]
        );
        // A folder, as a watch names it when it cannot name the file, is listed, and only the
        // section listed otherwise is checked.
        let now = now + Duration::from_secs(60);
        listings.get_mut("Group").unwrap()[2].listed.size += 1;
        watched.touched(&["Group".into()], now);
        let group = run(&mut watched, now, now + SETTLE * 2, &listings);
        assert_eq!(
            group,
            vec![(now + SETTLE, "Group/Section 005.one".to_owned(), true)]
        );
        // `""` lists every folder; nothing listed otherwise, nothing is checked.
        watched.touched(&["Grou".into(), String::new()], now + SETTLE * 2);
        assert!(run(&mut watched, now + SETTLE * 2, now + SETTLE * 4, &listings).is_empty());
    }

    /// Each file's image and how many times it was written, by path.
    type Images = HashMap<String, (Vec<u8>, u64)>;

    /// Section files in memory, each stamp read reported as it happens.
    #[derive(Clone)]
    struct Files {
        images: Arc<Mutex<Images>>,
        stamps: mpsc::Sender<String>,
    }

    impl Files {
        fn write(&self, path: &str, image: Vec<u8>) {
            let mut images = self.images.lock().unwrap();
            let version = images.get(path).map_or(0, |(_, version)| version + 1);
            images.insert(path.to_owned(), (image, version));
        }

        fn list(&self, folder: &str) -> Vec<Entry> {
            let images = self.images.lock().unwrap();
            images
                .iter()
                .filter(|(path, _)| split(path).0 == folder)
                .map(|(path, (image, version))| Entry {
                    name: split(path).1.to_owned(),
                    kind: EntryKind::File,
                    listed: Listed {
                        size: image.len() as u64,
                        modified: *version,
                    },
                })
                .collect()
        }

        fn known(&self, path: &str) -> Known {
            let (image, version) = self.images.lock().unwrap()[path].clone();
            Known {
                path: path.to_owned(),
                replica: None,
                found: Some((
                    Listed {
                        size: image.len() as u64,
                        modified: version,
                    },
                    Stamp::of(&image).unwrap(),
                )),
                image: None,
            }
        }
    }

    struct File(Files, String);

    impl Remote for File {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            let images = self.0.images.lock().unwrap();
            Ok(images[&self.1].0.clone())
        }

        fn stamp(&mut self) -> io::Result<Stamp> {
            let _ = self.0.stamps.send(self.1.clone());
            Stamp::of(&self.read()?).map_err(io::Error::other)
        }

        fn publish(&mut self, _: &Transaction) -> std::result::Result<(), CommitError> {
            unreachable!("nothing is queued")
        }

        fn confirm(&mut self, _: &Stamp) -> std::result::Result<(), CommitError> {
            unreachable!("nothing is queued")
        }
    }

    fn quiet(stamps: &Receiver<String>, wait: Duration) -> Vec<String> {
        let mut read = Vec::new();
        while let Ok(path) = stamps.recv_timeout(wait) {
            read.push(path);
        }
        read
    }

    #[test]
    fn a_watch_wakes_the_section_it_reports_and_a_lost_one_lists_again() {
        let (sender, stamps) = mpsc::channel();
        let files = Files {
            images: Arc::default(),
            stamps: sender,
        };
        let mut paths: Vec<String> = (0..6).map(path).collect();
        paths.sort();
        for path in &paths {
            files.write(
                path,
                onestore::create_section("s.one", path, "Author").unwrap(),
            );
        }
        let (armed, watches) = mpsc::channel();
        let remote = files.clone();
        let background = Background::start(
            false,
            move |reports| {
                armed.send(reports).unwrap();
                let (files, listed) = (remote.clone(), remote.clone());
                let bind = move |path: &str| File(files.clone(), path.to_owned());
                let list = move |folder: &str| Ok(listed.list(folder));
                Ok(((bind, list), true))
            },
            || {},
        )
        .unwrap();
        // A section the catalog read unchanged is not read again; a new one is.
        let mut known: Vec<Known> = paths[1..].iter().map(|path| files.known(path)).collect();
        known.push(Known {
            path: paths[0].clone(),
            replica: None,
            found: None,
            image: None,
        });
        background.watch(known);
        assert_eq!(quiet(&stamps, STAGGER * 4), vec![paths[0].clone()]);
        let watch = watches.recv().unwrap();
        assert!(quiet(&stamps, SETTLE).is_empty(), "idle reads nothing");

        // A folder reported without its file is listed; only the file listed otherwise is read.
        let other = &paths[4];
        files.write(
            other,
            onestore::create_section("s.one", "Other", "Author").unwrap(),
        );
        watch.touched(&[String::new()]);
        assert_eq!(quiet(&stamps, SETTLE * 2), vec![other.clone()]);
        assert_eq!(background.changed(), vec![other.clone()]);

        let changed = &paths[3];
        files.write(
            changed,
            onestore::create_section("s.one", "Changed", "Author").unwrap(),
        );
        watch.touched(std::slice::from_ref(changed));
        assert_eq!(quiet(&stamps, SETTLE * 2), vec![changed.clone()]);
        assert_eq!(background.changed(), vec![changed.clone()]);

        // A watch that ended leaves changes unreported, so every folder is listed again, and
        // the files listed otherwise since the last listing are read.
        let third = &paths[5];
        files.write(
            third,
            onestore::create_section("s.one", "Third", "Author").unwrap(),
        );
        watch.lost();
        let rewatch = watches.recv_timeout(SETTLE).unwrap();
        let mut read = quiet(&stamps, STAGGER * 4);
        read.sort();
        assert_eq!(read, vec![changed.clone(), third.clone()]);
        // The ended watch's connection is gone; only the current one's loss counts.
        watch.lost();
        assert!(quiet(&stamps, SETTLE).is_empty());
        drop(rewatch);
    }

    #[test]
    fn a_held_section_is_left_to_its_worker_which_the_watch_wakes() {
        let (sender, stamps) = mpsc::channel();
        let files = Files {
            images: Arc::default(),
            stamps: sender,
        };
        let held = path(0);
        files.write(
            &held,
            onestore::create_section("s.one", &held, "Author").unwrap(),
        );
        let (armed, watches) = mpsc::channel();
        let remote = files.clone();
        let background = Background::start(
            false,
            move |reports| {
                armed.send(reports).unwrap();
                let (files, listed) = (remote.clone(), remote.clone());
                let bind = move |path: &str| File(files.clone(), path.to_owned());
                let list = move |folder: &str| Ok(listed.list(folder));
                Ok(((bind, list), true))
            },
            || {},
        )
        .unwrap();
        background.watch(vec![files.known(&held)]);
        let watch = watches.recv().unwrap();
        assert!(quiet(&stamps, SETTLE).is_empty());
        // As `hold` leaves it to a session's worker.
        let (signal, woken) = Signal::new();
        if let Ok(mut watched) = background.0.watched.lock() {
            watched.sections[0].held = Some(Arc::downgrade(&signal));
            signal.watched.store(watched.reported, Ordering::Release);
        }
        assert!(
            signal.watched.load(Ordering::Acquire),
            "the watch reports, so the worker need not poll"
        );
        watch.touched(std::slice::from_ref(&held));
        assert!(woken.recv_timeout(SETTLE * 2).is_ok(), "the watch wakes it");
        assert!(
            quiet(&stamps, SETTLE).is_empty(),
            "the worker reads, not this"
        );
        // The watch ending hands polling back to the worker.
        watch.lost();
        assert!(woken.recv_timeout(SETTLE).is_ok());
        assert!(!signal.watched.load(Ordering::Acquire));
        drop(signal);
        drop(background);
    }
}
