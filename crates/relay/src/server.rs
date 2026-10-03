//! The relay: rooms of WebSocket peers, each message passed on to the peer it names, with
//! limits on everything a stranger can make it hold. A thread reads each connection and
//! another writes it, from a queue capped in bytes.

use crate::{Notice, SLOT, Verdict, ws};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::{BufReader, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, TcpListener, TcpStream},
    ops::RangeInclusive,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// What a relay allows; `snowbound-relay --help` explains each.
#[derive(Clone, Debug)]
pub struct Config {
    /// Counts a peer by the last `X-Forwarded-For` entry, the one its own proxy added.
    pub trust_forwarded: bool,
    pub max_connections: usize,
    pub max_connections_per_address: usize,
    pub max_rooms: usize,
    pub max_room_peers: usize,
    /// The largest message, in bytes.
    pub max_message: usize,
    /// The most bytes waiting to go to one peer before the relay hangs up on it.
    pub queue: usize,
    /// How long a connection may send nothing.
    pub idle: Duration,
    pub room_bytes_per_second: u64,
    pub joins_per_minute: u32,
    pub room_joins_per_minute: u32,
    /// Wrong codes an address may try in a minute before it is locked out, a minute the first
    /// time and twice as long each time after, up to an hour.
    pub failures_per_minute: u32,
    /// Wrong codes after which a code admits no one new.
    pub burn_after: u32,
    /// How long a peer joining a code's room has to meet its owner.
    pub pending: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            trust_forwarded: false,
            max_connections: 256,
            max_connections_per_address: 16,
            max_rooms: 128,
            max_room_peers: 16,
            max_message: 256 << 10,
            queue: 1 << 20,
            idle: Duration::from_secs(600),
            room_bytes_per_second: 4 << 20,
            joins_per_minute: 30,
            room_joins_per_minute: 30,
            failures_per_minute: 10,
            burn_after: 5,
            pending: Duration::from_secs(20),
        }
    }
}

const HANDSHAKE: Duration = Duration::from_secs(10);
/// How long a polled session's `GET` waits for something to bring, how long one of its
/// connections may idle between requests, and how long a session may ask nothing.
const WAIT: Duration = Duration::from_secs(25);
const KEPT: Duration = Duration::from_secs(60);
const IDLE_POLL: Duration = Duration::from_secs(60);
const WRITE: Duration = Duration::from_secs(30);
const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);
/// Addresses remembered at once; past it, new ones wait.
const ADDRESSES: usize = 1 << 16;
const NAMEPLATES: RangeInclusive<u32> = 1..=999;
const STACK: usize = 256 << 10;

/// Serves `listener` until it fails.
pub fn serve(listener: TcpListener, config: Config) -> std::io::Result<()> {
    let relay = Arc::new(Relay {
        config,
        state: Mutex::default(),
        connections: AtomicUsize::new(0),
        started: Instant::now(),
    });
    let sweeping = Arc::clone(&relay);
    thread::Builder::new().name("sweep".into()).spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            sweeping.sweep(Instant::now());
        }
    })?;
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            // Out of descriptors, most likely: let some close.
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        if relay.connections.fetch_add(1, Ordering::AcqRel) >= relay.config.max_connections {
            relay.connections.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let serving = Arc::clone(&relay);
        let spawned = thread::Builder::new().stack_size(STACK).spawn(move || {
            serving.connection(stream);
            serving.connections.fetch_sub(1, Ordering::AcqRel);
        });
        if spawned.is_err() {
            relay.connections.fetch_sub(1, Ordering::AcqRel);
        }
    }
    Ok(())
}

struct Relay {
    config: Config,
    state: Mutex<State>,
    /// Connections open, joined or not.
    connections: AtomicUsize,
    started: Instant,
}

#[derive(Default)]
struct State {
    rooms: HashMap<String, Room>,
    addresses: HashMap<IpAddr, Address>,
    /// Peers that reach the relay by requests rather than a WebSocket, by session.
    polls: HashMap<String, Poll>,
}

