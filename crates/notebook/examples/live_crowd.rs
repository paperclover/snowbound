//! A crowd on one shared notebook: a host, and guests that join it through a relay, half of
//! them typing on the same page. Reports how long keys and carets take to reach the others,
//! what the relay passes on, and what the relay, the host and the guests cost.
//!
//! `cargo run --release -p notebook --features live --example live_crowd -- RELAY_BINARY
//! [PEERS=30] [TYPISTS=PEERS/2] [SECONDS=60]`, the relay's binary built with
//! `cargo build --release -p relay`. The relay, the host and the guests each run as a process
//! of their own, so `ps` tells their costs apart.
//!
//! For watching a crowd in the app: `fixture FOLDER PARAGRAPHS` writes the notebook the
//! host shares, and `guests URL CODE PEERS SECONDS` has that many join the app's share of it
//! and move their carets about its page.

use notebook::{
    Replica,
    live::{
        Caret, Guid, Hello, Presence, Spot,
        share::{self, Guest, Host, Sharing},
    },
    session::{Background, Notebook, Section},
};
use onestore::{
    ExGuid,
    op::{Edit, Op, PageOp},
};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Keys each typist types a second, and how often observers look.
const KEYS_PER_SECOND: f64 = 5.0;
const LOOK: Duration = Duration::from_millis(2);
/// Guests that time what they see.
const OBSERVERS: usize = 3;

fn until(what: &str, limit: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn hello(name: &str) -> Hello {
    Hello::new(name.into(), None).unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("host") => return host(&args[1], args[2].parse().unwrap()),
        Some("fixture") => {
            fixture(Path::new(&args[1]), args[2].parse().unwrap());
            return;
        }
        Some("guests") => {
            let seconds = Duration::from_secs(args[4].parse().unwrap());
            return guests(&args[1], &args[2], args[3].parse().unwrap(), seconds);
        }
        _ => {}
    }
    let binary = args.first().expect("the relay's binary");
    let number = |at: usize, default: usize| args.get(at).map_or(default, |n| n.parse().unwrap());
    let peers = number(1, 30);
    let typists = number(2, peers / 2);
    let seconds = number(3, 60) as u64;
    crowd(binary, peers, typists, Duration::from_secs(seconds));
}

/// Writes the notebook `Garden` in `directory`: a section whose page has `paragraphs`
/// paragraphs, `Typed:` then `Note 1:` and on. Its folder.
fn fixture(directory: &Path, paragraphs: usize) -> std::path::PathBuf {
    let folder = directory.join("Garden");
    std::fs::create_dir_all(&folder).unwrap();
    let mut image = onestore::create_section("Garden.one", "Typed:", "Fixture").unwrap();
    for n in (1..paragraphs).rev() {
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, image).unwrap();
        let (space, ..) = section.pages().unwrap()[0];
        let page = section.page(space).unwrap();
        let (text, _) = texts(&page).swap_remove(0);
        let right = onestore::page::text::new_id().unwrap();
        let ops = [
            PageOp::Split {
                text,
                at: 6,
                paragraph: onestore::page::text::new_id().unwrap(),
                right,
                lists: Vec::new(),
            },
            PageOp::Text {
                text: right,
                range: 0..0,
                with: format!("Note {n}:"),
            },
        ];
        let edit = Edit {
            at: 134_000_000_000_000_000,
            ops: ops.into_iter().map(|op| Op::Page { space, op }).collect(),
        };
        section.apply("Fixture", &edit).unwrap();
        section.seal().unwrap();
        image = section.image();
    }
    std::fs::write(folder.join("Garden.one"), image).unwrap();
    folder
}

