//! Live Share: a notebook one Snowbound holds, opened on others through a short code. The
//! host serves its notebook's storage verbs (`session::Storage`) and batches guests' ops into
//! guarded publications. A guest runs the same replica and durable queue as on an SMB share;
//! protected sections and older peers use transactions. A guest first meets the host in the code's room, where
//! the host welcomes it with the share's room and secret; a new share has a new secret, so
//! stopping retires every guest. Large bodies travel a chunk at a time, each answered before
//! the next, so a relay never holds much for a slow peer.

use super::{
    Event, Hello, Line, Live, Peer, Presence, Reach, Relayed, Room, Sender,
    wire::{
        self, Delta, Failure, Reply, Request, Touched, Welcome, WireEntry, WireStamp, Written, kind,
    },
};
use crate::{Error, Result, background::Reports, discover, session::Storage};
use onestore::{CommitError, CommitState, RevisionIndex, Stamp, Store, Transaction};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

mod batch;

/// The most bytes one message of a read or an upload carries.
const CHUNK: usize = 128 << 10;
/// The most bytes of chunks a guest has asked for and not yet been given.
const WINDOW: usize = 512 << 10;
/// How long a request waits for its reply.
const TIMEOUT: Duration = Duration::from_secs(60);
/// The largest file read or written whole.
const LIMIT: usize = 256 << 20;
/// Snapshots of files being read, per guest, and how long one is kept unread.
const SNAPSHOTS: usize = 8;
const SNAPSHOT_AGE: Duration = Duration::from_secs(120);
/// Sections whose image a host keeps to check guests' commits on and tell them what changed,
/// and a guest keeps to take those changes on without reading.
const IMAGES: usize = 4;
/// Earlier images of those a host keeps besides, to tell a guest that missed a change what
/// it was, and the most bytes they hold.
const VERSIONS: usize = 16;
const VERSIONS_BYTES: usize = 64 << 20;
/// How long a report of a change settles in a guest's background: its host reports each
/// commit once, at once.
pub const SETTLE: Duration = Duration::from_millis(20);
/// The longest a relay may ask a guest joining to wait before it is told so.
const PATIENT: Duration = Duration::from_secs(10);
/// What a host says leaving as it stops sharing.
const STOPPED: &str = "stopped";
/// What a host says hanging up on a guest that asks too much too fast.
const FLOODED: &str = "flooded";
/// A guest's requests a host holds at once, waiting and in hand, past which it hangs up.
const QUEUED: usize = 32;
/// The threads working through each guest's requests.
const WORKERS: usize = 2;
/// Requests a guest may start each second, and at once: every request but a read's later
/// chunks and an upload's, which come from memory or go to it.
const STARTS: f64 = 100.0;
const BURST: f64 = 200.0;

/// A code as typed, `7kq 4mz 9xr`, as it is shown, `7KQ-4MZ-9XR`; none where it is not a
/// code (`super::code`).
pub fn code(typed: &str) -> Option<String> {
    let (number, secret) = super::code::parse(typed)?;
    super::code::format(number, &secret)
}

/// What a host keeps of a share to take it up again after a relaunch.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sharing {
    pub share: [u8; 16],
    /// The share room's secret, which every guest welcomed holds.
    pub secret: [u8; 16],
    /// The code, or its secret alone until it has a number (`super::code`).
    pub code: String,
    pub password: String,
}

impl Sharing {
    /// A new share: its own id, room secret and code.
    pub fn new(password: &str) -> io::Result<Self> {
        let mut random = [0; 32];
        getrandom::fill(&mut random)
            .map_err(|_| io::Error::other("System random source failed"))?;
        Ok(Self {
            share: random[..16].try_into().expect("16 bytes"),
            secret: random[16..].try_into().expect("16 bytes"),
            code: super::code::secret()?,
            password: password.to_owned(),
        })
    }
}

/// Where a guest keeps a share's notebook: `live://` and the share's id.
pub fn location(share: &[u8; 16]) -> String {
    format!("live://{}", super::hex(share))
}

/// Why joining failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not a code, or one mistyped, which spends none of the relay's tries.
    Malformed,
    /// The person sharing runs a Snowbound of another Live Share version: the newer one's
    /// `true` where it is theirs, so this one should update.
    Version { theirs_newer: bool },
    /// The code's secret or password is wrong.
    Wrong,
    /// No one shares with the code's number now.
    NoOne,
    /// The code had too many wrong tries.
    Expired,
    /// Too many wrong codes from this network; try again after the wait, where known.
    TooMany(Option<Duration>),
    /// The relay is full.
    Busy,
    /// The relay couldn't be reached, and why, and no one answered on this network.
    Unreachable(super::Trouble),
    /// The relay let this end in, but no one answered.
    TimedOut,
}

/// Meets the host sharing `code` (and `password`) as `me`, on the networks `reach` names and
/// through `relay`: the share it welcomes this end to.
pub fn join(
    me: Hello,
    code: &str,
    password: &str,
    reach: Option<Reach>,
    relay: Option<&str>,
) -> std::result::Result<Welcome, Refusal> {
    let code = self::code(code).ok_or(Refusal::Malformed)?;
    let (welcomed, welcome) = mpsc::channel();
    let (changed, waiting) = mpsc::channel();
    let live = Live::start(
        me,
        &Room::join(&code, password),
        reach,
        relay,
        move |event| match event {
            Event::Frame {
                kind: kind::WELCOME,
                body,
                ..
            } => {
                if let Ok(body) = minicbor::decode::<Welcome>(body) {
                    let _ = welcomed.send(body);
                }
            }
            _ => {
                let _ = changed.send(());
            }
        },
    )
    .map_err(|_| Refusal::Unreachable(super::Trouble::Other))?;
    let start = Instant::now();
    loop {
        if let Ok(welcome) = welcome.try_recv() {
            return Ok(welcome);
        }
        if let Some(version) = live.other_version() {
            return Err(Refusal::Version {
                theirs_newer: version > wire::VERSION,
            });
        }
        if live.failed() > 0 {
            return Err(Refusal::Wrong);
        }
        // A peer on this network may yet answer what the relay refused.
        let waited = start.elapsed();
        let settled = waited > Duration::from_secs(3) || reach.is_none();
        match live.relayed() {
            Relayed::Refused(404, _) if settled => return Err(Refusal::NoOne),
            Relayed::Refused(410, _) => return Err(Refusal::Expired),
            // A short wait, as a crowd joining at once meets, passes as the relay is asked
            // again.
            Relayed::Refused(429, wait) if wait.is_none_or(|wait| wait > PATIENT) => {
                return Err(Refusal::TooMany(wait));
            }
            Relayed::Refused(503, _) if settled => return Err(Refusal::Busy),
            Relayed::Unreachable(trouble) if waited > Duration::from_secs(10) => {
                return Err(Refusal::Unreachable(trouble));
            }
            Relayed::Unknown if relay.is_none() && waited > Duration::from_secs(10) => {
                return Err(Refusal::NoOne);
            }
            _ if waited > Duration::from_secs(20) => return Err(Refusal::TimedOut),
            _ => {}
        }
        let _ = waiting.recv_timeout(Duration::from_millis(100));
    }
}

