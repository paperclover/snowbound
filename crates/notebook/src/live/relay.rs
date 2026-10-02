//! Meeting peers through a relay (`crates/relay`): one WebSocket to the room, carrying a
//! stream to each peer in it, each opened with SPAKE2 and sealed end to end as on a LAN, so
//! the relay sees only who talks to whom, when, and how much. A stream that breaks makes this
//! end join the room again, which ends every stream it had there; peers then meet afresh.

use super::{OPENING, PATIENCE, Pipe, Shared, Side, code_parts};
use ::relay::{Notice, SLOT, Verdict, ws};
use base64::Engine;
use rustls::{ClientConfig, ClientConnection, RootCertStore, pki_types::ServerName};
use std::{
    collections::{HashMap, HashSet},
    io::{self, BufReader, Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, OnceLock, atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant},
};

const CONNECT: Duration = Duration::from_secs(10);
/// How often a quiet connection pings the relay, and how long it waits to hear anything.
const KEEPALIVE: Duration = Duration::from_secs(30);
const QUIET: Duration = Duration::from_secs(75);
/// A connection that lasted this long was no failure, so the next waits only a second.
const STEADY: Duration = Duration::from_secs(60);
/// The most sent in one message, well under any relay's cap.
const CHUNK: usize = 64 << 10;
/// The largest message read from the relay.
const MOST: usize = 1 << 20;

type Reader = ws::Reader<BufReader<Box<dyn Read + Send>>>;

/// Joins `shared`'s room at the relay at `url`, and keeps joining whenever it falls out.
pub(super) fn join(shared: &Arc<Shared>, url: &str) -> io::Result<()> {
    let address = parse(url)?;
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name("live relay".into())
        .spawn(move || keep(&shared, &address))?;
    Ok(())
}

/// A relay's address: `ws://` or `wss://`, a host, and the path the relay's `/v1/` follows.
#[derive(Debug, PartialEq)]
struct Address {
    tls: bool,
    /// The host and port as the URL gave them, for the `Host` header.
    authority: String,
    host: String,
    port: u16,
    path: String,
}

fn parse(url: &str) -> io::Result<Address> {
    let bad = || io::Error::new(io::ErrorKind::InvalidInput, format!("Not a relay: {url}"));
    let (tls, rest) = match url.split_once("://") {
        Some(("wss", rest)) => (true, rest),
        Some(("ws", rest)) => (false, rest),
        _ => return Err(bad()),
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, port.parse().map_err(|_| bad())?),
        _ => (authority, if tls { 443 } else { 80 }),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err(bad());
    }
    Ok(Address {
        tls,
        authority: authority.into(),
        host: host.into(),
        port,
        path: path.trim_end_matches('/').into(),
    })
}

enum Failure {
    Network(io::Error),
    /// The relay answered, but with this HTTP status and how long to wait.
    Refused(u16, Option<Duration>),
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure::Network(error)
    }
}

/// Stays in the room at `address`, joining again whenever the connection ends, until
/// `shared` stops.
fn keep(shared: &Arc<Shared>, address: &Address) {
    let mut wait = Duration::from_secs(1);
    // The number the relay gave this end's code, asked for again on joining again.
    let mut nameplate = None;
    while !shared.stopped.load(Ordering::Acquire) {
        let began = Instant::now();
        let tag = shared.room.tag();
        let path = match &tag {
            Some(tag) => format!("{}/v1/room/{tag}", address.path),
            None => match nameplate {
                Some(number) => format!("{}/v1/claim?nameplate={number}", address.path),
                None => format!("{}/v1/claim", address.path),
            },
        };
        match connect(address, &path, tag.is_none()) {
            Ok((socket, reader)) => {
                shared.state.lock().unwrap().relay = Some(Arc::clone(&socket));
                if !shared.stopped.load(Ordering::Acquire) {
                    session(shared, &socket, reader, &mut nameplate);
                }
                shared.state.lock().unwrap().relay = None;
                socket.hang_up();
            }
            Err(Failure::Refused(410, _)) => {
                eprintln!("Live: the code has expired; ask for a new one");
                return;
            }
            Err(Failure::Refused(status, retry)) => {
                eprintln!("Live: the relay refused to let this end in ({status})");
                wait = wait.max(retry.unwrap_or_default());
            }
            Err(Failure::Network(error)) => {
                eprintln!("Live: no relay at {}: {error}", address.authority);
            }
        }
        if began.elapsed() >= STEADY {
            wait = Duration::from_secs(1);
        }
        thread::sleep(wait);
        wait = (wait * 2).min(PATIENCE);
    }
}

