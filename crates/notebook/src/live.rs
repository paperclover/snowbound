//! Live presence and Live Share: who else has the notebook open, the page they are on and
//! their caret, straight from one Snowbound to another, or through a relay (`crates/relay`)
//! where they aren't on one network; and a notebook one machine holds opened on another
//! (`share`). Peers find each other with mDNS (`_snowbound._tcp`) or in the relay's room, and
//! meet through a secret both hold, a room's or a code typed on both, which SPAKE2 turns into
//! the keys every frame after the opening is sealed with. Each connection has a thread reading
//! and one writing. A connection whose frames arrive out of order is dropped and met again
//! from scratch.

pub use ::relay::code;
mod group;
pub mod proxy;
mod relay;
pub mod share;
mod transport;
pub use transport::Trouble;
pub mod wire;
pub use wire::{Caret, Guid, Hello, Presence, Spot};

use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use minicbor::Encode;
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use wire::{Sealer, Side, kind};

const SERVICE: &str = "_snowbound._tcp.local.";
/// How long a connection may be quiet before a ping, and before the peer counts as gone.
const PING: Duration = Duration::from_secs(15);
const GONE: Duration = Duration::from_secs(45);
const OPENING: Duration = Duration::from_secs(5);
/// The most often presence goes to a peer.
const PRESENCE_EVERY: Duration = Duration::from_millis(100);
/// The longest wait before meeting again.
const PATIENCE: Duration = Duration::from_secs(30);
/// Wrong tries of a code met off any relay before it admits no one new, as a relay burns one.
const TRIES: u32 = 5;

mod model;
pub use model::{Event, Peer, Reach, Relayed, Room};
use model::{code_parts, hex};

/// Presence on the network while it lives; dropping it leaves.
pub struct Live {
    shared: Arc<Shared>,
    address: SocketAddr,
}

struct Shared {
    me: Hello,
    room: Room,
    secret: Vec<u8>,
    reach: Option<Reach>,
    state: Mutex<State>,
    events: Box<dyn Fn(Event) + Send + Sync>,
    stopped: AtomicBool,
    connections: AtomicU64,
}

#[derive(Default)]
struct State {
    presence: Presence,
    /// Counts changes to `presence`, so a writer sends only the newest.
    generation: u64,
    peers: BTreeMap<[u8; 16], Link>,
    /// The code others type, once known.
    code: Option<String>,
    /// The relay connection open now, to hang up on leaving.
    relay: Option<Arc<relay::Socket>>,
    relayed: Option<Relayed>,
    /// Meetings that failed on the secret: wrong codes tried here, or this end's.
    failed: u32,
    /// The code admits no one new.
    burned: bool,
    /// The Live Share version of the last peer met that speaks another.
    outdated: Option<u16>,
    daemon: Option<ServiceDaemon>,
}

impl State {
    /// The relay's group, in a notebook's room while the relay is reached.
    fn group(&self) -> Option<&relay::Group> {
        self.relay.as_ref()?.group.as_ref()
    }
}

/// Presence sent at most every `PRESENCE_EVERY`, and only the newest.
#[derive(Default)]
struct Paced {
    due: Option<Instant>,
    last: Option<Instant>,
    sent: Option<u64>,
}

impl Paced {
    /// How long to wait for news: until presence is due, else `idle`.
    fn wait(&self, idle: Duration) -> Duration {
        self.due
            .map_or(idle, |due| due.saturating_duration_since(Instant::now()))
    }

    /// Hears that presence changed.
    fn changed(&mut self) {
        let last = self.last;
        self.due
            .get_or_insert_with(|| last.map_or_else(Instant::now, |at| at + PRESENCE_EVERY));
    }

    /// The presence to send now, where it is due and new.
    fn due(&mut self, state: &Mutex<State>) -> Option<Presence> {
        self.due.filter(|due| *due <= Instant::now())?;
        self.due = None;
        let state = state.lock().unwrap();
        if self.sent == Some(state.generation) {
            return None;
        }
        (self.sent, self.last) = (Some(state.generation), Some(Instant::now()));
        Some(state.presence.clone())
    }
}