/// A notebook shared while it lives: the share's room, serving the notebook's storage to the
/// guests in it, and the code's room, welcoming whoever knows the code.
pub struct Host {
    me: Hello,
    notebook: String,
    reach: Option<Reach>,
    relay: Option<String>,
    sharing: Mutex<Sharing>,
    /// The share's room and the code's, until it stops.
    room: Mutex<Option<Live>>,
    pairing: Mutex<Option<Live>>,
    served: Arc<Served>,
    events: Arc<dyn Fn() + Send + Sync>,
}

impl Host {
    /// Shares `storage`, the notebook named `notebook`, as `sharing` says, as `me`, where
    /// `reach` and `relay` say. `events` runs on a network thread whenever the guests or the
    /// code change. What guests change reaches the host as its own watch on the notebook's
    /// folder reports it, and reaches the other guests at once.
    pub fn start(
        storage: Box<dyn Storage>,
        me: Hello,
        sharing: Sharing,
        notebook: &str,
        reach: Option<Reach>,
        relay: Option<&str>,
        events: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let events: Arc<dyn Fn() + Send + Sync> = Arc::new(events);
        let served = Arc::new(Served {
            storage,
            images: Mutex::default(),
            snapshots: Mutex::default(),
            puts: Mutex::default(),
            guests: Mutex::default(),
            writers: Mutex::default(),
            host: Mutex::default(),
            room: OnceLock::new(),
        });
        let serving = Hello {
            serves: Some(sharing.share),
            ..me.clone()
        };
        let (heard, told) = (Arc::clone(&served), Arc::clone(&events));
        let room = Live::start(
            serving,
            &Room::Notebook(sharing.secret),
            reach,
            relay,
            move |event| match event {
                Event::Met(hello, line) => heard.admit(hello.peer, line),
                Event::Left(hello) => heard.forget(&hello.peer),
                Event::Frame {
                    from, kind, body, ..
                } if wire::KNOWN.contains(&kind) && kind > 256 && kind != kind::REPLY => {
                    heard.queue(&from.peer, kind, body);
                }
                Event::Changed => told(),
                _ => {}
            },
        )?;
        let _ = served.room.set(room.sender());
        let host = Self {
            notebook: notebook.to_owned(),
            reach,
            relay: relay.map(str::to_owned),
            pairing: Mutex::new(Some(pair(&me, &sharing, notebook, reach, relay, &events)?)),
            me,
            sharing: Mutex::new(sharing),
            room: Mutex::new(Some(room)),
            served,
            events,
        };
        Ok(host)
    }

    /// The code guests type, once it has its number, and none once stopped. A code with too
    /// many wrong tries is replaced by one with a new secret.
    pub fn code(&self) -> Option<String> {
        let mut pairing = self.pairing.lock().unwrap();
        let pairing = pairing.as_mut()?;
        if pairing.burned() {
            let mut sharing = self.sharing.lock().unwrap();
            sharing.code = super::code::secret().ok()?;
            let relay = self.relay.as_deref();
            *pairing = pair(
                &self.me,
                &sharing,
                &self.notebook,
                self.reach,
                relay,
                &self.events,
            )
            .ok()?;
        }
        let code = pairing.code();
        if let Some(code) = &code {
            self.sharing.lock().unwrap().code = code.clone();
        }
        code
    }

    /// The share as it stands, to take up again after a relaunch.
    pub fn sharing(&self) -> Sharing {
        self.code();
        self.sharing.lock().unwrap().clone()
    }

    /// How the relay last answered the code's room.
    pub fn relayed(&self) -> Relayed {
        let pairing = self.pairing.lock().unwrap();
        pairing.as_ref().map_or(Relayed::Unknown, Live::relayed)
    }

    /// The peers in the share's room.
    pub fn guests(&self) -> Vec<Peer> {
        let room = self.room.lock().unwrap();
        room.as_ref().map(Live::peers).unwrap_or_default()
    }

    pub fn set_presence(&self, presence: Presence) {
        if let Some(room) = &*self.room.lock().unwrap() {
            room.set_presence(presence);
        }
    }

    /// Has `listener` hear the catalog paths guests change from now on, sooner than a watch
    /// on the notebook's folder would.
    pub fn on_changed(&self, listener: crate::session::Listener) {
        *self.served.host.lock().unwrap() = Some(listener);
    }

    /// Tells every guest the files at these catalog paths changed, with what changed in the
    /// sections a guest read lately.
    pub fn touched(&self, paths: &[String]) {
        for path in paths {
            self.served.changed_here(path);
        }
        self.served.tell(paths);
    }

    /// Stops sharing: no one new is welcomed, and every guest hears so and is let go.
    pub fn stop(&self) {
        drop(self.pairing.lock().unwrap().take());
        let room = self.room.lock().unwrap().take();
        if let Some(room) = room {
            room.leave(STOPPED);
        }
    }
}

/// The code's room, welcoming whoever knows the code to the share.
fn pair(
    me: &Hello,
    sharing: &Sharing,
    notebook: &str,
    reach: Option<Reach>,
    relay: Option<&str>,
    events: &Arc<dyn Fn() + Send + Sync>,
) -> io::Result<Live> {
    let welcome = Welcome {
        share: sharing.share,
        secret: sharing.secret,
        notebook: notebook.to_owned(),
        host: me.name.clone(),
    };
    let told = Arc::clone(events);
    Live::start(
        Hello {
            serves: None,
            ..me.clone()
        },
        &Room::share(&sharing.code, &sharing.password),
        reach,
        relay,
        move |event| match event {
            Event::Met(_, line) => {
                let _ = line.send(kind::WELCOME, &welcome);
            }
            Event::Changed => told(),
            _ => {}
        },
    )
}

/// A host's side of its guests' storage requests.
struct Served {
    storage: Box<dyn Storage>,
    /// Sections' images by path, with their stamps, to check commits on.
    images: Mutex<Vec<Image>>,
    /// Files being read a chunk at a time, by guest and the read's first request.
    snapshots: Mutex<ByGuest<(Arc<Vec<u8>>, Instant)>>,
    /// Bytes a later request carries, by guest and upload.
    puts: Mutex<ByGuest<Vec<u8>>>,
    guests: Mutex<BTreeMap<[u8; 16], Admitted>>,
    writers: Mutex<HashMap<String, mpsc::SyncSender<batch::Waiting>>>,
    /// Hears the paths guests changed, as the host's own notebook should.
    host: Mutex<Option<crate::session::Listener>>,
    /// The share's room, to tell guests what changed.
    room: OnceLock<Sender>,
}

/// A guest as its host serves it: the line to it, its requests waiting for its workers, how
/// many more it may start now, and the sections it read lately, newest first, as it keeps
/// their images.
struct Admitted {
    line: Line,
    queue: mpsc::SyncSender<(u16, Vec<u8>)>,
    starts: f64,
    counted: Instant,
    held: Vec<String>,
}

/// A section's path, stamp and image.
type Image = (String, Stamp, Arc<Vec<u8>>);
/// What a guest's requests left, by guest and request.
type ByGuest<T> = HashMap<([u8; 16], u64), T>;

/// Whether a guest may name `path`: a catalog path inside the notebook, and not presence's
/// own secret, which guests meet in the share's room instead.
fn allowed(path: &str) -> bool {
    path.split('/').all(|part| {
        !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', '\0', ':'])
    }) && !path.to_ascii_lowercase().starts_with(".snowbound/live")
}

