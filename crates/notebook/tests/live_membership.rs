#![cfg(feature = "live")]

#[path = "support/live.rs"]
mod live;
use live::*;
use notebook::live::share::{self, Guest, Host, Sharing};
use notebook::session::Notebook;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[test]
fn approval_keeps_credentials_before_welcome_and_a_decline_grants_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let saved = Arc::new(Mutex::new(None));
    let fail = Arc::new(AtomicBool::new(false));
    let (kept, failing) = (Arc::clone(&saved), Arc::clone(&fail));
    let mut sharing = Sharing::new("").unwrap();
    sharing.approve = true;
    let host = Host::start(
        Notebook::open(&folder, directory.path().join("host"))
            .unwrap()
            .into_storage(),
        hello("Ada"),
        sharing,
        "Garden",
        None,
        Some(&url),
        || {},
        move |sharing| {
            if failing.load(Ordering::Acquire) {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            *kept.lock().unwrap() = Some(serde_json::to_vec(sharing).unwrap());
            Ok(())
        },
    )
    .unwrap();
    let code = self::code(&host);
    let (reply, result) = mpsc::channel();
    let joining = url.clone();
    std::thread::spawn(move || {
        let _ = reply.send(share::join(hello("Grace"), &code, "", None, Some(&joining)));
    });
    until("the request never arrived", || host.requests().len() == 1);
    assert!(result.try_recv().is_err());
    assert!(host.devices().is_empty());
    let peer = host.requests()[0].peer;
    fail.store(true, Ordering::Release);
    assert!(host.allow(&peer).is_err());
    assert!(host.devices().is_empty());
    assert_eq!(host.requests().len(), 1);
    fail.store(false, Ordering::Release);
    host.allow(&peer).unwrap();
    let welcome = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let persisted: Sharing =
        serde_json::from_slice(saved.lock().unwrap().as_ref().unwrap()).unwrap();
    assert_eq!(persisted.members[0].secret, welcome.secret);
    assert_ne!(welcome.secret, welcome.room);
    fail.store(true, Ordering::Release);
    assert!(host.remove(&welcome.secret).is_err());
    assert_eq!(host.sharing(), persisted);
    fail.store(false, Ordering::Release);

    let code = self::code(&host);
    let (reply, result) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = reply.send(share::join(hello("Alan"), &code, "", None, Some(&url)));
    });
    until("the second request never arrived", || {
        host.requests().len() == 1
    });
    host.decline(&host.requests()[0].peer);
    assert_eq!(
        result.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(share::Refusal::Declined)
    );
    assert_eq!(host.devices().len(), 1);
}

#[test]
fn removal_retires_one_credential_and_other_devices_reconnect_after_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host_cache = directory.path().join("host");
    let host = host(&folder, &host_cache, &sharing, &url);
    let original_code = code(&host);
    let (alice, _) = guest(
        "Alice",
        &original_code,
        &url,
        &directory.path().join("alice"),
    );
    let (bob, _) = guest("Bob", &original_code, &url, &directory.path().join("bob"));
    let before = host.sharing();
    let alice_key = before
        .members
        .iter()
        .find(|device| device.name == "Alice")
        .unwrap()
        .secret;
    let bob_key = before
        .members
        .iter()
        .find(|device| device.name == "Bob")
        .unwrap()
        .secret;
    host.remove(&bob_key).unwrap();
    until("Bob was not removed", || {
        bob.stopped() && bob.host().is_none()
    });
    until("presence did not move to the new room", || {
        host.guests().len() == 1
    });
    let current = host.sharing();
    assert_ne!(before.secret, current.secret);
    assert_eq!(current.members.len(), 1);
    assert_ne!(original_code, code(&host));
    assert!(alice.host().is_some());
    assert!(!alice.stopped());
    let live = Notebook::open_hosted(Arc::clone(&alice), directory.path().join("alice")).unwrap();
    assert_eq!(live.catalog().sections.len(), 2);
    let restarted: Sharing =
        serde_json::from_slice(&serde_json::to_vec(&current).unwrap()).unwrap();
    drop(host);
    until("Alice's connection did not close", || {
        alice.host().is_none()
    });
    let host = self::host(&folder, &host_cache, &restarted, &url);
    until("Alice did not reconnect", || alice.host().is_some());
    assert_eq!(host.devices()[0].0.secret, alice_key);
    let forged = Guest::start(
        hello("Alice"),
        current.share,
        bob_key,
        None,
        Some(&url),
        || {},
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(forged.host().is_none());
    assert!(Notebook::open_hosted(forged, directory.path().join("forged")).is_err());
}

#[test]
fn cancelling_a_join_retires_its_pending_request() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let mut sharing = Sharing::new("").unwrap();
    sharing.approve = true;
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let code = code(&host);
    let alive = Arc::new(AtomicBool::new(true));
    let continuing = Arc::clone(&alive);
    let (reply, result) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = reply.send(share::join_while(
            hello("Grace"),
            &code,
            "",
            None,
            Some(&url),
            |_| continuing.load(Ordering::Acquire),
        ));
    });
    until("the request never arrived", || !host.requests().is_empty());
    alive.store(false, Ordering::Release);
    assert_eq!(
        result.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(share::Refusal::Cancelled)
    );
    until("the cancelled request stayed", || {
        host.requests().is_empty()
    });
    assert!(host.devices().is_empty());
}