/// A peer in a room by requests: where its messages wait for its next `GET`.
struct Poll {
    tag: String,
    slot: u32,
    outbox: Arc<Outbox>,
    /// When it last asked anything; one quiet past `IDLE_POLL` has gone.
    last: Instant,
}

struct Room {
    /// The slot of the peer that claimed a code's room, while it is there.
    owner: Option<u32>,
    next: u32,
    members: BTreeMap<u32, Member>,
    joins: Bucket,
    bytes: Bucket,
    failures: u32,
    burned: bool,
}

struct Member {
    outbox: Arc<Outbox>,
    address: IpAddr,
    /// When a peer joining a code's room joined, until its owner says it met it.
    pending: Option<Instant>,
}

/// What one address (or IPv6 /64) is doing.
struct Address {
    connections: usize,
    pending: usize,
    joins: Bucket,
    /// Wrong codes in the last minute.
    failures: VecDeque<Instant>,
    /// Lockouts in a row, each twice as long.
    strikes: u32,
    struck: Option<Instant>,
    locked: Option<Instant>,
}

enum Ask {
    /// A code's room, numbered by the relay or, coming back, as it was.
    Claim(Option<u32>),
    Room(String),
}

enum Refusal {
    NotFound,
    Gone,
    Wait(Duration),
    Full,
}