struct Link {
    connection: u64,
    peer: Peer,
    line: Line,
    pipe: Arc<dyn Pipe>,
}

/// What a connection's writer sends next.
enum Out {
    /// The newest presence.
    Presence,
    Frame(u16, Vec<u8>),
    /// A `Bye`, after which it hangs up.
    Bye(Vec<u8>),
}

/// The way to one connected peer: frames sent on it go after those sent before.
#[derive(Clone)]
pub struct Line(mpsc::Sender<Out>);

impl Line {
    pub fn send(&self, kind: u16, body: &impl Encode<()>) -> io::Result<()> {
        let body = minicbor::to_vec(body).map_err(io::Error::other)?;
        self.0
            .send(Out::Frame(kind, body))
            .map_err(|_| io::ErrorKind::NotConnected.into())
    }

    /// Says `reason` after the frames sent before, then hangs up.
    pub fn hang_up(&self, reason: &str) {
        let bye = minicbor::to_vec(wire::Bye {
            reason: reason.into(),
        })
        .unwrap_or_default();
        let _ = self.0.send(Out::Bye(bye));
    }
}

/// A stream to one peer, read by one thread and written by another: a TCP connection, or
/// one carried through a relay.
trait Pipe: Send + Sync {
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize>;
    fn write(&self, bytes: &[u8]) -> io::Result<usize>;
    fn set_read_timeout(&self, timeout: Duration) -> io::Result<()>;
    /// Hangs up, ending the thread reading.
    fn shutdown(&self);
    /// Whether it goes straight to the peer, which is better than through a relay.
    fn direct(&self) -> bool;
    /// Hears whether the peer at the other end knew the secret.
    fn met(&self, _met: bool) {}
    /// Hears that frames arrived lost, repeated, reordered or forged, after which this end
    /// meets its peers again.
    fn broken(&self) {}
}

impl Pipe for TcpStream {
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        Read::read(&mut &*self, buffer)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<usize> {
        Write::write(&mut &*self, bytes)
    }

    fn set_read_timeout(&self, timeout: Duration) -> io::Result<()> {
        TcpStream::set_read_timeout(self, Some(timeout))
    }

    fn shutdown(&self) {
        let _ = TcpStream::shutdown(self, Shutdown::Both);
    }

    fn direct(&self) -> bool {
        true
    }
}

impl Read for &dyn Pipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        Pipe::read(*self, buffer)
    }
}

