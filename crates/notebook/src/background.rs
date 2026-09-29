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
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

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
/// A check costs one stamp read; a section whose replica has edits waiting, or whose file
/// moved past the replica's base, has its replica opened for the synchronization steps that
/// publish or rebase it, then closed again. A section without a replica gets one, its offline
/// copy, where the notebook keeps them. A replica a session holds is left to that session's
/// worker. Dropping requests cancellation without waiting for the step in flight.
pub struct Background(Arc<Shared>);

/// What the background thread and whatever reports changes to it share.
pub(crate) struct Shared {
    signal: Arc<Signal>,
    watched: Mutex<Watched>,
    /// Asked for by `discard`, once the thread stops.
    discard: AtomicBool,
}

/// A connection's report of its notebook folder's changes, from a watch armed on connecting.
pub(crate) struct Reports {
    shared: Weak<Shared>,
    connection: u64,
}

#[derive(Default)]
struct Watched {
    sections: Vec<Watch>,
    /// Sections whose file changed since `changed` was last asked.
    changed: Vec<String>,
    /// Counts connections, so a watch that ended with an earlier one is not this one's.
    connection: u64,
    /// The current connection's watch ended: changes since went unreported.
    lost: bool,
}

struct Watch {
    path: String,
    replica: Option<PathBuf>,
    /// The file's stamp when last reached.
    stamp: Option<Stamp>,
    status: SyncStatus,
    /// When the section is next checked.
    due: Instant,
}

impl Background {
    /// How often each section is checked while a watch reports its folder's changes, against
    /// a report that went missing; OneNote 2010, watching, checks nothing on a timer.
    pub const BACKSTOP: Duration = Duration::from_secs(60 * 60);
    /// How often each section is checked where nothing reports its folder's changes.
    pub const UNWATCHED: Duration = Duration::from_secs(15);

