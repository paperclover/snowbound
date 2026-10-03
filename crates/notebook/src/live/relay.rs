//! Meeting peers through a relay (`crates/relay`): one WebSocket to the room, carrying
//! streams to peers in it, each opened with SPAKE2 and sealed end to end as on a LAN, so the
//! relay sees only who talks to whom, when, and how much. In a code's room every two peers
//! have a stream. In a notebook's room only a host and each guest do; everything else goes
//! to the room's group (`group`), sent once and copied by the relay. A stream or group frame
//! that breaks makes this end join the room again, which ends every stream it had there;
//! peers then meet afresh.

use super::{
    Event, Hello, OPENING, PATIENCE, Paced, Peer, Pipe, Presence, Relayed as Answer, Room, Shared,
    Side, code_parts, group,
    transport::{self, Address, Failure, parse},
    wire::kind,
};
use ::relay::{BROADCAST, GROUP, Notice, SLOT, Verdict, ws};
use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    io::{self, BufReader, Read, Write},
    sync::{Arc, Mutex, atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant},
};

/// How often a quiet connection pings the relay.
const KEEPALIVE: Duration = Duration::from_secs(30);
/// A connection that lasted this long was no failure, so the next waits only a second.
const STEADY: Duration = Duration::from_secs(60);
/// The most sent in one message, well under any relay's cap.
const CHUNK: usize = 64 << 10;
/// The largest message read from the relay.
const MOST: usize = 1 << 20;

type Reader = ws::Reader<BufReader<Box<dyn Read + Send>>>;

/// Joins `shared`'s room at the relay at `url`, and keeps joining whenever it falls out;
/// `port` is where it listens, to advertise once the relay numbers its code.
pub(super) fn join(shared: &Arc<Shared>, url: &str, port: u16) -> io::Result<()> {
    let address = parse(url)?;
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name("live relay".into())
        .spawn(move || keep(&shared, &address, port))?;
    Ok(())
}

/// Records how the relay answered, telling `shared`'s events where it changed.
fn answered(shared: &Shared, answer: Answer) {
    let mut state = shared.state.lock().unwrap();
    if state.relayed.as_ref() != Some(&answer) {
        state.relayed = Some(answer);
        drop(state);
        (shared.events)(Event::Changed);
    }
}

/// Stays in the room at `address`, joining again whenever the connection ends, until
/// `shared` stops.
fn keep(shared: &Arc<Shared>, address: &Address, port: u16) {
    let mut wait = Duration::from_secs(1);
    let owner = shared.room.owner();
    // The number of this end's code, asked for again on joining again.
    let mut nameplate = owner
        .then(|| code_parts(shared.state.lock().unwrap().code.as_deref()?).0)
        .flatten();
    while !shared.stopped.load(Ordering::Acquire) {
        let began = Instant::now();
        let path = match (owner, shared.tag()) {
            (false, Some(tag)) => format!("{}/v1/room/{tag}", address.path),
            (false, None) => return,
            (true, _) => match nameplate {
                Some(number) => format!("{}/v1/claim?nameplate={number}", address.path),
                None => format!("{}/v1/claim", address.path),
            },
        };
        match connect(shared, address, &path, owner) {
            Ok((socket, reader)) => {
                shared.state.lock().unwrap().relay = Some(Arc::clone(&socket));
                answered(shared, Answer::Joined);
                if !shared.stopped.load(Ordering::Acquire) {
                    session(shared, &socket, reader, &mut nameplate, port);
                }
                shared.state.lock().unwrap().relay = None;
                socket.hang_up();
            }
            Err(Failure::Refused(status, retry)) => {
                eprintln!("Live: the relay refused to let this end in ({status})");
                answered(shared, Answer::Refused(status, retry));
                if status == 410 {
                    return;
                }
                wait = wait.max(retry.unwrap_or_default());
            }
            Err(Failure::Trouble(trouble, error)) => {
                eprintln!("Live: no relay at {}: {error}", address.authority);
                answered(shared, Answer::Unreachable(trouble));
            }
        }
        if began.elapsed() >= STEADY {
            wait = Duration::from_secs(1);
        }
        thread::sleep(wait);
        wait = (wait * 2).min(PATIENCE);
    }
}

/// Opens the relay's `path` at `address`: the connection, and what reads its messages.
fn connect(
    shared: &Shared,
    address: &Address,
    path: &str,
    owner: bool,
) -> Result<(Arc<Socket>, Reader), Failure> {
    let connection = transport::connect(address, path)?;
    let reader = ws::Reader::new(BufReader::new(connection.reader), MOST, false);
    let group = matches!(shared.room, Room::Notebook(_)).then(|| Group {
        keys: group::Keys::new(&shared.secret),
        members: Mutex::default(),
        out: Mutex::default(),
    });
    let socket = Arc::new(Socket {
        send: Mutex::new(connection.writer),
        close: connection.close,
        owner,
        links: Mutex::default(),
        group,
    });
    Ok((socket, reader))
}

