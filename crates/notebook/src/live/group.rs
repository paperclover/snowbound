//! Frames a peer in a notebook's room sends once through the relay, which copies them to
//! everyone or to the peers named (`relay::GROUP`): presence, hellos, and a host's news of
//! its files. Each is sealed under a key of its sender's own, derived from the room's secret
//! and an id the sender picks for each connection, so the relay sees only ciphertext.
//!
//! A frame carries its number, which is its nonce, and the count of broadcasts its sender has
//! sent: a frame numbered no later than the last, or a broadcast missing before it, means the
//! relay lost, repeated, reordered or forged one, and this end meets the room again. A frame
//! to some peers goes unseen by the others, so only what can tell itself stale is sent so.
//! The room's members hold one key, so a frame proves its sender is in the room, not which
//! member it is; members are trusted alike, as each may change the notebook itself.

use super::{Hello, Peer};
use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::io;

/// A frame's sender id, number, broadcasts, and whether it is a broadcast.
const HEADER: usize = 16 + 8 + 8 + 1;

/// The room's group key.
pub(super) struct Keys([u8; 32]);

impl Keys {
    pub(super) fn new(secret: &[u8]) -> Self {
        Self(mac(secret, b"Snowbound live v2 group"))
    }

    fn cipher(&self, sender: &[u8; 16]) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&mac(&self.0, sender)).expect("a 32-byte key")
    }
}

fn mac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key).expect("any key length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// Seals this end's frames for one connection to the relay.
pub(super) struct Sealer {
    id: [u8; 16],
    cipher: Aes256Gcm,
    number: u64,
    broadcasts: u64,
}

impl Sealer {
    pub(super) fn new(keys: &Keys) -> io::Result<Self> {
        let mut id = [0; 16];
        getrandom::fill(&mut id).map_err(|_| io::Error::other("System random source failed"))?;
        Ok(Self {
            cipher: keys.cipher(&id),
            id,
            number: 0,
            broadcasts: 0,
        })
    }

    /// Message `kind` holding `body`, to everyone where `broadcast`.
    pub(super) fn seal(&mut self, kind: u16, body: &[u8], broadcast: bool) -> io::Result<Vec<u8>> {
        self.number += 1;
        self.broadcasts += u64::from(broadcast);
        let mut header = Vec::with_capacity(HEADER);
        header.extend_from_slice(&self.id);
        header.extend_from_slice(&self.number.to_be_bytes());
        header.extend_from_slice(&self.broadcasts.to_be_bytes());
        header.push(u8::from(broadcast));
        let clear = [&kind.to_be_bytes()[..], body].concat();
        let sealed = self
            .cipher
            .encrypt(
                &nonce(self.number).into(),
                Payload {
                    msg: &clear,
                    aad: &header,
                },
            )
            .map_err(|_| io::Error::other("A frame could not be sealed"))?;
        Ok([header, sealed].concat())
    }
}

fn nonce(number: u64) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[4..].copy_from_slice(&number.to_be_bytes());
    nonce
}

/// A peer in the room as its frames say: the id its frames are sealed under, how far they
/// have come, and the peer once its hello has.
pub(super) struct Member {
    id: [u8; 16],
    cipher: Aes256Gcm,
    number: u64,
    broadcasts: u64,
    pub(super) peer: Option<Peer>,
}

impl Member {
    /// The member whose first frame is `frame`.
    pub(super) fn new(keys: &Keys, frame: &[u8]) -> io::Result<Self> {
        let id: [u8; 16] = frame
            .get(..16)
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| broken("A group frame without its sender"))?;
        Ok(Self {
            cipher: keys.cipher(&id),
            id,
            number: 0,
            broadcasts: 0,
            peer: None,
        })
    }

    /// The kind and body of `frame`, the next from this member; an error of kind
    /// `InvalidData` means the relay tampered with its frames.
    pub(super) fn open(&mut self, frame: &[u8]) -> io::Result<(u16, Vec<u8>)> {
        let (header, sealed) = frame
            .split_at_checked(HEADER)
            .ok_or_else(|| broken("A group frame cut short"))?;
        let field = |at: usize| u64::from_be_bytes(header[at..at + 8].try_into().unwrap());
        let (number, broadcasts, broadcast) = (field(16), field(24), header[32] == 1);
        if header[..16] != self.id {
            return Err(broken("A group frame from another sender in its slot"));
        }
        let mut clear = self
            .cipher
            .decrypt(
                &nonce(number).into(),
                Payload {
                    msg: sealed,
                    aad: header,
                },
            )
            .map_err(|_| broken("A group frame that does not open"))?;
        // The first frame heard from a member is where its count starts.
        let first = self.number == 0;
        let due = self.broadcasts + u64::from(broadcast);
        if !first && (number <= self.number || broadcasts != due) {
            return Err(broken("A group frame lost, repeated or out of order"));
        }
        (self.number, self.broadcasts) = (number, broadcasts);
        let body = clear.split_off(2.min(clear.len()));
        let kind = u16::from_be_bytes(clear.try_into().map_err(|_| broken("An empty frame"))?);
        Ok((kind, body))
    }

    pub(super) fn hello(&self) -> Option<&std::sync::Arc<Hello>> {
        self.peer.as_ref().map(|peer| &peer.hello)
    }
}

fn broken(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_frames_out_of_place_are_caught() {
        let keys = Keys::new(b"room secret");
        let mut ada = Sealer::new(&keys).unwrap();
        let frames: Vec<_> = (0..4)
            .map(|n| ada.seal(16, &[n], n != 1).unwrap())
            .collect();
        let member = || Member::new(&keys, &frames[0]).unwrap();

        let mut grace = member();
        for (n, frame) in frames.iter().enumerate() {
            assert_eq!(grace.open(frame).unwrap(), (16, vec![n as u8]));
        }
        // A frame to others alone goes unseen without a break.
        let mut unaddressed = member();
        unaddressed.open(&frames[0]).unwrap();
        assert_eq!(unaddressed.open(&frames[2]).unwrap(), (16, vec![2]));
        // A repeat, a frame reordered, a lost broadcast, an altered frame, another key.
        let mut repeated = member();
        repeated.open(&frames[0]).unwrap();
        assert!(repeated.open(&frames[0]).is_err());
        let mut reordered = member();
        reordered.open(&frames[1]).unwrap();
        assert!(reordered.open(&frames[0]).is_err());
        let mut lost = member();
        lost.open(&frames[0]).unwrap();
        assert!(lost.open(&frames[3]).is_err());
        let mut altered = frames[3].clone();
        *altered.last_mut().unwrap() ^= 1;
        assert!(member().open(&altered).is_err());
        let stranger = Keys::new(b"another secret");
        assert!(
            Member::new(&stranger, &frames[0])
                .unwrap()
                .open(&frames[0])
                .is_err()
        );
    }
}
