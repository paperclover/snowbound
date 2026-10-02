//! What peers say to each other: an opening in the clear that meets through the secret both
//! hold (SPAKE2), then frames sealed under the keys it agreed, each a message kind and a CBOR
//! map. A later version adds kinds and fields; a reader skips the kinds and fields it doesn't
//! know, so every version speaks to every other.

use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use hmac::{Hmac, Mac};
use minicbor::{Decode, Encode};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::io::{self, Read, Write};

/// The opening's version. Frames after it never change shape; they grow by kinds and fields.
pub const VERSION: u16 = 1;
/// The largest block either side reads.
const MOST: usize = 16 << 20;

/// Message kinds.
pub mod kind {
    pub const HELLO: u16 = 1;
    /// Keeps a quiet connection open; it says nothing else.
    pub const PING: u16 = 2;
    pub const PRESENCE: u16 = 16;
}

/// The kinds this version reads, as `Hello::kinds` lists them.
pub const KNOWN: &[u16] = &[kind::HELLO, kind::PING, kind::PRESENCE];

/// The first message each way, and the only one with a name and a picture.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Hello {
    /// Random for each run of the app.
    #[cbor(n(0), with = "minicbor::bytes")]
    pub peer: [u8; 16],
    #[n(1)]
    pub name: String,
    /// A PNG of the person, at most 96 pixels a side.
    #[cbor(n(2), with = "minicbor::bytes")]
    pub picture: Option<Vec<u8>>,
    /// The app and its version, for diagnostics.
    #[n(3)]
    pub app: String,
    /// The message kinds the sender reads; a request waits for its kind to be listed.
    #[n(4)]
    pub kinds: Vec<u16>,
}

impl Hello {
    /// A hello from a new peer, named `name`, reading the kinds this version reads.
    pub fn new(name: String, picture: Option<Vec<u8>>) -> io::Result<Self> {
        let mut peer = [0; 16];
        getrandom::fill(&mut peer).map_err(|_| io::Error::other("System random source failed"))?;
        Ok(Self {
            peer,
            name,
            picture,
            app: format!("Snowbound {}", env!("CARGO_PKG_VERSION")),
            kinds: KNOWN.to_vec(),
        })
    }
}

/// Where someone is: the section and page they have open and their caret on it.
#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Presence {
    /// The section file's identity (its header's guidFile).
    #[cbor(n(0), with = "minicbor::bytes")]
    pub section: Option<[u8; 16]>,
    /// The page's object space.
    #[n(1)]
    pub page: Option<Guid>,
    #[n(2)]
    pub caret: Option<Caret>,
}

/// A selection, collapsed where `anchor` is `focus`.
#[derive(Clone, Copy, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Caret {
    #[n(0)]
    pub anchor: Spot,
    #[n(1)]
    pub focus: Spot,
}

/// A place in a text object, in UTF-16 code units, as ops address text.
#[derive(Clone, Copy, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Spot {
    #[n(0)]
    pub text: Guid,
    #[n(1)]
    pub offset: u32,
}

/// An `ExGuid`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(array)]
pub struct Guid {
    #[cbor(n(0), with = "minicbor::bytes")]
    pub guid: [u8; 16],
    #[n(1)]
    pub n: u32,
}

impl From<onestore::ExGuid> for Guid {
    fn from(id: onestore::ExGuid) -> Self {
        Self {
            guid: id.guid,
            n: id.n,
        }
    }
}

impl From<Guid> for onestore::ExGuid {
    fn from(id: Guid) -> Self {
        Self {
            guid: id.guid,
            n: id.n,
        }
    }
}

/// The opening, sent in the clear by the side that connected and answered by the other.
#[derive(Encode, Decode)]
#[cbor(map)]
struct Open {
    #[n(0)]
    version: u16,
    /// The room it means, as discovery names it.
    #[n(1)]
    room: String,
    #[cbor(n(2), with = "minicbor::bytes")]
    pake: Vec<u8>,
}

/// One direction's AEAD key and the count of frames sealed under it, which is each frame's nonce.
pub struct Sealer {
    cipher: Aes256Gcm,
    count: u64,
}