impl Write for &dyn Pipe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Pipe::write(*self, bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Live {
    /// Starts listening as `me` in `room`, advertised and looked for where `reach` says, or
    /// not at all with `None`, leaving peers to `connect`, and in the room at `relay`
    /// (`wss://live.example.net`) where given. `events` runs on a network thread.
    pub fn start(
        me: Hello,
        room: &Room,
        reach: Option<Reach>,
        relay: Option<&str>,
        events: impl Fn(Event) + Send + Sync + 'static,
    ) -> io::Result<Live> {
        let host = match reach {
            Some(Reach::Network) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            Some(Reach::Loopback) | None => IpAddr::V4(Ipv4Addr::LOCALHOST),
        };
        let listener = TcpListener::bind((host, 0))?;
        let mut address = listener.local_addr()?;
        if address.ip().is_unspecified() {
            address.set_ip(IpAddr::V4(Ipv4Addr::LOCALHOST));
        }
        let code = match room {
            Room::Code { code, .. } => match code_parts(code) {
                (Some(number), secret) => code::format(number, &secret),
                // Off any relay, the end sharing a secret alone numbers it itself.
                (None, secret) if relay.is_none() => {
                    let mut number = [0; 4];
                    getrandom::fill(&mut number)
                        .map_err(|_| io::Error::other("System random source failed"))?;
                    code::format(u32::from_le_bytes(number) % code::NAMEPLATES, &secret)
                }
                (None, _) => None,
            },
            Room::Notebook(_) => None,
        };
        let shared = Arc::new(Shared {
            me,
            room: room.clone(),
            secret: room.secret(),
            reach,
            state: Mutex::new(State {
                code,
                ..State::default()
            }),
            events: Box::new(events),
            stopped: AtomicBool::new(false),
            connections: AtomicU64::new(0),
        });
        if let Some(relay) = relay {
            relay::join(&shared, relay, address.port())?;
        }
        let accepting = Arc::clone(&shared);
        thread::Builder::new()
            .name("live accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if accepting.stopped.load(Ordering::Acquire) {
                        return;
                    }
                    if let (Ok(stream), Some(tag)) = (stream, accepting.tag()) {
                        let shared = Arc::clone(&accepting);
                        thread::spawn(move || shared.run(Arc::new(stream), Side::Responder, &tag));
                    }
                }
            })?;
        shared.advertise(address.port());
        Ok(Live { shared, address })
    }

    /// Where this end listens.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The code others type to meet this end: the one it was given, or the one the relay
    /// or this end numbered.
    pub fn code(&self) -> Option<String> {
        self.shared.state.lock().unwrap().code.clone()
    }

    /// How the relay last answered.
    pub fn relayed(&self) -> Relayed {
        let state = self.shared.state.lock().unwrap();
        state.relayed.clone().unwrap_or(Relayed::Unknown)
    }

    /// Meetings that failed on the secret: wrong tries of this end's code, or this end's own
    /// wrong code.
    pub fn failed(&self) -> u32 {
        self.shared.state.lock().unwrap().failed
    }

    /// The Live Share version of the last peer met that speaks another, which one of the two
    /// must update to meet.
    pub fn other_version(&self) -> Option<u16> {
        self.shared.state.lock().unwrap().outdated
    }

    /// Whether this end's code had too many wrong tries and admits no one new.
    pub fn burned(&self) -> bool {
        self.shared.state.lock().unwrap().burned
    }

    /// Connects to a peer at `address` that discovery did not find.
    pub fn connect(&self, address: SocketAddr) {
        let shared = Arc::clone(&self.shared);
        thread::spawn(move || shared.dial(address));
    }

    /// Says where this end is now; peers hear only the newest of quick changes, at most every
    /// tenth of a second.
    pub fn set_presence(&self, presence: Presence) {
        let mut state = self.shared.state.lock().unwrap();
        if state.presence == presence {
            return;
        }
        state.presence = presence;
        state.generation += 1;
        for link in state.peers.values() {
            if self.shared.carries_presence(&*link.pipe) {
                let _ = link.line.0.send(Out::Presence);
            }
        }
        if let Some(group) = state.group() {
            group.send(relay::Out::Presence);
        }
    }

    /// The peers in the room now, by id.
    pub fn peers(&self) -> Vec<Peer> {
        self.shared.peers()
    }

    /// A way to send to this room's peers that doesn't keep it open.
    pub fn sender(&self) -> Sender {
        Sender(Arc::downgrade(&self.shared))
    }

    /// The line to `peer`, while it is connected.
    pub fn line(&self, peer: &[u8; 16]) -> Option<Line> {
        let state = self.shared.state.lock().unwrap();
        state.peers.get(peer).map(|link| link.line.clone())
    }

    /// Says `reason` to every peer and leaves, waiting a moment for them to hear it.
    pub fn leave(self, reason: &str) {
        let bye = minicbor::to_vec(wire::Bye {
            reason: reason.into(),
        })
        .unwrap_or_default();
        for link in self.shared.state.lock().unwrap().peers.values() {
            let _ = link.line.0.send(Out::Bye(bye.clone()));
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while !self.shared.state.lock().unwrap().peers.is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Sends to a room's peers while the room is open.
#[derive(Clone)]
pub struct Sender(std::sync::Weak<Shared>);

impl Sender {
    /// Sends message `kind` holding `body` to the peers `to` names, or to everyone: once to
    /// the room's group through the relay, and to each peer met directly.
    pub fn send(&self, kind: u16, body: &impl Encode<()>, to: Option<&[[u8; 16]]>) {
        let (Some(shared), Ok(body)) = (self.0.upgrade(), minicbor::to_vec(body)) else {
            return;
        };
        let state = shared.state.lock().unwrap();
        let group = state.group();
        let named = |id: &[u8; 16]| to.is_none_or(|to| to.contains(id));
        let mut slots = Vec::new();
        for (id, link) in state.peers.iter().filter(|(id, _)| named(id)) {
            match group {
                Some(group) if !link.pipe.direct() => slots.extend(group.slot(id)),
                _ => {
                    let _ = link.line.0.send(Out::Frame(kind, body.clone()));
                }
            }
        }
        let Some(group) = group else {
            return;
        };
        match to {
            None => group.send(relay::Out::Frame(kind, body, None)),
            Some(to) => {
                let unlinked = to.iter().filter(|id| !state.peers.contains_key(*id));
                slots.extend(unlinked.filter_map(|id| group.slot(id)));
                if !slots.is_empty() {
                    group.send(relay::Out::Frame(kind, body, Some(slots)));
                }
            }
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        // Wakes the accepting thread to see it has stopped.
        let _ = TcpStream::connect_timeout(&self.address, OPENING);
        let mut state = self.shared.state.lock().unwrap();
        if let Some(daemon) = state.daemon.take() {
            let _ = daemon.shutdown();
        }
        for link in state.peers.values() {
            link.pipe.shutdown();
        }
        if let Some(relay) = &state.relay {
            relay.hang_up();
        }
    }
}

impl Shared {
    /// The peers in the room: those met directly, and the rest as the relay's group or a
    /// stream through the relay last heard of them.
    fn peers(&self) -> Vec<Peer> {
        let state = self.state.lock().unwrap();
        let mut peers = BTreeMap::new();
        if let Some(group) = state.group() {
            for member in group.members.lock().unwrap().values() {
                if let Some(peer) = &member.peer {
                    peers.insert(peer.hello.peer, peer.clone());
                }
            }
        }
        for (id, link) in &state.peers {
            if link.pipe.direct() || !peers.contains_key(id) {
                peers.insert(*id, link.peer.clone());
            }
        }
        peers.into_values().collect()
    }

    /// Whether peer `id` is met directly, which is where it says everything.
    fn direct(&self, id: &[u8; 16]) -> bool {
        let state = self.state.lock().unwrap();
        state.peers.get(id).is_some_and(|link| link.pipe.direct())
    }

    /// Whether presence goes on `pipe`: not on a stream through the relay in a notebook's
    /// room, whose group carries it.
    fn carries_presence(&self, pipe: &dyn Pipe) -> bool {
        pipe.direct() || matches!(self.room, Room::Code { .. })
    }

    /// The room's tag as it stands: a code's, once numbered.
    fn tag(&self) -> Option<String> {
        match &self.room {
            Room::Code { .. } => code_parts(self.state.lock().unwrap().code.as_deref()?)
                .0
                .map(|number| format!("code-{number}")),
            room => room.tag(),
        }
    }

    /// Advertises this end on `port` once its room has a tag, where its reach says, and
    /// connects to the peers in its room that discovery finds with a higher id than its own,
    /// which leave the connecting to it.
    fn advertise(self: &Arc<Self>, port: u16) {
        let (Some(reach), Some(tag)) = (self.reach, self.tag()) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if state.daemon.is_some() || self.stopped.load(Ordering::Acquire) {
            return;
        }
        match discover(self, reach, tag, port) {
            Ok(daemon) => state.daemon = Some(daemon),
            Err(error) => eprintln!("Live: no discovery: {error}"),
        }
    }

    fn knows(&self, peer: &str) -> bool {
        let state = self.state.lock().unwrap();
        state.peers.keys().any(|id| hex(id) == peer)
    }

    /// Connects to `address`, and again while the peer is there and the connection was the
    /// one this end kept.
    fn dial(self: Arc<Self>, address: SocketAddr) {
        let mut wait = Duration::from_secs(1);
        while let Ok(stream) = TcpStream::connect_timeout(&address, OPENING) {
            let Some(tag) = self.tag() else {
                return;
            };
            if !Arc::clone(&self).run(Arc::new(stream), Side::Initiator, &tag)
                || self.stopped.load(Ordering::Acquire)
            {
                return;
            }
            thread::sleep(wait);
            wait = (wait * 2).min(PATIENCE);
        }
    }

    /// Counts a meeting that failed on the secret: the end sharing a code burns it after too
    /// many, and an end that typed one gives up at once.
    fn failed(&self) {
        let mut state = self.state.lock().unwrap();
        state.failed += 1;
        match &self.room {
            Room::Code { owner: true, .. } if state.failed >= TRIES => state.burned = true,
            Room::Code { owner: false, .. } => self.stopped.store(true, Ordering::Release),
            _ => {}
        }
        drop(state);
        (self.events)(Event::Changed);
    }

    /// Meets the peer at the other end of `pipe` in room `tag`, then reads from it until it
    /// goes: whether it was the connection kept to that peer.
    fn run(self: Arc<Self>, pipe: Arc<dyn Pipe>, side: Side, tag: &str) -> bool {
        if self.stopped.load(Ordering::Acquire) || self.state.lock().unwrap().burned {
            pipe.shutdown();
            return false;
        }
        let mut stream: &dyn Pipe = &*pipe;
        let met = (|| {
            stream.set_read_timeout(OPENING)?;
            let (mut send, mut receive) = wire::open(&mut stream, side, tag, &self.secret)?;
            send.send(&mut stream, kind::HELLO, &self.me)?;
            let (first, body) = receive.receive(&mut stream)?;
            if first != kind::HELLO {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "No hello"));
            }
            let hello: Hello = minicbor::decode(&body)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "A malformed hello"))?;
            stream.set_read_timeout(GONE)?;
            Ok((send, receive, hello))
        })();
        let other = (met.as_ref().err())
            .and_then(|error| error.get_ref()?.downcast_ref::<wire::Version>())
            .copied();
        // A peer of another version guessed nothing, the keys never being agreed.
        pipe.met(met.is_ok() || other.is_some());
        let (send, mut receive, hello) = match met {
            Ok(met) => met,
            Err(error) => {
                eprintln!("Live: no meeting in {tag}: {error}");
                if let Some(wire::Version(version)) = other {
                    self.state.lock().unwrap().outdated = Some(version);
                    if matches!(self.room, Room::Code { owner: false, .. }) {
                        self.stopped.store(true, Ordering::Release);
                    }
                    (self.events)(Event::Changed);
                } else if error.kind() == io::ErrorKind::InvalidData {
                    self.failed();
                }
                pipe.shutdown();
                return false;
            }
        };
        let peer = hello.peer;
        let name = hello.name.clone();
        let hello = Arc::new(hello);
        let connection = self.connections.fetch_add(1, Ordering::Relaxed);
        let (out, outgoing) = mpsc::channel();
        let line = Line(out);
        {
            let mut state = self.state.lock().unwrap();
            // A peer met both directly and through a relay keeps the direct connection, as
            // both ends then agree.
            let kept = match state.peers.get(&peer) {
                _ if peer == self.me.peer => false,
                Some(link) if link.pipe.direct() || !pipe.direct() => false,
                Some(link) => {
                    link.pipe.shutdown();
                    true
                }
                None => true,
            };
            if !kept {
                drop(state);
                pipe.shutdown();
                return false;
            }
            if self.carries_presence(&*pipe) {
                let _ = line.0.send(Out::Presence);
            }
            state.peers.insert(
                peer,
                Link {
                    connection,
                    peer: Peer {
                        hello: Arc::clone(&hello),
                        presence: None,
                    },
                    line: line.clone(),
                    pipe: Arc::clone(&pipe),
                },
            );
        }
        let writing = Arc::clone(&pipe);
        let shared = Arc::clone(&self);
        thread::spawn(move || shared.write(&*writing, send, outgoing));
        (self.events)(Event::Met(&hello, &line));
        (self.events)(Event::Changed);
        let ended = loop {
            let (message, body) = match receive.receive(&mut stream) {
                Ok(frame) => frame,
                Err(error) => break Some(error),
            };
            match message {
                kind::HELLO | kind::PING => {}
                kind::PRESENCE => {
                    let Ok(presence) = minicbor::decode::<Presence>(&body) else {
                        break None;
                    };
                    if let Some(link) = self.state.lock().unwrap().peers.get_mut(&peer) {
                        link.peer.presence = Some(presence);
                    }
                    (self.events)(Event::Changed);
                }
                kind => {
                    (self.events)(Event::Frame {
                        from: &hello,
                        kind,
                        body: &body,
                    });
                    // The peer hangs up after its bye.
                    if kind == kind::BYE {
                        break None;
                    }
                }
            }
        };
        if let Some(error) = ended.filter(|error| error.kind() == io::ErrorKind::InvalidData) {
            eprintln!("Live: the connection to {name} broke ({error}); meeting again");
            pipe.broken();
        }
        pipe.shutdown();
        let mut state = self.state.lock().unwrap();
        if state
            .peers
            .get(&peer)
            .is_some_and(|link| link.connection == connection)
        {
            state.peers.remove(&peer);
            drop(state);
            (self.events)(Event::Left(&hello));
            (self.events)(Event::Changed);
        }
        true
    }

    /// Sends the newest presence when woken, at most every `PRESENCE_EVERY`, frames as they
    /// come, and a ping when quiet.
    fn write(&self, pipe: &dyn Pipe, mut send: Sealer, outgoing: mpsc::Receiver<Out>) {
        let mut stream = pipe;
        let mut paced = Paced::default();
        loop {
            let result = match outgoing.recv_timeout(paced.wait(PING)) {
                Ok(Out::Presence) => {
                    paced.changed();
                    Ok(())
                }
                Ok(Out::Frame(kind, body)) => send.send_encoded(&mut stream, kind, &body),
                Ok(Out::Bye(body)) => {
                    let _ = send.send_encoded(&mut stream, kind::BYE, &body);
                    pipe.shutdown();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) if paced.due.is_none() => {
                    send.send(&mut stream, kind::PING, &())
                }
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let result = result.and_then(|()| match paced.due(&self.state) {
                Some(presence) => send.send(&mut stream, kind::PRESENCE, &presence),
                None => Ok(()),
            });
            if result.is_err() {
                pipe.shutdown();
                return;
            }
        }
    }
}