impl Relay {
    /// Answers a connection's requests one after another, as a polled session sends them,
    /// until one takes the connection over as a WebSocket or it ends.
    fn connection(&self, stream: TcpStream) {
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(HANDSHAKE));
        let _ = stream.set_write_timeout(Some(WRITE));
        let Ok(reading) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(reading);
        while let Ok(head) = ws::head(&mut reader) {
            if !self.request(&stream, &mut reader, &head) {
                return;
            }
            // Between a session's requests, a connection may idle as long as one waits.
            let _ = stream.set_read_timeout(Some(KEPT));
        }
    }

    /// Answers the request `head`: whether the connection serves another.
    fn request(&self, stream: &TcpStream, reader: &mut BufReader<TcpStream>, head: &str) -> bool {
        let address = self.address(stream, head);
        let target = head.split(' ').nth(1).unwrap_or_default();
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if let Some(id) = path.strip_prefix("/v1/poll/") {
            return self.polled(stream, reader, head, id);
        }
        let ask = match path {
            "/health" => {
                respond(stream, "200 OK", "application/json", "", &self.health());
                return false;
            }
            "/v1/claim" => Ask::Claim(
                query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("nameplate="))
                    .and_then(|number| number.parse().ok()),
            ),
            _ => match path.strip_prefix("/v1/room/").filter(|tag| valid(tag)) {
                Some(tag) => Ask::Room(tag.into()),
                None => {
                    respond(stream, "404 Not Found", "text/plain", "", "No such page\n");
                    return false;
                }
            },
        };
        if query.split('&').any(|pair| pair == "poll=1") {
            let outbox = Arc::new(Outbox::mailbox(self.config.queue));
            return match self.join(address, ask, &outbox, Instant::now()) {
                Ok((tag, slot)) => {
                    let mut id = [0; 16];
                    if getrandom::fill(&mut id).is_err() {
                        return false;
                    }
                    let id: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
                    let poll = Poll {
                        tag,
                        slot,
                        outbox,
                        last: Instant::now(),
                    };
                    self.state.lock().unwrap().polls.insert(id.clone(), poll);
                    reply(stream, "200 OK", format!("session {id}").as_bytes())
                }
                Err(refusal) => {
                    refuse(stream, refusal);
                    false
                }
            };
        }
        let upgrade = ws::header(head, "Upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
        let (true, Some(key)) = (
            upgrade && head.starts_with("GET "),
            ws::header(head, "Sec-WebSocket-Key"),
        ) else {
            respond(
                stream,
                "400 Bad Request",
                "text/plain",
                "",
                "A WebSocket, or ?poll=1\n",
            );
            return false;
        };
        let Ok(writing) = stream.try_clone() else {
            return false;
        };
        let outbox = Arc::new(Outbox::new(writing, self.config.queue));
        let (tag, slot) = match self.join(address, ask, &outbox, Instant::now()) {
            Ok(joined) => joined,
            Err(refusal) => {
                refuse(stream, refusal);
                return false;
            }
        };
        let switching = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: {}\r\n\r\n",
            ws::accept(key)
        );
        let writer = Arc::clone(&outbox);
        if (&*stream).write_all(switching.as_bytes()).is_ok()
            && thread::Builder::new()
                .stack_size(STACK)
                .spawn(move || writer.drain())
                .is_ok()
        {
            let _ = stream.set_read_timeout(Some(self.config.idle));
            let mut reader = ws::Reader::new(reader, self.config.max_message, true);
            while let Ok(message) = reader.read() {
                if !self.heard(&tag, slot, &outbox, message) {
                    break;
                }
            }
        }
        let mut state = self.state.lock().unwrap();
        depart(&mut state, &self.config, &tag, slot, Instant::now());
        false
    }

    /// A polled session's request: `GET` waits for what is to go to it, `POST` brings what
    /// it sends, each a run of WebSocket frames. Whether the connection serves another.
    fn polled(
        &self,
        stream: &TcpStream,
        reader: &mut BufReader<TcpStream>,
        head: &str,
        id: &str,
    ) -> bool {
        let length: usize = ws::header(head, "Content-Length")
            .and_then(|length| length.parse().ok())
            .unwrap_or(0);
        if length > self.config.max_message * 4 {
            return false;
        }
        let mut body = vec![0; length];
        if std::io::Read::read_exact(reader, &mut body).is_err() {
            return false;
        }
        let session = {
            let mut state = self.state.lock().unwrap();
            state.polls.get_mut(id).map(|poll| {
                poll.last = Instant::now();
                (poll.tag.clone(), poll.slot, Arc::clone(&poll.outbox))
            })
        };
        let Some((tag, slot, outbox)) = session else {
            return reply(stream, "410 Gone", b"No such session\n");
        };
        if head.starts_with("POST ") {
            let mut frames = ws::Reader::new(&body[..], self.config.max_message, true);
            while let Ok(message) = frames.read() {
                if !self.heard(&tag, slot, &outbox, message) {
                    self.end_poll(id);
                    return reply(stream, "410 Gone", b"Closed\n");
                }
            }
            return reply(stream, "200 OK", b"");
        }
        let _ = stream.set_write_timeout(Some(WRITE));
        match outbox.take(WAIT) {
            Some(bytes) => {
                if let Some(poll) = self.state.lock().unwrap().polls.get_mut(id) {
                    poll.last = Instant::now();
                }
                reply(stream, "200 OK", &bytes)
            }
            None => {
                self.end_poll(id);
                reply(stream, "410 Gone", b"Closed\n")
            }
        }
    }

    /// Ends the polled session `id`: it leaves its room.
    fn end_poll(&self, id: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(poll) = state.polls.remove(id) {
            depart(
                &mut state,
                &self.config,
                &poll.tag,
                poll.slot,
                Instant::now(),
            );
        }
    }

    /// Where a peer counts: the address it connected from, or its proxy says it did.
    fn address(&self, stream: &TcpStream, head: &str) -> IpAddr {
        let forwarded = ws::header(head, "X-Forwarded-For")
            .filter(|_| self.config.trust_forwarded)
            .and_then(|value| value.rsplit(',').next()?.trim().parse().ok());
        let ip = forwarded
            .or_else(|| stream.peer_addr().ok().map(|address| address.ip()))
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        match ip {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => IpAddr::V4(v4),
                // One subscriber is usually given a whole /64.
                None => {
                    let [a, b, c, d, ..] = v6.segments();
                    IpAddr::V6(Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
                }
            },
            v4 => v4,
        }
    }

    fn health(&self) -> String {
        let state = self.state.lock().unwrap();
        let peers: usize = state.rooms.values().map(|room| room.members.len()).sum();
        format!(
            "{{\"rooms\":{},\"peers\":{peers},\"connections\":{},\"seconds\":{}}}\n",
            state.rooms.len(),
            self.connections.load(Ordering::Acquire),
            self.started.elapsed().as_secs()
        )
    }

    /// Puts a peer from `address` in the room it asks for, telling it and those it may talk
    /// to: its room's tag and its slot.
    fn join(
        &self,
        address: IpAddr,
        ask: Ask,
        outbox: &Arc<Outbox>,
        now: Instant,
    ) -> Result<(String, u32), Refusal> {
        let config = &self.config;
        let mut state = self.state.lock().unwrap();
        let State {
            rooms, addresses, ..
        } = &mut *state;
        if !addresses.contains_key(&address) && addresses.len() >= ADDRESSES {
            return Err(Refusal::Full);
        }
        let client = addresses
            .entry(address)
            .or_insert_with(|| Address::new(config, now));
        client.refresh(now);
        if let Some(until) = client.locked {
            return Err(Refusal::Wait(until - now));
        }
        if client.connections >= config.max_connections_per_address {
            return Err(Refusal::Wait(Duration::from_secs(10)));
        }
        let per_minute = f64::from(config.joins_per_minute);
        client
            .joins
            .take(per_minute, per_minute / 60.0, now)
            .map_err(Refusal::Wait)?;
        let (tag, nameplate) = match ask {
            Ask::Claim(back) => {
                let free = |number: u32| {
                    rooms
                        .get(&code(number))
                        .is_none_or(|room| room.owner.is_none() && !room.burned)
                };
                let number = back
                    .filter(|number| NAMEPLATES.contains(number) && free(*number))
                    .or_else(|| {
                        (0..32)
                            .filter_map(|_| {
                                let mut bytes = [0; 4];
                                getrandom::fill(&mut bytes).ok()?;
                                Some(1 + u32::from_le_bytes(bytes) % NAMEPLATES.end())
                            })
                            .chain(NAMEPLATES)
                            .find(|number| free(*number))
                    })
                    .ok_or(Refusal::Full)?;
                (code(number), Some(number))
            }
            Ask::Room(tag) if tag.starts_with("code-") => {
                let room = rooms
                    .get(&tag)
                    .filter(|room| room.owner.is_some())
                    .ok_or(Refusal::NotFound)?;
                if room.burned {
                    return Err(Refusal::Gone);
                }
                if client.failures.len() + client.pending >= config.failures_per_minute as usize {
                    let wait = client.failures.front().map_or(config.pending, |first| {
                        (*first + MINUTE).saturating_duration_since(now)
                    });
                    return Err(Refusal::Wait(wait));
                }
                (tag, None)
            }
            Ask::Room(tag) => (tag, None),
        };
        if !rooms.contains_key(&tag) && rooms.len() >= config.max_rooms {
            return Err(Refusal::Full);
        }
        let room = rooms.entry(tag.clone()).or_insert_with(|| Room {
            owner: None,
            next: 1,
            members: BTreeMap::new(),
            joins: Bucket::full(f64::from(config.room_joins_per_minute), now),
            bytes: Bucket::full(config.room_bytes_per_second as f64, now),
            failures: 0,
            burned: false,
        });
        let per_minute = f64::from(config.room_joins_per_minute);
        let admitted = room.members.len() < config.max_room_peers;
        let joined = admitted.then(|| room.joins.take(per_minute, per_minute / 60.0, now));
        let refusal = match joined {
            None => Some(Refusal::Full),
            Some(Err(wait)) => Some(Refusal::Wait(wait)),
            Some(Ok(())) => None,
        };
        if let Some(refusal) = refusal {
            if room.members.is_empty() {
                rooms.remove(&tag);
            }
            return Err(refusal);
        }
        let slot = room.next;
        room.next += 1;
        let pending = room.owner.is_some() && nameplate.is_none();
        let visible: Vec<u32> = match room.owner {
            Some(owner) if pending => vec![owner],
            _ => (room.members.iter())
                .filter(|(_, member)| member.pending.is_none())
                .map(|(slot, _)| *slot)
                .collect(),
        };
        if let Some(number) = nameplate {
            room.owner = Some(slot);
            outbox.push(notice(Notice::Nameplate(number)));
        }
        outbox.push(notice(Notice::Welcome {
            you: slot,
            members: visible.clone(),
        }));
        for other in visible {
            room.members[&other]
                .outbox
                .push(notice(Notice::Joined(slot)));
        }
        room.members.insert(
            slot,
            Member {
                outbox: Arc::clone(outbox),
                address,
                pending: pending.then_some(now),
            },
        );
        client.connections += 1;
        client.pending += usize::from(pending);
        Ok((tag, slot))
    }

    /// Acts on a message from `slot`: false once it is gone or broke the protocol.
    fn heard(&self, tag: &str, slot: u32, outbox: &Outbox, message: ws::Message) -> bool {
        match message {
            ws::Message::Binary(mut data) => {
                if data.len() < SLOT {
                    return false;
                }
                let rate = self.config.room_bytes_per_second as f64;
                let wait = match self.state.lock().unwrap().rooms.get_mut(tag) {
                    Some(room) => room.bytes.spend(data.len() as f64, rate, Instant::now()),
                    None => return false,
                };
                // Waiting here slows the sender alone, as its socket fills.
                thread::sleep(wait);
                let to = u32::from_be_bytes(data[..SLOT].try_into().expect("a slot"));
                let state = self.state.lock().unwrap();
                let Some(room) = state.rooms.get(tag) else {
                    return false;
                };
                let (Some(from), Some(target)) = (room.members.get(&slot), room.members.get(&to))
                else {
                    return room.members.contains_key(&slot);
                };
                // One waiting for the code's owner talks to the owner alone.
                if (from.pending.is_none() || room.owner == Some(to))
                    && (target.pending.is_none() || room.owner == Some(slot))
                {
                    data[..SLOT].copy_from_slice(&slot.to_be_bytes());
                    target.outbox.push(ws::frame(ws::BINARY, &data, None));
                }
                true
            }
            ws::Message::Text(text) => {
                if let Ok(verdict) = text.parse() {
                    self.judge(tag, slot, verdict);
                }
                true
            }
            ws::Message::Ping(payload) => {
                outbox.push(ws::frame(ws::PONG, &payload, None));
                true
            }
            ws::Message::Pong => true,
            ws::Message::Close => false,
        }
    }

    /// Takes a code's owner's word on the peer in a slot waiting to meet it.
    fn judge(&self, tag: &str, from: u32, verdict: Verdict) {
        let mut state = self.state.lock().unwrap();
        let State {
            rooms, addresses, ..
        } = &mut *state;
        let Some(room) = rooms.get_mut(tag).filter(|room| room.owner == Some(from)) else {
            return;
        };
        match verdict {
            Verdict::Met(slot) => {
                let Some(member) = room.members.get_mut(&slot) else {
                    return;
                };
                if member.pending.take().is_none() {
                    return;
                }
                if let Some(address) = addresses.get_mut(&member.address) {
                    address.pending -= 1;
                }
                let newcomer = Arc::clone(&member.outbox);
                for (other, peer) in &room.members {
                    if *other != slot && *other != from && peer.pending.is_none() {
                        newcomer.push(notice(Notice::Joined(*other)));
                        peer.outbox.push(notice(Notice::Joined(slot)));
                    }
                }
            }
            Verdict::Failed(slot) => {
                if room
                    .members
                    .get(&slot)
                    .is_some_and(|member| member.pending.is_some())
                {
                    depart(&mut state, &self.config, tag, slot, Instant::now());
                }
            }
        }
    }

    /// Fails the peers that waited too long to meet a code's owner, and forgets quiet addresses.
    fn sweep(&self, now: Instant) {
        let config = &self.config;
        let mut state = self.state.lock().unwrap();
        let late: Vec<(String, u32)> = (state.rooms.iter())
            .flat_map(|(tag, room)| {
                (room.members.iter())
                    .filter(|(_, member)| {
                        member.pending.is_some_and(|since| {
                            now.saturating_duration_since(since) >= config.pending
                        })
                    })
                    .map(|(slot, _)| (tag.clone(), *slot))
            })
            .collect();
        for (tag, slot) in late {
            depart(&mut state, config, &tag, slot, now);
        }
        let quiet: Vec<String> = (state.polls.iter())
            .filter(|(_, poll)| now.saturating_duration_since(poll.last) >= IDLE_POLL)
            .map(|(id, _)| id.clone())
            .collect();
        for id in quiet {
            if let Some(poll) = state.polls.remove(&id) {
                depart(&mut state, config, &poll.tag, poll.slot, now);
            }
        }
        state.addresses.retain(|_, address| {
            address.refresh(now);
            !address.quiet(config, now)
        });
    }
}

