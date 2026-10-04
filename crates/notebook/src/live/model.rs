use super::{Hello, Line, Presence, code};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

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
    pub(super) fn tag(&self) -> Option<String> {
        match self {
            Room::Notebook(id) => Some(hex(&Sha256::digest(
                [&b"Snowbound room "[..], id].concat(),
            )[..8])),
            Room::Code { code, .. } => code_parts(code).0.map(|number| format!("code-{number}")),
        }
    }

    pub(super) fn secret(&self) -> Vec<u8> {
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

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn owner(&self) -> bool {
        matches!(self, Room::Code { owner: true, .. })
    }
}

/// A code's room number, if it has one, and its secret: a whole code, or a secret alone.
pub(super) fn code_parts(code: &str) -> (Option<u32>, String) {
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
    /// A stream to a peer opened, with the line to it.
    Met(&'a Arc<Hello>, &'a Line),
    /// A peer's stream ended.
    Left(&'a Arc<Hello>),
    /// A frame of a kind presence doesn't read itself, on a stream or to the group.
    Frame {
        from: &'a Arc<Hello>,
        kind: u16,
        body: &'a [u8],
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

/// Why a relay could not be reached, as a person can act on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// The relay's name, or the proxy's, didn't resolve.
    Dns,
    /// Nothing answered at the relay's address: a firewall, or the relay is down.
    Unreachable,
    TimedOut,
    /// The proxy itself couldn't be reached.
    ProxyUnreachable,
    /// The proxy asks for a name and password (407).
    ProxyAuthentication,
    /// The proxy refused to connect to the relay, with this status.
    ProxyRefused(u16),
    /// The relay's certificate wasn't one the system trusts: something on the way presents its
    /// own.
    Certificate,
    /// Something on the way refused both WebSockets and plain requests to the relay.
    Blocked,
    Other,
}
