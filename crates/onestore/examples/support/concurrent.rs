use onestore::{
    CommitState, ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use serde_json::json;
use std::{
    fs,
    io::{self, Write},
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn document_view(bytes: &[u8]) -> Result<serde_json::Value, onestore::Error> {
    let store = Store::parse(bytes)?;
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let mut texts = serde_json::Map::new();
    for (sid, _) in document.pages()? {
        let space = &document.spaces[&sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (id, node) in &revision.nodes {
            if let Kind::RichText { text, .. } = &node.kind
                && text.starts_with("Document w")
            {
                let runs=revision.text_runs(*id)?.into_iter().map(|run|json!({"text":run.text,"bold":run.format.bold.unwrap_or(false),"size":run.format.font_size,"color":run.format.color.unwrap_or(0xff000000)})).collect::<Vec<_>>();
                texts.insert(id.to_string(), json!({"text":text,"runs":runs}));
            }
        }
    }
    Ok(texts.into())
}

pub fn run(
    args: &[String],
    mut read: impl FnMut(&str) -> io::Result<Vec<u8>>,
    mut commit: impl FnMut(
        &str,
        &[u8],
        ExGuid,
        ExGuid,
        std::ops::Range<u32>,
        &str,
    ) -> Result<(), onestore::CommitError>,
) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 7 || !["read", "write", "edit"].contains(&args[0].as_str()) {
        return Err(
            "Expected read|write|edit FILE ACTOR OPERATIONS START_FILE STOP_FILE SEED.".into(),
        );
    }
    let mut random: u64 = args[6].parse()?;
    let operations: usize = args[3].parse()?;
    if operations == 0 {
        return Err("Choose at least one operation.".into());
    }
    let timeout = match std::env::var("ONESTORE_CLIENT_TIMEOUT_MS") {
        Ok(value) => value.parse::<u64>()?,
        Err(std::env::VarError::NotPresent) => 600_000,
        Err(error) => return Err(error.into()),
    };
    if timeout == 0 {
        return Err("Choose a positive client timeout.".into());
    }
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(timeout))
        .ok_or("Client timeout exceeds the clock range.")?;
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
    let documents = std::env::var_os("ONESTORE_OFFLINE_DOCUMENTS").is_some();
    let maintenance = std::env::var_os("ONESTORE_MAINTENANCE_DIR").map(std::path::PathBuf::from);
    let mut completed = 0;
    let mut attempts = 0;
    while completed < operations || (args[0] == "read" && !Path::new(&args[5]).exists()) {
        if Instant::now() > deadline {
            return Err("Concurrent client timed out.".into());
        }
        if let Some(control) = &maintenance
            && !control.join("resume").exists()
            && ((args[0] != "read" && completed == operations / 2)
                || (args[0] == "read" && control.join("pause").exists()))
        {
            fs::write(control.join(format!("paused-{}", args[2])), b"paused")?;
            log(json!({"event": "paused", "completed": completed}))?;
            while !control.join("resume").exists() {
                if Instant::now() > deadline {
                    return Err("Maintenance pause timed out.".into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            log(json!({"event": "resumed", "completed": completed}))?;
        }
        attempts += 1;
        random = random
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        thread::sleep(Duration::from_millis((random >> 32) % 7));
        let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
        let source = match read(&args[1]) {
            Ok(source) => source,
            Err(error)
                if [
                    io::ErrorKind::WouldBlock,
                    io::ErrorKind::ResourceBusy,
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
            "transaction": store.header.transaction_count, "text": text, "documents":if documents {Some(document_view(&source).map_err(preserve)?)}else{None}}),
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
        let result = commit(&args[1], &source, *sid, *oid, range, &replacement);
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
