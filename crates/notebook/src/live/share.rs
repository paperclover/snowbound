//! Live Share: a notebook one Snowbound holds, opened on others through a short code. The
//! host serves its notebook's storage verbs (`session::Storage`) to each peer in the share's
//! room; a guest runs the replica, queue and merge it runs on an SMB share against those
//! verbs, so offline queueing, rebases and conflict pages work as there, and the host's files
//! stay what its own storage writes. A guest first meets the host in the code's room, where
//! the host welcomes it with the share's room and secret; a new share has a new secret, so
//! stopping retires every guest. Large bodies travel a chunk at a time, each answered before
//! the next, so a relay never holds much for a slow peer.
//!
//! The code's words come from the EFF's short word list
//! (<https://www.eff.org/dice>, CC BY 3.0 US), `yo-yo` replaced by `yarn`.

use super::{
    Event, Hello, Line, Live, Peer, Presence, Reach, Relayed, Room,
    wire::{self, Failure, Reply, Request, Touched, Welcome, WireEntry, WireStamp, kind},
};
use crate::{Error, Result, background::Reports, discover, session::Storage};
use onestore::{CommitError, CommitState, RevisionIndex, Stamp, Store, Transaction};
use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

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
/// Sections whose image a host keeps to check guests' commits on.
const IMAGES: usize = 4;
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

const WORDS: &str = include_str!("words.txt");

/// Two random words for a code, `violet-otter`.
pub fn words() -> io::Result<String> {
    let list: Vec<&str> = WORDS.lines().collect();
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|_| io::Error::other("System random source failed"))?;
    let [first, second] = [&bytes[..4], &bytes[4..]]
        .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("4 bytes")) as usize);
    let first = first % list.len();
    let mut second = second % (list.len() - 1);
    if second >= first {
        second += 1;
    }
    Ok(format!("{}-{}", list[first], list[second]))
}

/// A code as typed, `412 Violet otter`, in the form it is met by, `412-violet-otter`; none
/// where it is not a number and two words.
pub fn code(typed: &str) -> Option<String> {
    let parts: Vec<String> = typed
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect();
    match &parts[..] {
        [number, first, second]
            if number.parse::<u32>().is_ok()
                && [first, second]
                    .iter()
                    .all(|word| word.chars().all(|c| c.is_ascii_lowercase())) =>
        {
            Some(parts.join("-"))
        }
        _ => None,
    }
}

/// What a host keeps of a share to take it up again after a relaunch.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sharing {
    pub share: [u8; 16],
    /// The share room's secret, which every guest welcomed holds.
    pub secret: [u8; 16],
    /// The code: its words, with its number in front once one was given.
    pub code: String,
    pub password: String,
}