/// The folder holding catalog path `path`.
fn folder(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(String::new(), |(folder, _)| folder.to_owned())
}

fn refused(kind: io::ErrorKind, message: &str) -> Error {
    io::Error::new(kind, message.to_owned()).into()
}

impl Served {
    /// Serves `peer` on `line`, through workers of its own that end as it leaves.
    fn admit(self: &Arc<Self>, peer: [u8; 16], line: &Line) {
        let (queue, waiting) = mpsc::sync_channel::<(u16, Vec<u8>)>(QUEUED - WORKERS);
        let waiting = Arc::new(Mutex::new(waiting));
        for _ in 0..WORKERS {
            let (served, waiting, line) =
                (Arc::downgrade(self), Arc::clone(&waiting), line.clone());
            thread::spawn(move || {
                loop {
                    let next = waiting.lock().unwrap().recv();
                    let (Ok((kind, body)), Some(served)) = (next, served.upgrade()) else {
                        return;
                    };
                    let _ = line.send(kind::REPLY, &served.handle(&peer, kind, &body));
                }
            });
        }
        let guest = Admitted {
            line: line.clone(),
            queue,
            starts: BURST,
            counted: Instant::now(),
            held: Vec::new(),
        };
        self.guests.lock().unwrap().insert(peer, guest);
    }

    /// Hands a request from `peer` to its workers, or hangs up on a guest that has too many
    /// waiting or starts them too fast.
    fn queue(&self, peer: &[u8; 16], kind: u16, body: &[u8]) {
        let mut guests = self.guests.lock().unwrap();
        let Some(guest) = guests.get_mut(peer) else {
            return;
        };
        let continued = kind == kind::PUT
            || matches!(kind, kind::READ | kind::READ_FILE)
                && minicbor::decode::<Request>(body).is_ok_and(|request| request.handle.is_some());
        let now = Instant::now();
        guest.starts =
            (guest.starts + now.duration_since(guest.counted).as_secs_f64() * STARTS).min(BURST);
        guest.counted = now;
        if !continued {
            guest.starts -= 1.0;
        }
        if guest.starts < 0.0 || guest.queue.try_send((kind, body.to_vec())).is_err() {
            guest.line.hang_up(FLOODED);
            guests.remove(peer);
        }
    }

    fn forget(&self, peer: &[u8; 16]) {
        self.guests.lock().unwrap().remove(peer);
        self.snapshots
            .lock()
            .unwrap()
            .retain(|(guest, _), _| guest != peer);
        self.puts
            .lock()
            .unwrap()
            .retain(|(guest, _), _| guest != peer);
    }

    /// Tells every guest, and the host, that a guest changed the files at `paths`.
    fn changed(&self, paths: &[String]) {
        self.tell(paths);
        if let Some(listener) = &*self.host.lock().unwrap() {
            listener(paths);
        }
    }

    /// Tells every guest the files at `paths` changed, and what they are now.
    fn tell(&self, paths: &[String]) {
        let touched = Touched {
            paths: paths.to_vec(),
            stamps: (paths.iter())
                .map(|path| self.storage.stamp(path).ok().map(|stamp| digest(&stamp)))
                .collect(),
        };
        if let Some(room) = self.room.get() {
            room.send(kind::TOUCHED, &touched, None);
        }
    }

    /// Notes that `guest` holds the section at `path`, as it now keeps its image.
    fn holds(guest: &mut Admitted, path: &str) {
        guest.held.retain(|held| held != path);
        guest.held.insert(0, path.to_owned());
        guest.held.truncate(IMAGES);
    }

    fn handle(self: &Arc<Self>, peer: &[u8; 16], kind: u16, body: &[u8]) -> Reply {
        let request = match minicbor::decode::<Request>(body) {
            Ok(request) => request,
            Err(_) => {
                return failed(
                    0,
                    refused(io::ErrorKind::InvalidData, "A malformed request"),
                );
            }
        };
        let id = request.id;
        match self.answer(peer, kind, request) {
            Ok(reply) => Reply { id, ..reply },
            Err(error) => failed(id, error),
        }
    }

    fn answer(self: &Arc<Self>, peer: &[u8; 16], kind: u16, request: Request) -> Result<Reply> {
        let path = request.path.as_str();
        if !(path.is_empty() && matches!(kind, kind::LIST | kind::PUT) || allowed(path))
            || request.to.as_deref().is_some_and(|to| !allowed(to))
        {
            return Err(refused(
                io::ErrorKind::PermissionDenied,
                "Outside the notebook",
            ));
        }
        let to = || {
            request
                .to
                .as_deref()
                .ok_or_else(|| refused(io::ErrorKind::InvalidInput, "No target"))
        };
        let stamp = || -> Result<Stamp> {
            Ok(request
                .stamp
                .as_ref()
                .ok_or_else(|| refused(io::ErrorKind::InvalidInput, "No stamp"))?
                .try_into()?)
        };
        let done = Reply::default();
        Ok(match kind {
            kind::LIST => Reply {
                entries: Some(
                    self.storage
                        .entries(path)?
                        .into_iter()
                        .filter(|entry| allowed(&within(path, &entry.name)))
                        .map(|entry| wire_entry(&entry))
                        .collect(),
                ),
                ..done
            },
            kind::STAMP => Reply {
                stamp: Some((&self.storage.stamp(path)?).into()),
                ..done
            },
            kind::EXISTS => Reply {
                exists: Some(self.storage.exists(path)),
                ..done
            },
            kind::READ | kind::READ_FILE => self.read(peer, kind, &request)?,
            kind::PUT => {
                let handle = request.handle.unwrap_or_default();
                let bytes = request.bytes.unwrap_or_default();
                let mut puts = self.puts.lock().unwrap();
                let held = puts.entry((*peer, handle)).or_default();
                if request.offset != Some(held.len() as u64) || held.len() + bytes.len() > LIMIT {
                    return Err(refused(
                        io::ErrorKind::InvalidInput,
                        "An upload out of order",
                    ));
                }
                held.extend_from_slice(&bytes);
                done
            }
            kind::COMMIT => {
                let transaction = Transaction::from_bytes(&self.carried(peer, &request)?)?;
                self.commit(peer, path, &transaction)?;
                self.changed(&[path.to_owned()]);
                done
            }
            kind::EDITS => self.batch(peer, request)?,
            kind::CONFIRM => {
                self.storage.confirm(path, &stamp()?)?;
                done
            }
            kind::CREATE => {
                self.storage.create(path, &self.carried(peer, &request)?)?;
                self.changed(&[folder(path)]);
                done
            }
            kind::CREATE_DIRECTORY => {
                self.storage.create_directory(path)?;
                self.changed(&[folder(path)]);
                done
            }
            kind::HIDE => {
                self.storage.hide(path)?;
                done
            }
            kind::RENAME | kind::REPLACE => {
                let to = to()?;
                if kind == kind::RENAME {
                    self.storage.rename(path, to)?;
                } else {
                    self.storage.replace(path, to)?;
                }
                self.changed(&[folder(path), folder(to)]);
                done
            }
            kind::DELETE => {
                self.storage.delete(path)?;
                self.changed(&[folder(path)]);
                done
            }
            kind::PLACE => {
                let ancestor = request
                    .ancestor
                    .ok_or_else(|| refused(io::ErrorKind::InvalidInput, "No ancestor"))?;
                let name = request.name.as_deref().unwrap_or_default();
                self.storage.place(path, ancestor, name)?;
                self.changed(&[path.to_owned()]);
                done
            }
            kind::SUPERSEDE => {
                self.storage.supersede(path, &stamp()?, to()?)?;
                self.changed(&[folder(path)]);
                done
            }
            _ => return Err(refused(io::ErrorKind::Unsupported, "An unknown request")),
        })
    }