/// Takes `slot` out of room `tag` and hangs up on it. One still waiting to meet a code's
/// owner tried a wrong code; the owner leaving excuses those waiting for it.
fn depart(state: &mut State, config: &Config, tag: &str, slot: u32, now: Instant) {
    let State {
        rooms, addresses, ..
    } = state;
    let Some(room) = rooms.get_mut(tag) else {
        return;
    };
    let Some(member) = room.members.remove(&slot) else {
        return;
    };
    member.outbox.close();
    if let Some(address) = addresses.get_mut(&member.address) {
        address.connections -= 1;
        if member.pending.is_some() {
            address.pending -= 1;
            if let Some(lockout) = address.fail(now, config.failures_per_minute) {
                eprintln!(
                    "{}: locked out for {} s after too many wrong codes",
                    member.address,
                    lockout.as_secs()
                );
            }
        }
    }
    for peer in room.members.values() {
        peer.outbox.push(notice(Notice::Left(slot)));
    }
    if member.pending.is_some() {
        room.failures += 1;
        if room.failures >= config.burn_after && !room.burned {
            room.burned = true;
            eprintln!("{tag}: burned after {} wrong codes", room.failures);
            if let Some(owner) = room.owner.and_then(|owner| room.members.get(&owner)) {
                owner.outbox.push(notice(Notice::Burned));
            }
        }
    }
    if room.owner == Some(slot) {
        room.owner = None;
        for member in room.members.values_mut() {
            if member.pending.take().is_some() {
                if let Some(address) = addresses.get_mut(&member.address) {
                    address.pending -= 1;
                }
                member.outbox.close();
            }
        }
    }
    if room.members.is_empty() {
        rooms.remove(tag);
    }
}

