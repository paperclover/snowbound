//! The site as shipped: a code's page, the web build's files, nothing outside them, and crash
//! reports kept.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    process::{Child, Command, Stdio},
};

struct Site {
    child: Child,
    address: SocketAddr,
}

impl Site {
    fn start(root: &std::path::Path, options: &[&str]) -> Site {
        let mut child = Command::new(env!("CARGO_BIN_EXE_snowbound-site"))
            .args(["--listen", "127.0.0.1:0", "--root"])
            .arg(root)
            .args(options)
            .env_clear()
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line
            .split(' ')
            .nth(4)
            .unwrap()
            .trim_end_matches(',')
            .parse()
            .unwrap();
        Site { child, address }
    }

    fn get(&self, path: &str) -> String {
        self.send(format!("GET {path} HTTP/1.1\r\nHost: site\r\n\r\n").as_bytes())
    }

    /// The status line answering a crash report of `body` as `kind`, with `extra` headers.
    fn report(&self, kind: &str, body: &str, extra: &str) -> String {
        let request = format!(
            "POST /crash HTTP/1.1\r\nHost: site\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{extra}\r\n{body}",
            body.len()
        );
        let response = self.send(request.as_bytes());
        assert!(!response.contains(body), "{response}");
        response.lines().next().unwrap().to_owned()
    }

    fn send(&self, request: &[u8]) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.write_all(request).unwrap();
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        String::from_utf8_lossy(&response).into_owned()
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn a_code_gets_its_page_and_the_web_build_its_files() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("web");
    std::fs::create_dir_all(root.join("b/0f3a")).unwrap();
    std::fs::create_dir_all(root.join("fonts")).unwrap();
    std::fs::write(root.join("index.html"), "<p>the app</p>").unwrap();
    std::fs::write(root.join("b/0f3a/snowbound_bg.wasm"), b"\0asm").unwrap();
    std::fs::write(root.join("fonts/Face.ttf"), b"font").unwrap();
    std::fs::write(folder.path().join("secret.txt"), "secret").unwrap();
    let code = relay::code::format(412, "4MZ9XR").unwrap();
    let site = Site::start(&root, &[]);
    let page = site.get(&format!("/{code}"));
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains(&format!("snowbound://join/{code}")), "{page}");
    assert!(!page.contains("Open in Web"), "no web build joins yet");
    // Typed loosely, the code's page names it as shown.
    let loose = site.get(&format!("/{}", code.to_lowercase().replace('-', "")));
    assert!(loose.contains(&format!("<code>{code}</code>")), "{loose}");
    // The page that names a build is checked every load; a build is kept for good.
    let index = site.get("/");
    assert!(
        index.ends_with("<p>the app</p>") && index.contains("no-cache"),
        "{index}"
    );
    let wasm = site.get("/b/0f3a/snowbound_bg.wasm");
    assert!(wasm.contains("Content-Type: application/wasm"), "{wasm}");
    assert!(wasm.contains("max-age=31536000, immutable"), "{wasm}");
    assert!(site.get("/fonts/Face.ttf").contains("max-age=86400"));
    let missing = site.get("/b/9999/snowbound_bg.wasm");
    assert!(
        missing.starts_with("HTTP/1.1 404") && missing.contains("no-cache"),
        "{missing}"
    );
    for outside in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/b/../../secret.txt",
    ] {
        assert!(site.get(outside).starts_with("HTTP/1.1 404"), "{outside}");
    }
    // A mistyped code is no code: it is looked for as a file.
    let typo = format!("/8{}", &code[1..]);
    assert!(site.get(&typo).starts_with("HTTP/1.1 404"));
    drop(site);
    let site = Site::start(
        &root,
        &["--web", "https://snowbound.paperclover.net/?join="],
    );
    let page = site.get(&format!("/{code}"));
    assert!(
        page.contains(&format!("https://snowbound.paperclover.net/?join={code}")),
        "{page}"
    );
}

#[test]
fn crash_reports_are_kept_small_and_few_and_never_echoed() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("web");
    std::fs::create_dir_all(&root).unwrap();
    let site = Site::start(&root, &["--trust-forwarded", "true"]);
    let report = "Snowbound 2026-10-03-r46\npanicked at crates/canvas/src/view.rs:1:1";
    assert_eq!(
        site.report("text/plain; charset=utf-8", report, ""),
        "HTTP/1.1 200 OK"
    );
    // Kept beside the root, as sent, under a name that sorts by when it came.
    let crashes = folder.path().join("crashes");
    let kept: Vec<_> = std::fs::read_dir(&crashes)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), report);
    let name = kept[0].file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("20") && name.ends_with(".txt"), "{name}");

    assert_eq!(
        site.report("application/json", r#"{"panic":"x"}"#, ""),
        "HTTP/1.1 200 OK"
    );
    assert_eq!(
        site.report("text/html", "<p>no</p>", ""),
        "HTTP/1.1 415 Unsupported Media Type"
    );
    // Refused from its head alone, before any of it is read.
    let large = site.send(
        format!(
            "POST /crash HTTP/1.1\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
            relay::site::REPORT + 1
        )
        .as_bytes(),
    );
    assert!(large.starts_with("HTTP/1.1 413"), "{large}");
    let chunked = site.send(
        b"POST /crash HTTP/1.1\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
    );
    assert!(chunked.starts_with("HTTP/1.1 411"), "{chunked}");
    assert_eq!(site.report("text/plain", "\u{fffd}", ""), "HTTP/1.1 200 OK");
    let invalid = site.send(
        b"POST /crash HTTP/1.1\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n\xff\xfe",
    );
    assert!(invalid.starts_with("HTTP/1.1 400"), "{invalid}");
    let empty =
        site.send(b"POST /crash HTTP/1.1\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n");
    assert!(empty.starts_with("HTTP/1.1 400"), "{empty}");

    // One sender is held to a few an hour; another, by the proxy's word, still gets in.
    let sent = (0..relay::site::PER_ADDRESS)
        .map(|_| site.report("text/plain", "again", ""))
        .filter(|status| status == "HTTP/1.1 200 OK")
        .count();
    assert_eq!(sent, relay::site::PER_ADDRESS - 4);
    assert_eq!(
        site.report("text/plain", "again", ""),
        "HTTP/1.1 429 Too Many Requests"
    );
    assert_eq!(
        site.report(
            "text/plain",
            "elsewhere",
            "X-Forwarded-For: 203.0.113.9\r\n"
        ),
        "HTTP/1.1 200 OK"
    );
    assert_eq!(
        std::fs::read_dir(&crashes).unwrap().count(),
        relay::site::PER_ADDRESS
    );
    assert!(site.get("/crash").starts_with("HTTP/1.1 404"));
}
