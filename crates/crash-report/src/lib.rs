//! Local crash capture shared by the desktop and mobile hosts. Uploads belong to the host.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(unix)]
pub use native::record_signal;

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use web_time::Instant;

pub const ADDRESS: &std::ffi::CStr = c"https://snowbound.paperclover.net/crash";

/// The host's private report file.
#[cfg(not(target_arch = "wasm32"))]
static REPORT: OnceLock<PathBuf> = OnceLock::new();
/// The backend drawing and its adapter, as "Metal (Apple M2)".
pub static RENDERER: Mutex<String> = Mutex::new(String::new());
/// Names from the notebook catalog, hidden in panic reports.
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static STARTED: OnceLock<Instant> = OnceLock::new();
/// The first panic's report is kept; the ones it sets off would only hide it.
static KEPT: AtomicBool = AtomicBool::new(false);

/// Bounds on what a panic adds, so a report stays well under what the site takes.
const MESSAGE: usize = 2 << 10;
const FRAMES: usize = 48;
const BACKTRACE: usize = 32 << 10;
const NAMED: usize = 256;

/// Reports each panic on `system`, then hands the report to `then` to log.
pub fn hook(
    build: &str,
    platform: String,
    system: String,
    then: impl Fn(&str) + Send + Sync + 'static,
) {
    let heading = format!("Snowbound {build}, {platform}\nSystem: {system}\n");
    #[cfg(not(target_arch = "wasm32"))]
    let _ = native::HEADING.set(heading.clone());
    STARTED.get_or_init(Instant::now);
    let home = home();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload_as_str()
            .unwrap_or("Box<dyn Any>")
            .chars()
            .take(MESSAGE)
            .collect::<String>();
        let at = info.location().map(ToString::to_string).unwrap_or_default();
        // Another thread may hold the lock, or this one may have panicked holding it.
        let names = NAMES
            .try_lock()
            .map(|names| names.clone())
            .unwrap_or_default();
        let renderer = RENDERER
            .try_lock()
            .map(|name| name.clone())
            .unwrap_or_default();
        let thread = std::thread::current();
        let report = format!(
            "{heading}Renderer: {renderer}\nThread: {}\nUptime: {} s\n\
             Panic: {}\nAt: {}\n\nBacktrace:\n{}",
            scrub(thread.name().unwrap_or("unnamed"), &home, &names),
            STARTED
                .get()
                .map_or(0, |started| started.elapsed().as_secs()),
            scrub(&message, &home, &names),
            scrub(&at, &home, &[]),
            scrub(&frames(&backtrace()), &home, &[]),
        );
        if !KEPT.swap(true, Ordering::Relaxed) {
            keep(&report);
        }
        then(&report);
    }));
}