/// The host's process: shares a fixture notebook of a page with `paragraphs` paragraphs
/// through `url`, says its code, and reports what the section grew to once its input closes.
fn host(url: &str, paragraphs: usize) {
    let directory = tempfile::tempdir().unwrap();
    let folder = fixture(directory.path(), paragraphs);
    let file = folder.join("Garden.one");
    let storage = Notebook::open(&folder, directory.path().join("cache"))
        .unwrap()
        .into_storage();
    let host = Host::start(
        storage,
        hello("Host"),
        Sharing::new("").unwrap(),
        "Garden",
        None,
        Some(url),
        || {},
    )
    .unwrap();
    until("no code", Duration::from_secs(30), || {
        host.code().is_some_and(|code| share::code(&code).is_some())
    });
    println!("code {}", host.code().unwrap());
    let _ = std::io::stdin().read_to_end(&mut Vec::new());
    let image = std::fs::read(&file).unwrap();
    if let Some(output) = std::env::var_os("SNOWBOUND_LIVE_EVIDENCE") {
        let output = Path::new(&output);
        std::fs::create_dir_all(output).unwrap();
        std::fs::copy(&file, output.join("Garden.one")).unwrap();
    }
    let size = image.len();
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, image).unwrap();
    let pages = section.pages().unwrap().len();
    let conflicts: usize = section
        .conflicts()
        .unwrap()
        .iter()
        .map(|(_, c)| c.len())
        .sum();
    println!("section {size} bytes, {pages} pages, {conflicts} conflict pages");
}

/// A guest as the app holds one: its share, the notebook's background, the section open
/// and its file's identity.
struct Open {
    guest: Arc<Guest>,
    _background: Background,
    section: Section,
    file: [u8; 16],
}

fn join(name: &str, code: &str, url: &str, cache: &Path) -> Open {
    let welcome = share::join(hello(name), code, "", None, Some(url)).unwrap();
    let guest = Guest::start(
        hello(name),
        welcome.share,
        welcome.secret,
        None,
        Some(url),
        || {},
    )
    .unwrap();
    until("no host", Duration::from_secs(120), || {
        guest.host().is_some()
    });
    let mut notebook = Notebook::open_hosted(Arc::clone(&guest), cache).unwrap();
    let background = Background::hosted(Arc::clone(&guest), || {}).unwrap();
    background.watch(notebook.replicas());
    let replica = notebook.replica_path("Garden.one").unwrap();
    std::fs::create_dir_all(replica.parent().unwrap()).unwrap();
    let image = notebook.read_section("Garden.one").unwrap();
    let file = onestore::Header::parse(&image[..1024]).unwrap().file_id;
    let replica = Replica::open_or_create(&replica, None, || Ok(image)).unwrap();
    let section =
        Section::resume_hosted("Garden.one".into(), replica, Arc::clone(&guest), || {}).unwrap();
    background.hold("Garden.one", &section);
    Open {
        guest,
        _background: background,
        section,
        file,
    }
}

/// Where in the open page's `paragraph`th text, `offset` units in, `open`'s guest is.
fn presence(open: &Open, paragraph: usize, offset: u32) -> Option<Presence> {
    let (space, texts) = texts_of(&open.section)?;
    let (text, _) = texts.get(paragraph)?;
    let at = Spot {
        text: Guid {
            guid: text.guid,
            n: text.n,
        },
        offset,
    };
    Some(Presence {
        section: Some(open.file),
        page: Some(Guid {
            guid: space.guid,
            n: space.n,
        }),
        caret: Some(Caret {
            anchor: at,
            focus: at,
        }),
    })
}