impl Sharing {
    /// A new share: its own id and secret, and new words.
    pub fn new(password: &str) -> io::Result<Self> {
        let mut random = [0; 32];
        getrandom::fill(&mut random)
            .map_err(|_| io::Error::other("System random source failed"))?;
        Ok(Self {
            share: random[..16].try_into().expect("16 bytes"),
            secret: random[16..].try_into().expect("16 bytes"),
            code: words()?,
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
    /// Not a number and two words.
    Malformed,
    /// The code's words or password are wrong.
    Wrong,
    /// No one shares with the code's number now.
    NoOne,
    /// The code had too many wrong tries.
    Expired,
    /// Too many wrong codes from this network; try again after the wait, where known.
    TooMany(Option<Duration>),
    /// The relay is full.
    Busy,
    /// The relay couldn't be reached, and no one answered on this network.
    Unreachable,
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
    .map_err(|_| Refusal::Unreachable)?;
    let start = Instant::now();
    loop {
        if let Ok(welcome) = welcome.try_recv() {
            return Ok(welcome);
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
            Relayed::Refused(429, wait) => return Err(Refusal::TooMany(wait)),
            Relayed::Refused(503, _) if settled => return Err(Refusal::Busy),
            Relayed::Unreachable if waited > Duration::from_secs(10) => {
                return Err(Refusal::Unreachable);
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
    /// many wrong tries is replaced by one with new words.
    pub fn code(&self) -> Option<String> {
        let mut pairing = self.pairing.lock().unwrap();
        let pairing = pairing.as_mut()?;
        if pairing.burned() {
            let mut sharing = self.sharing.lock().unwrap();
            sharing.code = words().ok()?;
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

    /// Tells every guest the files at these catalog paths changed.
    pub fn touched(&self, paths: &[String]) {
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
}

/// A guest as its host serves it: the line to it, its requests waiting for its workers, and
/// how many more it may start now.
struct Admitted {
    line: Line,
    queue: mpsc::SyncSender<(u16, Vec<u8>)>,
    starts: f64,
    counted: Instant,
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

    /// Tells every guest the files at `paths` changed.
    fn tell(&self, paths: &[String]) {
        let touched = Touched {
            paths: paths.to_vec(),
        };
        for guest in self.guests.lock().unwrap().values() {
            let _ = guest.line.send(kind::TOUCHED, &touched);
        }
    }

    fn handle(&self, peer: &[u8; 16], kind: u16, body: &[u8]) -> Reply {
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

    fn answer(&self, peer: &[u8; 16], kind: u16, request: Request) -> Result<Reply> {
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
                self.commit(path, &transaction)?;
                self.tell(&[path.to_owned()]);
                done
            }
            kind::CONFIRM => {
                self.storage.confirm(path, &stamp()?)?;
                done
            }
            kind::CREATE => {
                self.storage.create(path, &self.carried(peer, &request)?)?;
                self.tell(&[folder(path)]);
                done
            }
            kind::CREATE_DIRECTORY => {
                self.storage.create_directory(path)?;
                self.tell(&[folder(path)]);
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
                self.tell(&[folder(path), folder(to)]);
                done
            }
            kind::DELETE => {
                self.storage.delete(path)?;
                self.tell(&[folder(path)]);
                done
            }
            kind::PLACE => {
                let ancestor = request
                    .ancestor
                    .ok_or_else(|| refused(io::ErrorKind::InvalidInput, "No ancestor"))?;
                let name = request.name.as_deref().unwrap_or_default();
                self.storage.place(path, ancestor, name)?;
                self.tell(&[path.to_owned()]);
                done
            }
            kind::SUPERSEDE => {
                self.storage.supersede(path, &stamp()?, to()?)?;
                self.tell(&[folder(path)]);
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

    fn keep(&self, path: &str, image: Arc<Vec<u8>>) {
        let Ok(stamp) = Stamp::of(&image) else {
            return;
        };
        let mut images = self.images.lock().unwrap();
        images.retain(|(held, ..)| held != path);
        images.insert(0, (path.to_owned(), stamp, image));
        images.truncate(IMAGES);
    }

    /// Commits a guest's transaction once the section it makes parses.
    fn commit(&self, path: &str, transaction: &Transaction) -> Result<()> {
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
        self.keep(path, Arc::new(next));
        Ok(())
    }
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

    /// Reads the file at `path` a chunk at a time, as `kind` reads it.
    fn read(&self, kind: u16, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        let first = self.ask(
            kind,
            Request {
                path: path.to_owned(),
                offset: Some(0),
                limit: Some(limit as u64),
                ..Request::default()
            },
        )?;
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
        Ok(image)
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

    fn stamp(&self, path: &str) -> io::Result<Stamp> {
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
    fn heard(&self, event: Event) {
        let serves = |hello: &Hello| hello.serves == Some(self.share);
        match event {
            Event::Met(hello, line) if serves(hello) => {
                *self.host.lock().unwrap() = Some((Arc::clone(hello), line.clone()));
            }
            Event::Left(hello) if serves(hello) => {
                *self.host.lock().unwrap() = None;
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
                kind::TOUCHED if serves(from) => {
                    if let Ok(touched) = minicbor::decode::<Touched>(body)
                        && let Some(reports) = &*self.watch.lock().unwrap()
                    {
                        reports.touched(&touched.paths);
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
}

impl HostedRemote {
    pub fn new(guest: &Arc<Guest>, path: &str) -> Self {
        Self {
            guest: Arc::clone(guest),
            path: path.to_owned(),
        }
    }
}

impl crate::Remote for HostedRemote {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        self.guest.read(kind::READ, &self.path, LIMIT)
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        self.guest.stamp(&self.path)
    }

    fn publish(&mut self, transaction: &Transaction) -> std::result::Result<(), CommitError> {
        self.guest.commit(&self.path, transaction)
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