/// Hides `path`'s names, a notebook's or a section's within it, in reports from now on.
pub fn conceal(path: &str) {
    let Ok(mut names) = NAMES.lock() else {
        return;
    };
    for name in path.split(['/', '\\']) {
        let name = [".onetoc2", ".onepkg", ".one"]
            .iter()
            .find_map(|extension| name.strip_suffix(extension))
            .unwrap_or(name);
        if !name.is_empty() && names.len() < NAMED && !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    // Longer first, so a name holding another is hidden whole.
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
}

/// `text` with `home` as `~`, `names` as `<name>`, and every path but a source file's as
/// `<path>`.
fn scrub(text: &str, home: &str, names: &[String]) -> String {
    let mut text = if home.len() > 1 {
        text.replace(home, "~")
    } else {
        text.to_owned()
    };
    for name in names {
        if name.chars().count() >= 3 {
            text = text.replace(name.as_str(), "<name>");
            continue;
        }
        let mut hidden = String::with_capacity(text.len());
        let mut kept = 0;
        for (at, found) in text.match_indices(name) {
            let end = at + found.len();
            let word = |char: char| char.is_alphanumeric() || char == '_';
            if text[..at].chars().next_back().is_some_and(word)
                || text[end..].chars().next().is_some_and(word)
            {
                continue;
            }
            hidden.push_str(&text[kept..at]);
            hidden.push_str("<name>");
            kept = end;
        }
        hidden.push_str(&text[kept..]);
        text = hidden;
    }
    hide_paths(&text)
}

fn hide_paths(text: &str) -> String {
    let mut hidden = String::with_capacity(text.len());
    let mut rest = text;
    let mut previous = None;
    while let Some(next) = rest.chars().next() {
        let begins = matches!(
            previous,
            None | Some(' ' | '\t' | '\n' | '"' | '\'' | '`' | '(' | '[' | '{' | '=' | ',' | ':')
        ) && (rest.starts_with('/')
            || rest.starts_with("~/")
            || rest.starts_with("~\\")
            || rest.starts_with("\\\\")
            || rest.get(1..3) == Some(":\\") && next.is_ascii_alphabetic());
        if !begins {
            hidden.push(next);
            previous = Some(next);
            rest = &rest[next.len_utf8()..];
            continue;
        }
        let word = &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())];
        if word
            .trim_end_matches(|c: char| c.is_ascii_digit() || matches!(c, ':' | ',' | ')'))
            .ends_with(".rs")
        {
            hidden.push_str(word);
            previous = word.chars().last();
            rest = &rest[word.len()..];
            continue;
        }
        // A quoted path runs to its quote, spaces and all; another to a break in the sentence.
        let end = match previous {
            Some(quote @ ('"' | '\'' | '`')) => rest.find([quote, '\n']),
            _ => [": ", ", ", ")", "\n"]
                .iter()
                .filter_map(|stop| rest.find(stop))
                .min(),
        };
        hidden.push_str("<path>");
        previous = Some('>');
        rest = &rest[end.unwrap_or(rest.len())..];
    }
    hidden
}

