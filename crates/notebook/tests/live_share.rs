//! Live Share: guests open a notebook a host serves through a relay, edit it through the
//! replica and queue they use on a share, queue while the host is away, conflict as on a
//! share, pass protected sections through as ciphertext, and are refused with a wrong code.
#![cfg(feature = "live")]

use notebook::{
    live::share::{self, Refusal, Sharing},
    session::{Notebook, SyncState},
};
use onestore::Arena;
use std::sync::Arc;

#[path = "support/live.rs"]
mod live;
use live::*;

/// A guest opens a section through the host, its edit lands in the host's file, and the
/// host's own edit reaches the guest.
#[test]
fn a_guest_edits_the_host_s_notebook() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let (guest, notebook) = guest("Grace", &code(&host), &url, &directory.path().join("grace"));
    let paths: Vec<String> = notebook
        .catalog()
        .sections
        .iter()
        .map(|section| section.path.clone())
        .collect();
    assert_eq!(paths, ["Garden.one", "Sealed.one"]);
    let section = open(&notebook, &guest, "Garden.one", None);
    let file = folder.join("Garden.one");
    let id = replace(&section, &std::fs::read(&file).unwrap(), 0..8, "Grace's");
    published(&section, id);
    assert_eq!(
        server::text(&std::fs::read(&file).unwrap()).2,
        "Grace's text"
    );
    assert_eq!(host.guests().len(), 1);

    // The host's own edit, as its app commits one.
    let image = std::fs::read(&file).unwrap();
    let (space, text, _) = server::text(&image);
    std::fs::write(&file, server::typed(&image, space, text, 0..7, "Ada's")).unwrap();
    host.touched(&["Garden.one".into()]);
    until("the guest never saw the host's edit", || {
        section
            .page(space)
            .is_ok_and(|page| server::page_texts(&page).contains(&"Ada's text".to_owned()))
    });
}

/// While the host is away a guest's edits wait in its replica, the notebook opens from its
/// last listing, and once the host is back the edits reach its file.
#[test]
fn a_guest_queues_while_the_host_is_away() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host_cache = directory.path().join("host");
    let host = host(&folder, &host_cache, &sharing, &url);
    let cache = directory.path().join("grace");
    let (guest, notebook) = guest("Grace", &code(&host), &url, &cache);
    let section = open(&notebook, &guest, "Garden.one", None);
    let file = folder.join("Garden.one");
    let image = std::fs::read(&file).unwrap();

    // Ada's computer goes to sleep.
    drop(host);
    until("the host never left", || guest.host().is_none());
    let id = replace(&section, &image, 0..8, "Offline");
    until("the section never said the host was away", || {
        section
            .sync_status()
            .is_ok_and(|status| status.state() == SyncState::NotConnected && status.queued > 0)
    });
    assert_eq!(std::fs::read(&file).unwrap(), image);
    // The notebook opens from its last listing while the host is away.
    let reopened = Notebook::open_hosted(Arc::clone(&guest), &cache).unwrap();
    assert_eq!(reopened.catalog().sections.len(), 2);

    let host = self::host(&folder, &host_cache, &sharing, &url);
    published(&section, id);
    assert_eq!(
        server::text(&std::fs::read(&file).unwrap()).2,
        "Offline text"
    );
    drop(host);
}

/// Two guests that change the same words while apart: the second to publish gets OneNote's
/// conflict page, in its replica and in the host's file.
#[test]
fn two_guests_on_one_page_conflict_as_on_a_share() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host_cache = directory.path().join("host");
    let host = host(&folder, &host_cache, &sharing, &url);
    let code = code(&host);
    let (grace, grace_notebook) = guest("Grace", &code, &url, &directory.path().join("grace"));
    let (alan, alan_notebook) = guest("Alan", &code, &url, &directory.path().join("alan"));
    let graces = open(&grace_notebook, &grace, "Garden.one", None);
    let alans = open(&alan_notebook, &alan, "Garden.one", None);
    let file = folder.join("Garden.one");
    let image = std::fs::read(&file).unwrap();

    drop(host);
    until("the host never left", || {
        grace.host().is_none() && alan.host().is_none()
    });
    let first = replace(&graces, &image, 0..8, "Grace's");
    let second = replace(&alans, &image, 0..8, "Alan's");
    let host = self::host(&folder, &host_cache, &sharing, &url);
    published(&graces, first);
    published(&alans, second);
    let stored = std::fs::read(&file).unwrap();
    let conflicts = server::conflicts(&stored);
    assert_eq!(conflicts.len(), 1, "one page holds a conflict page");
    let (user, kept) = &conflicts[0].1[0];
    assert_eq!(user, "Guest");
    let texts: Vec<String> = server::pages(&stored)
        .iter()
        .flat_map(|(_, page)| server::page_texts(page))
        .chain(kept.iter().cloned())
        .collect();
    assert!(
        texts.contains(&"Grace's text".to_owned()) && texts.contains(&"Alan's text".to_owned()),
        "{texts:?}"
    );
    until("the host's conflict never reached a guest", || {
        !alans.conflicts().unwrap().is_empty() || !graces.conflicts().unwrap().is_empty()
    });
    drop(host);
}