impl Sealer {
    fn new(key: &[u8], purpose: &[u8]) -> Self {
        let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key).expect("any key length");
        mac.update(purpose);
        Self {
            cipher: Aes256Gcm::new_from_slice(&mac.finalize().into_bytes()).expect("a 32-byte key"),
            count: 0,
        }
    }

    fn nonce(&mut self) -> [u8; 12] {
        let mut nonce = [0; 12];
        nonce[4..].copy_from_slice(&self.count.to_be_bytes());
        self.count += 1;
        nonce
    }

    /// Writes message `kind` holding `body`.
    pub fn send(
        &mut self,
        to: &mut impl Write,
        kind: u16,
        body: &impl Encode<()>,
    ) -> io::Result<()> {
        let mut clear = kind.to_be_bytes().to_vec();
        minicbor::encode(body, &mut clear).map_err(io::Error::other)?;
        let nonce = self.nonce();
        let sealed = self
            .cipher
            .encrypt(&nonce.into(), clear.as_slice())
            .map_err(|_| io::Error::other("A frame could not be sealed"))?;
        write_block(to, &sealed)
    }

    /// Reads the next message: its kind and body.
    pub fn receive(&mut self, from: &mut impl Read) -> io::Result<(u16, Vec<u8>)> {
        let sealed = read_block(from)?;
        let nonce = self.nonce();
        let mut clear = self
            .cipher
            .decrypt(&nonce.into(), sealed.as_slice())
            .map_err(|_| invalid("A frame does not open under the agreed key"))?;
        let body = clear.split_off(2.min(clear.len()));
        let kind = u16::from_be_bytes(clear.try_into().map_err(|_| invalid("An empty frame"))?);
        Ok((kind, body))
    }
}

/// Which end of the connection this is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Initiator,
    Responder,
}

/// Meets the peer at the other end of `stream` in `room` through `secret`: the sealers to
/// send and to receive with. A peer holding another secret goes unnoticed here; its first
/// frame then fails to open.
pub fn open(
    stream: &mut (impl Read + Write),
    side: Side,
    room: &str,
    secret: &[u8],
) -> io::Result<(Sealer, Sealer)> {
    let password = Password::new(secret);
    let [initiator, responder] = [b"initiator", b"responder"]
        .map(|role| Identity::new(&[&role[..], room.as_bytes()].concat()));
    let (pake, message) = match side {
        Side::Initiator => Spake2::<Ed25519Group>::start_a(&password, &initiator, &responder),
        Side::Responder => Spake2::<Ed25519Group>::start_b(&password, &initiator, &responder),
    };
    let ours = Open {
        version: VERSION,
        room: room.into(),
        pake: message,
    };
    if side == Side::Initiator {
        write_block(stream, &minicbor::to_vec(&ours).map_err(io::Error::other)?)?;
    }
    let theirs: Open =
        minicbor::decode(&read_block(stream)?).map_err(|_| invalid("A malformed opening"))?;
    if theirs.version != VERSION || theirs.room != room {
        return Err(invalid("The peer means another room or version"));
    }
    if side == Side::Responder {
        write_block(stream, &minicbor::to_vec(&ours).map_err(io::Error::other)?)?;
    }
    let key = pake
        .finish(&theirs.pake)
        .map_err(|_| invalid("A malformed key exchange"))?;
    let [from_initiator, from_responder] = [
        Sealer::new(&key, b"Snowbound live v1 initiator"),
        Sealer::new(&key, b"Snowbound live v1 responder"),
    ];
    Ok(match side {
        Side::Initiator => (from_initiator, from_responder),
        Side::Responder => (from_responder, from_initiator),
    })
}

fn write_block(to: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let length = u32::try_from(bytes.len())
        .ok()
        .filter(|length| *length as usize <= MOST)
        .ok_or_else(|| invalid("A frame too large to send"))?;
    to.write_all(&[&length.to_be_bytes()[..], bytes].concat())
}

fn read_block(from: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0; 4];
    from.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MOST {
        return Err(invalid("A frame too large to read"));
    }
    let mut bytes = vec![0; length];
    from.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
