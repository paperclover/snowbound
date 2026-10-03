//! The site as shipped: a code's page, the web build's files, nothing outside them.

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
        let mut stream = TcpStream::connect(self.address).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: site\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
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
