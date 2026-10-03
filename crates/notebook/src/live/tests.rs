use super::*;
use std::time::Instant;

/// The code for room `number` and `secret`.
fn code(number: u32, secret: &str) -> String {
    super::code::format(number, secret).unwrap()
}

fn hello(name: &str) -> Hello {
    Hello::new(name.into(), Some(vec![1, 2, 3])).unwrap()
}

/// Waits until `done` holds of `live`'s peers.
fn until(live: &Live, done: impl Fn(&[Peer]) -> bool) -> Vec<Peer> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let peers = live.peers();
        if done(&peers) {
            return peers;
        }
        assert!(Instant::now() < deadline, "peers stayed {peers:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn caret(offset: u32) -> Presence {
    let spot = Spot {
        text: Guid {
            guid: [7; 16],
            n: 3,
        },
        offset,
    };
    Presence {
        section: Some([5; 16]),
        page: Some(Guid {
            guid: [6; 16],
            n: 1,
        }),
        caret: Some(Caret {
            anchor: spot,
            focus: spot,
        }),
    }
}

/// Two ends of one code meet, greet each other by name and picture, hear each other's newest
/// caret, and see the other leave.
#[test]
fn peers_meet_and_follow_presence() {
    let room = Room::join(&code(7, "ABCDEF"), "");
    let ada = Live::start(hello("Ada"), &room, None, None, |_| {}).unwrap();
    let grace = Live::start(hello("Grace"), &room, None, None, |_| {}).unwrap();
    ada.set_presence(caret(1));
    ada.connect(grace.address());
    let seen = until(&grace, |peers| {
        peers.iter().any(|peer| peer.presence.is_some())
    });
    assert_eq!(seen[0].hello.name, "Ada");
    assert_eq!(seen[0].hello.picture.as_deref(), Some(&[1, 2, 3][..]));
    assert_eq!(seen[0].presence, Some(caret(1)));
    until(&ada, |peers| {
        peers.len() == 1 && peers[0].hello.name == "Grace"
    });
    for offset in 2..20 {
        ada.set_presence(caret(offset));
    }
    until(&grace, |peers| peers[0].presence == Some(caret(19)));
    drop(ada);
    until(&grace, <[Peer]>::is_empty);
}

/// A peer holding another code never meets: its first frame does not open.
#[test]
fn another_code_never_meets() {
    let ada = Live::start(
        hello("Ada"),
        &Room::join(&code(7, "ABCDEF"), ""),
        None,
        None,
        |_| {},
    )
    .unwrap();
    let mallory = Live::start(
        hello("Mallory"),
        &Room::join(&code(7, "ABCDEG"), ""),
        None,
        None,
        |_| {},
    )
    .unwrap();
    mallory.connect(ada.address());
    thread::sleep(Duration::from_millis(500));
    assert!(ada.peers().is_empty() && mallory.peers().is_empty());
}

/// A field or a kind a later version adds is skipped by this one.
#[test]
fn later_fields_and_kinds_are_skipped() {
    #[derive(Encode)]
    #[cbor(map)]
    struct Later {
        #[cbor(n(0), with = "minicbor::bytes")]
        section: Option<[u8; 16]>,
        #[n(9)]
        mood: String,
    }
    use minicbor::Encode;
    let body = minicbor::to_vec(Later {
        section: Some([5; 16]),
        mood: "curious".into(),
    })
    .unwrap();
    let presence: Presence = minicbor::decode(&body).unwrap();
    assert_eq!(
        presence,
        Presence {
            section: Some([5; 16]),
            ..Presence::default()
        }
    );

    let room = Room::join(&code(4, "QJETHR"), "");
    let grace = Live::start(hello("Grace"), &room, None, None, |_| {}).unwrap();
    // A later version: it greets, says something new, then where it is.
    let later = thread::spawn(move || {
        let mut stream = TcpStream::connect(grace.address()).unwrap();
        let (mut send, mut receive) = wire::open(
            &mut stream,
            Side::Initiator,
            &room.tag().unwrap(),
            &room.secret(),
        )
        .unwrap();
        send.send(&mut stream, kind::HELLO, &hello("Later"))
            .unwrap();
        receive.receive(&mut stream).unwrap();
        send.send(&mut stream, 999, &"a chat message").unwrap();
        send.send(&mut stream, kind::PRESENCE, &caret(4)).unwrap();
        until(&grace, |peers| {
            peers
                .first()
                .is_some_and(|peer| peer.presence == Some(caret(4)))
        });
    });
    later.join().unwrap();
}