/// Opens a WebSocket to `path` at `address`: the connection, and its reading half.
fn connect(address: &Address, path: &str, owner: bool) -> Result<(Arc<Socket>, Reader), Failure> {
    let target = (address.host.as_str(), address.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No address for the relay"))?;
    let tcp = TcpStream::connect_timeout(&target, CONNECT)?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(QUIET))?;
    tcp.set_write_timeout(Some(QUIET))?;
    let (reading, mut writing): (Box<dyn Read + Send>, Box<dyn Write + Send>) = if address.tls {
        let name = ServerName::try_from(address.host.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut connection = ClientConnection::new(tls(), name).map_err(io::Error::other)?;
        while connection.is_handshaking() {
            connection.complete_io(&mut &tcp)?;
        }
        let connection = Arc::new(Mutex::new(connection));
        (
            Box::new(TlsReader {
                tcp: tcp.try_clone()?,
                tls: Arc::clone(&connection),
                plain: Vec::new(),
                at: 0,
            }),
            Box::new(TlsWriter {
                tcp: tcp.try_clone()?,
                tls: connection,
            }),
        )
    } else {
        (Box::new(tcp.try_clone()?), Box::new(tcp.try_clone()?))
    };
    let mut key = [0; 16];
    getrandom::fill(&mut key).map_err(|_| io::Error::other("System random source failed"))?;
    let key = base64::engine::general_purpose::STANDARD.encode(key);
    write!(
        writing,
        "GET {path} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: Snowbound/{}\r\n\r\n",
        address.authority,
        env!("CARGO_PKG_VERSION"),
    )?;
    let mut reader = BufReader::new(reading);
    let head = ws::head(&mut reader)?;
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0);
    if status != 101 {
        let retry = ws::header(&head, "Retry-After")
            .and_then(|seconds| seconds.parse().ok())
            .map(Duration::from_secs);
        return Err(Failure::Refused(status, retry));
    }
    if ws::header(&head, "Sec-WebSocket-Accept") != Some(ws::accept(&key).as_str()) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Not a relay").into());
    }
    let socket = Arc::new(Socket {
        send: Mutex::new(writing),
        tcp,
        owner,
        links: Mutex::default(),
    });
    Ok((socket, ws::Reader::new(reader, MOST, false)))
}

/// The certificate authorities the system trusts, then Mozilla's for a system whose store is
/// missing or stale, as updates trust them.
fn tls() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        roots.add_parsable_certificates(webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        Arc::new(
            ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("ring speaks TLS 1.2 and 1.3")
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }))
}

/// TLS read on one thread while another writes: the socket is read without the lock, and
/// what arrives is decrypted under it.
struct TlsReader {
    tcp: TcpStream,
    tls: Arc<Mutex<ClientConnection>>,
    plain: Vec<u8>,
    at: usize,
}

impl Read for TlsReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.at == self.plain.len() {
            self.plain.clear();
            self.at = 0;
            let mut raw = vec![0; 16 << 10];
            let length = self.tcp.read(&mut raw)?;
            if length == 0 {
                return Ok(0);
            }
            let mut tls = self.tls.lock().unwrap();
            let mut arrived = &raw[..length];
            let mut closed = false;
            while !arrived.is_empty() {
                tls.read_tls(&mut arrived)?;
                tls.process_new_packets()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                let mut chunk = [0; 4096];
                loop {
                    match tls.reader().read(&mut chunk) {
                        Ok(0) => {
                            closed = true;
                            break;
                        }
                        Ok(length) => self.plain.extend_from_slice(&chunk[..length]),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => return Err(error),
                    }
                }
            }
            while tls.wants_write() {
                tls.write_tls(&mut &self.tcp)?;
            }
            if closed && self.plain.is_empty() {
                return Ok(0);
            }
        }
        let length = buffer.len().min(self.plain.len() - self.at);
        buffer[..length].copy_from_slice(&self.plain[self.at..self.at + length]);
        self.at += length;
        Ok(length)
    }
}