/// Advertises `shared` as in room `tag` on `port` and connects to the peers in its room that
/// discovery finds with a higher id than its own.
fn discover(
    shared: &Arc<Shared>,
    reach: Reach,
    tag: String,
    port: u16,
) -> mdns_sd::Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let id = hex(&shared.me.peer);
    let properties = [("v", "1"), ("room", tag.as_str()), ("peer", id.as_str())];
    let host = format!("snowbound-{id}.local.");
    let info = match reach {
        Reach::Network => {
            ServiceInfo::new(SERVICE, &id, &host, "", port, &properties[..])?.enable_addr_auto()
        }
        Reach::Loopback => {
            daemon.disable_interface(IfKind::All)?;
            daemon.enable_interface(IfKind::LoopbackV4)?;
            ServiceInfo::new(
                SERVICE,
                &id,
                &host,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                port,
                &properties[..],
            )?
        }
    };
    daemon.register(info)?;
    let found = daemon.browse(SERVICE)?;
    let shared = Arc::downgrade(shared);
    thread::Builder::new()
        .name("live discovery".into())
        .spawn(move || {
            while let Ok(event) = found.recv() {
                let ServiceEvent::ServiceResolved(service) = event else {
                    continue;
                };
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                let (Some(room), Some(peer)) = (
                    service.get_property_val_str("room"),
                    service.get_property_val_str("peer"),
                ) else {
                    continue;
                };
                if room != tag || peer <= id.as_str() || shared.knows(peer) {
                    continue;
                }
                let mut addresses: Vec<IpAddr> =
                    service.addresses.iter().map(|ip| ip.to_ip_addr()).collect();
                addresses.sort_by_key(|ip| (!ip.is_ipv4(), !ip.is_loopback()));
                if let Some(ip) = addresses.first() {
                    let address = SocketAddr::new(*ip, service.port);
                    thread::spawn(move || shared.dial(address));
                }
            }
        })
        .map_err(|error| mdns_sd::Error::Msg(error.to_string()))?;
    Ok(daemon)
}

/// `text` with `%XX` escapes decoded, as a URL's name and password.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match text
            .get(at + 1..at + 3)
            .filter(|_| bytes[at] == b'%')
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            Some(byte) => {
                decoded.push(byte);
                at += 3;
            }
            None => {
                decoded.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests;