/// `backtrace`'s frames after the panic machinery's, at most `FRAMES`.
fn frames(backtrace: &str) -> String {
    let starts = |line: &str| {
        let line = line.trim_start();
        line.split_once(": ").is_some_and(|(number, _)| {
            !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
        })
    };
    let lines: Vec<&str> = backtrace.lines().collect();
    let panicking = lines
        .iter()
        .rposition(|line| starts(line) && line.contains("panicking::"));
    let mut kept = String::new();
    let mut count = 0;
    for line in &lines[panicking.map_or(0, |at| at + 1)..] {
        if starts(line) {
            // A system library's frame, which says nothing without its symbols.
            if line.ends_with(": <unknown>") {
                continue;
            }
            count += 1;
            if count > FRAMES || kept.len() > BACKTRACE {
                kept.push_str("   …\n");
                break;
            }
        } else if count == 0 {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

#[cfg(not(target_arch = "wasm32"))]
fn backtrace() -> String {
    std::backtrace::Backtrace::force_capture().to_string()
}

/// The browser's stack, as wasm32-unknown-unknown's std has no backtrace.
#[cfg(target_arch = "wasm32")]
fn backtrace() -> String {
    let error = js_sys::Error::new("");
    js_sys::Reflect::get(&error, &"stack".into())
        .ok()
        .and_then(|stack| stack.as_string())
        .unwrap_or_default()
        .lines()
        .enumerate()
        .map(|(number, line)| format!("{number:4}: {}\n", line.trim()))
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn home() -> String {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    home.map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
fn home() -> String {
    String::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn keep(report: &str) {
    use std::io::Write;
    if let Some(path) = REPORT.get() {
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        if let Ok(mut file) = options.open(path) {
            let _ = file.write_all(report.as_bytes());
            let _ = file.sync_all();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn kept() -> Option<String> {
    std::fs::read_to_string(REPORT.get()?).ok()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn forget() {
    if let Some(path) = REPORT.get() {
        let _ = std::fs::remove_file(path);
    }
}

/// The browser keeps the report in local storage, which a panic can still reach.
#[cfg(target_arch = "wasm32")]
const STORED: &str = "snowbound-crash";

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

#[cfg(target_arch = "wasm32")]
fn keep(report: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(STORED, report);
    }
}

#[cfg(target_arch = "wasm32")]
pub fn kept() -> Option<String> {
    storage()?.get_item(STORED).ok()?
}

#[cfg(target_arch = "wasm32")]
pub fn forget() {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(STORED);
    }
}

/// Keeps reports at `path`; installs the host's native fault capture without replacing a pending report.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_path(path: PathBuf) -> std::io::Result<()> {
    if REPORT.get().is_some() {
        return Ok(());
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    native::prepare(&path)?;
    let _ = REPORT.set(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_hide_the_home_folder_paths_and_names() {
        let names = vec!["Work Notes".to_owned(), "Meetings".to_owned()];
        let scrub = |text| scrub(text, "/Users/ada", &names);
        assert_eq!(
            scrub(
                r#"called `Result::unwrap()` on an `Err` value: Io { path: "/Users/ada/OneNote Notebooks/Work Notes/To Do.one", kind: NotFound }"#
            ),
            r#"called `Result::unwrap()` on an `Err` value: Io { path: "<path>", kind: NotFound }"#
        );
        assert_eq!(
            scrub("Cannot read /Volumes/Share/Shared Notes/a.one: denied"),
            "Cannot read <path>: denied"
        );
        assert_eq!(
            scrub(r"Cannot read C:\Users\ada\Notes\b.one, retrying"),
            "Cannot read <path>, retrying"
        );
        assert_eq!(scrub("opening smb://nas/notes/x.one"), "opening smb:<path>");
        assert_eq!(
            scrub("section Meetings of Work Notes is locked"),
            "section <name> of <name> is locked"
        );
        // Sources stay, the home folder as ~.
        assert_eq!(
            scrub("at /Users/ada/.cargo/registry/src/winit-0.30/src/lib.rs:12:5"),
            "at ~/.cargo/registry/src/winit-0.30/src/lib.rs:12:5"
        );
        assert_eq!(
            scrub("index out of bounds: the len is 3 but the index is 5"),
            "index out of bounds: the len is 3 but the index is 5"
        );
    }

    #[test]
    fn names_are_kept_from_a_sections_path() {
        conceal("Projects/Snow Plan.one");
        let names = NAMES.lock().unwrap().clone();
        assert!(names.contains(&"Projects".to_owned()), "{names:?}");
        assert!(names.contains(&"Snow Plan".to_owned()), "{names:?}");
        assert!(!names.iter().any(|name| name.ends_with(".one")));
    }

    #[test]
    fn short_names_are_hidden_without_breaking_diagnostics() {
        let names = vec!["AI".into(), "x".into(), "日".into()];
        assert_eq!(
            scrub("section 'AI': FAIL index x 日", "", &names),
            "section '<name>': FAIL index <name> <name>"
        );
    }

    #[test]
    fn backtraces_start_past_the_panic() {
        let backtrace = "   0: std::backtrace::Backtrace::force_capture
   1: snowbound::crash::hook::{{closure}}
             at ./src/crash.rs:30:9
   2: std::panicking::rust_panic_with_hook
   3: core::panicking::panic_fmt
   4: snowbound::State::frame
             at ./src/main.rs:1500:13
   5: <unknown>
   6: main
";
        assert_eq!(
            frames(backtrace),
            "   4: snowbound::State::frame\n             at ./src/main.rs:1500:13\n   6: main\n"
        );
        let deep: String = (0..100)
            .map(|frame| format!("{frame:4}: f{frame}\n"))
            .collect();
        let kept = frames(&deep);
        assert_eq!(kept.lines().count(), FRAMES + 1);
        assert!(kept.ends_with("…\n"));
    }

    /// A report written by a panic is what the next launch finds, until it is forgotten.
    #[test]
    fn a_report_outlives_the_run_until_answered() {
        let folder = std::env::temp_dir().join(format!("snowbound-crash-{}", std::process::id()));
        let _ = REPORT.set(folder.join("crash.txt"));
        let report = REPORT.get().unwrap();
        let _ = std::fs::remove_file(report);
        assert_eq!(kept(), None);
        keep("Snowbound development\nPanic: x");
        assert_eq!(kept().as_deref(), Some("Snowbound development\nPanic: x"));
        forget();
        assert_eq!(kept(), None);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