/// Two ends of one notebook find each other by mDNS on this computer's loopback.
#[test]
#[ignore = "multicasts mDNS on the loopback interface"]
fn peers_find_each_other_on_loopback() {
    let room = Room::Notebook([9; 16]);
    let ada = Live::start(hello("Ada"), &room, Some(Reach::Loopback), None, |_| {}).unwrap();
    let grace = Live::start(hello("Grace"), &room, Some(Reach::Loopback), None, |_| {}).unwrap();
    until(&ada, |peers| peers.len() == 1);
    until(&grace, |peers| peers.len() == 1);
}

/// Two sealers that met over loopback: Ada's to send with and Grace's to receive with.
fn sealers() -> (Sealer, Sealer) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let ada = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        wire::open(&mut stream, Side::Initiator, "room", b"secret")
            .unwrap()
            .0
    });
    let (mut stream, _) = listener.accept().unwrap();
    let grace = wire::open(&mut stream, Side::Responder, "room", b"secret")
        .unwrap()
        .1;
    (ada.join().unwrap(), grace)
}

/// Ada's carets at offsets `0..count`, each frame as its own block of bytes.
fn frames(ada: &mut Sealer, count: u32) -> Vec<Vec<u8>> {
    (0..count)
        .map(|offset| {
            let mut block = Vec::new();
            ada.send(&mut block, kind::PRESENCE, &caret(offset))
                .unwrap();
            block
        })
        .collect()
}

/// A frame lost, repeated, reordered, altered, forged or sealed for another meeting is caught
/// where it lands, before anything in it or after it is read.
#[test]
fn frames_out_of_place_are_caught() {
    let (mut other, _) = sealers();
    let elsewhere = frames(&mut other, 3).remove(2);
    type Edit = dyn Fn(&mut Vec<Vec<u8>>);
    let cases: [(&str, Box<Edit>, &str); 6] = [
        (
            "lost",
            Box::new(|sent| drop(sent.remove(2))),
            "Frame 3 came where frame 2 was due",
        ),
        (
            "repeated",
            Box::new(|sent| {
                let again = sent[1].clone();
                sent.insert(2, again);
            }),
            "Frame 1 came where frame 2 was due",
        ),
        (
            "reordered",
            Box::new(|sent| sent.swap(2, 3)),
            "Frame 3 came where frame 2 was due",
        ),
        (
            "altered",
            Box::new(|sent| *sent[2].last_mut().unwrap() ^= 1),
            "Frame 2 does not open",
        ),
        (
            "forged",
            Box::new(|sent| {
                // Its length and number kept, the rest made up.
                let mut forged = sent[2].clone();
                forged[12..].fill(7);
                sent.insert(2, forged);
            }),
            "Frame 2 does not open",
        ),
        (
            "from another meeting",
            Box::new(move |sent| sent.insert(2, elsewhere.clone())),
            "Frame 2 does not open",
        ),
    ];
    for (name, tamper, expected) in cases {
        let (mut ada, mut grace) = sealers();
        let mut sent = frames(&mut ada, 5);
        tamper(&mut sent);
        let stream = sent.concat();
        let mut arriving = &stream[..];
        for offset in 0..2 {
            let (message, body) = grace.receive(&mut arriving).unwrap();
            assert_eq!(message, kind::PRESENCE);
            assert_eq!(minicbor::decode::<Presence>(&body).unwrap(), caret(offset));
        }
        let error = grace.receive(&mut arriving).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{name}");
        assert!(error.to_string().starts_with(expected), "{name}: {error}");
    }
}

/// A relay on this computer, with `config`'s limits: its URL.
fn relay(config: ::relay::server::Config) -> (String, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || ::relay::server::serve(listener, config));
    (format!("ws://{address}"), address)
}

/// The status a relay at `address` answers a WebSocket to `path` with.
fn status(address: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: relay\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .unwrap();
    let head = ::relay::ws::head(&mut stream).unwrap();
    head.lines().next().unwrap().to_owned()
}

/// Two ends of one notebook that share no network meet in the relay's room, and see each
/// other leave.
#[test]
fn peers_meet_through_a_relay() {
    let (url, _) = relay(Default::default());
    let room = Room::Notebook([3; 16]);
    let ada = Live::start(hello("Ada"), &room, None, Some(&url), |_| {}).unwrap();
    ada.set_presence(caret(1));
    let grace = Live::start(hello("Grace"), &room, None, Some(&url), |_| {}).unwrap();
    until(&grace, |peers| {
        peers.len() == 1 && peers[0].presence == Some(caret(1))
    });
    until(&ada, |peers| {
        peers.len() == 1 && peers[0].hello.name == "Grace"
    });
    ada.set_presence(caret(2));
    until(&grace, |peers| peers[0].presence == Some(caret(2)));
    drop(ada);
    until(&grace, <[Peer]>::is_empty);
}

