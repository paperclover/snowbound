#[path = "../src/flush.rs"]
mod flush;

use onestore::{
    CommitState, ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use serde_json::json;
use std::{
    env, fs,
    io::{self, Write},
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "init" {
        let bytes =
            onestore::create_section("synthetic.one", "Concurrent edits:", "Concurrency test")?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[1])?;
        file.write_all(&bytes)?;
        flush::flush(&file)?;
        return Ok(());
    }
    if args.len() != 7 || !["read", "write", "edit"].contains(&args[0].as_str()) {
        return Err("Usage: concurrent_client init FILE | read|write|edit FILE ACTOR OPERATIONS START_FILE STOP_FILE SEED".into());
    }
    let mut random: u64 = args[6].parse()?;
    let operations: usize = args[3].parse()?;
    if operations == 0 {
        return Err("Choose at least one operation.".into());
    }
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut output = io::stdout().lock();
    let mut log = |event: serde_json::Value| -> io::Result<()> {
        writeln!(output, "{event}")?;
        output.flush()
    };
    log(json!({"event": "ready", "pid": std::process::id(), "actor": args[2]}))?;
    while !Path::new(&args[4]).exists() {
        if Instant::now() > deadline {
            return Err("Start barrier timed out.".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let mut completed = 0;
    let mut attempts = 0;
    while completed < operations || (args[0] == "read" && !Path::new(&args[5]).exists()) {
        if Instant::now() > deadline {
            return Err("Concurrent client timed out.".into());
        }
        attempts += 1;
        random = random
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        thread::sleep(Duration::from_millis((random >> 32) % 7));
        let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
        let source = match onestore::read_file(&args[1]) {
            Ok(source) => source,
            Err(error)
                if [
                    io::ErrorKind::WouldBlock,
                    io::ErrorKind::PermissionDenied,
                    io::ErrorKind::NotFound,
                ]
                .contains(&error.kind()) =>
            {
                log(
                    json!({"event": "read_busy", "attempt": attempts, "kind": format!("{:?}", error.kind())}),
                )?;
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(error) => {
                log(
                    json!({"event": "read_error", "attempt": attempts, "kind": format!("{:?}", error.kind())}),
                )?;
                return Err(error.into());
            }
        };
        let preserve = |error: onestore::Error| {
            let path = Path::new(&args[4])
                .parent()
                .unwrap()
                .join(format!("invalid-{}-{attempts}.one", std::process::id()));
            if let Err(failure) = fs::write(&path, &source) {
                eprintln!(
                    "Could not save invalid snapshot {}: {failure}",
                    path.display()
                );
            }
            error
        };
        let store = Store::parse(&source).map_err(preserve)?;
        if !store.checksum_mismatches.is_empty() {
            return Err(preserve(onestore::Error {
                offset: store.checksum_mismatches[0],
                message: "A reader observed transaction checksum damage.",
            })
            .into());
        }
        let index = RevisionIndex::parse(&store).map_err(preserve)?;
        index.validate_current().map_err(preserve)?;
        let document = Document::parse(&index).map_err(preserve)?;
        let mut targets = Vec::new();
        for (sid, page) in document.pages().map_err(preserve)? {
            let space = &document.spaces[&sid];
            let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
            let mut pending = vec![page];
            let mut seen = std::collections::BTreeSet::new();
            while let Some(oid) = pending.pop() {
                if !seen.insert(oid) {
                    continue;
                }
                let node = &revision.nodes[&oid];
                pending.extend(
                    node.children
                        .iter()
                        .chain(&node.content)
                        .chain(&node.structure)
                        .copied(),
                );
                if let Kind::RichText { text, .. } = &node.kind
                    && text.starts_with("Concurrent edits:")
                {
                    revision.text_runs(oid).map_err(preserve)?;
                    targets.push((sid, oid, text));
                }
            }
        }
        let [(sid, oid, text)] = targets.as_slice() else {
            return Err(preserve(onestore::Error {
                offset: 0,
                message: "Expected one concurrent-edit paragraph.",
            })
            .into());
        };
        let read_finished = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
        log(
            json!({"event": "read", "attempt": attempts, "started_us": started, "finished_us": read_finished,
            "transaction": store.header.transaction_count, "text": text}),
        )?;
        if args[0] == "read" {
            completed += 1;
            continue;
        }
        let token = format!(" [{}:{}]", args[2], completed);
        let offset = u32::try_from(text.encode_utf16().count())?;
        let mut range = offset..offset;
        let mut replacement = token.clone();
        if args[0] == "edit" {
            let prefix = "Concurrent edits:";
            let mut boundaries = vec![u32::try_from(prefix.encode_utf16().count())?];
            for character in text[prefix.len()..].chars() {
                boundaries.push(boundaries.last().unwrap() + character.len_utf16() as u32);
            }
            let first = ((random >> 16) % boundaries.len() as u64) as usize;
            let second = ((random >> 40) % boundaries.len() as u64) as usize;
            range = boundaries[first.min(second)]..boundaries[first.max(second)];
            replacement = format!(" café 🦀{token}");
        }
        log(
            json!({"event": "intent", "attempt": attempts, "operation": completed,
            "source_transaction": store.header.transaction_count, "before": text,
            "range": [range.start, range.end], "replacement": replacement, "token": token}),
        )?;
        thread::sleep(Duration::from_millis((random >> 48) % 13));
        let commit_started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
        let result = onestore::commit_file_text(&args[1], &source, *sid, *oid, range, &replacement);
        let finished = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
        match result {
            Ok(()) => {
                log(
                    json!({"event": "commit", "attempt": attempts, "operation": completed, "token": token,
                    "started_us": commit_started, "finished_us": finished, "source_transaction": store.header.transaction_count}),
                )?;
                completed += 1;
            }
            Err(error)
                if error.state == CommitState::NotCommitted
                    && [
                        io::ErrorKind::WouldBlock,
                        io::ErrorKind::ResourceBusy,
                        io::ErrorKind::PermissionDenied,
                        io::ErrorKind::NotFound,
                    ]
                    .contains(&error.error.kind()) =>
            {
                log(
                    json!({"event": "retry", "attempt": attempts, "started_us": commit_started,
                    "finished_us": finished, "kind": format!("{:?}", error.error.kind())}),
                )?;
            }
            Err(error) => {
                log(
                    json!({"event": "commit_error", "attempt": attempts, "operation": completed,
                    "token": token, "state": format!("{:?}", error.state), "kind": format!("{:?}", error.error.kind()),
                    "started_us": commit_started, "finished_us": finished}),
                )?;
                return Err(error.into());
            }
        }
    }
    log(json!({"event": "done", "completed": completed, "attempts": attempts}))?;
    Ok(())
}