    /// The bytes a request carries, itself or in its uploads.
    fn carried(&self, peer: &[u8; 16], request: &Request) -> Result<Vec<u8>> {
        match (&request.bytes, request.handle) {
            (Some(bytes), _) => Ok(bytes.clone()),
            (None, Some(handle)) => self
                .puts
                .lock()
                .unwrap()
                .remove(&(*peer, handle))
                .ok_or_else(|| refused(io::ErrorKind::InvalidInput, "No such upload")),
            (None, None) => Ok(Vec::new()),
        }
    }

    /// A chunk of a file as it stood when its read began.
    fn read(&self, peer: &[u8; 16], kind: u16, request: &Request) -> Result<Reply> {
        let offset = request.offset.unwrap_or_default() as usize;
        let mut snapshots = self.snapshots.lock().unwrap();
        snapshots.retain(|_, (_, read)| read.elapsed() < SNAPSHOT_AGE);
        if let (kind::READ, None, Some(base)) = (kind, request.handle, &request.stamp)
            && let Ok(base) = Stamp::try_from(base)
            && let Some(changes) = self.changes(&request.path, &base)?
        {
            if let Some(guest) = self.guests.lock().unwrap().get_mut(peer) {
                Self::holds(guest, &request.path);
            }
            return Ok(changes);
        }
        let (image, handle) = match request.handle {
            Some(handle) => {
                let (image, read) = snapshots
                    .get_mut(&(*peer, handle))
                    .ok_or_else(|| refused(io::ErrorKind::TimedOut, "The read went stale"))?;
                *read = Instant::now();
                (Arc::clone(image), handle)
            }
            None => {
                drop(snapshots);
                if kind == kind::READ
                    && let Some(guest) = self.guests.lock().unwrap().get_mut(peer)
                {
                    Self::holds(guest, &request.path);
                }
                let limit = (request.limit.unwrap_or(LIMIT as u64) as usize).min(LIMIT);
                let image = match kind {
                    kind::READ => self.image(&request.path)?,
                    _ => Arc::new(self.storage.read_file(&request.path, limit)?),
                };
                if image.len() > limit {
                    return Err(io::Error::from(io::ErrorKind::FileTooLarge).into());
                }
                snapshots = self.snapshots.lock().unwrap();
                if snapshots.keys().filter(|(guest, _)| guest == peer).count() >= SNAPSHOTS {
                    snapshots.retain(|(guest, _), _| guest != peer);
                }
                (image, request.id)
            }
        };
        let end = image.len().min(offset.saturating_add(CHUNK));
        let bytes = image.get(offset..end).unwrap_or_default().to_vec();
        if end < image.len() {
            snapshots.insert((*peer, handle), (Arc::clone(&image), Instant::now()));
        } else {
            snapshots.remove(&(*peer, handle));
        }
        Ok(Reply {
            bytes: Some(bytes),
            length: Some(image.len() as u64),
            handle: Some(handle),
            stamp: (kind == kind::READ)
                .then(|| Stamp::of(&image).ok())
                .flatten()
                .map(|stamp| (&stamp).into()),
            ..Reply::default()
        })
    }

    /// What changed in the section at `path` since the image with stamp `base`, where that
    /// image is kept and the changes are much smaller than the section.
    fn changes(&self, path: &str, base: &Stamp) -> Result<Option<Reply>> {
        let before = self
            .images
            .lock()
            .unwrap()
            .iter()
            .find_map(|(held, at, image)| (held == path && at == base).then(|| Arc::clone(image)));
        let Some(before) = before else {
            return Ok(None);
        };
        let after = self.image(path)?;
        let writes = match delta(&before, &after) {
            Some(writes) => writes,
            None if before == after => Vec::new(),
            None => return Ok(None),
        };
        Ok(Some(Reply {
            writes: Some(writes),
            length: Some(after.len() as u64),
            stamp: Some((&Stamp::of(&after)?).into()),
            ..Reply::default()
        }))
    }

    /// The section or TOC at `path` as it stands: the image kept for it while its stamp holds.
    fn image(&self, path: &str) -> Result<Arc<Vec<u8>>> {
        let stamp = self.storage.stamp(path)?;
        let kept = self
            .images
            .lock()
            .unwrap()
            .iter()
            .find_map(|(held, at, image)| {
                (held == path && *at == stamp).then(|| Arc::clone(image))
            });
        if let Some(image) = kept {
            return Ok(image);
        }
        let image = Arc::new(self.storage.read(path)?);
        self.keep(path, Arc::clone(&image));
        Ok(image)
    }

    /// Keeps `image` as the section at `path` now, with the newest of each of `IMAGES`
    /// sections and earlier images up to `VERSIONS` and `VERSIONS_BYTES`.
    fn keep(&self, path: &str, image: Arc<Vec<u8>>) {
        let Ok(stamp) = Stamp::of(&image) else {
            return;
        };
        let mut images = self.images.lock().unwrap();
        images.retain(|(held, at, _)| held != path || *at != stamp);
        images.insert(0, (path.to_owned(), stamp, image));
        let (mut newest, mut versions, mut bytes) = (Vec::new(), 0, 0);
        images.retain(|(held, _, image)| {
            if !newest.contains(held) {
                newest.push(held.clone());
                return newest.len() <= IMAGES;
            }
            versions += 1;
            bytes += image.len();
            newest.iter().take(IMAGES).any(|kept| kept == held)
                && versions <= VERSIONS
                && bytes <= VERSIONS_BYTES
        });
    }