struct TlsWriter {
    tcp: TcpStream,
    tls: Arc<Mutex<ClientConnection>>,
}

impl Write for TlsWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut tls = self.tls.lock().unwrap();
        let length = tls.writer().write(bytes)?;
        while tls.wants_write() {
            tls.write_tls(&mut &self.tcp)?;
        }
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One connection to a relay's room.
pub(super) struct Socket {
    send: Mutex<Box<dyn Write + Send>>,
    tcp: TcpStream,
    /// Whether this end claimed the room for its code, and so tells the relay who knew it.
    owner: bool,
    links: Mutex<Links>,
}

/// The streams a socket carries, by the slot of the peer at the other end.
#[derive(Default)]
struct Links {
    /// This end's slot.
    me: u32,
    inboxes: HashMap<u32, mpsc::Sender<Vec<u8>>>,
    /// Slots whose stream ended; a peer that comes back has a new slot.
    ended: HashSet<u32>,
}

impl Socket {
    fn send(&self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let mut mask = [0; 4];
        getrandom::fill(&mut mask).map_err(|_| io::Error::other("System random source failed"))?;
        let frame = ws::frame(opcode, payload, Some(mask));
        self.send.lock().unwrap().write_all(&frame)
    }

    pub(super) fn hang_up(&self) {
        let _ = self.tcp.shutdown(Shutdown::Both);
    }

    fn forget(&self, slot: u32) {
        let mut links = self.links.lock().unwrap();
        links.inboxes.remove(&slot);
        links.ended.insert(slot);
    }
}

/// Reads notices and peers' bytes from the relay until the connection ends, opening a stream
/// to each peer in the room.
fn session(
    shared: &Arc<Shared>,
    socket: &Arc<Socket>,
    mut reader: Reader,
    nameplate: &mut Option<u32>,
) {
    let mut tag = shared.room.tag();
    // Pings while the session lasts: dropping `_beat` at its end stops them.
    let (_beat, beats) = mpsc::channel::<()>();
    let pinging = Arc::clone(socket);
    thread::spawn(move || {
        while let Err(mpsc::RecvTimeoutError::Timeout) = beats.recv_timeout(KEEPALIVE) {
            if pinging.send(ws::PING, &[]).is_err() {
                return;
            }
        }
    });
    while let Ok(message) = reader.read() {
        match message {
            ws::Message::Text(text) => match text.parse() {
                Ok(Notice::Nameplate(number)) => {
                    *nameplate = Some(number);
                    tag = Some(format!("code-{number}"));
                    let super::Room::Code(words) = &shared.room else {
                        continue;
                    };
                    let code = format!("{number}-{}", code_parts(words).1);
                    let mut state = shared.state.lock().unwrap();
                    if state.code.as_ref() != Some(&code) {
                        eprintln!("Live: the code is {code}");
                        state.code = Some(code);
                        drop(state);
                        (shared.notify)();
                    }
                }
                Ok(Notice::Welcome { you, members }) => {
                    socket.links.lock().unwrap().me = you;
                    for slot in members {
                        meet(shared, socket, tag.as_deref(), slot, None);
                    }
                }
                Ok(Notice::Joined(slot)) => meet(shared, socket, tag.as_deref(), slot, None),
                Ok(Notice::Left(slot)) => socket.forget(slot),
                Ok(Notice::Burned) => {
                    // Coming back, ask for another number.
                    *nameplate = None;
                    eprintln!("Live: too many wrong tries; the code admits no one new");
                }
                Err(()) => {}
            },
            ws::Message::Binary(data) => {
                if let Some((slot, bytes)) = data.split_first_chunk::<SLOT>() {
                    let slot = u32::from_be_bytes(*slot);
                    meet(shared, socket, tag.as_deref(), slot, Some(bytes.to_vec()));
                }
            }
            ws::Message::Ping(payload) => {
                let _ = socket.send(ws::PONG, &payload);
            }
            ws::Message::Pong => {}
            ws::Message::Close => break,
        }
    }
    socket.links.lock().unwrap().inboxes.clear();
}