/// One connection to a relay's room.
pub(super) struct Socket {
    send: Mutex<Box<dyn Write + Send>>,
    close: Box<dyn Fn() + Send + Sync>,
    /// Whether this end claimed the room for its code, and so tells the relay who knew it.
    owner: bool,
    links: Mutex<Links>,
    /// A notebook's room's group.
    pub(super) group: Option<Group>,
}

/// A notebook's room's group: its key, the peers heard in it, and the way to this end's
/// thread sending to it.
pub(super) struct Group {
    keys: group::Keys,
    /// The peers heard in the group, by slot.
    pub(super) members: Mutex<HashMap<u32, group::Member>>,
    /// To the thread sending this end's group frames, while connected.
    out: Mutex<Option<mpsc::Sender<Out>>>,
}

/// What this end sends the group next.
pub(super) enum Out {
    /// The newest presence, to everyone.
    Presence,
    /// A message kind and its encoded body, to everyone or to the peers in these slots.
    Frame(u16, Vec<u8>, Option<Vec<u32>>),
}

impl Group {
    pub(super) fn send(&self, out: Out) {
        if let Some(sender) = &*self.out.lock().unwrap() {
            let _ = sender.send(out);
        }
    }

    /// The slot of the member that is peer `id`.
    pub(super) fn slot(&self, id: &[u8; 16]) -> Option<u32> {
        let members = self.members.lock().unwrap();
        members
            .iter()
            .find_map(|(slot, member)| (member.hello()?.peer == *id).then_some(*slot))
    }
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
        (self.close)();
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
    port: u16,
) {
    let mut tag = shared.tag();
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
    if let Some(group) = &socket.group {
        let (out, outgoing) = mpsc::channel();
        *group.out.lock().unwrap() = Some(out);
        let (shared, socket) = (Arc::clone(shared), Arc::clone(socket));
        thread::spawn(move || speak(&shared, &socket, outgoing));
    }
    let notebook = socket.group.is_some();
    while let Ok(message) = reader.read() {
        match message {
            ws::Message::Text(text) => match text.parse() {
                Ok(Notice::Nameplate(number)) => {
                    *nameplate = Some(number);
                    tag = Some(format!("code-{number}"));
                    let super::Room::Code { code, .. } = &shared.room else {
                        continue;
                    };
                    let Some(code) = super::code::format(number, &code_parts(code).1) else {
                        continue;
                    };
                    let mut state = shared.state.lock().unwrap();
                    if state.code.as_ref() != Some(&code) {
                        state.code = Some(code);
                        drop(state);
                        shared.advertise(port);
                        (shared.events)(Event::Changed);
                    }
                }
                Ok(Notice::Welcome { you, members }) => {
                    socket.links.lock().unwrap().me = you;
                    for slot in members.into_iter().filter(|_| !notebook) {
                        meet(shared, socket, tag.as_deref(), slot, None);
                    }
                }
                Ok(Notice::Joined(slot)) if !notebook => {
                    meet(shared, socket, tag.as_deref(), slot, None);
                }
                Ok(Notice::Joined(_)) => {}
                Ok(Notice::Left(slot)) => {
                    socket.forget(slot);
                    let left = (socket.group.as_ref())
                        .and_then(|group| group.members.lock().unwrap().remove(&slot));
                    if left.is_some() {
                        (shared.events)(Event::Changed);
                    }
                }
                Ok(Notice::Burned) => {
                    // Coming back, ask for another number.
                    *nameplate = None;
                    shared.state.lock().unwrap().burned = true;
                    (shared.events)(Event::Changed);
                }
                Err(()) => {}
            },
            ws::Message::Binary(data) => match data.split_first_chunk::<SLOT>() {
                Some((slot, frame)) if u32::from_be_bytes(*slot) & GROUP != 0 => {
                    let slot = u32::from_be_bytes(*slot) & !GROUP;
                    heard(shared, socket, tag.as_deref(), slot, frame);
                }
                Some((slot, bytes)) => {
                    let slot = u32::from_be_bytes(*slot);
                    meet(shared, socket, tag.as_deref(), slot, Some(bytes.to_vec()));
                }
                None => {}
            },
            ws::Message::Ping(payload) => {
                let _ = socket.send(ws::PONG, &payload);
            }
            ws::Message::Pong => {}
            ws::Message::Close => break,
        }
    }
    socket.links.lock().unwrap().inboxes.clear();
    if let Some(group) = &socket.group {
        *group.out.lock().unwrap() = None;
        group.members.lock().unwrap().clear();
        (shared.events)(Event::Changed);
    }
}

