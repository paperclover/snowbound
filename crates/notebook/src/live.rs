//! Live presence: who else has the notebook open, the page they are on and their caret,
//! straight from one Snowbound to another, or through a relay (`crates/relay`) where they
//! aren't on one network. Peers find each other with mDNS (`_snowbound._tcp`) or in the
//! relay's room, and meet through a secret both hold, a notebook's identity or a code typed
//! on both, which SPAKE2 turns into the keys every frame after the opening is sealed with.
//! Each connection has a thread reading and one writing. A connection whose frames arrive
//! out of order is dropped and met again from scratch.

mod relay;
pub mod wire;
pub use wire::{Caret, Guid, Hello, Presence, Spot};

use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
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
    time::Duration,
};
use wire::{Sealer, Side, kind};

const SERVICE: &str = "_snowbound._tcp.local.";
/// How long a connection may be quiet before a ping, and before the peer counts as gone.
const PING: Duration = Duration::from_secs(15);
const GONE: Duration = Duration::from_secs(45);
const OPENING: Duration = Duration::from_secs(5);
/// The longest wait before meeting again.
const PATIENCE: Duration = Duration::from_secs(30);

/// The secret peers meet through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Room {
    /// Everyone with the notebook: its table of contents' file identity, which only its files
    /// hold.
    Notebook([u8; 16]),
    /// A code typed on both: `7-violet-otter`, whose number names it on the network and whose
    /// words only the two people know. Its words alone (`violet-otter`) ask the relay for a
    /// free number, and `Live::code` then has the whole code.
    Code(String),
}

impl Room {
    /// What names the room in the clear: a hash of a notebook's identity, a code's number;
    /// none for a code the relay hasn't numbered.
    fn tag(&self) -> Option<String> {
        match self {
            Room::Notebook(id) => Some(hex(&Sha256::digest(
                [&b"Snowbound room "[..], id].concat(),
            )[..8])),
            Room::Code(code) => code_parts(code).0.map(|number| format!("code-{number}")),
        }
    }

    fn secret(&self) -> Vec<u8> {
        match self {
            Room::Notebook(id) => id.to_vec(),
            Room::Code(code) => code_parts(code).1.to_lowercase().into_bytes(),
        }
    }
}

/// A code's number, if it has one, and its words.
fn code_parts(code: &str) -> (Option<u32>, &str) {
    let code = code.trim();
    match code.split_once('-') {
        Some((number, words)) if number.parse::<u32>().is_ok() => (number.parse().ok(), words),
        _ => (None, code),
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

/// Presence on the network while it lives; dropping it leaves.
pub struct Live {
    shared: Arc<Shared>,
    address: SocketAddr,
    daemon: Option<ServiceDaemon>,
}

struct Shared {
    me: Hello,
    room: Room,
    secret: Vec<u8>,
    state: Mutex<State>,
    notify: Box<dyn Fn() + Send + Sync>,
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
}

struct Link {
    connection: u64,
    peer: Peer,
    wake: mpsc::Sender<()>,
    pipe: Arc<dyn Pipe>,
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
    /// (`wss://live.example.net`) where given. `notify` runs on a network thread whenever
    /// `peers` or `code` changes.
    pub fn start(
        me: Hello,
        room: &Room,
        reach: Option<Reach>,
        relay: Option<&str>,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Live> {
        let tag = room.tag();
        if tag.is_none() && relay.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Only a relay can number a code",
            ));
        }
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
            Room::Code(code) if tag.is_some() => Some(code.trim().to_owned()),
            _ => None,
        };
        let shared = Arc::new(Shared {
            me,
            room: room.clone(),
            secret: room.secret(),
            state: Mutex::new(State {
                code,
                ..State::default()
            }),
            notify: Box::new(notify),
            stopped: AtomicBool::new(false),
            connections: AtomicU64::new(0),
        });
        if let Some(relay) = relay {
            relay::join(&shared, relay)?;
        }
        let accepting = Arc::clone(&shared);
        let accepted = tag.clone();
        thread::Builder::new()
            .name("live accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if accepting.stopped.load(Ordering::Acquire) {
                        return;
                    }
                    if let (Ok(stream), Some(tag)) = (stream, accepted.clone()) {
                        let shared = Arc::clone(&accepting);
                        thread::spawn(move || shared.run(Arc::new(stream), Side::Responder, &tag));
                    }
                }
            })?;
        let daemon = match (reach, tag) {
            (Some(reach), Some(tag)) => {
                Some(advertise(&shared, reach, tag, address.port()).map_err(io::Error::other)?)
            }
            _ => None,
        };
        Ok(Live {
            shared,
            address,
            daemon,
        })
    }

    /// Where this end listens.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The code others type to meet this end: the one it was given, or the one the relay
    /// numbered.
    pub fn code(&self) -> Option<String> {
        self.shared.state.lock().unwrap().code.clone()
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
            let _ = link.wake.send(());
        }
    }

    /// The peers connected now, by id.
    pub fn peers(&self) -> Vec<Peer> {
        let state = self.shared.state.lock().unwrap();
        state.peers.values().map(|link| link.peer.clone()).collect()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        if let Some(daemon) = &self.daemon {
            let _ = daemon.shutdown();
        }
        // Wakes the accepting thread to see it has stopped.
        let _ = TcpStream::connect_timeout(&self.address, OPENING);
        let state = self.shared.state.lock().unwrap();
        for link in state.peers.values() {
            link.pipe.shutdown();
        }
        if let Some(relay) = &state.relay {
            relay.hang_up();
        }
    }
}