    /// Commits `guest`'s transaction once the section it makes parses.
    fn commit(&self, guest: &[u8; 16], path: &str, transaction: &Transaction) -> Result<()> {
        let not_committed = |error: io::Error| {
            Error::Remote(CommitError {
                state: CommitState::NotCommitted,
                error,
            })
        };
        let image = self.image(path).map_err(|error| match error {
            Error::Io(error) => not_committed(error),
            error => error,
        })?;
        if Stamp::of(&image).ok().as_ref() != Some(transaction.base()) {
            return Err(not_committed(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "The section changed since",
            )));
        }
        let mut next = (*image).clone();
        let checked = transaction.apply(&mut next).and_then(|()| {
            let store = Store::parse(&next)?;
            RevisionIndex::parse(&store).map(drop)
        });
        if let Err(error) = checked {
            return Err(not_committed(io::Error::new(
                io::ErrorKind::InvalidData,
                error.to_string(),
            )));
        }
        self.storage.commit(path, transaction)?;
        self.tell_delta(path, &image, &next, Some(guest));
        self.keep(path, Arc::new(next));
        Ok(())
    }

    /// Tells the guests holding the section at `path` what a commit changed in it, from
    /// `before` to `after`, where that is small enough to send: all but the guest whose
    /// commit it was, which has it already.
    fn tell_delta(&self, path: &str, before: &[u8], after: &[u8], committed: Option<&[u8; 16]>) {
        let (Ok(base), Some(writes)) = (Stamp::of(before), delta(before, after)) else {
            return;
        };
        let delta = Delta {
            path: path.to_owned(),
            base: (&base).into(),
            length: after.len() as u64,
            writes,
        };
        let mut guests = self.guests.lock().unwrap();
        let holding: Vec<[u8; 16]> = guests
            .iter_mut()
            .filter(|(id, guest)| Some(*id) != committed && guest.held.iter().any(|h| h == path))
            .map(|(id, guest)| {
                Self::holds(guest, path);
                *id
            })
            .collect();
        drop(guests);
        if let (Some(room), false) = (self.room.get(), holding.is_empty()) {
            room.send(kind::DELTA, &delta, Some(&holding));
        }
    }

    /// The host's own change to the section at `path`, which guests hear as a delta where
    /// the host kept the image before it.
    fn changed_here(&self, path: &str) {
        let kept = self
            .images
            .lock()
            .unwrap()
            .iter()
            .find_map(|(held, at, image)| (held == path).then(|| (at.clone(), Arc::clone(image))));
        let Some((at, before)) = kept else {
            return;
        };
        if self.storage.stamp(path).is_ok_and(|now| now == at) {
            return;
        }
        if let Ok(after) = self.storage.read(path) {
            self.tell_delta(path, &before, &after, None);
            self.keep(path, Arc::new(after));
        }
    }
}

/// A stamp as `Touched` names it.
fn digest(stamp: &Stamp) -> u64 {
    let hash = Sha256::new()
        .chain_update(stamp.header)
        .chain_update(stamp.length.to_le_bytes())
        .finalize();
    u64::from_le_bytes(hash[..8].try_into().expect("eight bytes"))
}

/// The writes that make `after` of `before`, a commit's appended bytes, patches and header,
/// where they are much less than `after` itself.
fn delta(before: &[u8], after: &[u8]) -> Option<Vec<Written>> {
    const BLOCK: usize = 4096;
    if before.len() < 1024 || after.len() < before.len() {
        return None;
    }
    let mut writes = vec![Written {
        offset: 0,
        bytes: after[..1024].to_vec(),
    }];
    let mut at = 1024;
    while at < before.len() {
        let end = (at + BLOCK).min(before.len());
        if before[at..end] != after[at..end] {
            let first = (at..end).find(|&i| before[i] != after[i]).unwrap_or(at);
            let last = (at..end).rfind(|&i| before[i] != after[i]).unwrap_or(first) + 1;
            match writes.last_mut() {
                Some(write) if write.offset as usize + write.bytes.len() + 64 >= first => {
                    let from = write.offset as usize;
                    write.bytes = after[from..last].to_vec();
                }
                _ => writes.push(Written {
                    offset: first as u64,
                    bytes: after[first..last].to_vec(),
                }),
            }
        }
        at = end;
    }
    if after.len() > before.len() {
        writes.push(Written {
            offset: before.len() as u64,
            bytes: after[before.len()..].to_vec(),
        });
    }
    let sent: usize = writes.iter().map(|write| write.bytes.len()).sum();
    (sent <= after.len() / 2).then_some(writes)
}

/// `image` with `writes`, `length` long: none where a write falls outside it.
fn written(image: &[u8], length: u64, writes: &[Written]) -> Option<Vec<u8>> {
    let length = usize::try_from(length)
        .ok()
        .filter(|length| *length >= image.len())?;
    let mut next = image.to_vec();
    next.resize(length, 0);
    for write in writes {
        let offset = usize::try_from(write.offset).ok()?;
        next.get_mut(offset..offset.checked_add(write.bytes.len())?)?
            .copy_from_slice(&write.bytes);
    }
    Some(next)
}

