#[path = "../../onestore/examples/support/concurrent.rs"]
mod concurrent;

use notebook::smb::{Client, Credentials};
use notebook::{EditStatus, Error, Remote, Replica, SmbRemote};
use onestore::{CommitError, CommitState, ExGuid, RevisionIndex, Store, Transaction};
use serde_json::json;
use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod support {
    pub mod view;
}
use concurrent::{DocumentView, document_view};
use support::view::view;

impl DocumentView {
    fn changes(&self, before: &Self) -> Self {
        let difference =
            |old: &serde_json::Map<String, serde_json::Value>,
             new: &serde_json::Map<String, serde_json::Value>| {
                old.keys()
                    .chain(new.keys())
                    .filter(|id| old.get(*id) != new.get(*id))
                    .map(|id| (id.clone(), new.get(id).cloned().unwrap_or_default()))
                    .collect()
            };
        Self {
            texts: difference(&before.texts, &self.texts),
            graph: difference(&before.graph, &self.graph),
        }
    }
}

fn now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros()
}

#[derive(Clone)]
enum Pause {
    Outage(PathBuf),
    FormatReply(PathBuf),
}

struct Traced<R> {
    remote: R,
    before: Option<(String, Option<DocumentView>)>,
    /// The last image read, which publications apply to.
    read: Vec<u8>,
    pause: Option<Pause>,
    documents: bool,
}