/// A protected section's guest unlocks it with the password; the host only ever stores
/// what the guest sealed.
#[test]
fn a_protected_section_passes_through_as_ciphertext() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let (guest, notebook) = guest("Grace", &code(&host), &url, &directory.path().join("grace"));
    assert!(notebook.unlock("Sealed.one", "wrong password").is_err());
    let key = notebook.unlock("Sealed.one", PASSWORD).unwrap();
    let section = open(&notebook, &guest, "Sealed.one", Some(&key));
    let file = folder.join("Sealed.one");
    let arena = Arena::default();
    let mut unlocked =
        onestore::Section::unlock(&arena, std::fs::read(&file).unwrap(), &key).unwrap();
    let (space, ..) = unlocked.pages().unwrap()[0];
    let page = unlocked.page(space).unwrap();
    let text = page
        .objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find_map(|p| p.text().map(|t| t.id)),
            _ => None,
        })
        .unwrap();
    let id = replaced(&section, space, text, 0..6, "Guarded");
    published(&section, id);
    let stored = std::fs::read(&file).unwrap();
    let marker = "Guarded"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<u8>>();
    assert!(
        !stored.windows(marker.len()).any(|window| window == marker),
        "the host's file holds the new text in the clear"
    );
    assert!(onestore::Section::open(&Arena::default(), stored.clone()).is_err());
    let arena = Arena::default();
    let reread = onestore::Section::unlock(&arena, stored, &key).unwrap();
    let texts = server::page_texts(&reread.page(space).unwrap());
    assert!(texts.contains(&"Guarded text".to_owned()), "{texts:?}");
}

/// A wrong code is refused without the guest learning anything; the host's code burns after
/// too many wrong tries and is replaced by new words; too many wrong codes from one network
/// lock it out; and a guest can't reach outside the notebook or presence's secret.
#[test]
fn wrong_codes_are_refused_and_counted() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(relay::server::Config {
        burn_after: 2,
        failures_per_minute: 3,
        ..Default::default()
    });
    let sharing = Sharing::new("").unwrap();
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let code = code(&host);
    let (number, secret) = notebook::live::code::parse(&code).unwrap();
    let wrong = notebook::live::code::format(number, &mistaken(&secret)).unwrap();
    let join = |code: &str| share::join(hello("Mallory"), code, "", None, Some(&url));
    assert_eq!(join("not a code").unwrap_err(), Refusal::Malformed);
    // A symbol mistyped fails its check here, spending none of the two tries before a burn.
    let typo = format!(
        "{}{}",
        if code.starts_with('7') { '8' } else { '7' },
        &code[1..]
    );
    assert_eq!(join(&typo).unwrap_err(), Refusal::Malformed);
    assert_eq!(
        join(&code.to_lowercase().replace('-', " ")).map(|_| ()),
        Ok(())
    );
    assert_eq!(join(&wrong).unwrap_err(), Refusal::Wrong);
    assert_eq!(join(&wrong).unwrap_err(), Refusal::Wrong);
    // The code burned, and the host shares a new secret, under a number of its own.
    until("the code never changed", || {
        host.code()
            .is_some_and(|now| now != code && share::code(&now).is_some())
    });
    let fresh = host.code().unwrap();
    assert!(matches!(
        join(&code).unwrap_err(),
        Refusal::Expired | Refusal::NoOne
    ));
    let (number, secret) = notebook::live::code::parse(&fresh).unwrap();
    let wrong = notebook::live::code::format(number, &mistaken(&secret)).unwrap();
    assert_eq!(join(&wrong).unwrap_err(), Refusal::Wrong);
    // A third wrong code in a minute locks this network out, even from the right code.
    assert!(matches!(
        join(&fresh).unwrap_err(),
        Refusal::TooMany(Some(_))
    ));

    // Joined, a guest still can't reach outside the notebook.
    std::fs::create_dir_all(folder.join(".snowbound")).unwrap();
    std::fs::write(folder.join(".snowbound/live.json"), b"{}").unwrap();
    std::fs::write(directory.path().join("secret.txt"), b"secret").unwrap();
    let url = relay(Default::default());
    let host = self::host(&folder, &directory.path().join("host"), &sharing, &url);
    let (_guest, notebook) = guest(
        "Grace",
        &self::code(&host),
        &url,
        &directory.path().join("g"),
    );
    let storage = notebook.into_storage();
    for path in [
        "../secret.txt",
        ".snowbound/live.json",
        "/etc/hosts",
        "a//b",
    ] {
        let error = storage.read_file(path, 1024).unwrap_err();
        assert!(
            error.to_string().contains("Outside the notebook"),
            "{path}: {error}"
        );
    }
    assert!(storage.rename_root("Elsewhere", &[]).is_err());
}