/// `peers` guests join the share `code` names through `url` and, for `seconds`, put their
/// carets in the page's paragraphs, each moving every few seconds.
fn guests(url: &str, code: &str, peers: usize, seconds: Duration) {
    let directory = tempfile::tempdir().unwrap();
    let names = [
        "Ada", "Grace", "Alan", "Barbara", "Edsger", "Frances", "Donald", "Margaret", "Ken",
        "Radia", "Dennis", "Hedy", "Linus", "Karen", "John", "Sophie",
    ];
    let joining: Vec<_> = (0..peers)
        .map(|n| {
            let name = format!("{} {n}", names[n % names.len()]);
            let (code, url, cache) = (
                code.to_owned(),
                url.to_owned(),
                directory.path().join(n.to_string()),
            );
            thread::sleep(Duration::from_millis(100));
            thread::spawn(move || join(&name, &code, &url, &cache))
        })
        .collect();
    let guests: Vec<Open> = joining
        .into_iter()
        .map(|joined| joined.join().unwrap())
        .collect();
    let began = Instant::now();
    let mut turn = 0;
    while began.elapsed() < seconds {
        for (n, open) in guests.iter().enumerate() {
            let paragraphs = texts_of(&open.section).map_or(1, |(_, texts)| texts.len());
            if (n + turn) % 4 == 0 || turn == 0 {
                let at = (n * 7 + turn) % paragraphs;
                if let Some(presence) = presence(open, at, (turn % 5) as u32) {
                    open.guest.set_presence(presence);
                }
            }
        }
        turn += 1;
        thread::sleep(Duration::from_secs(1));
    }
}

/// The page's object space and its paragraphs' texts: each one's object and what it reads.
fn texts_of(section: &Section) -> Option<(ExGuid, Vec<(ExGuid, String)>)> {
    let (space, ..) = *section.pages().ok()?.first()?;
    let page = section.page(space).ok()?;
    Some((space, texts(&page)))
}

fn texts(page: &onestore::page::Page) -> Vec<(ExGuid, String)> {
    let outlines = page.objects.iter().filter_map(|object| match object {
        onestore::page::PageObject::Outline(outline) => Some(outline),
        _ => None,
    });
    outlines
        .flat_map(|outline| &outline.paragraphs)
        .filter_map(|p| p.text().map(|t| (t.id, t.text.text().to_owned())))
        .collect()
}

/// The key typist `typist` types `n`th, unique to both.
fn key(typist: usize, n: usize) -> char {
    char::from_u32(0x4E00 + (typist * 600 + n) as u32).unwrap()
}

fn typist_of(key: char) -> Option<(usize, usize)> {
    let at = (key as u32).checked_sub(0x4E00)? as usize;
    Some((at / 600, at % 600))
}

