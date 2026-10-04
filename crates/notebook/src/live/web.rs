//! Browser Live Share: event-driven relay connections carrying the same sealed streams.

pub use ::relay::code;
#[path = "group.rs"]
mod group;
#[path = "model.rs"]
mod model;
#[path = "share.rs"]
pub mod share;
#[path = "wire.rs"]
pub mod wire;
use model::hex;
pub use model::{Event, Peer, Reach, Relayed, Room, Trouble};
pub use wire::{Caret, Guid, Hello, Presence, Spot};

use minicbor::Encode;
use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use wasm_bindgen::prelude::*;
use wire::{Side, kind};

#[wasm_bindgen(module = "/src/live/web.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = liveConnect)]
    fn connect(url: &str, message: &JsValue, closed: &JsValue) -> Result<u32, JsValue>;
    #[wasm_bindgen(catch, js_name = liveSend)]
    fn send(id: u32, bytes: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = liveClose)]
    fn close(id: u32);
}

pub struct Live(Arc<Shared>);

struct Shared {
    me: Hello,
    room: Room,
    url: String,
    events: Box<dyn Fn(Event) + Send + Sync>,
    stopped: AtomicBool,
    state: Mutex<State>,
}

struct State {
    socket: u32,
    slot: u32,
    relayed: Relayed,
    failed: u32,
    outdated: Option<u16>,
    burned: bool,
    streams: BTreeMap<u32, Stream>,
    members: BTreeMap<u32, group::Member>,
    sealer: Option<group::Sealer>,
    presence: Presence,
    scheduled: bool,
}

struct Stream {
    bytes: Vec<u8>,
    opening: Option<wire::Opening>,
    receive: Option<wire::Sealer>,
    line: Line,
    peer: Option<Peer>,
}

#[derive(Clone)]
pub struct Line {
    shared: Weak<Shared>,
    slot: u32,
    sealer: Arc<Mutex<Option<wire::Sealer>>>,
}

impl Line {
    pub fn send(&self, kind: u16, body: &impl Encode<()>) -> io::Result<()> {
        let shared = self.shared.upgrade().ok_or(io::ErrorKind::NotConnected)?;
        let mut bytes = Vec::new();
        self.sealer
            .lock()
            .unwrap()
            .as_mut()
            .ok_or(io::ErrorKind::NotConnected)?
            .send(&mut bytes, kind, body)?;
        shared.stream(self.slot, &bytes)
    }

    pub fn hang_up(&self, reason: &str) {
        let _ = self.send(
            kind::BYE,
            &wire::Bye {
                reason: reason.into(),
            },
        );
    }
}

#[derive(Clone)]
pub struct Sender(Weak<Shared>);

impl Sender {
    pub fn send(&self, kind: u16, body: &impl Encode<()>, to: Option<&[[u8; 16]]>) {
        if let Some(shared) = self.0.upgrade() {
            match to {
                None => {
                    let _ = shared.group(kind, body, None);
                }
                Some(to) => {
                    let slots: Vec<_> = shared
                        .state
                        .lock()
                        .unwrap()
                        .members
                        .iter()
                        .filter(|(_, m)| {
                            m.peer.as_ref().is_some_and(|p| to.contains(&p.hello.peer))
                        })
                        .map(|(slot, _)| *slot)
                        .collect();
                    for slot in slots {
                        let _ = shared.group(kind, body, Some(slot));
                    }
                }
            }
        }
    }
}

