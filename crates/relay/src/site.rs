//! `snowbound-site`, snowbound.paperclover.net: the hosted web build's files from a folder,
//! and for a path that is a Live Share code (`/7KQ-4MZ-9XR`) a page that opens it in
//! Snowbound, or in the web build where `Site::web` says it joins. A build's module and
//! JavaScript sit in `b/<hash>/`, named by their contents, so they are kept for good;
//! `index.html`, which names them, is checked on every load. GET and HEAD only; a proxy in
//! front terminates TLS.

use crate::{code, ws};
use std::{
    io::{self, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

/// What the site serves.
#[derive(Clone, Debug)]
pub struct Site {
    /// The web build's folder; `/` is its `index.html`.
    pub root: PathBuf,
    /// Where Open in Web goes, the code appended (`https://snowbound.paperclover.net/?join=`);
    /// none offers only Snowbound.
    pub web: Option<String>,
}

/// Serves `site` on `listener` until it fails.
pub fn serve(listener: TcpListener, site: Site) -> io::Result<()> {
    let site = Arc::new(site);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let site = Arc::clone(&site);
        thread::spawn(move || {
            let _ = answer(&site, stream);
        });
    }
    Ok(())
}

fn answer(site: &Site, stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let head = ws::head(&mut BufReader::new(&stream))?;
    let mut words = head.split(' ');
    let (method, target) = (
        words.next().unwrap_or_default(),
        words.next().unwrap_or("/"),
    );
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let (status, kind, body) = if !matches!(method, "GET" | "HEAD") {
        (
            "405 Method Not Allowed",
            "text/plain",
            b"GET only\n".to_vec(),
        )
    } else if let Some((number, secret)) = code::parse(path.trim_matches('/')) {
        let shown = code::format(number, &secret).unwrap_or_default();
        (
            "200 OK",
            "text/html; charset=utf-8",
            landing(&shown, site).into_bytes(),
        )
    } else {
        match file(&site.root, path) {
            Some(file) => match std::fs::read(&file) {
                Ok(bytes) => ("200 OK", content_type(&file), bytes),
                Err(_) => ("404 Not Found", "text/plain", b"Not found\n".to_vec()),
            },
            None => ("404 Not Found", "text/plain", b"Not found\n".to_vec()),
        }
    };
    let cache = if !status.starts_with("200") || kind.starts_with("text/html") {
        "no-cache"
    } else if path.starts_with("/b/") {
        "public, max-age=31536000, immutable"
    } else {
        // Fonts and dictionaries.
        "public, max-age=86400"
    };
    let mut writer = &stream;
    write!(
        writer,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: {cache}\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if method != "HEAD" {
        writer.write_all(&body)?;
    }
    Ok(())
}

/// The file `path` names in `root`, `index.html` for a folder; none outside it.
fn file(root: &Path, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path.trim_start_matches('/'));
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || path.contains(['%', '\\', '\0'])
    {
        return None;
    }
    let mut file = root.join(relative);
    if file.is_dir() {
        file = file.join("index.html");
    }
    file.is_file().then_some(file)
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("gz") => "application/gzip",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The page a shared notebook's link opens: the code, and the ways to open it.
fn landing(shown: &str, site: &Site) -> String {
    let web = site.web.as_ref().map_or_else(String::new, |web| {
        format!(r#"<a class="other" href="{web}{shown}">Open in Web</a>"#)
    });
    format!(
        r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Shared notebook {shown} · Snowbound</title>
<style>
  :root {{ color-scheme: light dark; font-family: system-ui, sans-serif; }}
  body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; background: Canvas; color: CanvasText; }}
  main {{ max-width: 26rem; padding: 2rem; text-align: center; }}
  h1 {{ font-size: 1.3rem; font-weight: 600; margin: 0 0 .5rem; }}
  p {{ margin: .5rem 0 1.5rem; opacity: .75; }}
  code {{ display: block; font: 600 2rem ui-monospace, monospace; letter-spacing: .08em; margin: 1rem 0 1.5rem; }}
  a {{ display: inline-block; margin: .25rem; padding: .6rem 1.1rem; border-radius: .5rem; text-decoration: none; font-weight: 600; }}
  .open {{ background: #3768c7; color: white; }}
  .other {{ border: 1px solid #3768c788; color: inherit; }}
</style>
<main>
  <h1>Someone shared a notebook with you</h1>
  <code>{shown}</code>
  <a class="open" href="snowbound://join/{shown}">Open in Snowbound</a>
  {web}
  <p>If Snowbound doesn’t open, open it yourself, choose Open Shared Notebook, and enter this code.</p>
</main>
</html>
"#
    )
}