fn notice(notice: Notice) -> Vec<u8> {
    ws::frame(ws::TEXT, notice.to_string().as_bytes(), None)
}

fn code(number: u32) -> String {
    format!("code-{number}")
}

/// A room tag as clients make them: a hash in hex, or `code-` and a number.
fn valid(tag: &str) -> bool {
    (1..=64).contains(&tag.len())
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Answers a polled session's request, keeping the connection: whether that worked.
fn reply(mut stream: &TcpStream, status: &str, body: &[u8]) -> bool {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).is_ok() && stream.write_all(body).is_ok()
}

fn refuse(stream: &TcpStream, refusal: Refusal) {
    let (status, headers, body) = match refusal {
        Refusal::NotFound => ("404 Not Found", String::new(), "No such code\n"),
        Refusal::Gone => ("410 Gone", String::new(), "The code has expired\n"),
        Refusal::Wait(wait) => (
            "429 Too Many Requests",
            // Rounded up, so that a client waiting so long finds it over.
            format!(
                "Retry-After: {}\r\n",
                wait.as_secs() + u64::from(wait.subsec_nanos() > 0)
            ),
            "Too many tries\n",
        ),
        Refusal::Full => (
            "503 Service Unavailable",
            "Retry-After: 30\r\n".into(),
            "The relay is full\n",
        ),
    };
    respond(stream, status, "text/plain", &headers, body);
}