/// Advertises `shared` as in room `tag` on `port` and connects to the peers in its room that
/// discovery finds with a higher id than its own, which leave the connecting to it.
fn advertise(
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
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name("live discovery".into())
        .spawn(move || {
            while let Ok(event) = found.recv() {
                let ServiceEvent::ServiceResolved(service) = event else {
                    continue;
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
                    let shared = Arc::clone(&shared);
                    thread::spawn(move || shared.dial(address));
                }
            }
        })
        .map_err(|error| mdns_sd::Error::Msg(error.to_string()))?;
    Ok(daemon)
}

impl Shared {
    fn knows(&self, peer: &str) -> bool {
        let state = self.state.lock().unwrap();
        state.peers.keys().any(|id| hex(id) == peer)
    }

    /// Connects to `address`, and again while the peer is there and the connection was the
    /// one this end kept.
    fn dial(self: Arc<Self>, address: SocketAddr) {
        let Some(tag) = self.room.tag() else {
            return;
        };
        let mut wait = Duration::from_secs(1);
        while let Ok(stream) = TcpStream::connect_timeout(&address, OPENING) {
            if !Arc::clone(&self).run(Arc::new(stream), Side::Initiator, &tag)
                || self.stopped.load(Ordering::Acquire)
            {
                return;
            }
            thread::sleep(wait);
            wait = (wait * 2).min(PATIENCE);
        }
    }

    /// Meets the peer at the other end of `pipe` in room `tag`, then reads from it until it
    /// goes: whether it was the connection kept to that peer.
    fn run(self: Arc<Self>, pipe: Arc<dyn Pipe>, side: Side, tag: &str) -> bool {
        if self.stopped.load(Ordering::Acquire) {
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
        pipe.met(met.is_ok());
        let (send, mut receive, hello) = match met {
            Ok(met) => met,
            Err(error) => {
                eprintln!("Live: no meeting in {tag}: {error}");
                pipe.shutdown();
                return false;
            }
        };
        let peer = hello.peer;
        let name = hello.name.clone();
        let connection = self.connections.fetch_add(1, Ordering::Relaxed);
        let (wake, woken) = mpsc::channel();
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
            let _ = wake.send(());
            state.peers.insert(
                peer,
                Link {
                    connection,
                    peer: Peer {
                        hello: Arc::new(hello),
                        presence: None,
                    },
                    wake,
                    pipe: Arc::clone(&pipe),
                },
            );
        }
        (self.notify)();
        let writing = Arc::clone(&pipe);
        let shared = Arc::clone(&self);
        thread::spawn(move || shared.write(&*writing, send, woken));
        let ended = loop {
            let (message, body) = match receive.receive(&mut stream) {
                Ok(frame) => frame,
                Err(error) => break Some(error),
            };
            if message != kind::PRESENCE {
                continue;
            }
            let Ok(presence) = minicbor::decode::<Presence>(&body) else {
                break None;
            };
            if let Some(link) = self.state.lock().unwrap().peers.get_mut(&peer) {
                link.peer.presence = Some(presence);
            }
            (self.notify)();
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
            (self.notify)();
        }
        true
    }

    /// Sends the newest presence whenever woken, and a ping when quiet.
    fn write(&self, pipe: &dyn Pipe, mut send: Sealer, woken: mpsc::Receiver<()>) {
        let mut stream = pipe;
        let mut sent = None;
        loop {
            let result = match woken.recv_timeout(PING) {
                Ok(()) => {
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
