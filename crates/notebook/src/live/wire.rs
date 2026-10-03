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
pub const VERSION: u16 = 2;

/// The version of the opening a peer of another version sent, as `open` fails with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version(pub u16);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "The peer speaks Live Share version {}, this end {VERSION}",
            self.0
        )
    }
}

impl std::error::Error for Version {}
/// The largest block either side reads.
const MOST: usize = 16 << 20;

/// Message kinds.
pub mod kind {
    pub const HELLO: u16 = 1;
    /// Keeps a quiet connection open; it says nothing else.
    pub const PING: u16 = 2;
    pub const BYE: u16 = 3;
    /// A `Hello` in answer to one heard in a notebook's room's group.
    pub const HELLO_BACK: u16 = 4;
    pub const PRESENCE: u16 = 16;
    /// A host's files changed: `Touched`.
    pub const TOUCHED: u16 = 18;
    /// A section's bytes as a commit changed them: `Delta`.
    pub const DELTA: u16 = 19;
    pub const WELCOME: u16 = 32;
    /// Storage requests to a host, each a `Request` answered by a `Reply`.
    pub const LIST: u16 = 257;
    pub const STAMP: u16 = 258;
    /// A section's consistent image, a chunk at a time.
    pub const READ: u16 = 259;
    pub const COMMIT: u16 = 260;
    pub const CONFIRM: u16 = 261;
    pub const CREATE: u16 = 262;
    pub const CREATE_DIRECTORY: u16 = 263;
    pub const HIDE: u16 = 264;
    pub const RENAME: u16 = 265;
    pub const REPLACE: u16 = 266;
    pub const DELETE: u16 = 267;
    pub const PLACE: u16 = 268;
    pub const SUPERSEDE: u16 = 269;
    /// Any other file as it stands, a chunk at a time.
    pub const READ_FILE: u16 = 270;
    pub const EXISTS: u16 = 271;
    /// A chunk of the bytes a later request carries.
    pub const PUT: u16 = 272;
    pub const REPLY: u16 = 511;
}

/// The kinds this version reads, as `Hello::kinds` lists them.
pub const KNOWN: &[u16] = &[
    kind::HELLO,
    kind::PING,
    kind::BYE,
    kind::HELLO_BACK,
    kind::PRESENCE,
    kind::TOUCHED,
    kind::DELTA,
    kind::WELCOME,
    kind::LIST,
    kind::STAMP,
    kind::READ,
    kind::COMMIT,
    kind::CONFIRM,
    kind::CREATE,
    kind::CREATE_DIRECTORY,
    kind::HIDE,
    kind::RENAME,
    kind::REPLACE,
    kind::DELETE,
    kind::PLACE,
    kind::SUPERSEDE,
    kind::READ_FILE,
    kind::EXISTS,
    kind::PUT,
    kind::REPLY,
];

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
    /// The share this peer hosts, whose storage requests it answers.
    #[cbor(n(5), with = "minicbor::bytes")]
    pub serves: Option<[u8; 16]>,
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
            serves: None,
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

/// Why a peer leaves: `left`, or `stopped` for a host that stopped sharing.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Bye {
    #[n(0)]
    pub reason: String,
}

/// What the host of a share gives a peer that knew its code: the share's room, and names to
/// show it by.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Welcome {
    #[cbor(n(0), with = "minicbor::bytes")]
    pub share: [u8; 16],
    #[cbor(n(1), with = "minicbor::bytes")]
    pub secret: [u8; 16],
    #[n(2)]
    pub notebook: String,
    /// The host's name for itself, as `Hello::name`.
    #[n(3)]
    pub host: String,
}

/// Paths a host's files changed at, by catalog path; `""` is the notebook's folder.
#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Touched {
    #[n(0)]
    pub paths: Vec<String>,
    /// Each path's stamp now, as `share::digest` hashes it, where it has one: a guest holding
    /// that image knows the stamp without asking.
    #[n(1)]
    pub stamps: Vec<Option<u64>>,
}

