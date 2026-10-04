//! The connection to a relay, wherever the network lets one through: straight to it, or
//! through the proxy `proxy` names by `CONNECT`, over TLS that trusts what the system trusts
//! (so a proxy that inspects HTTPS with its own authority works once the system trusts it);
//! as a WebSocket, or where something on the way refuses WebSockets, as HTTPS requests the
//! relay answers the same messages over (`GET` waits for what is to come, `POST` sends). What
//! fails is told apart, so a person can be told what to try.

use super::proxy::{self, Proxy};
use ::relay::ws;
use base64::Engine;
use rustls::{ClientConfig, ClientConnection, RootCertStore, pki_types::ServerName};
use std::{
    io::{self, BufReader, Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const CONNECT: Duration = Duration::from_secs(10);
/// How long a connection may hear nothing: the relay hears a ping every 30 s and answers.
pub(super) const QUIET: Duration = Duration::from_secs(75);
/// The most a poll's answer holds.
const MOST: usize = 4 << 20;
/// Set once WebSockets were refused, so later connections go straight to polling.
static POLLING: AtomicBool = AtomicBool::new(false);

pub use super::model::Trouble;

pub(super) enum Failure {
    Trouble(Trouble, io::Error),
    /// The relay answered, with this HTTP status and how long to wait.
    Refused(u16, Option<Duration>),
}

impl Failure {
    fn other(error: io::Error) -> Self {
        Failure::Trouble(Trouble::Other, error)
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure::other(error)
    }
}

/// A relay's address: `ws://` or `wss://`, a host, and the path the relay's `/v1/` follows.
#[derive(Debug, PartialEq)]
pub(super) struct Address {
    pub tls: bool,
    /// The host and port as the URL gave them, for the `Host` header.
    pub authority: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub(super) fn parse(url: &str) -> io::Result<Address> {
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

/// A connection to the relay: what it writes the relay's messages into, what it reads them
/// from, and how to hang up.
pub(super) struct Connection {
    pub writer: Box<dyn Write + Send>,
    pub reader: Box<dyn Read + Send>,
    pub close: Box<dyn Fn() + Send + Sync>,
}

/// Opens the relay's `path` at `address`, as a WebSocket, else as HTTPS requests where
/// something on the way refused the WebSocket.
pub(super) fn connect(address: &Address, path: &str) -> Result<Connection, Failure> {
    if !POLLING.load(Ordering::Acquire) {
        match websocket(address, path) {
            Err(Failure::Refused(status, _)) if !relay_status(status) => {
                POLLING.store(true, Ordering::Release);
            }
            Err(Failure::Trouble(Trouble::Other, _)) => {
                POLLING.store(true, Ordering::Release);
            }
            connected => return connected,
        }
    }
    poll(address, path).map_err(|failure| match failure {
        Failure::Refused(status, _) if !relay_status(status) => Failure::Trouble(
            Trouble::Blocked,
            io::Error::other(format!("Refused with {status}")),
        ),
        failure => failure,
    })
}

/// Whether the relay itself answers with `status`, rather than something on the way.
fn relay_status(status: u16) -> bool {
    matches!(status, 404 | 410 | 429 | 503)
}

/// A stream to the relay, through the proxy where there is one, over TLS where its address
/// asks.
struct Stream {
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    tcp: TcpStream,
}

fn open(address: &Address) -> Result<Stream, Failure> {
    let proxy = proxy::for_host(&address.host, address.tls);
    let (host, port) = match &proxy {
        Some(proxy) => (proxy.host.as_str(), proxy.port),
        None => (address.host.as_str(), address.port),
    };
    let reached = |trouble| match proxy {
        Some(_) => Trouble::ProxyUnreachable,
        None => trouble,
    };
    let target = (host, port)
        .to_socket_addrs()
        .map_err(|error| Failure::Trouble(reached(Trouble::Dns), error))?
        .next()
        .ok_or_else(|| Failure::Trouble(reached(Trouble::Dns), io::ErrorKind::NotFound.into()))?;
    let tcp = TcpStream::connect_timeout(&target, CONNECT).map_err(|error| {
        let trouble = match error.kind() {
            io::ErrorKind::TimedOut => Trouble::TimedOut,
            _ => Trouble::Unreachable,
        };
        Failure::Trouble(reached(trouble), error)
    })?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(QUIET))?;
    tcp.set_write_timeout(Some(QUIET))?;
    if let Some(proxy) = &proxy {
        tunnel(&tcp, proxy, address)?;
    }
    if !address.tls {
        return Ok(Stream {
            reader: Box::new(tcp.try_clone()?),
            writer: Box::new(tcp.try_clone()?),
            tcp,
        });
    }
    let name = ServerName::try_from(address.host.clone())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut connection = ClientConnection::new(tls(), name).map_err(io::Error::other)?;
    while connection.is_handshaking() {
        connection.complete_io(&mut &tcp).map_err(|error| {
            let rejected = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<rustls::Error>())
                .is_some_and(|tls| matches!(tls, rustls::Error::InvalidCertificate(_)));
            match rejected {
                true => Failure::Trouble(Trouble::Certificate, error),
                false => Failure::other(error),
            }
        })?;
    }
    let connection = Arc::new(Mutex::new(connection));
    Ok(Stream {
        reader: Box::new(TlsReader {
            tcp: tcp.try_clone()?,
            tls: Arc::clone(&connection),
            plain: Vec::new(),
            at: 0,
        }),
        writer: Box::new(TlsWriter {
            tcp: tcp.try_clone()?,
            tls: connection,
        }),
        tcp,
    })
}

/// Asks `proxy` on `tcp` to connect through to the relay.
fn tunnel(tcp: &TcpStream, proxy: &Proxy, address: &Address) -> Result<(), Failure> {
    let target = format!("{}:{}", address.host, address.port);
    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(authorization) = proxy.authorization() {
        request += &format!("Proxy-Authorization: {authorization}\r\n");
    }
    request += "\r\n";
    (&mut &*tcp)
        .write_all(request.as_bytes())
        .map_err(|error| Failure::Trouble(Trouble::ProxyUnreachable, error))?;
    let head =
        ws::head(&mut &*tcp).map_err(|error| Failure::Trouble(Trouble::ProxyUnreachable, error))?;
    match status(&head) {
        200 => Ok(()),
        407 => Err(Failure::Trouble(
            Trouble::ProxyAuthentication,
            io::Error::new(io::ErrorKind::PermissionDenied, "The proxy asks for a name"),
        )),
        status => Err(Failure::Trouble(
            Trouble::ProxyRefused(status),
            io::Error::other(format!("The proxy answered {status}")),
        )),
    }
}

fn status(head: &str) -> u16 {
    head.split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0)
}

fn retry(head: &str) -> Option<Duration> {
    ws::header(head, "Retry-After")
        .and_then(|seconds| seconds.parse().ok())
        .map(Duration::from_secs)
}

/// Opens a WebSocket to `path` at `address`.
fn websocket(address: &Address, path: &str) -> Result<Connection, Failure> {
    let Stream {
        reader,
        mut writer,
        tcp,
    } = open(address)?;
    let mut key = [0; 16];
    getrandom::fill(&mut key).map_err(|_| io::Error::other("System random source failed"))?;
    let key = base64::engine::general_purpose::STANDARD.encode(key);
    write!(
        writer,
        "GET {path} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: Snowbound/{}\r\n\r\n",
        address.authority,
        env!("CARGO_PKG_VERSION"),
    )?;
    let mut reader = BufReader::new(reader);
    let head = ws::head(&mut reader)?;
    if status(&head) != 101 {
        return Err(Failure::Refused(status(&head), retry(&head)));
    }
    if ws::header(&head, "Sec-WebSocket-Accept") != Some(ws::accept(&key).as_str()) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Not a relay").into());
    }
    Ok(Connection {
        writer,
        reader: Box::new(reader),
        close: Box::new(move || {
            let _ = tcp.shutdown(Shutdown::Both);
        }),
    })
}

