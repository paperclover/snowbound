//! Live presence: who else has the notebook open, the page they are on and their caret,
//! straight from one Snowbound to another. Peers find each other with mDNS
//! (`_snowbound._tcp`) and meet through a secret both hold, a notebook's identity or a code
//! typed on both, which SPAKE2 turns into the keys every frame after the opening is sealed
//! with. The lower peer id connects; each connection has a thread reading and one writing.

pub mod wire;
pub use wire::{Caret, Guid, Hello, Presence, Spot};

use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io,
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

/// The secret peers meet through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Room {
    /// Everyone with the notebook: its table of contents' file identity, which only its files
    /// hold.
    Notebook([u8; 16]),
    /// A code typed on both: `7-violet-otter`, whose number names it on the network and whose
    /// words only the two people know.
    Code(String),
}

impl Room {
    /// What names the room in the clear: a hash of a notebook's identity, a code's number.
    fn tag(&self) -> String {
        match self {
            Room::Notebook(id) => hex(&Sha256::digest([&b"Snowbound room "[..], id].concat())[..8]),
            Room::Code(code) => format!("code-{}", code.split('-').next().unwrap_or_default()),
        }
    }

    fn secret(&self) -> Vec<u8> {
        match self {
            Room::Notebook(id) => id.to_vec(),
            Room::Code(code) => code.trim().to_lowercase().into_bytes(),
        }
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
    tag: String,
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
}

struct Link {
    connection: u64,
    peer: Peer,
    wake: mpsc::Sender<()>,
    stream: TcpStream,
}

impl Live {
    /// Starts listening as `me` in `room`, advertised and looked for where `reach` says, or
    /// not at all with `None`, leaving peers to `connect`. `notify` runs on a network thread
    /// whenever `peers` changes.
    pub fn start(
        me: Hello,
        room: &Room,
        reach: Option<Reach>,
        notify: impl Fn() + Send + Sync + 'static,
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
        let shared = Arc::new(Shared {
            me,
            tag: room.tag(),
            secret: room.secret(),
            state: Mutex::default(),
            notify: Box::new(notify),
            stopped: AtomicBool::new(false),
            connections: AtomicU64::new(0),
        });
        let accepting = Arc::clone(&shared);
        thread::Builder::new()
            .name("live accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if accepting.stopped.load(Ordering::Acquire) {
                        return;
                    }
                    if let Ok(stream) = stream {
                        let shared = Arc::clone(&accepting);
                        thread::spawn(move || shared.run(stream, Side::Responder));
                    }
                }
            })?;
        let daemon = match reach {
            Some(reach) => {
                Some(advertise(&shared, reach, address.port()).map_err(io::Error::other)?)
            }
            None => None,
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
        for link in self.shared.state.lock().unwrap().peers.values() {
            let _ = link.stream.shutdown(Shutdown::Both);
        }
    }
}

/// Advertises `shared` on `port` and connects to the peers in its room that discovery finds
/// with a higher id than its own, which leave the connecting to it.
fn advertise(shared: &Arc<Shared>, reach: Reach, port: u16) -> mdns_sd::Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let id = hex(&shared.me.peer);
    let properties = [
        ("v", "1"),
        ("room", shared.tag.as_str()),
        ("peer", id.as_str()),
    ];
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
                if room != shared.tag || peer <= id.as_str() || shared.knows(peer) {
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

    fn dial(self: Arc<Self>, address: SocketAddr) {
        if let Ok(stream) = TcpStream::connect_timeout(&address, OPENING) {
            self.run(stream, Side::Initiator);
        }
    }

    /// Meets the peer at the other end of `stream`, then reads from it until it goes.
    fn run(self: Arc<Self>, mut stream: TcpStream, side: Side) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let met = (|| {
            stream.set_read_timeout(Some(OPENING))?;
            stream.set_nodelay(true)?;
            let (mut send, mut receive) = wire::open(&mut stream, side, &self.tag, &self.secret)?;
            send.send(&mut stream, kind::HELLO, &self.me)?;
            let (first, body) = receive.receive(&mut stream)?;
            if first != kind::HELLO {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "No hello"));
            }
            let hello: Hello = minicbor::decode(&body)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "A malformed hello"))?;
            stream.set_read_timeout(Some(GONE))?;
            Ok((send, receive, hello))
        })();
        let (send, mut receive, hello) = match met {
            Ok(met) => met,
            Err(error) => {
                eprintln!("Live: no meeting with {:?}: {error}", stream.peer_addr());
                return;
            }
        };
        let peer = hello.peer;
        let connection = self.connections.fetch_add(1, Ordering::Relaxed);
        let (wake, woken) = mpsc::channel();
        {
            let mut state = self.state.lock().unwrap();
            if peer == self.me.peer || state.peers.contains_key(&peer) {
                return;
            }
            let Ok(writing) = stream.try_clone() else {
                return;
            };
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
                    stream: writing,
                },
            );
        }
        (self.notify)();
        if let Ok(writing) = stream.try_clone() {
            let shared = Arc::clone(&self);
            thread::spawn(move || shared.write(writing, send, woken));
        }
        while let Ok((message, body)) = receive.receive(&mut stream) {
            if message != kind::PRESENCE {
                continue;
            }
            let Ok(presence) = minicbor::decode::<Presence>(&body) else {
                break;
            };
            if let Some(link) = self.state.lock().unwrap().peers.get_mut(&peer) {
                link.peer.presence = Some(presence);
            }
            (self.notify)();
        }
        let _ = stream.shutdown(Shutdown::Both);
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
    }

    /// Sends the newest presence whenever woken, and a ping when quiet.
    fn write(&self, mut stream: TcpStream, mut send: Sealer, woken: mpsc::Receiver<()>) {
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
                let _ = stream.shutdown(Shutdown::Both);
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
