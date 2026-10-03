//! Live presence and Live Share: who else has the notebook open, the page they are on and
//! their caret, straight from one Snowbound to another, or through a relay (`crates/relay`)
//! where they aren't on one network; and a notebook one machine holds opened on another
//! (`share`). Peers find each other with mDNS (`_snowbound._tcp`) or in the relay's room, and
//! meet through a secret both hold, a room's or a code typed on both, which SPAKE2 turns into
//! the keys every frame after the opening is sealed with. Each connection has a thread reading
//! and one writing. A connection whose frames arrive out of order is dropped and met again
//! from scratch.

pub use ::relay::code;
pub mod proxy;
mod relay;
pub mod share;
mod transport;
pub use transport::Trouble;
pub mod wire;
pub use wire::{Caret, Guid, Hello, Presence, Spot};

use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use minicbor::Encode;
use sha2::{Digest, Sha256};
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
/// The longest wait before meeting again.
const PATIENCE: Duration = Duration::from_secs(30);
/// Wrong tries of a code met off any relay before it admits no one new, as a relay burns one.
const TRIES: u32 = 5;

/// The secret peers meet through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Room {
    /// A notebook's room, or a share's: a random secret only its members hold.
    Notebook([u8; 16]),
    /// A code typed on both ends (`code`), with a password where one is set: its room's
    /// number names it on the network, and its secret and password only the two people know.
    /// Its `owner`, the end sharing it, takes the number from a relay (the one the code has,
    /// coming back; any free one for a secret alone) or picks one where it has no relay, and
    /// judges every try, burning the code after too many wrong ones.
    Code {
        code: String,
        password: String,
        owner: bool,
    },
}

impl Room {
    /// The room a code typed on this end leads to.
    pub fn join(code: &str, password: &str) -> Self {
        Self::Code {
            code: code.to_owned(),
            password: password.to_owned(),
            owner: false,
        }
    }

    /// The room of a code this end shares: its secret, or a whole code to keep its number.
    pub fn share(code: &str, password: &str) -> Self {
        Self::Code {
            code: code.to_owned(),
            password: password.to_owned(),
            owner: true,
        }
    }

    /// What names the room in the clear: a hash of a room's secret, a code's number; none for
    /// a code without one yet.
    fn tag(&self) -> Option<String> {
        match self {
            Room::Notebook(id) => Some(hex(&Sha256::digest(
                [&b"Snowbound room "[..], id].concat(),
            )[..8])),
            Room::Code { code, .. } => code_parts(code).0.map(|number| format!("code-{number}")),
        }
    }

    fn secret(&self) -> Vec<u8> {
        match self {
            Room::Notebook(id) => id.to_vec(),
            Room::Code { code, password, .. } => {
                let mut secret = code_parts(code).1.into_bytes();
                if !password.is_empty() {
                    secret.push(b'\n');
                    secret.extend_from_slice(password.as_bytes());
                }
                secret
            }
        }
    }

    fn owner(&self) -> bool {
        matches!(self, Room::Code { owner: true, .. })
    }
}

/// A code's room number, if it has one, and its secret: a whole code, or a secret alone.
fn code_parts(code: &str) -> (Option<u32>, String) {
    match code::parse(code) {
        Some((number, secret)) => (Some(number), secret),
        None => (None, code.trim().to_uppercase()),
    }
}

/// Where peers are looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Every network this computer is on.
    Network,
    /// This computer alone, for two copies of the app side by side.
    Loopback,
}

#[derive(Clone, Debug)]
pub struct Peer {
    pub hello: Arc<Hello>,
    /// None until the peer first says where it is.
    pub presence: Option<Presence>,
}

/// What happened, as `Live::start`'s `events` hears it on a network thread.
pub enum Event<'a> {
    /// The peers, their presence, the code or the relay's answer changed.
    Changed,
    /// A peer was met, with the line to it.
    Met(&'a Arc<Hello>, &'a Line),
    /// A peer's connection ended.
    Left(&'a Arc<Hello>),
    /// A frame of a kind presence doesn't read itself, with the line to answer on.
    Frame {
        from: &'a Arc<Hello>,
        kind: u16,
        body: &'a [u8],
        line: &'a Line,
    },
}

/// How the relay last answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Relayed {
    /// Not asked yet, or no relay.
    Unknown,
    /// In the room.
    Joined,
    /// Answered with this HTTP status, and how long it asked to wait.
    Refused(u16, Option<Duration>),
    /// Not reached, and why.
    Unreachable(Trouble),
}

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

    /// Says where this end is now; peers hear only the newest of quick changes.
    pub fn set_presence(&self, presence: Presence) {
        let mut state = self.shared.state.lock().unwrap();
        if state.presence == presence {
            return;
        }
        state.presence = presence;
        state.generation += 1;
        for link in state.peers.values() {
            let _ = link.line.0.send(Out::Presence);
        }
    }

    /// The peers connected now, by id.
    pub fn peers(&self) -> Vec<Peer> {
        let state = self.shared.state.lock().unwrap();
        state.peers.values().map(|link| link.peer.clone()).collect()
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
            let _ = line.0.send(Out::Presence);
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
                        line: &line,
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

    /// Sends the newest presence whenever woken, frames as they come, and a ping when quiet.
    fn write(&self, pipe: &dyn Pipe, mut send: Sealer, outgoing: mpsc::Receiver<Out>) {
        let mut stream = pipe;
        let mut sent = None;
        loop {
            let result = match outgoing.recv_timeout(PING) {
                Ok(Out::Presence) => {
                    let presence = {
                        let state = self.state.lock().unwrap();
                        if sent == Some(state.generation) {
                            continue;
                        }
                        sent = Some(state.generation);
                        state.presence.clone()
                    };
                    send.send(&mut stream, kind::PRESENCE, &presence)
                }
                Ok(Out::Frame(kind, body)) => send.send_encoded(&mut stream, kind, &body),
                Ok(Out::Bye(body)) => {
                    let _ = send.send_encoded(&mut stream, kind::BYE, &body);
                    pipe.shutdown();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => send.send(&mut stream, kind::PING, &()),
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
