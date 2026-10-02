//! The WebSocket framing both ends speak (RFC 6455): whole text and binary messages, pings and
//! closes, each message capped in size.

use base64::Engine;
use sha1::{Digest, Sha1};
use std::io::{self, Read};

pub const TEXT: u8 = 1;
pub const BINARY: u8 = 2;
pub const CLOSE: u8 = 8;
pub const PING: u8 = 9;
pub const PONG: u8 = 10;
/// The longest HTTP head either end reads.
pub const HEAD: usize = 8 << 10;

#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// The `Sec-WebSocket-Accept` answering a `Sec-WebSocket-Key` of `key`.
pub fn accept(key: &str) -> String {
    let digest = Sha1::digest(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// One unfragmented frame holding `payload`, masked with `mask` as a client's must be.
pub fn frame(opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode);
    let masked = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        length @ 0..=125 => frame.push(masked | length as u8),
        length @ 126..=0xffff => {
            frame.push(masked | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(masked | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    match mask {
        Some(mask) => {
            frame.extend_from_slice(&mask);
            frame.extend(payload.iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
        }
        None => frame.extend_from_slice(payload),
    }
    frame
}

/// An HTTP head, through the blank line that ends it, read a byte at a time so that nothing
/// after it is consumed.
pub fn head(from: &mut impl Read) -> io::Result<String> {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= HEAD {
            return Err(invalid("An HTTP head too long"));
        }
        from.read_exact(&mut byte)?;
        head.push(byte[0]);
    }
    String::from_utf8(head).map_err(|_| invalid("An HTTP head that isn't UTF-8"))
}

/// The value of header `name` in `head`.
pub fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Reads one direction of a connection as whole messages, joining fragments.
pub struct Reader<R> {
    inner: R,
    /// The largest message, in bytes.
    most: usize,
    /// Whether frames arrive masked, as a server reads them.
    masked: bool,
    /// A message's opcode and its fragments so far.
    partial: Option<(u8, Vec<u8>)>,
}

impl<R: Read> Reader<R> {
    pub fn new(inner: R, most: usize, masked: bool) -> Self {
        Self {
            inner,
            most,
            masked,
            partial: None,
        }
    }

    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// The next message, or an error of kind `InvalidData` for one breaking the protocol or
    /// the cap.
    pub fn read(&mut self) -> io::Result<Message> {
        loop {
            let mut head = [0; 2];
            self.inner.read_exact(&mut head)?;
            let (last, opcode) = (head[0] & 0x80 != 0, head[0] & 0x0f);
            if head[0] & 0x70 != 0 || (head[1] & 0x80 != 0) != self.masked {
                return Err(invalid(
                    "A WebSocket frame with reserved bits or the wrong mask",
                ));
            }
            let length = match head[1] & 0x7f {
                126 => {
                    let mut length = [0; 2];
                    self.inner.read_exact(&mut length)?;
                    u64::from(u16::from_be_bytes(length))
                }
                127 => {
                    let mut length = [0; 8];
                    self.inner.read_exact(&mut length)?;
                    u64::from_be_bytes(length)
                }
                length => u64::from(length),
            };
            let control = opcode & 0x08 != 0;
            let room = match (&self.partial, control) {
                (_, true) if !last => return Err(invalid("A fragmented control frame")),
                (_, true) => 125,
                (Some((_, so_far)), false) => self.most - so_far.len(),
                (None, false) => self.most,
            };
            if length > room as u64 {
                return Err(invalid("A WebSocket message too large"));
            }
            let mut mask = [0; 4];
            if self.masked {
                self.inner.read_exact(&mut mask)?;
            }
            let mut payload = vec![0; length as usize];
            self.inner.read_exact(&mut payload)?;
            if self.masked {
                for (byte, mask) in payload.iter_mut().zip(mask.iter().cycle()) {
                    *byte ^= mask;
                }
            }
            let (opcode, payload) = match (opcode, &mut self.partial) {
                (PING, _) => return Ok(Message::Ping(payload)),
                (PONG, _) => return Ok(Message::Pong),
                (CLOSE, _) => return Ok(Message::Close),
                (TEXT | BINARY, None) if last => (opcode, payload),
                (TEXT | BINARY, None) => {
                    self.partial = Some((opcode, payload));
                    continue;
                }
                (0, Some((_, so_far))) => {
                    so_far.extend_from_slice(&payload);
                    if !last {
                        continue;
                    }
                    self.partial.take().expect("a partial message")
                }
                _ => return Err(invalid("An unexpected WebSocket frame")),
            };
            return Ok(if opcode == TEXT {
                Message::Text(
                    String::from_utf8(payload).map_err(|_| invalid("Text that isn't UTF-8"))?,
                )
            } else {
                Message::Binary(payload)
            });
        }
    }
}

pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_read_back_masked_or_not_and_fragments_join() {
        let mut bytes = frame(TEXT, b"welcome 1", Some([1, 2, 3, 4]));
        bytes.extend(frame(BINARY, &[7; 70_000], Some([9, 8, 7, 6])));
        bytes.extend(frame(PING, b"hi", Some([0; 4])));
        let mut reader = Reader::new(&bytes[..], 1 << 20, true);
        assert_eq!(reader.read().unwrap(), Message::Text("welcome 1".into()));
        assert_eq!(reader.read().unwrap(), Message::Binary(vec![7; 70_000]));
        assert_eq!(reader.read().unwrap(), Message::Ping(b"hi".to_vec()));

        // "ab" then "cd" as a fragmented binary message, a ping between them.
        let mut fragments = vec![BINARY, 2, b'a', b'b'];
        fragments.extend(frame(PING, b"", None));
        fragments.extend([0x80, 2, b'c', b'd']);
        let mut reader = Reader::new(&fragments[..], 4, false);
        assert_eq!(reader.read().unwrap(), Message::Ping(vec![]));
        assert_eq!(reader.read().unwrap(), Message::Binary(b"abcd".to_vec()));
    }

    #[test]
    fn a_message_over_the_cap_or_masked_wrongly_is_refused() {
        let big = frame(BINARY, &[0; 10], None);
        let error = Reader::new(&big[..], 9, false).read().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let unmasked = frame(BINARY, b"x", None);
        assert!(Reader::new(&unmasked[..], 9, true).read().is_err());
    }

    /// RFC 6455's own example.
    #[test]
    fn accepts_the_rfc_key() {
        assert_eq!(
            accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }
}