fn within(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

fn wire_entry(entry: &discover::Entry) -> WireEntry {
    WireEntry {
        name: entry.name.clone(),
        kind: match entry.kind {
            discover::EntryKind::File => 0,
            discover::EntryKind::Directory => 1,
            discover::EntryKind::Other => 2,
            discover::EntryKind::Evicted => 3,
        },
        size: entry.listed.size,
        modified: entry.listed.modified,
    }
}

fn entry(entry: &WireEntry) -> discover::Entry {
    discover::Entry {
        name: entry.name.clone(),
        kind: match entry.kind {
            0 => discover::EntryKind::File,
            1 => discover::EntryKind::Directory,
            3 => discover::EntryKind::Evicted,
            _ => discover::EntryKind::Other,
        },
        listed: discover::Listed {
            size: entry.size,
            modified: entry.modified,
        },
    }
}

fn state_number(state: CommitState) -> u8 {
    match state {
        CommitState::NotCommitted => 0,
        CommitState::Unknown => 1,
        CommitState::Committed => 2,
    }
}

fn failed(id: u64, error: Error) -> Reply {
    let (kind, state) = match &error {
        Error::Io(error) | Error::RemoteIo(error) => (error.kind(), None),
        Error::Remote(error) => (error.error.kind(), Some(state_number(error.state))),
        Error::Document(_) => (io::ErrorKind::InvalidData, None),
        _ => (io::ErrorKind::Other, None),
    };
    let message = match &error {
        Error::Remote(error) => error.error.to_string(),
        error => error.to_string(),
    };
    Reply {
        id,
        failure: Some(Failure {
            kind: wire::error_number(kind),
            message,
            state,
        }),
        ..Reply::default()
    }
}

/// How a request failed.
enum Failed {
    /// It never left this end.
    Unsent(io::Error),
    /// Its answer was lost: whatever it asked may have happened.
    Lost(io::Error),
    /// The host refused it.
    Refused(Failure),
}

impl Failed {
    fn io(self) -> io::Error {
        match self {
            Failed::Unsent(error) | Failed::Lost(error) => error,
            Failed::Refused(failure) => {
                io::Error::new(wire::error_kind(failure.kind), failure.message)
            }
        }
    }

    /// As a commit's failure: one never sent was not committed, one whose answer was lost may
    /// have been.
    fn commit(self) -> CommitError {
        let state = match &self {
            Failed::Unsent(_) => CommitState::NotCommitted,
            Failed::Lost(_) => CommitState::Unknown,
            Failed::Refused(failure) => match failure.state {
                Some(1) => CommitState::Unknown,
                Some(2) => CommitState::Committed,
                _ => CommitState::NotCommitted,
            },
        };
        CommitError {
            state,
            error: self.io(),
        }
    }
}

/// A guest's way to a share: the share's room, and the requests waiting on its host.
pub struct Guest {
    live: Live,
    inner: Arc<Inner>,
}

struct Inner {
    share: [u8; 16],
    /// The host and the line to it, while connected.
    host: Mutex<Option<(Arc<Hello>, Line)>>,
    pending: Mutex<HashMap<u64, mpsc::Sender<Reply>>>,
    next: AtomicU64,
    /// Where the host's reports of changed files go, while a background watches.
    watch: Mutex<Option<Reports>>,
    /// The host stopped sharing.
    stopped: AtomicBool,
    /// Bytes of chunks asked for and not yet given.
    asked: (Mutex<usize>, Condvar),
    /// The sections read lately, kept as the host's deltas change them, newest first.
    images: Mutex<Vec<(String, Arc<Vec<u8>>)>>,
    /// The stamps of sections whose image held is the host's now, as its last report of
    /// them said.
    current: Mutex<HashMap<String, Stamp>>,
}

impl Guest {
    /// Joins share `share` through its `secret` as `me`, where `reach` and `relay` say.
    /// `events` runs on a network thread whenever the host comes or goes, or the peers change.
    pub fn start(
        me: Hello,
        share: [u8; 16],
        secret: [u8; 16],
        reach: Option<Reach>,
        relay: Option<&str>,
        events: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Arc<Self>> {
        let inner = Arc::new(Inner {
            share,
            host: Mutex::default(),
            pending: Mutex::default(),
            next: AtomicU64::new(1),
            watch: Mutex::default(),
            stopped: AtomicBool::new(false),
            asked: Default::default(),
            images: Mutex::default(),
            current: Mutex::default(),
        });
        let heard = Arc::clone(&inner);
        let live = Live::start(me, &Room::Notebook(secret), reach, relay, move |event| {
            heard.heard(event);
            events();
        })?;
        Ok(Arc::new(Self { live, inner }))
    }

    /// Where the notebook is kept: `location(share)`.
    pub fn location(&self) -> String {
        location(&self.inner.share)
    }

    /// The host, while connected.
    pub fn host(&self) -> Option<Arc<Hello>> {
        let host = self.inner.host.lock().unwrap();
        host.as_ref().map(|(hello, _)| Arc::clone(hello))
    }

    /// Whether the host said it stopped sharing.
    pub fn stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::Acquire)
    }

    /// Everyone in the share's room: the host and the other guests.
    pub fn peers(&self) -> Vec<Peer> {
        self.live.peers()
    }

    pub fn set_presence(&self, presence: Presence) {
        self.live.set_presence(presence);
    }

    /// Sends the host's reports of changed files to `reports` while it stays connected.
    pub(crate) fn watch(&self, reports: Reports) -> io::Result<()> {
        if self.host().is_none() {
            return Err(self.offline());
        }
        *self.inner.watch.lock().unwrap() = Some(reports);
        Ok(())
    }

    fn offline(&self) -> io::Error {
        let message = if self.stopped() {
            "The host stopped sharing this notebook"
        } else {
            "The computer sharing this notebook can’t be reached"
        };
        io::Error::new(io::ErrorKind::NotConnected, message)
    }

    /// Asks the host `kind` of `request`, waiting for its reply.
    fn request(&self, kind: u16, mut request: Request) -> std::result::Result<Reply, Failed> {
        let Some((_, line)) = self.inner.host.lock().unwrap().clone() else {
            return Err(Failed::Unsent(self.offline()));
        };
        let chunked = matches!(kind, kind::READ | kind::READ_FILE | kind::PUT);
        if chunked {
            let (asked, room) = &self.inner.asked;
            let mut asked = asked.lock().unwrap();
            while *asked + CHUNK > WINDOW {
                asked = room.wait(asked).unwrap();
            }
            *asked += CHUNK;
        }
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        request.id = id;
        let (answer, answered) = mpsc::channel();
        self.inner.pending.lock().unwrap().insert(id, answer);
        let sent = line.send(kind, &request);
        let reply = match sent {
            Err(error) => Err(Failed::Unsent(error)),
            Ok(()) => match answered.recv_timeout(TIMEOUT) {
                Ok(reply) => Ok(reply),
                Err(mpsc::RecvTimeoutError::Timeout) => Err(Failed::Lost(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The computer sharing this notebook didn’t answer",
                ))),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(Failed::Lost(self.offline())),
            },
        };
        self.inner.pending.lock().unwrap().remove(&id);
        if chunked {
            let (asked, room) = &self.inner.asked;
            *asked.lock().unwrap() -= CHUNK;
            room.notify_all();
        }
        let reply = reply?;
        match reply.failure {
            Some(failure) => Err(Failed::Refused(failure)),
            None => Ok(reply),
        }
    }

    fn ask(&self, kind: u16, request: Request) -> io::Result<Reply> {
        self.request(kind, request).map_err(Failed::io)
    }

    /// Puts `bytes` in `request`, or uploads them first where they are large.
    fn carry(&self, request: &mut Request, bytes: Vec<u8>) -> std::result::Result<(), Failed> {
        if bytes.len() <= CHUNK {
            request.bytes = Some(bytes);
            return Ok(());
        }
        let handle = self.inner.next.fetch_add(1, Ordering::Relaxed);
        for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
            let put = Request {
                handle: Some(handle),
                offset: Some((index * CHUNK) as u64),
                bytes: Some(chunk.to_vec()),
                ..Request::default()
            };
            // Nothing the upload carries happens before the request that uses it.
            self.request(kind::PUT, put)
                .map_err(|failed| match failed {
                    Failed::Lost(error) => Failed::Unsent(error),
                    failed => failed,
                })?;
        }
        request.handle = Some(handle);
        Ok(())
    }

    /// Reads the file at `path` a chunk at a time, as `kind` reads it; a section held, as
    /// the changes to it.
    fn read(&self, kind: u16, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        let held = (kind == kind::READ)
            .then(|| self.inner.image(path))
            .flatten();
        let first = self.ask(
            kind,
            Request {
                path: path.to_owned(),
                offset: Some(0),
                limit: Some(limit as u64),
                stamp: (held.as_deref())
                    .and_then(|image| Stamp::of(image).ok())
                    .map(|stamp| (&stamp).into()),
                ..Request::default()
            },
        )?;
        if let (Some(writes), Some(held)) = (&first.writes, held) {
            let stamp: Option<Stamp> = first.stamp.as_ref().and_then(|s| s.try_into().ok());
            let image = written(&held, first.length.unwrap_or_default(), writes)
                .filter(|image| stamp.is_some() && Stamp::of(image).ok() == stamp)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Changes that miss"))?;
            self.inner.hold(path, Arc::new(image.clone()));
            return Ok(image);
        }
        let length = first.length.unwrap_or_default() as usize;
        if length > limit {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let mut image = first.bytes.unwrap_or_default();
        while image.len() < length {
            let chunk = self
                .ask(
                    kind,
                    Request {
                        path: path.to_owned(),
                        offset: Some(image.len() as u64),
                        handle: first.handle,
                        ..Request::default()
                    },
                )?
                .bytes
                .unwrap_or_default();
            if chunk.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            image.extend_from_slice(&chunk);
        }
        image.truncate(length);
        if kind == kind::READ {
            self.inner.hold(path, Arc::new(image.clone()));
        }
        Ok(image)
    }

    /// The image of the section at `path` with stamp `stamp`, where this guest holds it.
    fn held(&self, path: &str, stamp: &Stamp) -> Option<Vec<u8>> {
        let image = self.inner.image(path)?;
        (Stamp::of(&image).ok().as_ref() == Some(stamp)).then(|| image.to_vec())
    }

    pub(crate) fn entries(&self, folder: &str) -> io::Result<Vec<discover::Entry>> {
        let reply = self.ask(
            kind::LIST,
            Request {
                path: folder.to_owned(),
                ..Request::default()
            },
        )?;
        Ok(reply
            .entries
            .unwrap_or_default()
            .iter()
            .map(entry)
            .collect())
    }

    /// The stamp of the file at `path`: the host's last report of it where this guest holds
    /// that image, else the host's answer.
    fn stamp(&self, path: &str) -> io::Result<Stamp> {
        if let Some(stamp) = self.inner.current.lock().unwrap().get(path) {
            return Ok(stamp.clone());
        }
        let reply = self.ask(
            kind::STAMP,
            Request {
                path: path.to_owned(),
                ..Request::default()
            },
        )?;
        reply
            .stamp
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?
            .try_into()
    }

    fn commit(
        &self,
        path: &str,
        transaction: &Transaction,
    ) -> std::result::Result<(), CommitError> {
        let mut request = Request {
            path: path.to_owned(),
            ..Request::default()
        };
        self.carry(&mut request, transaction.to_bytes())
            .map_err(Failed::commit)?;
        self.request(kind::COMMIT, request)
            .map(drop)
            .map_err(Failed::commit)
    }

    fn confirm(&self, path: &str, base: &Stamp) -> std::result::Result<(), CommitError> {
        let request = Request {
            path: path.to_owned(),
            stamp: Some(base.into()),
            ..Request::default()
        };
        self.request(kind::CONFIRM, request)
            .map(drop)
            .map_err(Failed::commit)
    }

    /// A request on `path` that answers nothing but whether it happened.
    fn verb(&self, kind: u16, path: &str, request: Request) -> Result<()> {
        self.ask(
            kind,
            Request {
                path: path.to_owned(),
                ..request
            },
        )?;
        Ok(())
    }
}