impl<R: Remote> Remote for Traced<R> {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        let started = now();
        let bytes = self.remote.read()?;
        let observed = view(&bytes).map_err(|error| io::Error::other(error.to_string()))?;
        let documents = if self.documents {
            Some(document_view(&bytes).map_err(io::Error::other)?)
        } else {
            None
        };
        println!(
            "{}",
            json!({"event":"read", "started_us":started, "finished_us":now(), "text":observed.text, "documents":documents.as_ref().map(|view| &view.texts), "document_graph":documents.as_ref().map(|view| &view.graph)})
        );
        self.before = Some((observed.text, documents));
        self.read.clone_from(&bytes);
        Ok(bytes)
    }
    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        let mut image = self.read.clone();
        transaction.apply(&mut image).map_err(|error| CommitError {
            state: CommitState::NotCommitted,
            error: io::Error::other(error.to_string()),
        })?;
        let after = view(&image).map_err(|error| CommitError {
            state: CommitState::NotCommitted,
            error: io::Error::other(error.to_string()),
        })?;
        let documents = if self.documents {
            Some(document_view(&image).map_err(|error| CommitError {
                state: CommitState::NotCommitted,
                error: io::Error::other(error),
            })?)
        } else {
            None
        };
        let changes = if let Some(after) = &documents {
            let before = self
                .before
                .as_ref()
                .and_then(|(_, documents)| documents.as_ref())
                .ok_or_else(|| CommitError {
                    state: CommitState::NotCommitted,
                    error: io::Error::other("Document publication has no observed source"),
                })?;
            Some(after.changes(before))
        } else {
            None
        };
        let pause = match &self.pause {
            Some(Pause::Outage(marker)) => Some((
                marker,
                marker.parent().unwrap().join("offline-outage-resumed"),
                "outage",
            )),
            Some(Pause::FormatReply(marker))
                if changes.as_ref().is_some_and(|changes| {
                    changes.texts.len() == 1
                        && self
                            .before
                            .as_ref()
                            .and_then(|(_, before)| before.as_ref())
                            .is_some_and(|before| {
                                changes.texts.keys().all(|id| before.texts.contains_key(id))
                            })
                }) =>
            {
                Some((marker, marker.with_extension("resume"), "format"))
            }
            _ => None,
        };
        if let Some((marker, resumed, kind)) = pause
            && !resumed.exists()
        {
            std::fs::write(marker, after.revision.to_string()).map_err(|error| CommitError {
                state: CommitState::NotCommitted,
                error,
            })?;
            println!(
                "{}",
                json!({"event":"publication_paused", "revision":after.revision.to_string(), "kind":kind, "at_us":now()})
            );
            let deadline = Instant::now() + Duration::from_secs(120);
            while !resumed.exists() {
                if Instant::now() >= deadline {
                    return Err(CommitError {
                        state: CommitState::NotCommitted,
                        error: io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Offline publication barrier timed out",
                        ),
                    });
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        let started = now();
        let result = self.remote.publish(transaction);
        let finished = now();
        println!(
            "{}",
            json!({"event":"remote_attempt", "started_us":started, "finished_us":finished,
            "revision":after.revision.to_string(), "space":after.space.to_string(), "object":after.object.to_string(), "before":self.before.as_ref().map(|(text,_)|text), "after":after.text,
            "state":format!("{:?}", result.as_ref().map_or_else(|error| error.state, |_| CommitState::Committed)),
            "documents":documents.as_ref().map(|view| &view.texts), "document_graph":documents.as_ref().map(|view| &view.graph),
            "document_changes":changes.as_ref().map(|view| &view.texts), "document_graph_changes":changes.as_ref().map(|view| &view.graph)})
        );
        if result
            .as_ref()
            .is_err_and(|error| error.state == CommitState::Unknown)
            && let Some(Pause::FormatReply(marker)) = &self.pause
            && marker.with_extension("isolate").exists()
        {
            std::fs::write(marker.with_extension("isolate"), serde_json::to_vec(&json!({"space":after.space.to_string(), "revision":after.revision.to_string(), "after_us":finished})).unwrap())
                .map_err(|error| CommitError { state:CommitState::Unknown, error })?;
            println!(
                "{}",
                json!({"event":"confirmation_paused", "revision":after.revision.to_string(), "at_us":now()})
            );
            let deadline = Instant::now() + Duration::from_secs(120);
            while !marker.with_extension("confirmation-resume").exists() {
                if Instant::now() >= deadline {
                    return Err(CommitError {
                        state: CommitState::Unknown,
                        error: io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Offline confirmation barrier timed out",
                        ),
                    });
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        result
    }
    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        let captured = (|| -> Result<_, Box<dyn std::error::Error>> {
            let store = Store::parse(snapshot)?;
            let index = RevisionIndex::parse(&store)?;
            let revisions = index
                .spaces
                .iter()
                .map(|(id, space)| {
                    (
                        id.to_string(),
                        space
                            .revisions
                            .keys()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let current = index
                .spaces
                .iter()
                .filter_map(|(id, space)| {
                    space
                        .labels
                        .get(&(ExGuid::default(), 1))
                        .map(|revision| (id.to_string(), revision.to_string()))
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let path = env::var_os("ONESTORE_OFFLINE_CONFIRM_DIR").map(|directory| {
                PathBuf::from(directory).join(format!("{}-{}.one", std::process::id(), now()))
            });
            if let Some(path) = &path {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?
                    .write_all(snapshot)?;
            }
            Ok((revisions, current, path))
        })();
        let (revisions, current, capture) = captured.map_err(|error| CommitError {
            state: CommitState::NotCommitted,
            error: io::Error::other(error.to_string()),
        })?;
        let started = now();
        let result = self.remote.confirm(snapshot);
        println!(
            "{}",
            json!({"event":"remote_confirm", "started_us":started, "finished_us":now(), "revisions":revisions, "current_revisions":current, "capture":capture.as_ref().and_then(|path| path.file_name()).map(|name| name.to_string_lossy()), "text":self.before.as_ref().map(|(text,_)|text),
            "state":format!("{:?}", result.as_ref().map_or_else(|error| error.state, |_| CommitState::Committed)), "error":result.as_ref().err().map(|error|error.error.to_string())})
        );
        result
    }
}

/// The edit appending `token` at `at` of a text.
fn append(space: ExGuid, text: ExGuid, at: u32, token: &str) -> onestore::op::Edit {
    onestore::op::Edit {
        at: u64::try_from(now()).unwrap_or_default() * 10 + 116_444_736_000_000_000,
        ops: vec![onestore::op::Op::Page {
            space,
            op: onestore::op::PageOp::Text {
                text,
                range: at..at,
                with: token.to_owned(),
            },
        }],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 7
        || !["read", "write"].contains(&args[0].as_str())
        || !args[2].bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return Err("Expected read|write FILE ACTOR OPERATIONS START_FILE STOP_FILE SEED".into());
    }
    let documents = env::var_os("ONESTORE_OFFLINE_DOCUMENTS").is_some();
    let address = env::var("ONESTORE_SMB_LAB")?;
    let share = env::var("ONESTORE_SMB_SHARE")?;
    let initial = Client::connect(
        &address,
        &share,
        Credentials::default(),
        Duration::from_secs(5),
    )?;
    if args[0] == "read" {
        return concurrent::run(
            &args,
            |path| initial.read(path, 256 * 1024 * 1024),
            |_, _, _, _, _, _| unreachable!(),
        );
    }
    let timeout: u64 = env::var("ONESTORE_CLIENT_TIMEOUT_MS")
        .unwrap_or_else(|_| "600000".into())
        .parse()?;
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(timeout))
        .ok_or("Invalid timeout")?;
    let operations: usize = args[3].parse()?;
    let mut seed: u64 = args[6].parse()?;
    if operations == 0 {
        return Err("Expected positive operations".into());
    }
    let source = initial.read(&args[1], 256 * 1024 * 1024)?;
    drop(initial);
    let cache_path = Path::new(&args[4])
        .parent()
        .ok_or("Missing workload directory")?
        .join(format!("{}.sqlite", args[2]));
    let cache = Arc::new(Replica::create(&cache_path, &source)?);
    println!(
        "{}",
        json!({"event":"ready", "pid":std::process::id(), "actor":args[2], "offline":true, "document_operations":false,"document_graph":documents,"document_kinds":[]})
    );
    while !Path::new(&args[4]).exists() {
        if Instant::now() >= deadline {
            return Err("Start barrier timed out".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let path = args[1].clone();
    let outage = env::var_os("ONESTORE_OFFLINE_OUTAGE_DIR").map(PathBuf::from);
    let pause = outage
        .as_ref()
        .map(|directory| Pause::Outage(directory.join(format!("offline-paused-{}", args[2]))))
        .or_else(|| {
            env::var_os("ONESTORE_OFFLINE_FORMAT_REPLY_DIR").map(|directory| {
                Pause::FormatReply(
                    PathBuf::from(directory).join(format!("offline-paused-{}", args[2])),
                )
            })
        });
    let (fatal_tx, fatal_rx) = std::sync::mpsc::channel();
    let worker = cache.start_sync(Duration::from_millis(50), move || {
        let client = Client::connect(&address, &share, Credentials::default(), Duration::from_secs(5))?;
        println!("{}", json!({"event":"transport_connected", "at_us":now()}));
        Ok(Traced { remote: SmbRemote::new(client, &path, 256 * 1024 * 1024), before:None, read:Vec::new(), pause:pause.clone(), documents })
    }, move |result| {
        if let Err(error) = result {
            println!("{}", json!({"event":"sync_error", "error":error.to_string(), "at_us":now()}));
            if !matches!(error, Error::RemoteIo(_) | Error::Remote(_)) && !matches!(error, Error::Io(error) if error.kind() == io::ErrorKind::WouldBlock) {
                let _ = fatal_tx.send(error.to_string());
            }
        }
    })?;
    let mut ids = Vec::new();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut generated = 0;
        let mut received = 0;
        loop {
            match fatal_rx.try_recv() {
                Ok(error) => return Err(error.into()),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("Worker exited before completion".into());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            while received < ids.len() {
                match cache.status(ids[received])? {
                    Some(EditStatus::Published { revision }) => {
                        println!(
                            "{}",
                            json!({"event":"remote_receipt", "id":ids[received], "revision":revision.to_string(), "at_us":now()})
                        );
                        received += 1;
                    }
                    Some(_) => break,
                    None => return Err("Local intent disappeared".into()),
                }
            }
            if Instant::now() >= deadline {
                return Err("Offline workload timed out; cache retained".into());
            }
            let pending = cache.pending()?;
            let capacity = if outage
                .as_ref()
                .is_some_and(|directory| !directory.join("offline-outage-down").exists())
            {
                1
            } else {
                8
            };
            if generated < operations && pending.len() < capacity {
                let source = cache.snapshot()?;
                let target = view(&source)?;
                let at = u32::try_from(target.text.encode_utf16().count())?;
                let token = format!(" [{}:{}]", args[2], generated);
                let started = now();
                match cache.apply(
                    "Offline document writer",
                    append(target.space, target.object, at, &token),
                ) {
                    Ok(id) => {
                        println!(
                            "{}",
                            json!({"event":"local_commit", "id":id, "operation":generated, "space":target.space.to_string(), "object":target.object.to_string(), "before":target.text, "token":token, "started_us":started, "finished_us":now()})
                        );
                        ids.push(id);
                        generated += 1;
                    }
                    other => return Err(format!("Unexpected local result: {other:?}").into()),
                }
            }
            if let Some(conflict) = cache.conflict()? {
                // Concurrent appends meet at the end of the text: take the remote text and
                // append the queued tokens it lacks after it again.
                let remote = view(&cache.remote_snapshot()?)?;
                let tokens: Vec<String> = cache
                    .pending()?
                    .iter()
                    .flat_map(|queued| queued.edit.ops.iter())
                    .filter_map(|op| match op {
                        onestore::op::Op::Page {
                            op: onestore::op::PageOp::Text { with, .. },
                            ..
                        } => Some(with.clone()),
                        _ => None,
                    })
                    .filter(|token| !remote.text.contains(token.as_str()))
                    .collect();
                match cache.resolve(conflict.id, notebook::Resolution::Theirs) {
                    Ok(()) => {
                        let mut at = u32::try_from(remote.text.encode_utf16().count())?;
                        for token in &tokens {
                            cache.apply(
                                "Offline document writer",
                                append(remote.space, remote.object, at, token),
                            )?;
                            at += u32::try_from(token.encode_utf16().count())?;
                        }
                        println!(
                            "{}",
                            json!({"event":"reviewed_append", "id":conflict.id, "remote":remote.text, "tokens":tokens, "at_us":now()})
                        );
                    }
                    Err(Error::Io(error))
                        if [io::ErrorKind::WouldBlock, io::ErrorKind::InvalidInput]
                            .contains(&error.kind()) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if generated == operations && received == ids.len() {
                if !cache.pending()?.is_empty() {
                    return Err("Acknowledged queue did not drain".into());
                }
                break;
            }
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            thread::sleep(Duration::from_millis(1 + (seed >> 32) % 7));
        }
        Ok(())
    })();
    let stopped = worker.stop();
    result?;
    stopped?;
    drop(cache);
    let reopened = Replica::open(&cache_path)?;
    if !reopened.pending()?.is_empty() {
        return Err("Pending edits reappeared after reopen".into());
    }
    for id in ids {
        let Some(EditStatus::Published { revision }) = reopened.status(id)? else {
            return Err("Receipt did not survive reopen".into());
        };
        println!(
            "{}",
            json!({"event":"reopened_receipt", "id":id, "revision":revision.to_string()})
        );
    }
    println!(
        "{}",
        json!({"event":"done", "operations":operations, "at_us":now()})
    );
    Ok(())
}
