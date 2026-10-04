//! Measures Live Share's typing latency: how long after a keystroke lands in one guest's
//! replica the other guest's section shows it, and the same from the host to a guest.
//!
//! `cargo run -p notebook --features live --example live_latency -- lan|relay [URL]`: `lan`
//! meets by mDNS on this computer's loopback; `relay` through `URL`, or a relay this run
//! starts on loopback.

#[path = "../tests/support/server.rs"]
mod server;

use notebook::{
    Replica,
    live::{
        Hello, Reach,
        share::{self, Guest, Host, Sharing},
    },
    session::{Background, Notebook, Section},
};
use onestore::op::{Edit, Op, PageOp};
use std::{
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const KEYS: usize = 20;

fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(1));
    }
}

fn hello(name: &str) -> Hello {
    Hello::new(name.into(), None).unwrap()
}

/// A guest as the app holds one: the notebook, its background, and the section open.
struct Open {
    _background: Background,
    section: Section,
}

fn guest(
    name: &str,
    code: &str,
    reach: Option<Reach>,
    relay: Option<&str>,
    cache: &Path,
) -> (Arc<Guest>, Open) {
    let welcome = share::join(hello(name), code, "", reach, relay).unwrap();
    let guest = Guest::start(
        hello(name),
        welcome.share,
        welcome.secret,
        reach,
        relay,
        || {},
    )
    .unwrap();
    until("no host", || guest.host().is_some());
    let mut notebook = Notebook::open_hosted(Arc::clone(&guest), cache).unwrap();
    let background = Background::hosted(Arc::clone(&guest), || {}).unwrap();
    background.watch(notebook.replicas());
    let replica = notebook.replica_path("Garden.one").unwrap();
    std::fs::create_dir_all(replica.parent().unwrap()).unwrap();
    let replica =
        Replica::open_or_create(&replica, None, || notebook.read_section("Garden.one")).unwrap();
    let section =
        Section::resume_hosted("Garden.one".into(), replica, Arc::clone(&guest), || {}).unwrap();
    background.hold("Garden.one", &section);
    (
        guest,
        Open {
            _background: background,
            section,
        },
    )
}

/// Types `key` at the end of the page's text in `section`; returns the text it then reads.
fn type_key(section: &Section, key: char) -> String {
    let (space, text, before) = text_of(section);
    let at = before.encode_utf16().count() as u32;
    let op = PageOp::Text {
        text,
        range: at..at,
        with: key.to_string(),
    };
    let edit = Edit {
        at: 134_000_000_000_000_000,
        ops: vec![Op::Page { space, op }],
    };
    section.replica().apply("Typist", edit).unwrap();
    format!("{before}{key}")
}

fn text_of(section: &Section) -> (onestore::ExGuid, onestore::ExGuid, String) {
    let (space, ..) = section.pages().unwrap()[0];
    let page = section.page(space).unwrap();
    page.objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find_map(|p| p.text().map(|t| (space, t.id, t.text.text().to_owned()))),
            _ => None,
        })
        .unwrap()
}

fn report(what: &str, mut times: Vec<Duration>) {
    times.sort();
    let ms = |at: usize| times[at].as_secs_f64() * 1000.0;
    println!(
        "{what}: median {:.0} ms, p90 {:.0} ms, worst {:.0} ms over {} keys",
        ms(times.len() / 2),
        ms(times.len() * 9 / 10),
        ms(times.len() - 1),
        times.len()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (reach, url) = match args.first().map(String::as_str) {
        Some("lan") => (Some(Reach::Loopback), None),
        Some("relay") => (
            None,
            Some(args.get(1).cloned().unwrap_or_else(|| {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let url = format!("ws://{}", listener.local_addr().unwrap());
                thread::spawn(move || relay::server::serve(listener, Default::default()));
                url
            })),
        ),
        _ => panic!("lan or relay [URL]"),
    };
    let relay = url.as_deref();
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Garden");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("Garden.one"),
        onestore::create_section("Garden.one", "Typed:", "Fixture").unwrap(),
    )
    .unwrap();
    let storage = Notebook::open(&folder, directory.path().join("host"))
        .unwrap()
        .into_storage();
    let host = Host::start(
        storage,
        hello("Ada"),
        Sharing::new("").unwrap(),
        "Garden",
        reach,
        relay,
        || {},
        |_| Ok(()),
    )
    .unwrap();
    until("no code", || {
        host.code().is_some_and(|code| share::code(&code).is_some())
    });
    let code = host.code().unwrap();
    let (_grace, grace) = guest("Grace", &code, reach, relay, &directory.path().join("g"));
    let (_alan, alan) = guest("Alan", &code, reach, relay, &directory.path().join("a"));

    let mut times = Vec::new();
    for key in "abcdefghijklmnopqrstuvwxyz".chars().take(KEYS) {
        let typed = Instant::now();
        let expected = type_key(&grace.section, key);
        until("the other guest never saw a key", || {
            alan.section.events();
            text_of(&alan.section).2 == expected
        });
        times.push(typed.elapsed());
        thread::sleep(Duration::from_millis(150));
    }
    report("guest to guest", times);

    // The host's own keystrokes, published to its file as its app does, then reported to
    // the share as its app reports them.
    let file = folder.join("Garden.one");
    let mut times = Vec::new();
    for key in "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().take(KEYS) {
        let image = std::fs::read(&file).unwrap();
        let (space, text, before) = server::text(&image);
        let at = before.encode_utf16().count() as u32;
        let typed = Instant::now();
        let next = server::typed(&image, space, text, at..at, &key.to_string());
        std::fs::write(&file, &next).unwrap();
        host.touched(&["Garden.one".into()]);
        let expected = format!("{before}{key}");
        until("the guest never saw the host's key", || {
            alan.section.events();
            text_of(&alan.section).2 == expected
        });
        times.push(typed.elapsed());
        thread::sleep(Duration::from_millis(150));
    }
    report("host to guest", times);
}