impl Live {
    pub fn start(
        me: Hello,
        room: &Room,
        _: Option<Reach>,
        relay: Option<&str>,
        events: impl Fn(Event) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let tag = room.tag().ok_or(io::ErrorKind::InvalidInput)?;
        let relay = relay
            .ok_or(io::ErrorKind::NotConnected)?
            .trim_end_matches('/');
        if !relay.starts_with("wss://") && !relay.starts_with("ws://") {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let shared = Arc::new(Shared {
            me,
            room: room.clone(),
            url: format!("{relay}/v1/room/{tag}"),
            events: Box::new(events),
            stopped: AtomicBool::new(false),
            state: Mutex::new(State {
                socket: 0,
                slot: 0,
                relayed: Relayed::Unknown,
                failed: 0,
                outdated: None,
                burned: false,
                streams: BTreeMap::new(),
                members: BTreeMap::new(),
                sealer: None,
                presence: Presence::default(),
                scheduled: false,
            }),
        });
        shared.connect()?;
        let weak = Arc::downgrade(&shared);
        crate::task::spawn("live ping", move || async move {
            let (_, wait) = crate::task::channel();
            loop {
                crate::task::wait(&wait, Some(Duration::from_secs(15))).await;
                let Some(shared) = weak
                    .upgrade()
                    .filter(|s| !s.stopped.load(Ordering::Acquire))
                else {
                    break;
                };
                let lines: Vec<_> = shared
                    .state
                    .lock()
                    .unwrap()
                    .streams
                    .values()
                    .map(|s| s.line.clone())
                    .collect();
                for line in lines {
                    let _ = line.send(kind::PING, &());
                }
                if matches!(shared.room, Room::Notebook(_)) {
                    let _ = shared.group(kind::PING, &(), None);
                }
            }
        })?;
        Ok(Self(shared))
    }

    pub fn code(&self) -> Option<String> {
        match &self.0.room {
            Room::Code { code, .. } => Some(code.clone()),
            _ => None,
        }
    }
    pub fn relayed(&self) -> Relayed {
        self.0.state.lock().unwrap().relayed.clone()
    }
    pub fn failed(&self) -> u32 {
        self.0.state.lock().unwrap().failed
    }
    pub fn other_version(&self) -> Option<u16> {
        self.0.state.lock().unwrap().outdated
    }
    pub fn burned(&self) -> bool {
        self.0.state.lock().unwrap().burned
    }
    pub fn sender(&self) -> Sender {
        Sender(Arc::downgrade(&self.0))
    }
    pub fn peers(&self) -> Vec<Peer> {
        let state = self.0.state.lock().unwrap();
        let mut peers = BTreeMap::new();
        for peer in state
            .members
            .values()
            .filter_map(|m| m.peer.as_ref())
            .chain(state.streams.values().filter_map(|s| s.peer.as_ref()))
        {
            peers.insert(peer.hello.peer, peer.clone());
        }
        peers.into_values().collect()
    }
    pub fn line(&self, peer: &[u8; 16]) -> Option<Line> {
        self.0
            .state
            .lock()
            .unwrap()
            .streams
            .values()
            .find(|s| s.peer.as_ref().is_some_and(|p| &p.hello.peer == peer))
            .map(|s| s.line.clone())
    }
    pub fn set_presence(&self, presence: Presence) {
        let mut state = self.0.state.lock().unwrap();
        if state.presence == presence {
            return;
        }
        state.presence = presence;
        if std::mem::replace(&mut state.scheduled, true) {
            return;
        }
        drop(state);
        let weak = Arc::downgrade(&self.0);
        let _ = crate::task::spawn("live presence", move || async move {
            let (_, wait) = crate::task::channel();
            crate::task::wait(&wait, Some(Duration::from_millis(100))).await;
            if let Some(shared) = weak.upgrade() {
                let presence = {
                    let mut state = shared.state.lock().unwrap();
                    state.scheduled = false;
                    state.presence.clone()
                };
                let _ = shared.group(kind::PRESENCE, &presence, None);
            }
        });
    }
    pub fn leave(self, reason: &str) {
        let lines: Vec<_> = self
            .0
            .state
            .lock()
            .unwrap()
            .streams
            .values()
            .map(|s| s.line.clone())
            .collect();
        for line in lines {
            line.hang_up(reason);
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.stopped.store(true, Ordering::Release);
        close(self.0.state.lock().unwrap().socket);
    }
}

impl Shared {
    fn connect(self: &Arc<Self>) -> io::Result<()> {
        let weak = Arc::downgrade(self);
        let message = Closure::<dyn FnMut(JsValue)>::new(move |data: JsValue| {
            if let Some(shared) = weak.upgrade()
                && let Err(error) = shared.heard(data)
            {
                if let Some(version) = error
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<wire::Version>())
                {
                    shared.state.lock().unwrap().outdated = Some(version.0);
                } else if error.kind() == io::ErrorKind::InvalidData {
                    shared.state.lock().unwrap().failed += 1;
                }
                shared.disconnected();
            }
        })
        .into_js_value();
        let weak = Arc::downgrade(self);
        let closed = Closure::<dyn FnMut()>::new(move || {
            if let Some(shared) = weak.upgrade() {
                shared.disconnected();
            }
        })
        .into_js_value();
        let socket = connect(&self.url, &message, &closed).map_err(js_error)?;
        self.state.lock().unwrap().socket = socket;
        Ok(())
    }

    fn disconnected(self: &Arc<Self>) {
        let peers = {
            let mut state = self.state.lock().unwrap();
            close(state.socket);
            let peers: Vec<_> = state
                .streams
                .values()
                .filter_map(|s| s.peer.as_ref().map(|p| p.hello.clone()))
                .collect();
            state.streams.clear();
            state.members.clear();
            state.sealer = None;
            state.relayed = Relayed::Unreachable(Trouble::Other);
            peers
        };
        for hello in peers {
            (self.events)(Event::Left(&hello));
        }
        (self.events)(Event::Changed);
        let weak = Arc::downgrade(self);
        let _ = crate::task::spawn("live reconnect", move || async move {
            let (_, wait) = crate::task::channel();
            crate::task::wait(&wait, Some(Duration::from_secs(2))).await;
            if let Some(shared) = weak
                .upgrade()
                .filter(|s| !s.stopped.load(Ordering::Acquire))
            {
                let _ = shared.connect();
            }
        });
    }

    fn stream(&self, slot: u32, bytes: &[u8]) -> io::Result<()> {
        let socket = self.state.lock().unwrap().socket;
        for chunk in bytes.chunks(64 << 10) {
            send(socket, &[&slot.to_be_bytes()[..], chunk].concat()).map_err(js_error)?;
        }
        Ok(())
    }

    fn group(&self, kind: u16, body: &impl Encode<()>, to: Option<u32>) -> io::Result<()> {
        let body = minicbor::to_vec(body).map_err(io::Error::other)?;
        let (socket, sealed) = {
            let mut state = self.state.lock().unwrap();
            let sealed = state
                .sealer
                .as_mut()
                .ok_or(io::ErrorKind::NotConnected)?
                .seal(kind, &body, to.is_none())?;
            (state.socket, sealed)
        };
        let mut bytes = match to {
            Some(slot) => [&(::relay::GROUP | 1).to_be_bytes()[..], &slot.to_be_bytes()].concat(),
            None => ::relay::BROADCAST.to_be_bytes().to_vec(),
        };
        bytes.extend(sealed);
        send(socket, &bytes).map_err(js_error)
    }

    fn meet(self: &Arc<Self>, slot: u32) -> io::Result<()> {
        if self.state.lock().unwrap().streams.contains_key(&slot) {
            return Ok(());
        }
        let tag = self.room.tag().ok_or(io::ErrorKind::InvalidInput)?;
        let (opening, bytes) = wire::Opening::new(Side::Initiator, &tag, &self.room.secret())?;
        let line = Line {
            shared: Arc::downgrade(self),
            slot,
            sealer: Arc::new(Mutex::new(None)),
        };
        self.state.lock().unwrap().streams.insert(
            slot,
            Stream {
                bytes: Vec::new(),
                opening: Some(opening),
                receive: None,
                line,
                peer: None,
            },
        );
        self.stream(
            slot,
            &[&(bytes.len() as u32).to_be_bytes()[..], &bytes].concat(),
        )
    }

    fn heard(self: &Arc<Self>, data: JsValue) -> io::Result<()> {
        if let Some(text) = data.as_string() {
            let notice = text
                .parse::<::relay::Notice>()
                .map_err(|_| io::ErrorKind::InvalidData)?;
            match notice {
                ::relay::Notice::Welcome { you, members } => {
                    {
                        let mut state = self.state.lock().unwrap();
                        state.slot = you;
                        state.relayed = Relayed::Joined;
                        if matches!(self.room, Room::Notebook(_)) {
                            state.sealer =
                                Some(group::Sealer::new(&group::Keys::new(&self.room.secret()))?);
                        }
                    }
                    if matches!(self.room, Room::Code { .. }) {
                        for slot in members.into_iter().filter(|slot| *slot < you) {
                            self.meet(slot)?;
                        }
                    } else {
                        self.group(kind::HELLO, &self.me, None)?;
                        let presence = self.state.lock().unwrap().presence.clone();
                        self.group(kind::PRESENCE, &presence, None)?;
                    }
                }
                ::relay::Notice::Left(slot) => {
                    let hello = {
                        let mut state = self.state.lock().unwrap();
                        state.members.remove(&slot);
                        state
                            .streams
                            .remove(&slot)
                            .and_then(|s| s.peer.map(|p| p.hello))
                    };
                    if let Some(hello) = hello {
                        (self.events)(Event::Left(&hello));
                    }
                }
                ::relay::Notice::Burned => self.state.lock().unwrap().burned = true,
                _ => {}
            }
            (self.events)(Event::Changed);
            return Ok(());
        }
        let bytes = js_sys::Uint8Array::new(&data).to_vec();
        let (slot, bytes) = bytes
            .split_first_chunk::<4>()
            .ok_or(io::ErrorKind::InvalidData)?;
        let slot = u32::from_be_bytes(*slot);
        if slot & ::relay::GROUP != 0 {
            return self.heard_group(slot & !::relay::GROUP, bytes);
        }
        self.heard_stream(slot, bytes)
    }

    fn heard_group(self: &Arc<Self>, slot: u32, frame: &[u8]) -> io::Result<()> {
        let (kind, body, hello) = {
            let mut state = self.state.lock().unwrap();
            let member = match state.members.entry(slot) {
                std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::btree_map::Entry::Vacant(e) => e.insert(group::Member::new(
                    &group::Keys::new(&self.room.secret()),
                    frame,
                )?),
            };
            let (kind, body) = member.open(frame)?;
            (kind, body, member.hello().cloned())
        };
        match kind {
            kind::HELLO | kind::HELLO_BACK => {
                let hello: Hello =
                    minicbor::decode(&body).map_err(|_| io::ErrorKind::InvalidData)?;
                if hello.peer == self.me.peer {
                    return Ok(());
                }
                let serves = hello.serves.is_some() && self.me.serves.is_none();
                self.state
                    .lock()
                    .unwrap()
                    .members
                    .get_mut(&slot)
                    .unwrap()
                    .peer = Some(Peer {
                    hello: Arc::new(hello),
                    presence: None,
                });
                if kind == kind::HELLO {
                    self.group(kind::HELLO_BACK, &self.me, Some(slot))?;
                    let presence = self.state.lock().unwrap().presence.clone();
                    self.group(kind::PRESENCE, &presence, Some(slot))?;
                }
                if serves {
                    self.meet(slot)?;
                }
                (self.events)(Event::Changed);
            }
            kind::PRESENCE => {
                let presence = minicbor::decode(&body).map_err(|_| io::ErrorKind::InvalidData)?;
                if let Some(peer) = self
                    .state
                    .lock()
                    .unwrap()
                    .members
                    .get_mut(&slot)
                    .and_then(|m| m.peer.as_mut())
                {
                    peer.presence = Some(presence);
                }
                (self.events)(Event::Changed);
            }
            kind if kind < 256 => {
                if let Some(hello) = hello {
                    (self.events)(Event::Frame {
                        from: &hello,
                        kind,
                        body: &body,
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn heard_stream(self: &Arc<Self>, slot: u32, bytes: &[u8]) -> io::Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            let stream = state
                .streams
                .get_mut(&slot)
                .ok_or(io::ErrorKind::InvalidData)?;
            if stream.bytes.len() + bytes.len() > (16 << 20) + 4 {
                return Err(io::ErrorKind::InvalidData.into());
            }
            stream.bytes.extend_from_slice(bytes);
        }
        loop {
            let event = {
                let mut state = self.state.lock().unwrap();
                let stream = state.streams.get_mut(&slot).unwrap();
                let Some(length) = stream.bytes.get(..4) else {
                    break;
                };
                let length = u32::from_be_bytes(length.try_into().unwrap()) as usize;
                if length > 16 << 20 {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                if stream.bytes.len() < length + 4 {
                    break;
                }
                let bytes: Vec<_> = stream.bytes.drain(..length + 4).collect();
                if let Some(opening) = stream.opening.take() {
                    let (send, receive) = opening.finish(&bytes[4..])?;
                    *stream.line.sealer.lock().unwrap() = Some(send);
                    stream.receive = Some(receive);
                    Some((stream.line.clone(), None, kind::HELLO, Vec::new()))
                } else {
                    let (kind, body) = stream
                        .receive
                        .as_mut()
                        .ok_or(io::ErrorKind::InvalidData)?
                        .receive(&mut io::Cursor::new(bytes))?;
                    if stream.peer.is_none() {
                        if kind != kind::HELLO {
                            return Err(io::ErrorKind::InvalidData.into());
                        }
                        let hello = minicbor::decode::<Hello>(&body)
                            .map_err(|_| io::ErrorKind::InvalidData)?;
                        stream.peer = Some(Peer {
                            hello: Arc::new(hello),
                            presence: None,
                        });
                    }
                    Some((
                        stream.line.clone(),
                        stream.peer.as_ref().map(|p| p.hello.clone()),
                        kind,
                        body,
                    ))
                }
            };
            if let Some((line, hello, kind, body)) = event {
                match hello {
                    None => line.send(kind::HELLO, &self.me)?,
                    Some(hello) if kind == kind::HELLO => (self.events)(Event::Met(&hello, &line)),
                    Some(hello) => (self.events)(Event::Frame {
                        from: &hello,
                        kind,
                        body: &body,
                    }),
                }
            }
        }
        Ok(())
    }
}

fn js_error(error: JsValue) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotConnected,
        error
            .as_string()
            .unwrap_or_else(|| "Relay disconnected".into()),
    )
}