fn respond(mut stream: &TcpStream, status: &str, kind: &str, headers: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\
         {headers}\r\n{body}",
        body.len()
    );
}

impl Address {
    fn new(config: &Config, now: Instant) -> Self {
        Self {
            connections: 0,
            pending: 0,
            joins: Bucket::full(f64::from(config.joins_per_minute), now),
            failures: VecDeque::new(),
            strikes: 0,
            struck: None,
            locked: None,
        }
    }

    /// Forgets failures over a minute old, a lockout that has ended, and strikes after a
    /// quiet hour.
    fn refresh(&mut self, now: Instant) {
        while self
            .failures
            .front()
            .is_some_and(|failed| now.saturating_duration_since(*failed) >= MINUTE)
        {
            self.failures.pop_front();
        }
        self.locked = self.locked.filter(|until| *until > now);
        if self
            .struck
            .is_some_and(|struck| now.saturating_duration_since(struck) >= HOUR)
        {
            self.strikes = 0;
            self.struck = None;
        }
    }

    /// Counts a wrong code: at `limit` in a minute, the lockout it starts.
    fn fail(&mut self, now: Instant, limit: u32) -> Option<Duration> {
        self.refresh(now);
        self.failures.push_back(now);
        if self.failures.len() < limit as usize {
            return None;
        }
        self.failures.clear();
        self.strikes += 1;
        self.struck = Some(now);
        let lockout = MINUTE
            .saturating_mul(1 << (self.strikes - 1).min(6))
            .min(HOUR);
        self.locked = Some(now + lockout);
        Some(lockout)
    }