/// Seconds of CPU and kilobytes resident `ps` reports for `pid`.
fn cost(pid: u32) -> (f64, u64) {
    let output = Command::new("ps")
        .args(["-o", "time=,rss=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let mut words = text.split_whitespace();
    let time = words.next().unwrap_or("0:0");
    let seconds = time.split(':').fold(0.0, |total, part| {
        total * 60.0 + part.parse::<f64>().unwrap_or(0.0)
    });
    (
        seconds,
        words.next().and_then(|rss| rss.parse().ok()).unwrap_or(0),
    )
}

/// The relay's `/health`, as numbers by name.
fn health(address: &str) -> HashMap<String, f64> {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "GET /health HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default();
    body.trim()
        .trim_matches(['{', '}'])
        .split(',')
        .filter_map(|pair| {
            let (name, value) = pair.split_once(':')?;
            Some((name.trim_matches('"').to_owned(), value.parse().ok()?))
        })
        .collect()
}

fn report(what: &str, mut times: Vec<Duration>) {
    if times.is_empty() {
        println!("{what}: none seen");
        return;
    }
    times.sort();
    let ms = |at: usize| times[at].as_secs_f64() * 1000.0;
    println!(
        "{what}: median {:.0} ms, p90 {:.0} ms, p99 {:.0} ms, worst {:.0} ms ({} seen)",
        ms(times.len() / 2),
        ms(times.len() * 9 / 10),
        ms(times.len() * 99 / 100),
        ms(times.len() - 1),
        times.len()
    );
}

struct Spawned(Child);

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn crowd(binary: &str, peers: usize, typists: usize, window: Duration) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let address = format!("127.0.0.1:{port}");
    // Everyone comes from one address here, which a relay would take for one abuser.
    let relay = Spawned(
        Command::new(binary)
            .args(["--listen", &address])
            .args(["--max-connections-per-address", "1000"])
            .args(["--joins-per-minute", "10000"])
            .args(["--failures-per-minute", "10000"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    until("no relay", Duration::from_secs(10), || {
        TcpStream::connect(&address).is_ok()
    });
    let url = format!("ws://{address}");
    let mut hosting = Spawned(
        Command::new(std::env::current_exe().unwrap())
            .args(["host", &url, &typists.max(1).to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut said = BufReader::new(hosting.0.stdout.take().unwrap()).lines();
    let code = said.next().unwrap().unwrap();
    let code = code.strip_prefix("code ").unwrap().to_owned();
    println!("{peers} guests, {typists} typing {KEYS_PER_SECOND} keys a second, code {code}");

    let directory = tempfile::tempdir().unwrap();
    let began = Instant::now();
    let joining: Vec<_> = (0..peers)
        .map(|n| {
            let (code, url) = (code.clone(), url.clone());
            let cache = directory.path().join(format!("guest{n}"));
            thread::sleep(Duration::from_millis(50));
            thread::spawn(move || join(&format!("Guest {n}"), &code, &url, &cache))
        })
        .collect();
    let guests: Vec<Arc<Open>> = joining
        .into_iter()
        .map(|joined| Arc::new(joined.join().unwrap()))
        .collect();
    println!("all joined in {:.1} s", began.elapsed().as_secs_f64());
    until("not everyone met", Duration::from_secs(300), || {
        guests.iter().all(|open| open.guest.peers().len() >= peers)
    });
    println!("all met in {:.1} s", began.elapsed().as_secs_f64());

    let (_, texts) = texts_of(&guests[0].section).unwrap();
    assert!(texts.len() >= typists, "a paragraph for each typist");
    for open in &guests {
        open.guest.set_presence(presence(open, 0, 0).unwrap());
    }

    // When each typist typed each key, and said its caret moved past it.
    let typed: Arc<Mutex<HashMap<(usize, usize), Instant>>> = Arc::default();
    let moved: Arc<Mutex<HashMap<(usize, usize), Instant>>> = Arc::default();
    let seen_keys: Arc<Mutex<Vec<Duration>>> = Arc::default();
    let seen_carets: Arc<Mutex<Vec<Duration>>> = Arc::default();
    let done = Arc::new(AtomicBool::new(false));
    let (relay_before, host_before, crowd_before) = (
        cost(relay.0.id()),
        cost(hosting.0.id()),
        cost(std::process::id()),
    );
    let bytes_before = health(&address);
    let started = Instant::now();

    let mut threads = Vec::new();
    for (n, open) in guests.iter().enumerate().take(typists) {
        let (open, typed, done) = (Arc::clone(open), Arc::clone(&typed), Arc::clone(&done));
        let moved = Arc::clone(&moved);
        threads.push(thread::spawn(move || {
            let pause = Duration::from_secs_f64(1.0 / KEYS_PER_SECOND);
            thread::sleep(pause.mul_f64(n as f64 / typists as f64));
            let mut count = 0;
            while !done.load(Ordering::Acquire) && count < 600 {
                open.section.events();
                let Some((space, mut texts)) = texts_of(&open.section) else {
                    continue;
                };
                let (text, before) = texts.swap_remove(n);
                let end = before.encode_utf16().count() as u32;
                let edit = Edit {
                    at: 134_000_000_000_000_000,
                    ops: vec![Op::Page {
                        space,
                        op: PageOp::Text {
                            text,
                            range: end..end,
                            with: key(n, count).to_string(),
                        },
                    }],
                };
                typed.lock().unwrap().insert((n, count), Instant::now());
                open.section.replica().apply("Typist", edit).unwrap();
                count += 1;
                if let Some(presence) = presence(&open, 0, count as u32) {
                    moved.lock().unwrap().insert((n, count), Instant::now());
                    open.guest.set_presence(presence);
                }
                thread::sleep(pause);
            }
        }));
    }
    for open in guests.iter().skip(typists).take(OBSERVERS) {
        let (open, typed, done) = (Arc::clone(open), Arc::clone(&typed), Arc::clone(&done));
        let moved = Arc::clone(&moved);
        let (keys, carets) = (Arc::clone(&seen_keys), Arc::clone(&seen_carets));
        threads.push(thread::spawn(move || {
            let mut keys_seen = std::collections::HashSet::new();
            let mut carets_seen = HashMap::new();
            while !done.load(Ordering::Acquire) {
                open.section.events();
                let now = Instant::now();
                if let Some((_, texts)) = texts_of(&open.section) {
                    let chars = texts.iter().flat_map(|(_, text)| text.chars());
                    for (typist, n) in chars.filter_map(typist_of) {
                        if keys_seen.insert((typist, n))
                            && let Some(at) = typed.lock().unwrap().get(&(typist, n))
                        {
                            keys.lock().unwrap().push(now - *at);
                        }
                    }
                }
                for peer in open.guest.peers() {
                    let Some(typist) = (peer.hello.name.strip_prefix("Guest "))
                        .and_then(|n| n.parse::<usize>().ok())
                        .filter(|n| *n < typists)
                    else {
                        continue;
                    };
                    let Some(count) = peer.presence.and_then(|p| p.caret).map(|c| c.focus.offset)
                    else {
                        continue;
                    };
                    let count = count as usize;
                    if count > 0
                        && carets_seen.insert(typist, count) != Some(count)
                        && let Some(at) = moved.lock().unwrap().get(&(typist, count))
                    {
                        carets.lock().unwrap().push(now - *at);
                    }
                }
                thread::sleep(LOOK);
            }
        }));
    }
    thread::sleep(window);
    done.store(true, Ordering::Release);
    let elapsed = started.elapsed().as_secs_f64();
    let (relay_after, host_after, crowd_after) = (
        cost(relay.0.id()),
        cost(hosting.0.id()),
        cost(std::process::id()),
    );
    let bytes_after = health(&address);
    for thread in threads {
        thread.join().unwrap();
    }
    let keys = typed.lock().unwrap().len();
    report(
        "keys, typist to observer",
        std::mem::take(&mut seen_keys.lock().unwrap()),
    );
    report(
        "carets, typist to observer",
        std::mem::take(&mut seen_carets.lock().unwrap()),
    );
    let delta = |name: &str| bytes_after[name] - bytes_before.get(name).copied().unwrap_or(0.0);
    let members = (peers + 1) as f64;
    println!(
        "relay passed on {:.0} KB/s in, {:.0} KB/s out: per peer {:.1} KB/s in, {:.1} KB/s out",
        delta("bytes_in") / elapsed / 1000.0,
        delta("bytes_out") / elapsed / 1000.0,
        delta("bytes_in") / elapsed / 1000.0 / members,
        delta("bytes_out") / elapsed / 1000.0 / members,
    );
    let load = |name: &str, (before, _): (f64, u64), (after, rss): (f64, u64)| {
        println!(
            "{name}: {:.0}% of a core, {:.1} MB resident",
            (after - before) / elapsed * 100.0,
            rss as f64 / 1024.0
        );
    };
    load("relay", relay_before, relay_after);
    load("host", host_before, host_after);
    load(&format!("{peers} guests"), crowd_before, crowd_after);

    // Every key typed should reach the host's file; conflict pages show as extra pages.
    thread::sleep(Duration::from_secs(5));
    let observed = texts_of(&guests[typists.min(peers - 1)].section).map_or(0, |(_, texts)| {
        let chars = texts.iter().flat_map(|(_, text)| text.chars());
        chars.filter_map(typist_of).count()
    });
    println!("{keys} keys typed, {observed} on an observer's page after 5 s");
    drop(hosting.0.stdin.take());
    for line in said.map_while(Result::ok) {
        println!("host: {line}");
    }
}