/// An HTTP/1.1 connection that answers one request after another.
struct Http {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    tcp: TcpStream,
}

impl Http {
    fn open(address: &Address) -> Result<Self, Failure> {
        let Stream {
            reader,
            writer,
            tcp,
        } = open(address)?;
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            tcp,
        })
    }

    /// Sends `method` on `path` with `body`: the status, the head and the body answered.
    fn ask(
        &mut self,
        address: &Address,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> io::Result<(u16, String, Vec<u8>)> {
        write!(
            self.writer,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Snowbound/{}\r\n\
             Content-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
            address.authority,
            env!("CARGO_PKG_VERSION"),
            body.len()
        )?;
        self.writer.write_all(body)?;
        let head = ws::head(&mut self.reader)?;
        let length: usize = ws::header(&head, "Content-Length")
            .and_then(|length| length.parse().ok())
            .unwrap_or(0);
        if length > MOST {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "An answer too large",
            ));
        }
        let mut answer = vec![0; length];
        self.reader.read_exact(&mut answer)?;
        Ok((status(&head), head, answer))
    }
}

/// What a polled session shares between its reading, its sending and hanging up.
struct Session {
    /// Messages written and not yet sent.
    pending: Mutex<Vec<u8>>,
    ready: Condvar,
    closed: AtomicBool,
    /// The connections open now, to shut on hanging up.
    open: Mutex<Vec<TcpStream>>,
}

impl Session {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.ready.notify_all();
        for tcp in self.open.lock().unwrap().drain(..) {
            let _ = tcp.shutdown(Shutdown::Both);
        }
    }
}