/// Hands `bytes` from the peer in `slot` to its stream, or opens one: this end opens a stream
/// to a peer with a lower slot when told of it, and answers one with a higher slot when its
/// first bytes come.
fn meet(
    shared: &Arc<Shared>,
    socket: &Arc<Socket>,
    tag: Option<&str>,
    slot: u32,
    bytes: Option<Vec<u8>>,
) {
    let mut links = socket.links.lock().unwrap();
    if let Some(inbox) = links.inboxes.get(&slot) {
        if let Some(bytes) = bytes {
            let _ = inbox.send(bytes);
        }
        return;
    }
    let side = match bytes {
        None if slot < links.me => Side::Initiator,
        Some(_) if slot > links.me => Side::Responder,
        _ => return,
    };
    let Some(tag) = tag.filter(|_| !links.ended.contains(&slot)) else {
        return;
    };
    let (inbox, arriving) = mpsc::channel();
    if let Some(bytes) = bytes {
        let _ = inbox.send(bytes);
    }
    links.inboxes.insert(slot, inbox);
    let pipe = Arc::new(Relayed {
        socket: Arc::clone(socket),
        slot,
        inbox: Mutex::new(Inbox {
            arriving,
            chunk: Vec::new(),
            at: 0,
        }),
        timeout: Mutex::new(OPENING),
    });
    let shared = Arc::clone(shared);
    let tag = tag.to_owned();
    thread::spawn(move || shared.run(pipe, side, &tag));
}

/// The stream to one peer through the relay.
struct Relayed {
    socket: Arc<Socket>,
    slot: u32,
    inbox: Mutex<Inbox>,
    timeout: Mutex<Duration>,
}

struct Inbox {
    arriving: mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    at: usize,
}

impl Pipe for Relayed {
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut inbox = self.inbox.lock().unwrap();
        while inbox.at == inbox.chunk.len() {
            let timeout = *self.timeout.lock().unwrap();
            inbox.chunk = match inbox.arriving.recv_timeout(timeout) {
                Ok(chunk) => chunk,
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(io::ErrorKind::TimedOut.into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
            };
            inbox.at = 0;
        }
        let at = inbox.at;
        let length = buffer.len().min(inbox.chunk.len() - at);
        buffer[..length].copy_from_slice(&inbox.chunk[at..at + length]);
        inbox.at += length;
        Ok(length)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<usize> {
        if self.socket.links.lock().unwrap().ended.contains(&self.slot) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let length = bytes.len().min(CHUNK);
        let message = [&self.slot.to_be_bytes()[..], &bytes[..length]].concat();
        self.socket.send(ws::BINARY, &message)?;
        Ok(length)
    }

    fn set_read_timeout(&self, timeout: Duration) -> io::Result<()> {
        *self.timeout.lock().unwrap() = timeout;
        Ok(())
    }

    fn shutdown(&self) {
        self.socket.forget(self.slot);
    }

    fn direct(&self) -> bool {
        false
    }

    fn met(&self, met: bool) {
        if self.socket.owner {
            let verdict = if met { Verdict::Met } else { Verdict::Failed }(self.slot);
            let _ = self.socket.send(ws::TEXT, verdict.to_string().as_bytes());
        }
    }

    fn broken(&self) {
        self.socket.hang_up();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_addresses_parse() {
        let address = parse("wss://live.example.net").unwrap();
        assert_eq!(
            address,
            Address {
                tls: true,
                authority: "live.example.net".into(),
                host: "live.example.net".into(),
                port: 443,
                path: String::new(),
            }
        );
        let address = parse("ws://[::1]:7650/snowbound/").unwrap();
        assert_eq!((address.host.as_str(), address.port), ("::1", 7650));
        assert_eq!(address.path, "/snowbound");
        assert!(parse("https://live.example.net").is_err());
        assert!(parse("ws://:80").is_err());
    }
}