    /// Whether nothing about this address needs remembering.
    fn quiet(&self, config: &Config, now: Instant) -> bool {
        let mut joins = self.joins;
        let per_minute = f64::from(config.joins_per_minute);
        self.connections == 0
            && self.pending == 0
            && self.failures.is_empty()
            && self.locked.is_none()
            && self.strikes == 0
            && joins.take(per_minute, per_minute / 60.0, now).is_ok()
    }
}

/// A token bucket.
#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full(capacity: f64, now: Instant) -> Self {
        Self {
            tokens: capacity,
            at: now,
        }
    }

    fn refill(&mut self, capacity: f64, rate: f64, now: Instant) {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + rate * elapsed).min(capacity);
        self.at = self.at.max(now);
    }

    /// Takes one token from a bucket of `capacity` refilling at `rate` a second; else how
    /// long until one is there.
    fn take(&mut self, capacity: f64, rate: f64, now: Instant) -> Result<(), Duration> {
        self.refill(capacity, rate, now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - self.tokens) / rate))
        }
    }

    /// Spends `amount` from a bucket holding a second's worth at `rate`, into debt if need
    /// be: how long until the debt is repaid.
    fn spend(&mut self, amount: f64, rate: f64, now: Instant) -> Duration {
        self.refill(rate, rate, now);
        self.tokens -= amount;
        Duration::from_secs_f64((-self.tokens / rate).max(0.0))
    }
}

/// What waits to be written to one peer, capped in bytes.
struct Outbox {
    /// Where its frames go; none for a polled session's, which its `GET`s take.
    stream: Option<TcpStream>,
    queue: Mutex<Queue>,
    ready: Condvar,
    most: usize,
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Vec<u8>>,
    bytes: usize,
    closed: bool,
}

impl Outbox {
    fn new(stream: TcpStream, most: usize) -> Self {
        Self {
            stream: Some(stream),
            queue: Mutex::default(),
            ready: Condvar::new(),
            most,
        }
    }

    fn mailbox(most: usize) -> Self {
        Self {
            stream: None,
            queue: Mutex::default(),
            ready: Condvar::new(),
            most,
        }
    }