/// The end sharing a code asks the relay to number it; the other types the whole code. One
/// with the wrong words never meets, and the relay, told so by the end sharing, burns the
/// code once too many have tried.
#[test]
fn a_relay_numbers_a_code_and_burns_it_after_wrong_tries() {
    let (url, address) = relay(::relay::server::Config {
        burn_after: 2,
        ..Default::default()
    });
    let host = Live::start(
        hello("Ada"),
        &Room::share("ABCDEF", ""),
        None,
        Some(&url),
        |_| {},
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(code) = host.code() {
            break code;
        }
        assert!(Instant::now() < deadline, "no code");
        thread::sleep(Duration::from_millis(20));
    };
    let (number, secret) = super::code::parse(&code).unwrap();
    assert_eq!(secret, "ABCDEF");
    let guest = Live::start(
        hello("Grace"),
        &Room::join(&code, ""),
        None,
        Some(&url),
        |_| {},
    )
    .unwrap();
    until(&host, |peers| peers.len() == 1);
    until(&guest, |peers| peers.len() == 1);

    let path = format!("/v1/room/code-{number}");
    let wrong = Room::join(&self::code(number, "ABCDEG"), "");
    // An end that typed a wrong code gives up at once; Mallory tries twice, and the second
    // wrong try burns the code.
    for _ in 0..2 {
        let mallory = Live::start(hello("Mallory"), &wrong, None, Some(&url), |_| {}).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while mallory.failed() == 0 {
            assert!(Instant::now() < deadline, "Mallory never tried");
            thread::sleep(Duration::from_millis(20));
        }
        assert!(mallory.peers().is_empty());
    }
    until(&host, |_| host.burned());
    assert_eq!(status(address, &path), "HTTP/1.1 410 Gone");
    assert_eq!(host.peers().len(), 1, "Grace stays");
}

/// What a malicious relay does to one message on its way.
#[derive(Clone, Copy, Debug)]
enum Tamper {
    Drop,
    Repeat,
    Reorder,
    Alter,
    Inject,
}