/// Sends this end's group frames: its hello, asking everyone for theirs, then its presence
/// at most every `PRESENCE_EVERY`, and frames as they come.
fn speak(shared: &Shared, socket: &Socket, outgoing: mpsc::Receiver<Out>) {
    let Some(group) = &socket.group else {
        return;
    };
    let Ok(mut sealer) = group::Sealer::new(&group.keys) else {
        return;
    };
    let mut send = |kind: u16, body: &[u8], to: Option<&[u32]>| -> io::Result<()> {
        let sealed = sealer.seal(kind, body, to.is_none())?;
        let mut message = match to {
            None => BROADCAST.to_be_bytes().to_vec(),
            Some(slots) => {
                let mut message = (GROUP | slots.len() as u32).to_be_bytes().to_vec();
                slots
                    .iter()
                    .for_each(|slot| message.extend(slot.to_be_bytes()));
                message
            }
        };
        message.extend(sealed);
        socket.send(ws::BINARY, &message)
    };
    let hello = minicbor::to_vec(&shared.me).unwrap_or_default();
    if send(kind::HELLO, &hello, None).is_err() {
        return;
    }
    let mut paced = Paced::default();
    paced.changed();
    loop {
        let next = match paced.due {
            Some(_) => outgoing.recv_timeout(paced.wait(Duration::ZERO)),
            None => outgoing
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        let result = match next {
            Ok(Out::Presence) => {
                paced.changed();
                Ok(())
            }
            Ok(Out::Frame(kind, body, to)) => send(kind, &body, to.as_deref()),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let result = result.and_then(|()| match paced.due(&shared.state) {
            Some(presence) => send(
                kind::PRESENCE,
                &minicbor::to_vec(&presence).unwrap_or_default(),
                None,
            ),
            None => Ok(()),
        });
        if result.is_err() {
            return;
        }
    }
}

/// Takes in a group frame from the peer in `slot`: its hello, answered with this end's and
/// meeting it where it serves; its presence; or what else it says.
fn heard(shared: &Arc<Shared>, socket: &Arc<Socket>, tag: Option<&str>, slot: u32, frame: &[u8]) {
    let Some(group) = &socket.group else {
        return;
    };
    let opened = {
        let mut members = group.members.lock().unwrap();
        let member = match members.entry(slot) {
            Entry::Occupied(member) => Ok(member.into_mut()),
            Entry::Vacant(vacant) => {
                group::Member::new(&group.keys, frame).map(|m| vacant.insert(m))
            }
        };
        member.and_then(|member| {
            let (kind, body) = member.open(frame)?;
            Ok((kind, body, member.hello().cloned()))
        })
    };
    let (kind, body, hello) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("Live: the relay broke the room's frames ({error}); meeting again");
            socket.hang_up();
            return;
        }
    };
    match kind {
        kind::HELLO | kind::HELLO_BACK => {
            let Ok(hello) = minicbor::decode::<Hello>(&body) else {
                return;
            };
            if hello.peer == shared.me.peer {
                return;
            }
            let serves = hello.serves.is_some() && shared.me.serves.is_none();
            if let Some(member) = group.members.lock().unwrap().get_mut(&slot) {
                member.peer = Some(Peer {
                    hello: Arc::new(hello),
                    presence: None,
                });
            }
            if kind == kind::HELLO {
                let presence = minicbor::to_vec(&shared.state.lock().unwrap().presence);
                let presence = presence.unwrap_or_default();
                let me = minicbor::to_vec(&shared.me).unwrap_or_default();
                group.send(Out::Frame(kind::HELLO_BACK, me, Some(vec![slot])));
                group.send(Out::Frame(kind::PRESENCE, presence, Some(vec![slot])));
            }
            if serves {
                meet(shared, socket, tag, slot, None);
            }
            (shared.events)(Event::Changed);
        }
        kind::PRESENCE => {
            let Ok(presence) = minicbor::decode::<Presence>(&body) else {
                return;
            };
            let mut members = group.members.lock().unwrap();
            if let Some(peer) = members.get_mut(&slot).and_then(|m| m.peer.as_mut()) {
                peer.presence = Some(presence);
                drop(members);
                (shared.events)(Event::Changed);
            }
        }
        // A peer met directly says the rest there.
        kind if kind < 256 => {
            if let Some(hello) = hello.filter(|hello| !shared.direct(&hello.peer)) {
                (shared.events)(Event::Frame {
                    from: &hello,
                    kind,
                    body: &body,
                });
            }
        }
        _ => {}
    }
}

/// Hands `bytes` from the peer in `slot` to its stream, or opens one. In a code's room this
/// end opens a stream to a peer with a lower slot when told of it, and answers one with a
/// higher slot when its first bytes come; in a notebook's room a guest opens one to its host.
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
    let notebook = socket.group.is_some();
    let side = match bytes {
        None if notebook || slot < links.me => Side::Initiator,
        Some(_) if (notebook && shared.me.serves.is_some()) || (!notebook && slot > links.me) => {
            Side::Responder
        }
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
