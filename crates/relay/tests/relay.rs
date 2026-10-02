//! The relay as shipped, run as its own process and spoken to as a WebSocket.

use relay::ws::{self, Message};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

/// A relay process, killed when dropped.
struct Relay {
    child: Child,
    address: SocketAddr,
}

impl Relay {
    fn start(options: &[&str]) -> Relay {
        let mut child = Command::new(env!("CARGO_BIN_EXE_snowbound-relay"))
            .args(["--listen", "127.0.0.1:0"])
            .args(options)
            .env_clear()
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line.trim().rsplit(' ').next().unwrap().parse().unwrap();
        Relay { child, address }
    }

    fn get(&self, path: &str) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: relay\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn join(&self, path: &str) -> Result<Peer, String> {
        self.join_from(path, None)
    }

    /// Joins as if from `address`, as a proxy would say.
    fn join_from(&self, path: &str, address: Option<&str>) -> Result<Peer, String> {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let forwarded = address
            .map(|address| format!("X-Forwarded-For: {address}\r\n"))
            .unwrap_or_default();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: relay\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{forwarded}\r\n"
        )
        .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let head = ws::head(&mut reader).map_err(|error| error.to_string())?;
        if !head.starts_with("HTTP/1.1 101") {
            return Err(head);
        }
        assert!(head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
        Ok(Peer {
            stream,
            reader: ws::Reader::new(reader, 1 << 20, false),
        })
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Peer {
    stream: TcpStream,
    reader: ws::Reader<BufReader<TcpStream>>,
}

impl Peer {
    fn send(&mut self, opcode: u8, payload: &[u8]) {
        self.stream
            .write_all(&ws::frame(opcode, payload, Some([1, 2, 3, 4])))
            .unwrap();
    }

    fn say(&mut self, text: &str) {
        self.send(ws::TEXT, text.as_bytes());
    }

    fn send_to(&mut self, slot: u32, bytes: &[u8]) {
        self.send(ws::BINARY, &[&slot.to_be_bytes()[..], bytes].concat());
    }

    fn hear(&mut self) -> Message {
        self.reader.read().unwrap()
    }

    fn text(&mut self) -> String {
        match self.hear() {
            Message::Text(text) => text,
            other => panic!("heard {other:?}"),
        }
    }

    /// Whether the relay has hung up, waiting at most `patience`.
    fn closed(&mut self, patience: Duration) -> bool {
        self.stream.set_read_timeout(Some(patience)).unwrap();
        loop {
            match self.reader.read() {
                Ok(_) => continue,
                Err(error) => {
                    return !matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    );
                }
            }
        }
    }
}

fn status(refusal: Result<Peer, String>) -> String {
    match refusal {
        Ok(_) => panic!("joined"),
        Err(head) => head.lines().next().unwrap().to_owned(),
    }
}

/// The number of the code a claim was given.
fn claim(relay: &Relay) -> (Peer, u32) {
    let mut owner = relay.join("/v1/claim").unwrap();
    let number = owner
        .text()
        .strip_prefix("nameplate ")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(owner.text(), "welcome 1");
    (owner, number)
}

#[test]
fn health_answers() {
    let relay = Relay::start(&[]);
    let response = relay.get("/health");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"rooms\":0"), "{response}");
    assert!(relay.get("/nothing").starts_with("HTTP/1.1 404"));
    assert!(relay.get("/v1/room/abc").starts_with("HTTP/1.1 400"));
}