/// What a commit changed in a section a host serves: from the image with stamp `base`, the
/// image `length` long with `writes` in place. A guest holding the base needs read nothing.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Delta {
    #[n(0)]
    pub path: String,
    #[n(1)]
    pub base: WireStamp,
    #[n(2)]
    pub length: u64,
    #[n(3)]
    pub writes: Vec<Written>,
}

/// Bytes at an offset.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Written {
    #[n(0)]
    pub offset: u64,
    #[cbor(n(1), with = "minicbor::bytes")]
    pub bytes: Vec<u8>,
}

/// A storage request; its kind names the verb, and the verb what it carries.
#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Request {
    /// Names the reply; a `PUT`'s names the bytes a later request carries.
    #[n(0)]
    pub id: u64,
    /// A catalog path.
    #[n(1)]
    pub path: String,
    /// A rename's or replacement's target, or the file a supersession puts in place.
    #[n(2)]
    pub to: Option<String>,
    #[n(3)]
    pub offset: Option<u64>,
    #[n(4)]
    pub limit: Option<u64>,
    #[cbor(n(5), with = "minicbor::bytes")]
    pub bytes: Option<Vec<u8>>,
    /// A confirmation's or supersession's base; for a section's read, the image the guest
    /// holds, which the reply may give the changes to.
    #[n(6)]
    pub stamp: Option<WireStamp>,
    #[cbor(n(7), with = "minicbor::bytes")]
    pub ancestor: Option<[u8; 16]>,
    #[n(8)]
    pub name: Option<String>,
    /// The `PUT`s, by id, whose bytes this request carries in place of `bytes`; a read's
    /// snapshot after its first chunk.
    #[n(9)]
    pub handle: Option<u64>,
}

/// A request's answer: a failure, or what the verb gives.
#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Reply {
    #[n(0)]
    pub id: u64,
    #[n(1)]
    pub failure: Option<Failure>,
    #[cbor(n(2), with = "minicbor::bytes")]
    pub bytes: Option<Vec<u8>>,
    /// A read's whole length.
    #[n(3)]
    pub length: Option<u64>,
    #[n(4)]
    pub stamp: Option<WireStamp>,
    #[n(5)]
    pub entries: Option<Vec<WireEntry>>,
    #[n(6)]
    pub exists: Option<bool>,
    /// The snapshot a read's later chunks come from.
    #[n(7)]
    pub handle: Option<u64>,
    /// A read's changes to the image with the request's stamp, in place of its bytes.
    #[n(8)]
    pub writes: Option<Vec<Written>>,
}

/// Why a request failed: an `io::ErrorKind` as `error_kind` numbers it, and for a commit,
/// how far it got (`commit_state`).
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct Failure {
    #[n(0)]
    pub kind: u16,
    #[n(1)]
    pub message: String,
    #[n(2)]
    pub state: Option<u8>,
}

/// An `onestore::Stamp`.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct WireStamp {
    #[cbor(n(0), with = "minicbor::bytes")]
    pub header: Vec<u8>,
    #[n(1)]
    pub length: u64,
}

impl From<&onestore::Stamp> for WireStamp {
    fn from(stamp: &onestore::Stamp) -> Self {
        Self {
            header: stamp.header.to_vec(),
            length: stamp.length,
        }
    }
}

impl TryFrom<&WireStamp> for onestore::Stamp {
    type Error = io::Error;

    fn try_from(stamp: &WireStamp) -> io::Result<Self> {
        Ok(Self {
            header: stamp
                .header
                .as_slice()
                .try_into()
                .map_err(|_| invalid("A stamp's header is 1024 bytes"))?,
            length: stamp.length,
        })
    }
}

/// A folder's entry, as `discover::Entry`.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[cbor(map)]
pub struct WireEntry {
    #[n(0)]
    pub name: String,
    /// 0 a file, 1 a folder, 2 anything else, 3 a file kept elsewhere.
    #[n(1)]
    pub kind: u8,
    #[n(2)]
    pub size: u64,
    #[n(3)]
    pub modified: u64,
}