/// A relay in the middle of Grace's connection that passes on what the real one at
/// `upstream` says, except the first message to her from a peer once `armed`, which it
/// tampers with. Her next connection waits for `release`.
struct Malicious {
    url: String,
    armed: Arc<std::sync::atomic::AtomicBool>,
    tampered: mpsc::Receiver<()>,
    rejoined: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

fn malicious(upstream: SocketAddr, tamper: Tamper) -> Malicious {
    use ::relay::ws::{self, Message};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (tampered, told) = mpsc::channel();
    let (rejoined, heard) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let arming = Arc::clone(&armed);
    thread::spawn(move || {
        for (index, client) in listener.incoming().enumerate() {
            let mut client = client.unwrap();
            if index > 0 {
                let _ = rejoined.send(());
                let _ = released.recv();
            }
            let server = TcpStream::connect(upstream).unwrap();
            let (mut up, mut to_server) =
                (client.try_clone().unwrap(), server.try_clone().unwrap());
            thread::spawn(move || {
                let _ = io::copy(&mut up, &mut to_server);
                let _ = to_server.shutdown(Shutdown::Both);
            });
            let (tampered, armed) = (tampered.clone(), Arc::clone(&arming));
            thread::spawn(move || {
                let mut reading = io::BufReader::new(server);
                let head = ws::head(&mut reading).unwrap();
                client.write_all(head.as_bytes()).unwrap();
                let mut messages = ws::Reader::new(reading, 1 << 20, false);
                let mut held = None;
                while let Ok(message) = messages.read() {
                    let frames: Vec<Vec<u8>> = match message {
                        Message::Binary(mut data) => {
                            let mut out = vec![];
                            if index == 0 && armed.swap(false, std::sync::atomic::Ordering::AcqRel)
                            {
                                match tamper {
                                    Tamper::Drop => {}
                                    Tamper::Repeat => out = vec![data.clone(), data],
                                    Tamper::Reorder => held = Some(data),
                                    Tamper::Alter => {
                                        *data.last_mut().unwrap() ^= 1;
                                        out = vec![data];
                                    }
                                    Tamper::Inject => {
                                        // A slot and part of the sender's id, then
                                        // made-up bytes.
                                        let mut forged = data.clone();
                                        forged[16..].fill(7);
                                        out = vec![forged, data];
                                    }
                                }
                                let _ = tampered.send(());
                            } else {
                                out.push(data);
                                out.extend(held.take());
                            }
                            out.into_iter()
                                .map(|data| ws::frame(ws::BINARY, &data, None))
                                .collect()
                        }
                        Message::Text(text) => vec![ws::frame(ws::TEXT, text.as_bytes(), None)],
                        Message::Ping(payload) => vec![ws::frame(ws::PING, &payload, None)],
                        Message::Pong => vec![ws::frame(ws::PONG, &[], None)],
                        Message::Close => break,
                    };
                    if frames.iter().any(|frame| client.write_all(frame).is_err()) {
                        break;
                    }
                }
                let _ = client.shutdown(Shutdown::Both);
            });
        }
    });
    Malicious {
        url,
        armed,
        tampered: told,
        rejoined: heard,
        release,
    }
}

/// Starts `name`, recording each presence of its peer as applied, and `None` when the peer
/// goes.
fn recording(name: &str, room: &Room, relay: &str) -> (Live, Arc<Mutex<Vec<Option<Presence>>>>) {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let shared: Arc<std::sync::OnceLock<std::sync::Weak<Shared>>> = Arc::default();
    let (recorded, watched) = (Arc::clone(&heard), Arc::clone(&shared));
    let live = Live::start(hello(name), room, None, Some(relay), move |event| {
        if !matches!(event, Event::Changed) {
            return;
        }
        let Some(shared) = watched.get().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        let entry = match shared.peers().first() {
            None => Some(None),
            Some(peer) => peer.presence.clone().map(Some),
        };
        recorded.lock().unwrap().extend(entry);
    })
    .unwrap();
    shared.set(Arc::downgrade(&live.shared)).ok().unwrap();
    (live, heard)
}

/// Whatever a malicious relay does to a frame, the end it was for never applies it or
/// anything after it, drops the connection as broken, joins again and hears the newest
/// presence from scratch.
#[test]
fn a_malicious_relay_is_caught() {
    let room = Room::Notebook([4; 16]);
    for tamper in [
        Tamper::Drop,
        Tamper::Repeat,
        Tamper::Reorder,
        Tamper::Alter,
        Tamper::Inject,
    ] {
        let (url, address) = relay(Default::default());
        let ada = Live::start(hello("Ada"), &room, None, Some(&url), |_| {}).unwrap();
        ada.set_presence(caret(1));
        let relay = malicious(address, tamper);
        let (grace, heard) = recording("Grace", &room, &relay.url);
        until(&grace, |peers| {
            peers
                .first()
                .is_some_and(|peer| peer.presence == Some(caret(1)))
        });
        // Ada's next presence goes to everyone, so its loss is caught too.
        relay
            .armed
            .store(true, std::sync::atomic::Ordering::Release);
        ada.set_presence(caret(2));
        relay
            .tampered
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        ada.set_presence(caret(3));
        relay
            .rejoined
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        let before: Vec<_> = heard.lock().unwrap().clone();
        let gone = before.iter().position(Option::is_none).unwrap();
        assert!(
            !before[..gone].contains(&Some(caret(3))),
            "{tamper:?}: applied after the tampering: {before:?}"
        );
        assert!(grace.peers().is_empty(), "{tamper:?}");
        relay.release.send(()).unwrap();
        until(&grace, |peers| {
            peers
                .first()
                .is_some_and(|peer| peer.presence == Some(caret(3)))
        });
    }
}

/// Ends of two versions each learn the other's version from the opening, before any key is
/// agreed, so each can say which should update.
#[test]
fn another_version_is_named_before_any_key() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let older = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        #[derive(minicbor::Encode)]
        #[cbor(map)]
        struct Open {
            #[n(0)]
            version: u16,
            #[n(1)]
            room: String,
            #[cbor(n(2), with = "minicbor::bytes")]
            pake: Vec<u8>,
        }
        let open = minicbor::to_vec(Open {
            version: 1,
            room: "room".into(),
            pake: vec![0; 33],
        })
        .unwrap();
        stream
            .write_all(&[&(open.len() as u32).to_be_bytes()[..], &open].concat())
            .unwrap();
        let mut length = [0; 4];
        stream.read_exact(&mut length).unwrap();
        let mut answer = vec![0; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut answer).unwrap();
        answer
    });
    let (mut stream, _) = listener.accept().unwrap();
    let Err(error) = wire::open(&mut stream, Side::Responder, "room", b"secret") else {
        panic!("met a peer of another version");
    };
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    let version = error.get_ref().unwrap().downcast_ref::<wire::Version>();
    assert_eq!(version, Some(&wire::Version(1)));
    // The older end heard this one's opening, and with it its version.
    assert!(!older.join().unwrap().is_empty());
}