    /// Everything queued, waiting up to `wait` for something; none once closed.
    fn take(&self, wait: Duration) -> Option<Vec<u8>> {
        let mut queue = self.queue.lock().unwrap();
        if queue.frames.is_empty() && !queue.closed {
            queue = self.ready.wait_timeout(queue, wait).unwrap().0;
        }
        if queue.closed {
            return None;
        }
        queue.bytes = 0;
        Some(queue.frames.drain(..).flatten().collect())
    }

    /// Queues `frame`, or hangs up on a peer that reads too slowly to take it.
    fn push(&self, frame: Vec<u8>) {
        let mut queue = self.queue.lock().unwrap();
        if queue.closed {
            return;
        }
        if queue.bytes + frame.len() > self.most {
            drop(queue);
            self.close();
            return;
        }
        queue.bytes += frame.len();
        queue.frames.push_back(frame);
        self.ready.notify_one();
    }

    fn close(&self) {
        let mut queue = self.queue.lock().unwrap();
        *queue = Queue {
            closed: true,
            ..Queue::default()
        };
        self.ready.notify_one();
        drop(queue);
        if let Some(stream) = &self.stream {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    /// Writes what is queued until closed.
    fn drain(&self) {
        loop {
            let frame = {
                let mut queue = self.queue.lock().unwrap();
                loop {
                    if queue.closed {
                        return;
                    }
                    if let Some(frame) = queue.frames.pop_front() {
                        queue.bytes -= frame.len();
                        break frame;
                    }
                    queue = self.ready.wait(queue).unwrap();
                }
            };
            let Some(mut stream) = self.stream.as_ref() else {
                return;
            };
            if stream.write_all(&frame).is_err() {
                self.close();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ten wrong codes in a minute lock an address out for a minute, then two, then four,
    /// and a quiet hour forgives it.
    #[test]
    fn wrong_codes_lock_out_for_longer_each_time() {
        let config = Config::default();
        let start = Instant::now();
        let mut address = Address::new(&config, start);
        let mut now = start;
        for expected in [1, 2, 4] {
            for _ in 0..9 {
                assert_eq!(address.fail(now, 10), None);
            }
            assert_eq!(address.fail(now, 10), Some(MINUTE * expected));
            now += MINUTE * expected;
        }
        now += HOUR;
        address.refresh(now);
        assert_eq!(address.strikes, 0);
        assert!(address.quiet(&config, now));
    }

    /// Failures spread out over more than a minute never add up to a lockout.
    #[test]
    fn slow_wrong_codes_never_lock_out() {
        let start = Instant::now();
        let mut address = Address::new(&Config::default(), start);
        for minute in 0..30 {
            for second in [0, 30] {
                let now = start + MINUTE * minute + Duration::from_secs(second);
                assert_eq!(address.fail(now, 10), None);
            }
        }
    }

    #[test]
    fn buckets_refill_and_debts_are_waited_out() {
        let now = Instant::now();
        let mut joins = Bucket::full(2.0, now);
        assert!(joins.take(2.0, 1.0, now).is_ok());
        assert!(joins.take(2.0, 1.0, now).is_ok());
        assert_eq!(joins.take(2.0, 1.0, now), Err(Duration::from_secs(1)));
        assert!(joins.take(2.0, 1.0, now + Duration::from_secs(1)).is_ok());

        let mut bytes = Bucket::full(100.0, now);
        assert_eq!(bytes.spend(100.0, 100.0, now), Duration::ZERO);
        assert_eq!(bytes.spend(50.0, 100.0, now), Duration::from_millis(500));
    }

    #[test]
    fn ipv6_addresses_count_by_their_64() {
        let relay = Relay {
            config: Config {
                trust_forwarded: true,
                ..Config::default()
            },
            state: Mutex::default(),
            connections: AtomicUsize::new(0),
            started: Instant::now(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let head = "GET / HTTP/1.1\r\nX-Forwarded-For: 1.2.3.4, 2001:db8:1:2:3:4:5:6\r\n\r\n";
        assert_eq!(
            relay.address(&stream, head),
            "2001:db8:1:2::".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            relay.address(&stream, "GET / HTTP/1.1\r\n\r\n"),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }
}