/// The `io::ErrorKind`s a failure names, by number; any other is `Other`.
const ERROR_KINDS: [io::ErrorKind; 20] = [
    io::ErrorKind::Other,
    io::ErrorKind::NotFound,
    io::ErrorKind::PermissionDenied,
    io::ErrorKind::AlreadyExists,
    io::ErrorKind::InvalidInput,
    io::ErrorKind::InvalidData,
    io::ErrorKind::TimedOut,
    io::ErrorKind::WouldBlock,
    io::ErrorKind::ResourceBusy,
    io::ErrorKind::Unsupported,
    io::ErrorKind::FileTooLarge,
    io::ErrorKind::NotConnected,
    io::ErrorKind::ReadOnlyFilesystem,
    io::ErrorKind::DirectoryNotEmpty,
    io::ErrorKind::NotADirectory,
    io::ErrorKind::IsADirectory,
    io::ErrorKind::StorageFull,
    io::ErrorKind::UnexpectedEof,
    io::ErrorKind::Interrupted,
    io::ErrorKind::BrokenPipe,
];

pub fn error_number(kind: io::ErrorKind) -> u16 {
    ERROR_KINDS
        .iter()
        .position(|known| *known == kind)
        .unwrap_or(0) as u16
}

pub fn error_kind(number: u16) -> io::ErrorKind {
    ERROR_KINDS
        .get(usize::from(number))
        .copied()
        .unwrap_or(io::ErrorKind::Other)
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

/// One direction's AEAD key and the count of frames sealed under it. Each frame carries its
/// number, which is also its nonce, so a frame lost, repeated, reordered or forged on the way
/// is caught before anything in it or after it is read.
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

    /// Writes message `kind` holding `body`.
    pub fn send(
        &mut self,
        to: &mut impl Write,
        kind: u16,
        body: &impl Encode<()>,
    ) -> io::Result<()> {
        let body = minicbor::to_vec(body).map_err(io::Error::other)?;
        self.send_encoded(to, kind, &body)
    }

    /// `send` for a body already encoded.
    pub fn send_encoded(&mut self, to: &mut impl Write, kind: u16, body: &[u8]) -> io::Result<()> {
        let clear = [&kind.to_be_bytes()[..], body].concat();
        let number = self.count.to_be_bytes();
        let sealed = self
            .cipher
            .encrypt(&nonce(number).into(), clear.as_slice())
            .map_err(|_| io::Error::other("A frame could not be sealed"))?;
        self.count += 1;
        write_block(to, &[&number[..], &sealed].concat())
    }

    /// Reads the next message: its kind and body. An error of kind `InvalidData` means the
    /// stream broke, and nothing more on it can be trusted.
    pub fn receive(&mut self, from: &mut impl Read) -> io::Result<(u16, Vec<u8>)> {
        let block = read_block(from)?;
        let (number, sealed) = block
            .split_first_chunk::<8>()
            .ok_or_else(|| invalid("A frame without its number"))?;
        let due = self.count;
        if u64::from_be_bytes(*number) != due {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Frame {} came where frame {due} was due",
                    u64::from_be_bytes(*number)
                ),
            ));
        }
        let mut clear = self
            .cipher
            .decrypt(&nonce(*number).into(), sealed)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Frame {due} does not open under the agreed key"),
                )
            })?;
        self.count += 1;
        let body = clear.split_off(2.min(clear.len()));
        let kind = u16::from_be_bytes(clear.try_into().map_err(|_| invalid("An empty frame"))?);
        Ok((kind, body))
    }
}

fn nonce(number: [u8; 8]) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[4..].copy_from_slice(&number);
    nonce
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
    // A responder answers even a peer of another version, so that both ends can say which
    // should update.
    if side == Side::Responder {
        write_block(stream, &minicbor::to_vec(&ours).map_err(io::Error::other)?)?;
    }
    if theirs.version != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            Version(theirs.version),
        ));
    }
    if theirs.room != room {
        return Err(invalid("The peer means another room"));
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