impl Inner {
    fn image(&self, path: &str) -> Option<Arc<Vec<u8>>> {
        let images = self.images.lock().unwrap();
        images
            .iter()
            .find_map(|(held, image)| (held == path).then(|| Arc::clone(image)))
    }

    fn hold(&self, path: &str, image: Arc<Vec<u8>>) {
        let mut images = self.images.lock().unwrap();
        images.retain(|(held, _)| held != path);
        images.insert(0, (path.to_owned(), image));
        images.truncate(IMAGES);
    }

    /// Takes on a delta to an image held.
    fn apply(&self, delta: &Delta) {
        let held = self.image(&delta.path);
        let base: Option<Stamp> = (&delta.base).try_into().ok();
        if let Some(image) = held
            && Stamp::of(&image).ok() == base
            && let Some(next) = written(&image, delta.length, &delta.writes)
        {
            self.hold(&delta.path, Arc::new(next));
        }
    }

    /// Takes on this guest's own commit to an image held; the host's report, not this, says
    /// whether the image is the host's, as another's commit may have followed.
    fn published(&self, path: &str, transaction: &Transaction) {
        if let Some(image) = self.image(path) {
            let mut next = (*image).clone();
            if transaction.apply(&mut next).is_ok() {
                self.hold(path, Arc::new(next));
            }
        }
    }

    /// Takes the host's report of what the files at `paths` are now.
    fn touched(&self, touched: &Touched) {
        let mut current = self.current.lock().unwrap();
        for (at, path) in touched.paths.iter().enumerate() {
            let stamp = self.image(path).and_then(|image| Stamp::of(&image).ok());
            match stamp.filter(|stamp| touched.stamps.get(at) == Some(&Some(digest(stamp)))) {
                Some(stamp) => current.insert(path.clone(), stamp),
                None => current.remove(path),
            };
        }
    }

