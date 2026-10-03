//! Snowbound's Live Share relay, and what its clients share with it.
//!
//! A peer opens a WebSocket to `/v1/room/<tag>`, or to `/v1/claim` for a code's room, whose
//! number the relay chooses. Every peer in a room has a slot, a number the relay gives it
//! that is never reused in that room. A binary message is a slot (`u32`, big-endian) and
//! bytes: sent, the slot it goes to; received, the slot it came from. What the bytes are
//! the relay never knows; peers seal them end to end. Text messages are [`Notice`]s from the
//! relay and [`Verdict`]s to it.
//!
//! Of each pair of peers the one with the higher slot opens the stream between them.
//! A peer joining a code's room waits, hearing only the room's owner, until the owner says
//! its opening `met`; a peer that `failed`, left first, or stayed silent too long counts a
//! wrong code against its address and the code. See `resources/live-share.md`.

pub mod code;
pub mod server;
pub mod site;
pub mod ws;

use std::{fmt, str::FromStr};

/// The bytes before a binary message's payload: the slot it goes to or came from.
pub const SLOT: usize = 4;

/// What the relay tells a peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// The number of the code whose room this peer claimed; sent before `Welcome`.
    Nameplate(u32),
    /// This peer's slot, and the slots of those already in the room it may talk to.
    Welcome {
        you: u32,
        members: Vec<u32>,
    },
    /// Someone this peer may now talk to.
    Joined(u32),
    Left(u32),
    /// The code had too many wrong tries and admits no one new; sent to its owner.
    Burned,
}

/// What a code's owner tells the relay of the peer in a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The peer knew the code.
    Met(u32),
    /// The peer's opening failed: a wrong code.
    Failed(u32),
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Notice::Nameplate(number) => write!(f, "nameplate {number}"),
            Notice::Welcome { you, members } => {
                write!(f, "welcome {you}")?;
                members.iter().try_for_each(|slot| write!(f, " {slot}"))
            }
            Notice::Joined(slot) => write!(f, "joined {slot}"),
            Notice::Left(slot) => write!(f, "left {slot}"),
            Notice::Burned => f.write_str("burned"),
        }
    }
}

impl FromStr for Notice {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let mut words = text.split(' ');
        let name = words.next().ok_or(())?;
        let numbers: Vec<u32> = words
            .map(|word| word.parse().map_err(|_| ()))
            .collect::<Result<_, _>>()?;
        Ok(match (name, &numbers[..]) {
            ("nameplate", [number]) => Notice::Nameplate(*number),
            ("welcome", [you, members @ ..]) => Notice::Welcome {
                you: *you,
                members: members.to_vec(),
            },
            ("joined", [slot]) => Notice::Joined(*slot),
            ("left", [slot]) => Notice::Left(*slot),
            ("burned", []) => Notice::Burned,
            _ => return Err(()),
        })
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Verdict::Met(slot) => write!(f, "met {slot}"),
            Verdict::Failed(slot) => write!(f, "failed {slot}"),
        }
    }
}

impl FromStr for Verdict {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let (name, slot) = text.split_once(' ').ok_or(())?;
        let slot = slot.parse().map_err(|_| ())?;
        match name {
            "met" => Ok(Verdict::Met(slot)),
            "failed" => Ok(Verdict::Failed(slot)),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_and_verdicts_read_back() {
        for notice in [
            Notice::Nameplate(412),
            Notice::Welcome {
                you: 3,
                members: vec![1, 2],
            },
            Notice::Welcome {
                you: 1,
                members: vec![],
            },
            Notice::Joined(4),
            Notice::Left(4),
            Notice::Burned,
        ] {
            assert_eq!(notice.to_string().parse(), Ok(notice));
        }
        for verdict in [Verdict::Met(2), Verdict::Failed(9)] {
            assert_eq!(verdict.to_string().parse(), Ok(verdict));
        }
        assert_eq!("shrug 1".parse::<Notice>(), Err(()));
    }
}