    /// Starts the thread. `connect` binds each catalog path to its file, arming any watch
    /// that reports the folder's changes through `Reports`, and answers how long a section
    /// may go unchecked while nothing reports it; it runs again after a transport failure.
    /// `copies` keeps an offline copy of every section.
    pub(crate) fn start<R, B>(
        copies: bool,
        mut connect: impl FnMut(Reports) -> io::Result<(B, Duration)> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self>
    where
        R: Remote,
        B: FnMut(&str) -> R,
    {
        let (signal, receiver) = Signal::new();
        let shared = Arc::new(Shared {
            signal,
            watched: Mutex::default(),
            discard: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&shared);
        let owner = Arc::clone(&shared);
        thread::Builder::new()
            .name("onestore-background".into())
            .spawn(move || {
                let signal = &owner.signal;
                let mut bound: Option<(B, Duration)> = None;
                let mut news = false;
                while !signal.stopped.load(Ordering::Acquire) {
                    if signal.offline.load(Ordering::Acquire)
                        && !signal.requested.load(Ordering::Acquire)
                    {
                        bound = None;
                        let _ = receiver.recv();
                        continue;
                    }
                    let now = Instant::now();
                    let (path, replica, seen, connection) = {
                        let Ok(mut watched) = owner.watched.lock() else {
                            return;
                        };
                        if std::mem::take(&mut watched.lost) {
                            bound = None;
                            watched.rescan(now);
                        }
                        let due = watched.next().map(|index| &watched.sections[index]);
                        match due.filter(|watch| watch.due <= now) {
                            Some(watch) => (
                                watch.path.clone(),
                                watch.replica.clone(),
                                watch.stamp.clone(),
                                watched.connection,
                            ),
                            None => {
                                let wait = due.map(|watch| watch.due - now);
                                drop(watched);
                                signal.requested.store(false, Ordering::Release);
                                if std::mem::take(&mut news) {
                                    notify();
                                }
                                let _ = match wait {
                                    Some(wait) => receiver.recv_timeout(wait).ok(),
                                    None => receiver.recv().ok(),
                                };
                                continue;
                            }
                        }
                    };
                    let (bind, interval) = match &mut bound {
                        Some(bound) => bound,
                        None => {
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
                                // Changes while unwatched went unreported.
                                Ok(connected) => {
                                    watched.rescan(now);
                                    bound = Some(connected);
                                }
                                Err(error) => {
                                    let error = Error::RemoteIo(error);
                                    for watch in &mut watched.sections {
                                        news |= watch.fail(&error, None);
                                        watch.due = now + RETRY;
                                    }
                                }
                            }
                            continue;
                        }
                    };
                    let (queued, outcome) =
                        step(&mut bind(&path), replica.as_deref(), seen.as_ref(), copies);
                    let interval = *interval;
                    if outcome.as_ref().is_err_and(disconnected) {
                        bound = None;
                    }
                    let Ok(mut watched) = owner.watched.lock() else {
                        return;
                    };
                    let Watched {
                        sections, changed, ..
                    } = &mut *watched;
                    let Some(watch) = sections.iter_mut().find(|watch| watch.path == path) else {
                        continue;
                    };
                    let now = Instant::now();
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
                if owner.discard.load(Ordering::Acquire)
                    && let Ok(watched) = owner.watched.lock()
                {
                    for replica in watched
                        .sections
                        .iter()
                        .filter_map(|watch| watch.replica.as_ref())
                    {
                        discard(replica);
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
                let interval = match client.watch(&root, move |changed| match changed {
                    Ok(paths) => reports.touched(&paths),
                    Err(_) => reports.lost(),
                }) {
                    Ok(()) => Self::BACKSTOP,
                    Err(error) if error.kind() == io::ErrorKind::Unsupported => Self::UNWATCHED,
                    Err(error) => return Err(error),
                };
                let root = root.clone();
                let bind = move |path: &str| {
                    let file = match root.as_str() {
                        "" => path.to_owned(),
                        root => format!("{root}/{path}"),
                    };
                    crate::SmbRemote::new(Arc::clone(&client), file, limit)
                };
                Ok((bind, interval))
            },
            notify,
        )
    }

    /// Watches these sections, by catalog path and replica (`Notebook::replicas`); a section
    /// not watched before is first checked soon after, the next one a little later.
    pub fn watch(&self, sections: Vec<(String, Option<PathBuf>)>) {
        if let Ok(mut watched) = self.0.watched.lock() {
            watched.watch(sections, Instant::now());
        }
        self.0.signal.wake();
    }

    /// Checks the sections at or below these catalog paths soon, as a watch on the
    /// notebook's folder reports them changed; `""` names every section.
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

    /// Checks every section now, working offline included (Sync Now).
    pub fn wake(&self) {
        if let Ok(mut watched) = self.0.watched.lock() {
            let now = Instant::now();
            for watch in &mut watched.sections {
                watch.due = now;
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

    /// Working offline, nothing is checked until `wake`, or until working online again.
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
}

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

impl Watched {
    fn watch(&mut self, sections: Vec<(String, Option<PathBuf>)>, now: Instant) {
        let mut previous = std::mem::take(&mut self.sections);
        let mut new = 0;
        self.sections = sections
            .into_iter()
            .map(
                |(path, replica)| match previous.iter().position(|watch| watch.path == path) {
                    Some(index) => Watch {
                        replica,
                        ..previous.swap_remove(index)
                    },
                    None => {
                        new += 1;
                        Watch {
                            path,
                            replica,
                            stamp: None,
                            status: SyncStatus {
                                synced: None,
                                error: None,
                                queued: 0,
                            },
                            due: now + STAGGER * (new - 1),
                        }
                    }
                },
            )
            .collect();
    }

    /// Checks every section from `now`, one `STAGGER` after another.
    fn rescan(&mut self, now: Instant) {
        for (index, watch) in (0..).zip(&mut self.sections) {
            watch.due = now + STAGGER * index;
        }
    }

    fn touched(&mut self, paths: &[String], now: Instant) {
        for watch in &mut self.sections {
            if paths.iter().any(|path| within(&watch.path, path)) {
                watch.due = watch.due.min(now + SETTLE);
            }
        }
    }

    /// The section checked next.
    fn next(&self) -> Option<usize> {
        (0..self.sections.len()).min_by_key(|&index| self.sections[index].due)
    }
}

/// Deletes the replica at `replica` if it holds nothing unpublished and no one holds it.
fn discard(replica: &Path) {
    if matches!(crate::peek(replica), Ok((_, 0))) {
        for suffix in ["-wal", "-shm", ""] {
            let mut file = replica.as_os_str().to_owned();
            file.push(suffix);
            let _ = std::fs::remove_file(file);
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
/// holds its replica), then the file's stamp now and whether it changed since `seen`. With
/// `copies`, a section without a replica gets one from the file as it is now.
fn step<R: Remote>(
    remote: &mut R,
    replica: Option<&Path>,
    seen: Option<&Stamp>,
    copies: bool,
) -> (Option<u64>, Result<(Stamp, bool)>) {
    let stamp = match remote.stamp() {
        Ok(stamp) => stamp,
        Err(error) => return (None, Err(Error::RemoteIo(error))),
    };
    let moved = seen.is_some_and(|seen| *seen != stamp);
    let Some(replica) = replica else {
        return (Some(0), Ok((stamp, moved)));
    };
    if !replica.exists() {
        if !copies {
            return (Some(0), Ok((stamp, moved)));
        }
        let copied = (|| {
            let image = remote.read().map_err(Error::RemoteIo)?;
            if let Some(folder) = replica.parent() {
                std::fs::create_dir_all(folder)?;
            }
            Replica::seed(replica, &image)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    const BACKSTOP: Duration = Background::BACKSTOP;
    use onestore::{CommitError, Transaction};
    use std::{
        collections::HashMap,
        sync::mpsc::{self, Receiver},
    };

    fn sections(count: usize) -> Vec<(String, Option<PathBuf>)> {
        (0..count)
            .map(|n| {
                let folder = if n % 2 == 0 { "" } else { "Group/" };
                (format!("{folder}Section {n:03}.one"), None)
            })
            .collect()
    }

    /// Runs the schedule as the thread would with every check succeeding, up to `end`,
    /// answering which sections were checked when.
    fn run(watched: &mut Watched, end: Instant, interval: Duration) -> Vec<(Instant, String)> {
        let mut checks = Vec::new();
        while let Some(index) = watched.next() {
            let watch = &mut watched.sections[index];
            if watch.due > end {
                break;
            }
            checks.push((watch.due, watch.path.clone()));
            watch.due += interval;
        }
        checks
    }

    #[test]
    fn an_idle_notebook_is_read_once_staggered_then_left_alone() {
        let start = Instant::now();
        let mut watched = Watched::default();
        watched.watch(sections(200), start);
        let checks = run(&mut watched, start + BACKSTOP - SETTLE, BACKSTOP);
        assert_eq!(checks.len(), 200, "one first check each, then none");
        for pair in checks.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= STAGGER, "never a burst");
        }
        // Then each section costs one backstop check an interval: over a day, 24 stamp reads
        // where a 15-second poll made 5,760.
        let day = run(
            &mut watched,
            start + Duration::from_secs(86_400) - SETTLE,
            BACKSTOP,
        );
        assert_eq!(day.len(), 200 * (86_400 / BACKSTOP.as_secs() as usize - 1));
    }

    #[test]
    fn a_reported_change_checks_exactly_the_sections_it_names() {
        let start = Instant::now();
        let mut watched = Watched::default();
        watched.watch(sections(200), start);
        let now = start + Duration::from_secs(60);
        run(&mut watched, now, BACKSTOP);
        // A commit's several writes are reported apiece, and settle into one check.
        for _ in 0..3 {
            watched.touched(&["section 004.ONE".into()], now);
        }
        watched.touched(
            &["Section 004.one".into()],
            now + Duration::from_millis(300),
        );
        let checks = run(&mut watched, now + Duration::from_secs(60), BACKSTOP);
        assert_eq!(checks, vec![(now + SETTLE, "Section 004.one".to_owned())]);
        // A group's folder names the sections in it, and "" every section.
        let now = now + Duration::from_secs(60);
        watched.touched(&["Group".into()], now);
        let group = run(&mut watched, now + SETTLE, BACKSTOP);
        assert_eq!(group.len(), 100);
        assert!(group.iter().all(|(_, path)| path.starts_with("Group/")));
        watched.touched(&["Grou".into(), String::new()], now);
        assert_eq!(run(&mut watched, now + SETTLE, BACKSTOP).len(), 200);
    }

    /// Section files in memory, each stamp read reported as it happens.
    #[derive(Clone)]
    struct Files {
        images: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        stamps: mpsc::Sender<String>,
    }

    struct File(Files, String);

    impl Remote for File {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            let images = self.0.images.lock().unwrap();
            Ok(images[&self.1].clone())
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
    fn a_watch_wakes_the_section_it_reports_and_a_lost_one_rescans() {
        let (sender, stamps) = mpsc::channel();
        let files = Files {
            images: Arc::default(),
            stamps: sender,
        };
        let mut paths: Vec<String> = sections(6).into_iter().map(|(path, _)| path).collect();
        paths.sort();
        for path in &paths {
            let image = onestore::create_section("s.one", path, "Author").unwrap();
            files.images.lock().unwrap().insert(path.clone(), image);
        }
        let (armed, watches) = mpsc::channel();
        let remote = files.clone();
        let background = Background::start(
            false,
            move |reports| {
                armed.send(reports).unwrap();
                let files = remote.clone();
                Ok((
                    move |path: &str| File(files.clone(), path.to_owned()),
                    Duration::from_secs(3600),
                ))
            },
            || {},
        )
        .unwrap();
        background.watch(sections(6));
        let mut first = quiet(&stamps, STAGGER * 4);
        first.sort();
        assert_eq!(first, paths, "each section's first check");
        let watch = watches.recv().unwrap();
        assert!(quiet(&stamps, SETTLE).is_empty(), "idle reads nothing");

        let changed = &paths[3];
        files.images.lock().unwrap().insert(
            changed.clone(),
            onestore::create_section("s.one", "Changed", "Author").unwrap(),
        );
        watch.touched(std::slice::from_ref(changed));
        assert_eq!(quiet(&stamps, SETTLE * 2), vec![changed.clone()]);
        assert_eq!(background.changed(), vec![changed.clone()]);

        // A watch that ended leaves changes unreported, so everything is read again.
        watch.lost();
        let rewatch = watches.recv_timeout(SETTLE).unwrap();
        assert_eq!(quiet(&stamps, STAGGER * 4).len(), 6);
        // The ended watch's connection is gone; only the current one's loss counts.
        watch.lost();
        assert!(quiet(&stamps, SETTLE).is_empty());
        drop(rewatch);
    }
}