    fn heard(&self, event: Event) {
        let serves = |hello: &Hello| hello.serves == Some(self.share);
        match event {
            Event::Met(hello, line) if serves(hello) => {
                *self.host.lock().unwrap() = Some((Arc::clone(hello), line.clone()));
            }
            Event::Left(hello) if serves(hello) => {
                *self.host.lock().unwrap() = None;
                self.current.lock().unwrap().clear();
                // Each request waiting hears its answer was lost.
                self.pending.lock().unwrap().clear();
                if let Some(reports) = self.watch.lock().unwrap().take() {
                    reports.lost();
                }
            }
            Event::Frame {
                from, kind, body, ..
            } => match kind {
                kind::REPLY if serves(from) => {
                    if let Ok(reply) = minicbor::decode::<Reply>(body)
                        && let Some(waiting) = self.pending.lock().unwrap().remove(&reply.id)
                    {
                        let _ = waiting.send(reply);
                    }
                }
                kind::DELTA if serves(from) => {
                    if let Ok(delta) = minicbor::decode::<Delta>(body) {
                        self.apply(&delta);
                    }
                }
                kind::TOUCHED if serves(from) => {
                    if let Ok(touched) = minicbor::decode::<Touched>(body) {
                        self.touched(&touched);
                        if let Some(reports) = &*self.watch.lock().unwrap() {
                            reports.touched(&touched.paths);
                        }
                    }
                }
                kind::BYE
                    if serves(from)
                        && minicbor::decode::<wire::Bye>(body)
                            .is_ok_and(|bye| bye.reason == STOPPED) =>
                {
                    self.stopped.store(true, Ordering::Release);
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// A section a Live Share host serves, as a guest's replica publishes to it.
pub struct HostedRemote {
    guest: Arc<Guest>,
    path: String,
    /// The stamp last asked for, which an image the guest holds may already have.
    seen: Option<Stamp>,
    rejected: bool,
}

impl HostedRemote {
    pub fn new(guest: &Arc<Guest>, path: &str) -> Self {
        Self {
            guest: Arc::clone(guest),
            path: path.to_owned(),
            seen: None,
            rejected: false,
        }
    }
}

impl crate::Remote for HostedRemote {
    fn accepts_edits(&self) -> bool {
        !self.rejected
            && self
                .guest
                .host()
                .is_some_and(|host| host.ops == Some(1) && host.kinds.contains(&kind::EDITS))
    }

    fn publish_edits(
        &mut self,
        transaction: &Transaction,
        edits: &[crate::PendingEdit],
        revisions: &BTreeMap<onestore::ExGuid, onestore::ExGuid>,
    ) -> std::result::Result<(), CommitError> {
        let bytes = serde_json::to_vec(&batch::Edits {
            edits: edits
                .iter()
                .map(|edit| (edit.author.clone(), edit.edit.clone()))
                .collect(),
            revisions: revisions.clone(),
        })
        .map_err(|error| CommitError {
            state: CommitState::NotCommitted,
            error: io::Error::other(error),
        })?;
        let mut request = Request {
            path: self.path.clone(),
            stamp: Some(transaction.base().into()),
            ..Request::default()
        };
        self.guest
            .carry(&mut request, bytes)
            .map_err(Failed::commit)?;
        let result = self.guest.request(kind::EDITS, request);
        match result {
            Ok(reply) => {
                if let Some(stamp) = reply.stamp {
                    let stamp = Stamp::try_from(&stamp).map_err(|error| CommitError {
                        state: CommitState::Unknown,
                        error,
                    })?;
                    self.guest
                        .inner
                        .current
                        .lock()
                        .unwrap()
                        .insert(self.path.clone(), stamp);
                }
                self.seen = None;
                Ok(())
            }
            Err(Failed::Refused(failure))
                if wire::error_kind(failure.kind) == io::ErrorKind::Unsupported =>
            {
                self.publish(transaction)
            }
            Err(error) => {
                let error = error.commit();
                if error.state == CommitState::NotCommitted {
                    self.rejected = true;
                    self.guest.inner.current.lock().unwrap().remove(&self.path);
                }
                Err(error)
            }
        }
    }

    fn read(&mut self) -> io::Result<Vec<u8>> {
        self.rejected = false;
        if let Some(image) = (self.seen.as_ref()).and_then(|seen| self.guest.held(&self.path, seen))
        {
            return Ok(image);
        }
        self.guest.read(kind::READ, &self.path, LIMIT)
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        let stamp = self.guest.stamp(&self.path)?;
        self.seen = Some(stamp.clone());
        Ok(stamp)
    }

    fn publish(&mut self, transaction: &Transaction) -> std::result::Result<(), CommitError> {
        if let Err(error) = self.guest.commit(&self.path, transaction) {
            // The host's image moved on, and its report may not have come yet.
            self.guest.inner.current.lock().unwrap().remove(&self.path);
            return Err(error);
        }
        self.guest.inner.published(&self.path, transaction);
        Ok(())
    }

    fn confirm(&mut self, base: &Stamp) -> std::result::Result<(), CommitError> {
        self.guest.confirm(&self.path, base)
    }
}

/// A notebook a Live Share host serves, as a guest's `session::Notebook` reaches it. Its
/// folders as last listed are kept at `listed`, so the notebook opens while the host can't
/// be reached.
pub struct Hosted {
    guest: Arc<Guest>,
    listed: PathBuf,
}

/// Each folder's entries as last listed: name, `WireEntry::kind`, size and modified time.
type Listings = BTreeMap<String, Vec<(String, u8, u64, u64)>>;

impl Hosted {
    pub(crate) fn new(guest: Arc<Guest>, listed: PathBuf) -> Self {
        Self { guest, listed }
    }
}

/// The host's folders, or as they were last listed while it can't be reached.
struct Source<'a> {
    guest: &'a Guest,
    kept: Listings,
    listed: Listings,
}

impl discover::Source for Source<'_> {
    fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<discover::Entry>> {
        let entries = match self.guest.entries(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotConnected => self
                .kept
                .get(path)
                .ok_or(error)?
                .iter()
                .map(|(name, kind, size, modified)| {
                    entry(&WireEntry {
                        name: name.clone(),
                        kind: *kind,
                        size: *size,
                        modified: *modified,
                    })
                })
                .collect(),
            Err(error) => return Err(error),
        };
        if entries.len() > limit {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.listed.insert(
            path.to_owned(),
            entries
                .iter()
                .map(|listed| {
                    let wire = wire_entry(listed);
                    (wire.name, wire.kind, wire.size, wire.modified)
                })
                .collect(),
        );
        Ok(entries)
    }

    fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.guest.read(kind::READ, path, limit)
    }

    fn read_asset(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.guest.read(kind::READ_FILE, path, limit)
    }
}

impl Storage for Hosted {
    fn discover(
        &self,
        cache: &mut discover::Cache,
        limits: discover::Limits,
    ) -> Result<discover::Folder> {
        let kept = crate::fs::read(&self.listed)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut source = Source {
            guest: &self.guest,
            kept,
            listed: Listings::new(),
        };
        let folder = cache.discover(&mut source, limits)?;
        if let Ok(json) = serde_json::to_vec(&source.listed) {
            let _ = crate::fs::write(&self.listed, json);
        }
        Ok(folder)
    }

    fn location(&self) -> String {
        self.guest.location()
    }

    fn entries(&self, folder: &str) -> io::Result<Vec<discover::Entry>> {
        self.guest.entries(folder)
    }

    fn exists(&self, path: &str) -> bool {
        let request = Request {
            path: path.to_owned(),
            ..Request::default()
        };
        self.guest
            .ask(kind::EXISTS, request)
            .is_ok_and(|reply| reply.exists == Some(true))
    }

    fn stamp(&self, path: &str) -> io::Result<Stamp> {
        self.guest.stamp(path)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>> {
        Ok(self.guest.read(kind::READ, path, LIMIT)?)
    }

    fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>> {
        Ok(self.guest.read(kind::READ_FILE, path, limit)?)
    }

    fn create(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let mut request = Request {
            path: path.to_owned(),
            ..Request::default()
        };
        self.guest
            .carry(&mut request, bytes.to_vec())
            .map_err(Failed::io)?;
        self.guest.verb(kind::CREATE, path, request)
    }

    fn create_directory(&self, path: &str) -> Result<()> {
        self.guest
            .verb(kind::CREATE_DIRECTORY, path, Request::default())
    }

    fn hide(&self, path: &str) -> Result<()> {
        self.guest.verb(kind::HIDE, path, Request::default())
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        let request = Request {
            to: Some(to.to_owned()),
            ..Request::default()
        };
        self.guest.verb(kind::RENAME, from, request)
    }

    fn rename_root(&self, _: &str, _: &[String]) -> Result<String> {
        Err(refused(
            io::ErrorKind::Unsupported,
            "Only the computer sharing this notebook can rename its folder",
        ))
    }

    fn replace(&self, from: &str, to: &str) -> Result<()> {
        let request = Request {
            to: Some(to.to_owned()),
            ..Request::default()
        };
        self.guest.verb(kind::REPLACE, from, request)
    }

    fn delete(&self, path: &str) -> Result<()> {
        self.guest.verb(kind::DELETE, path, Request::default())
    }

    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()> {
        let request = Request {
            ancestor: Some(ancestor),
            name: Some(name.to_owned()),
            ..Request::default()
        };
        self.guest.verb(kind::PLACE, path, request)
    }

    fn commit(&self, path: &str, transaction: &Transaction) -> Result<()> {
        Ok(self.guest.commit(path, transaction)?)
    }

    fn confirm(&self, path: &str, base: &Stamp) -> std::result::Result<(), CommitError> {
        self.guest.confirm(path, base)
    }

    fn supersede(&self, path: &str, base: &Stamp, with: &str) -> Result<()> {
        let request = Request {
            to: Some(with.to_owned()),
            stamp: Some(WireStamp::from(base)),
            ..Request::default()
        };
        self.guest.verb(kind::SUPERSEDE, path, request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A commit's writes, found by comparing images, rebuild the image after it from the one
    /// before, and a change touching most of the file is left to be read whole.
    #[test]
    fn deltas_rebuild_the_image_after_a_commit() {
        let before: Vec<u8> = (0..20_000u32).map(|at| (at % 251) as u8).collect();
        let mut after = before.clone();
        after[3] ^= 1;
        after[5000..5010].fill(9);
        after[5050] ^= 1;
        after[17_000] ^= 1;
        after.extend_from_slice(&[7; 3000]);
        let writes = delta(&before, &after).unwrap();
        assert_eq!(
            writes.len(),
            4,
            "the header, two runs merged as one, one more, the tail"
        );
        assert_eq!(
            written(&before, after.len() as u64, &writes).unwrap(),
            after
        );
        let rewritten: Vec<u8> = before.iter().map(|byte| byte ^ 1).collect();
        assert!(delta(&before, &rewritten).is_none());
        assert!(delta(&before, &before[..10_000]).is_none());
        let outside = [Written {
            offset: after.len() as u64,
            bytes: vec![1],
        }];
        assert!(written(&before, after.len() as u64, &outside).is_none());
    }
}