/// Peers in a room hear who comes and goes and pass each other messages, which arrive
/// labelled with the sender's slot.
#[test]
fn peers_in_a_room_pass_messages() {
    let relay = Relay::start(&[]);
    let mut ada = relay.join("/v1/room/0123abcd").unwrap();
    assert_eq!(ada.text(), "welcome 1");
    let mut grace = relay.join("/v1/room/0123abcd").unwrap();
    assert_eq!(grace.text(), "welcome 2 1");
    assert_eq!(ada.text(), "joined 2");
    grace.send_to(1, b"hello");
    assert_eq!(
        ada.hear(),
        Message::Binary([&2u32.to_be_bytes()[..], b"hello"].concat())
    );
    ada.send_to(2, b"hi");
    assert_eq!(
        grace.hear(),
        Message::Binary([&1u32.to_be_bytes()[..], b"hi"].concat())
    );
    ada.send(ws::PING, b"p");
    assert_eq!(ada.hear(), Message::Pong);
    let health = relay.get("/health");
    assert!(health.contains("\"rooms\":1,\"peers\":2"), "{health}");
    drop(grace);
    assert_eq!(ada.text(), "left 2");
}

/// One joining a code's room hears only its owner until the owner says it met it; then the
/// others in the room, and they it.
#[test]
fn a_code_admits_whom_its_owner_met() {
    let relay = Relay::start(&[]);
    let (mut owner, number) = claim(&relay);
    let path = format!("/v1/room/code-{number}");
    let mut ada = relay.join(&path).unwrap();
    assert_eq!(ada.text(), "welcome 2 1");
    assert_eq!(owner.text(), "joined 2");
    owner.say("met 2");
    let mut grace = relay.join(&path).unwrap();
    assert_eq!(grace.text(), "welcome 3 1");
    assert_eq!(owner.text(), "joined 3");
    // Ada can't reach Grace before the owner meets her.
    ada.send_to(3, b"psst");
    grace.send_to(1, b"hello");
    assert_eq!(
        owner.hear(),
        Message::Binary([&3u32.to_be_bytes()[..], b"hello"].concat())
    );
    owner.say("met 3");
    assert_eq!(grace.text(), "joined 2");
    assert_eq!(ada.text(), "joined 3");
    ada.send_to(3, b"hi");
    assert_eq!(
        grace.hear(),
        Message::Binary([&2u32.to_be_bytes()[..], b"hi"].concat())
    );
    // Only the owner's word counts.
    let mut mallory = relay.join(&path).unwrap();
    assert_eq!(mallory.text(), "welcome 4 1");
    ada.say("met 4");
    grace.send_to(4, b"anyone?");
    owner.say("failed 4");
    assert!(mallory.closed(Duration::from_secs(5)));
    assert_eq!(owner.text(), "joined 4");
    assert_eq!(owner.text(), "left 4");
}

#[test]
fn an_unknown_code_or_room_is_refused() {
    let relay = Relay::start(&[]);
    assert_eq!(
        status(relay.join("/v1/room/code-5")),
        "HTTP/1.1 404 Not Found"
    );
    assert_eq!(
        status(relay.join("/v1/room/UPPER")),
        "HTTP/1.1 404 Not Found"
    );
    assert_eq!(status(relay.join("/v1/room/")), "HTTP/1.1 404 Not Found");
}

/// Five wrong codes burn a code: it admits no one new, and its owner hears so.
#[test]
fn wrong_codes_burn_a_code() {
    let relay = Relay::start(&["--failures-per-minute", "100"]);
    let (mut owner, number) = claim(&relay);
    let path = format!("/v1/room/code-{number}");
    for slot in 2..7 {
        let mut guess = relay.join(&path).unwrap();
        assert_eq!(guess.text(), format!("welcome {slot} 1"));
        assert_eq!(owner.text(), format!("joined {slot}"));
        owner.say(&format!("failed {slot}"));
        assert!(guess.closed(Duration::from_secs(5)));
        assert_eq!(owner.text(), format!("left {slot}"));
    }
    assert_eq!(owner.text(), "burned");
    assert_eq!(status(relay.join(&path)), "HTTP/1.1 410 Gone");
}

