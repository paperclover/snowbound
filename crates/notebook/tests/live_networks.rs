//! Live Share on hostile networks: through a proxy that asks for a password, through one that
//! refuses WebSockets (the relay is then reached by plain requests), and the troubles named
//! when the relay can't be reached. One test, as the proxy chosen holds for the process.
#![cfg(feature = "live")]

use notebook::live::{
    Trouble,
    proxy::{self, Proxy},
    share::{self, Refusal, Sharing},
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

#[path = "support/live.rs"]
mod live;
use live::*;

/// What a proxy on this computer did.
#[derive(Default)]
struct Seen {
    tunnels: AtomicUsize,
    refused: AtomicUsize,
}

/// An HTTP proxy that tunnels `CONNECT`s, asking for `credentials` where given and, with
/// `block_websockets`, refusing a WebSocket's upgrade inside the tunnel, as a proxy that
/// inspects HTTPS does.
fn proxy(credentials: Option<&'static str>, block_websockets: bool) -> (SocketAddr, Arc<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Seen::default());
    let counting = Arc::clone(&seen);
    thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let seen = Arc::clone(&counting);
            thread::spawn(move || {
                let _ = tunnel(client, credentials, block_websockets, &seen);
            });
        }
    });
    (address, seen)
}

fn tunnel(
    mut client: TcpStream,
    credentials: Option<&str>,
    block_websockets: bool,
    seen: &Seen,
) -> io::Result<()> {
    let head = relay::ws::head(&mut client)?;
    let target = head.split(' ').nth(1).unwrap_or_default().to_owned();
    let authorized = credentials
        .is_none_or(|expected| relay::ws::header(&head, "Proxy-Authorization") == Some(expected));
    if !head.starts_with("CONNECT ") || !authorized {
        client.write_all(
            b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\n\
              Content-Length: 0\r\n\r\n",
        )?;
        return Ok(());
    }
    let mut upstream = TcpStream::connect(&target)?;
    client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
    seen.tunnels.fetch_add(1, Ordering::Relaxed);
    if block_websockets {
        let request = relay::ws::head(&mut client)?;
        if relay::ws::header(&request, "Upgrade").is_some() {
            seen.refused.fetch_add(1, Ordering::Relaxed);
            client.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")?;
            return Ok(());
        }
        upstream.write_all(request.as_bytes())?;
    }
    let (mut from, mut to) = (client.try_clone()?, upstream.try_clone()?);
    thread::spawn(move || {
        let _ = io::copy(&mut from, &mut to);
        let _ = to.shutdown(std::net::Shutdown::Both);
    });
    let mut buffer = [0; 16 << 10];
    loop {
        let length = upstream.read(&mut buffer)?;
        if length == 0 {
            return Ok(());
        }
        client.write_all(&buffer[..length])?;
    }
}

fn through(address: SocketAddr, credentials: Option<(&str, &str)>) {
    proxy::use_proxy(Some(Some(Proxy {
        host: address.ip().to_string(),
        port: address.port(),
        credentials: credentials.map(|(name, password)| (name.into(), password.into())),
    })));
}

/// Shares a notebook and has a guest join it and publish an edit, all through `url`.
fn share_and_edit(directory: &std::path::Path, url: &str) {
    let folder = notebook(directory);
    let host = host(
        &folder,
        &directory.join("host"),
        &Sharing::new("").unwrap(),
        url,
    );
    let (guest, notebook) = guest("Grace", &code(&host), url, &directory.join("grace"));
    let section = open(&notebook, &guest, "Garden.one", None);
    let file = folder.join("Garden.one");
    let id = replace(&section, &std::fs::read(&file).unwrap(), 0..8, "Through");
    published(&section, id);
    assert_eq!(
        server::text(&std::fs::read(&file).unwrap()).2,
        "Through text"
    );
}

fn refusal(code: &str, url: &str) -> Refusal {
    share::join(hello("Mallory"), code, "", None, Some(url)).unwrap_err()
}

#[test]
fn hostile_networks() {
    let directory = tempfile::tempdir().unwrap();
    let url = relay(Default::default());
    let code = notebook::live::code::format(412, "4MZ9XR").unwrap();

    // A proxy that asks for a password: refused without, through with it.
    let (address, seen) = proxy(Some("Basic YWRhOnNlY3JldA=="), false);
    through(address, None);
    assert_eq!(
        refusal(&code, &url),
        Refusal::Unreachable(Trouble::ProxyAuthentication)
    );
    through(address, Some(("ada", "secret")));
    share_and_edit(&directory.path().join("connect"), &url);
    assert!(
        seen.tunnels.load(Ordering::Relaxed) >= 3,
        "host, guest and joiner tunnel"
    );

    // A proxy that refuses WebSockets: the relay is reached by requests instead.
    let (address, seen) = proxy(None, true);
    through(address, None);
    share_and_edit(&directory.path().join("blocked"), &url);
    assert!(seen.refused.load(Ordering::Relaxed) >= 1);

    // A proxy that isn't there, and a relay whose name doesn't resolve.
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    through(closed, None);
    assert_eq!(
        refusal(&code, &url),
        Refusal::Unreachable(Trouble::ProxyUnreachable)
    );
    proxy::use_proxy(Some(None));
    assert_eq!(
        refusal(&code, "wss://relay.invalid"),
        Refusal::Unreachable(Trouble::Dns)
    );
}
