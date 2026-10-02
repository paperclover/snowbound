use super::*;
use std::time::Instant;

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
    let room = Room::Code("7-violet-otter".into());
    let ada = Live::start(hello("Ada"), &room, None, || {}).unwrap();
    let grace = Live::start(hello("Grace"), &room, None, || {}).unwrap();
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
        &Room::Code("7-violet-otter".into()),
        None,
        || {},
    )
    .unwrap();
    let mallory = Live::start(
        hello("Mallory"),
        &Room::Code("7-violet-ocelot".into()),
        None,
        || {},
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

    let room = Room::Code("4-quiet-heron".into());
    let grace = Live::start(hello("Grace"), &room, None, || {}).unwrap();
    // A later version: it greets, says something new, then where it is.
    let later = thread::spawn(move || {
        let mut stream = TcpStream::connect(grace.address()).unwrap();
        let (mut send, mut receive) =
            wire::open(&mut stream, Side::Initiator, &room.tag(), &room.secret()).unwrap();
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
    let ada = Live::start(hello("Ada"), &room, Some(Reach::Loopback), || {}).unwrap();
    let grace = Live::start(hello("Grace"), &room, Some(Reach::Loopback), || {}).unwrap();
    until(&ada, |peers| peers.len() == 1);
    until(&grace, |peers| peers.len() == 1);
}
