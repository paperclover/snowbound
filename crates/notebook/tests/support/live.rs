//! What the Live Share tests share: a relay, a host and guests on a notebook of two sections.
#![allow(dead_code)]

use notebook::{
    EditStatus, Replica,
    live::{
        Hello,
        share::{self, Guest, Host, Sharing},
    },
    session::{Notebook, Section},
};
use onestore::{
    ExGuid,
    op::{Edit, Op, PageOp},
    protected::{Key, rekey},
};
use std::{
    net::TcpListener,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

#[path = "server.rs"]
pub mod server;

pub const PASSWORD: &str = "fixture password";

/// A relay on this computer with `config`'s limits: its URL.
pub fn relay(config: relay::server::Config) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    thread::spawn(move || relay::server::serve(listener, config));
    url
}

/// Another secret than `secret`, as a guess makes one.
pub fn mistaken(secret: &str) -> String {
    let first = if secret.starts_with('A') { 'B' } else { 'A' };
    format!("{first}{}", &secret[1..])
}

pub fn hello(name: &str) -> Hello {
    Hello::new(name.into(), None).unwrap()
}

pub fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A notebook folder holding `Garden.one`, a page reading "Original text", and a protected
/// `Sealed.one` reading "Sealed text".
pub fn notebook(root: &Path) -> std::path::PathBuf {
    let folder = root.join("Garden");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("Garden.one"),
        onestore::create_section("Garden.one", "Original text", "Fixture").unwrap(),
    )
    .unwrap();
    let plain = onestore::create_section("Sealed.one", "Sealed text", "Fixture").unwrap();
    let key = Key::new(PASSWORD).unwrap();
    std::fs::write(
        folder.join("Sealed.one"),
        rekey(&plain, None, Some(&key)).unwrap(),
    )
    .unwrap();
    folder
}

pub fn host(folder: &Path, cache: &Path, sharing: &Sharing, url: &str) -> Host {
    let storage = Notebook::open(folder, cache).unwrap().into_storage();
    Host::start(
        storage,
        hello("Ada"),
        sharing.clone(),
        "Garden",
        None,
        Some(url),
        || {},
        |_| Ok(()),
    )
    .unwrap()
}

/// The host's code once the relay has numbered it.
pub fn code(host: &Host) -> String {
    until("the code was never numbered", || {
        host.code().is_some_and(|code| share::code(&code).is_some())
    });
    host.code().unwrap()
}

/// `name` joins with `code` and opens the notebook in `cache`.
pub fn guest(name: &str, code: &str, url: &str, cache: &Path) -> (Arc<Guest>, Notebook) {
    let welcome = share::join(hello(name), code, "", None, Some(url)).unwrap();
    assert_eq!(
        (welcome.notebook.as_str(), welcome.host.as_str()),
        ("Garden", "Ada")
    );
    let guest = Guest::start(
        hello(name),
        welcome.share,
        welcome.secret,
        None,
        Some(url),
        || {},
    )
    .unwrap();
    until("the host was never met", || guest.host().is_some());
    let notebook = Notebook::open_hosted(Arc::clone(&guest), cache).unwrap();
    (guest, notebook)
}

pub fn open(notebook: &Notebook, guest: &Arc<Guest>, path: &str, key: Option<&Key>) -> Section {
    let replica = notebook.replica_path(path).unwrap();
    std::fs::create_dir_all(replica.parent().unwrap()).unwrap();
    let replica = Replica::open_or_create(&replica, key, || notebook.read_section(path)).unwrap();
    Section::resume_hosted(path.into(), replica, Arc::clone(guest), || {}).unwrap()
}

pub fn replace(section: &Section, image: &[u8], range: std::ops::Range<u32>, with: &str) -> u64 {
    let (space, text, _) = server::text(image);
    replaced(section, space, text, range, with)
}

pub fn replaced(
    section: &Section,
    space: ExGuid,
    text: ExGuid,
    range: std::ops::Range<u32>,
    with: &str,
) -> u64 {
    let op = PageOp::Text {
        text,
        range,
        with: with.into(),
    };
    let edit = Edit {
        at: 134_000_000_000_000_000,
        ops: vec![Op::Page { space, op }],
    };
    section.replica().apply("Guest", edit).unwrap()
}

pub fn published(section: &Section, id: u64) {
    until("the edit was never published", || {
        matches!(
            section.status(id).unwrap(),
            Some(EditStatus::Published { .. })
        )
    });
}