/// An owner coming back asks for its code's number again, and gets it while no one else
/// has claimed it.
#[test]
fn an_owner_comes_back_to_its_number() {
    let relay = Relay::start(&[]);
    let (owner, number) = claim(&relay);
    let mut ada = relay.join("/v1/room/0123abcd").unwrap();
    assert_eq!(ada.text(), "welcome 1");
    let mut taken = relay
        .join(&format!("/v1/claim?nameplate={number}"))
        .unwrap();
    assert_ne!(taken.text(), format!("nameplate {number}"));
    drop(owner);
    thread::sleep(Duration::from_millis(200));
    let mut again = relay
        .join(&format!("/v1/claim?nameplate={number}"))
        .unwrap();
    assert_eq!(again.text(), format!("nameplate {number}"));
}

/// Silence, or leaving before the owner says it met, counts as a wrong code; too many from
/// an address in a minute lock it out, while another address is still let in.
#[test]
fn an_address_trying_too_many_codes_is_locked_out() {
    let relay = Relay::start(&[
        "--trust-forwarded",
        "true",
        "--failures-per-minute",
        "3",
        "--pending",
        "1",
        "--burn-after",
        "100",
    ]);
    let (mut owner, number) = claim(&relay);
    let path = format!("/v1/room/code-{number}");
    let mallory = Some("203.0.113.9");
    // Silent: the relay gives up on it.
    let mut silent = relay.join_from(&path, mallory).unwrap();
    assert!(silent.closed(Duration::from_secs(5)));
    // Gone before the owner's word.
    drop(relay.join_from(&path, mallory).unwrap());
    // Refused by the owner.
    let mut refused = relay.join_from(&path, mallory).unwrap();
    assert_eq!(refused.text(), "welcome 4 1");
    owner.say("failed 4");
    assert!(refused.closed(Duration::from_secs(5)));
    let locked = status(relay.join_from(&path, mallory));
    assert_eq!(locked, "HTTP/1.1 429 Too Many Requests");
    let Err(again) = relay.join_from(&path, mallory) else {
        panic!("let in while locked out");
    };
    assert!(
        again.contains("Retry-After: 60"),
        "a minute's lockout: {again}"
    );
    // Its /64 neighbour is the same subscriber; another address isn't.
    let mut ada = relay.join_from(&path, Some("198.51.100.1")).unwrap();
    assert_eq!(ada.text(), "welcome 5 1");
}

/// At most three waiting at once from an address allowed three wrong codes a minute: it
/// can't run many guesses side by side.
#[test]
fn guesses_waiting_at_once_count_against_the_limit() {
    let relay = Relay::start(&["--failures-per-minute", "3"]);
    let (_owner, number) = claim(&relay);
    let path = format!("/v1/room/code-{number}");
    let _waiting: Vec<Peer> = (0..3).map(|_| relay.join(&path).unwrap()).collect();
    assert_eq!(status(relay.join(&path)), "HTTP/1.1 429 Too Many Requests");
}

#[test]
fn joins_rooms_and_messages_are_capped() {
    let relay = Relay::start(&[
        "--max-room-peers",
        "2",
        "--joins-per-minute",
        "5",
        "--max-message",
        "1000",
    ]);
    let mut ada = relay.join("/v1/room/aaaa").unwrap();
    let _grace = relay.join("/v1/room/aaaa").unwrap();
    assert_eq!(
        status(relay.join("/v1/room/aaaa")),
        "HTTP/1.1 503 Service Unavailable"
    );
    let _bbbb = relay.join("/v1/room/bbbb").unwrap();
    let _fifth = relay.join("/v1/room/cccc").unwrap();
    assert_eq!(
        status(relay.join("/v1/room/dddd")),
        "HTTP/1.1 429 Too Many Requests"
    );
    assert_eq!(ada.text(), "welcome 1");
    ada.send_to(2, &[0; 2000]);
    assert!(ada.closed(Duration::from_secs(5)));
}

#[test]
fn a_silent_connection_is_closed() {
    let relay = Relay::start(&["--idle", "1"]);
    let mut quiet = relay.join("/v1/room/bbbb").unwrap();
    assert_eq!(quiet.text(), "welcome 1");
    assert!(quiet.closed(Duration::from_secs(5)));
}