/// Joins `path` as HTTPS requests: the relay's answer names a session, whose messages a
/// `GET` waits for and a `POST` sends, each a run of WebSocket frames as on a WebSocket.
fn poll(address: &Address, path: &str) -> Result<Connection, Failure> {
    let mut http = Http::open(address)?;
    let joined = match path.contains('?') {
        true => format!("{path}&poll=1"),
        false => format!("{path}?poll=1"),
    };
    let (status, head, body) = http.ask(address, "GET", &joined, &[])?;
    if status != 200 {
        return Err(Failure::Refused(status, retry(&head)));
    }
    let session_id = String::from_utf8_lossy(&body)
        .strip_prefix("session ")
        .map(|id| id.trim().to_owned())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Not a relay"))?;
    let at = format!("{}/v1/poll/{session_id}", address.path);
    let session = Arc::new(Session {
        pending: Mutex::default(),
        ready: Condvar::new(),
        closed: AtomicBool::new(false),
        open: Mutex::new(vec![http.tcp.try_clone()?]),
    });
    let address = Arc::new(Address {
        tls: address.tls,
        authority: address.authority.clone(),
        host: address.host.clone(),
        port: address.port,
        path: address.path.clone(),
    });
    // Sends what is written, a batch a request, on a connection of its own.
    let (sending, to, posting) = (Arc::clone(&session), at.clone(), Arc::clone(&address));
    thread::Builder::new()
        .name("live relay post".into())
        .spawn(move || post(&sending, &posting, &to))?;
    let closing = Arc::clone(&session);
    Ok(Connection {
        writer: Box::new(PollWriter(Arc::clone(&session))),
        reader: Box::new(PollReader {
            session,
            address,
            at,
            http: Some(http),
            arrived: Vec::new(),
            read: 0,
        }),
        close: Box::new(move || closing.close()),
    })
}

fn post(session: &Session, address: &Address, at: &str) {
    let mut http: Option<Http> = None;
    loop {
        let batch = {
            let mut pending = session.pending.lock().unwrap();
            while pending.is_empty() && !session.closed.load(Ordering::Acquire) {
                pending = session.ready.wait(pending).unwrap();
            }
            if session.closed.load(Ordering::Acquire) {
                return;
            }
            std::mem::take(&mut *pending)
        };
        // A connection the relay or the proxy closed meanwhile is opened again, once.
        let sent = (0..2).any(|_| {
            if http.is_none() {
                http = Http::open(address).ok();
                if let (Some(opened), Ok(mut open)) = (&http, session.open.lock()) {
                    open.extend(opened.tcp.try_clone());
                }
            }
            let answered = http
                .as_mut()
                .map(|connection| connection.ask(address, "POST", at, &batch));
            match answered {
                Some(Ok((200 | 204, ..))) => true,
                Some(Ok((410, ..))) => {
                    session.close();
                    true
                }
                _ => {
                    http = None;
                    false
                }
            }
        });
        if !sent {
            session.close();
            return;
        }
    }
}

struct PollWriter(Arc<Session>);

impl Write for PollWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.0.pending.lock().unwrap().extend_from_slice(bytes);
        self.0.ready.notify_all();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads what each `GET` brings, waiting for the next when it is all read.
struct PollReader {
    session: Arc<Session>,
    address: Arc<Address>,
    at: String,
    http: Option<Http>,
    arrived: Vec<u8>,
    read: usize,
}

impl Read for PollReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.read == self.arrived.len() {
            if self.session.closed.load(Ordering::Acquire) {
                return Ok(0);
            }
            if self.http.is_none() {
                let opened = Http::open(&self.address).map_err(|failure| match failure {
                    Failure::Trouble(_, error) => error,
                    Failure::Refused(status, _) => io::Error::other(format!("{status}")),
                })?;
                self.session
                    .open
                    .lock()
                    .unwrap()
                    .extend(opened.tcp.try_clone());
                self.http = Some(opened);
            }
            let answered =
                self.http
                    .as_mut()
                    .expect("opened above")
                    .ask(&self.address, "GET", &self.at, &[]);
            match answered {
                Ok((200, _, body)) => {
                    self.arrived = body;
                    self.read = 0;
                }
                Ok((410, ..)) => {
                    self.session.close();
                    return Ok(0);
                }
                Ok((status, ..)) => {
                    return Err(io::Error::other(format!("The relay answered {status}")));
                }
                Err(error) => {
                    // Opened again once; a second failure ends the session.
                    if self.http.take().is_none() {
                        return Err(error);
                    }
                    self.http = Http::open(&self.address).ok();
                    if self.http.is_none() {
                        return Err(error);
                    }
                }
            }
        }
        let length = buffer.len().min(self.arrived.len() - self.read);
        buffer[..length].copy_from_slice(&self.arrived[self.read..self.read + length]);
        self.read += length;
        Ok(length)
    }
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