/// Files larger than one message go up and come back a chunk at a time, through a relay that
/// hangs up on a peer with more than its queue waiting.
#[test]
fn large_files_travel_in_chunks() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(relay::server::Config {
        queue: 400 << 10,
        ..Default::default()
    });
    let sharing = Sharing::new("").unwrap();
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let (_guest, notebook) = guest("Grace", &code(&host), &url, &directory.path().join("grace"));
    let storage = notebook.into_storage();
    let bytes: Vec<u8> = (0..3_000_000u32).map(|at| (at * 7 % 251) as u8).collect();
    storage
        .create("Garden_onefiles/big.bin", &bytes)
        .unwrap_err();
    std::fs::create_dir(folder.join("Garden_onefiles")).unwrap();
    storage.create("Garden_onefiles/big.bin", &bytes).unwrap();
    assert_eq!(
        std::fs::read(folder.join("Garden_onefiles/big.bin")).unwrap(),
        bytes
    );
    assert_eq!(
        storage
            .read_file("Garden_onefiles/big.bin", 4 << 20)
            .unwrap(),
        bytes
    );
    assert!(
        storage
            .read_file("Garden_onefiles/big.bin", 1 << 20)
            .is_err()
    );
}

/// A guest that floods its host with requests is hung up on once too many wait, having had
/// answers to few of them, and the host goes on serving the others.
#[test]
fn a_flooding_guest_is_hung_up_on() {
    use notebook::live::{
        Event, Live, Room,
        wire::{Bye, Request, kind},
    };
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let code = code(&host);
    let (grace, notebook) = guest("Grace", &code, &url, &directory.path().join("grace"));
    let welcome = share::join(hello("Mallory"), &code, "", None, Some(&url)).unwrap();
    let replies = Arc::new(AtomicUsize::new(0));
    let bye = Arc::new(Mutex::new(None));
    let (counted, said) = (Arc::clone(&replies), Arc::clone(&bye));
    let mallory = Live::start(
        hello("Mallory"),
        &Room::Notebook(welcome.secret),
        None,
        Some(&url),
        move |event| match event {
            Event::Frame {
                kind: kind::REPLY, ..
            } => {
                counted.fetch_add(1, Ordering::Relaxed);
            }
            Event::Frame {
                kind: kind::BYE,
                body,
                ..
            } => *said.lock().unwrap() = minicbor::decode::<Bye>(body).ok(),
            _ => {}
        },
    )
    .unwrap();
    // The line to the host, while Mallory's stream to it is open.
    let served = |live: &Live| {
        let host = live
            .peers()
            .into_iter()
            .find(|peer| peer.hello.serves == Some(welcome.share))?;
        live.line(&host.hello.peer)
    };
    until("Mallory never met the host", || served(&mallory).is_some());
    let line = served(&mallory).unwrap();
    const SENT: u64 = 5000;
    for id in 0..SENT {
        let request = Request {
            id,
            path: "Garden.one".into(),
            ..Request::default()
        };
        if line.send(kind::STAMP, &request).is_err() {
            break;
        }
    }
    until("the host never hung up on Mallory", || {
        bye.lock().unwrap().is_some() && served(&mallory).is_none()
    });
    assert_eq!(bye.lock().unwrap().as_ref().unwrap().reason, "flooded");
    let answered = replies.load(Ordering::Relaxed);
    // At most the burst a guest may start at once, what waits for the workers, and what the
    // rate refills while the flood arrives.
    assert!(answered < 300, "Mallory had {answered} answers of {SENT}");
    // Grace, asking at her own pace, is served as before.
    assert!(grace.host().is_some());
    assert_eq!(
        notebook.read_section("Garden.one").unwrap(),
        std::fs::read(folder.join("Garden.one")).unwrap()
    );
}

/// Stopping a share lets every guest go, saying so, and leaves its code and secret for no one:
/// the code no longer opens anything, and sharing again makes new ones.
#[test]
fn stopping_lets_every_guest_go_and_retires_the_code() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let code = code(&host);
    let (guest, _notebook) = guest("Grace", &code, &url, &directory.path().join("grace"));
    host.stop();
    until("the guest never heard the host stop", || {
        guest.stopped() && guest.host().is_none()
    });
    assert!(host.code().is_none() && host.guests().is_empty());
    assert!(matches!(
        share::join(hello("Alan"), &code, "", None, Some(&url)).unwrap_err(),
        Refusal::NoOne | Refusal::TimedOut
    ));
    let again = Sharing::new("").unwrap();
    assert!(again.secret != sharing.secret && again.share != sharing.share);
    assert_ne!(again.code, sharing.code);
}
