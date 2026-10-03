//! `snowbound-site`, snowbound.paperclover.net: the hosted web build's files from a folder,
//! and for a path that is a Live Share code (`/7KQ-4MZ-9XR`) a page that opens it in
//! Snowbound, or in the web build where `Site::web` says it joins. A build's module and
//! JavaScript sit in `b/<hash>/`, named by their contents, so they are kept for good;
//! `index.html`, which names them, is checked on every load. `POST /crash` keeps a crash
//! report Snowbound sends as a file of its own; everything else is GET and HEAD. A proxy in
//! front terminates TLS.

use crate::{code, ws};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, BufReader, Read, Write},
    net::{IpAddr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime},
};

/// The largest crash report kept.
pub const REPORT: usize = 64 << 10;
/// Reports kept an hour from one address (an IPv6 /64), and from everyone.
pub const PER_ADDRESS: usize = 10;
pub const PER_HOUR: usize = 500;
const HOUR: Duration = Duration::from_secs(60 * 60);

/// What the site serves.
#[derive(Clone, Debug)]
pub struct Site {
    /// The web build's folder; `/` is its `index.html`.
    pub root: PathBuf,
    /// Where Open in Web goes, the code appended (`https://snowbound.paperclover.net/?join=`);
    /// none offers only Snowbound.
    pub web: Option<String>,
    /// Where each crash report is kept, a file apiece.
    pub crashes: PathBuf,
    /// Counts senders by the address the proxy in front gives in `X-Forwarded-For`.
    pub trust_forwarded: bool,
}

/// When the reports of the last hour came, by sender and in all.
#[derive(Default)]
struct Received {
    by_address: HashMap<IpAddr, VecDeque<Instant>>,
    all: VecDeque<Instant>,
}

impl Received {
    /// Counts a report from `address` at `now`, unless it is one too many.
    fn admit(&mut self, address: IpAddr, now: Instant) -> bool {
        let recent = |times: &mut VecDeque<Instant>| {
            while times.front().is_some_and(|&time| now - time >= HOUR) {
                times.pop_front();
            }
        };
        recent(&mut self.all);
        self.by_address.retain(|_, times| {
            recent(times);
            !times.is_empty()
        });
        let sent = self.by_address.entry(address).or_default();
        if sent.len() >= PER_ADDRESS || self.all.len() >= PER_HOUR {
            return false;
        }
        sent.push_back(now);
        self.all.push_back(now);
        true
    }
}

/// Serves `site` on `listener` until it fails.
pub fn serve(listener: TcpListener, site: Site) -> io::Result<()> {
    let shared = Arc::new((site, Mutex::default()));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            let (site, received) = &*shared;
            let _ = answer(site, received, stream);
        });
    }
    Ok(())
}

fn answer(site: &Site, received: &Mutex<Received>, stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(&stream);
    let head = ws::head(&mut reader)?;
    let mut words = head.split(' ');
    let (method, target) = (
        words.next().unwrap_or_default(),
        words.next().unwrap_or("/"),
    );
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let (status, kind, body) = if (method, path) == ("POST", "/crash") {
        let address = crate::peer(&stream, &head, site.trust_forwarded);
        let admit = || {
            received
                .lock()
                .is_ok_and(|mut received| received.admit(address, Instant::now()))
        };
        let status = keep_crash(&site.crashes, &head, &mut reader, admit);
        (
            status,
            "text/plain",
            format!("{}\n", &status[4..]).into_bytes(),
        )
    } else if !matches!(method, "GET" | "HEAD") {
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
    let cache = if method == "POST" || !status.starts_with("200") || kind.starts_with("text/html") {
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

/// Keeps the report a `POST /crash` with `head` brings in `reader` in `folder`, where it is
/// small enough, plain text or JSON, and `admit` lets it in; answers the status.
fn keep_crash(
    folder: &Path,
    head: &str,
    reader: &mut impl Read,
    admit: impl FnOnce() -> bool,
) -> &'static str {
    let kind = ws::header(head, "Content-Type").unwrap_or_default();
    let kind = kind.split(';').next().unwrap_or_default().trim();
    let extension = if kind.eq_ignore_ascii_case("text/plain") {
        "txt"
    } else if kind.eq_ignore_ascii_case("application/json") {
        "json"
    } else {
        return "415 Unsupported Media Type";
    };
    if ws::header(head, "Transfer-Encoding").is_some() {
        return "411 Length Required";
    }
    let Some(length) = ws::header(head, "Content-Length").and_then(|value| value.parse().ok())
    else {
        return "411 Length Required";
    };
    if length == 0 {
        return "400 Bad Request";
    }
    if length > REPORT {
        return "413 Content Too Large";
    }
    if !admit() {
        return "429 Too Many Requests";
    }
    let mut report = vec![0; length];
    if reader.read_exact(&mut report).is_err() || std::str::from_utf8(&report).is_err() {
        return "400 Bad Request";
    }
    let mut tag = [0; 4];
    let _ = getrandom::fill(&mut tag);
    let tag: String = tag.iter().map(|byte| format!("{byte:02x}")).collect();
    let file = folder.join(format!(
        "{}-{tag}.{extension}",
        timestamp(SystemTime::now())
    ));
    let kept = std::fs::create_dir_all(folder).and_then(|()| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)?
            .write_all(&report)
    });
    match kept {
        Ok(()) => "200 OK",
        Err(error) => {
            eprintln!("Cannot keep a crash report in {}: {error}", file.display());
            "500 Internal Server Error"
        }
    }
}

/// `time` in UTC as `2026-10-03T21-04-05Z`, which sorts as it reads and names a file anywhere.
fn timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    // Howard Hinnant's civil_from_days, from 1970-01-01.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let of_era = shifted % 146_097;
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}Z",
        of_day / 3_600,
        of_day / 60 % 60,
        of_day % 60
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_dates_and_times() {
        let at = |seconds| timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds));
        assert_eq!(at(0), "1970-01-01T00-00-00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00-00-00Z");
        assert_eq!(at(1_791_072_245), "2026-10-04T00-04-05Z");
    }
}
