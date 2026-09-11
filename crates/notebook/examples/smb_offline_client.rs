#[path = "../../onestore/examples/support/concurrent.rs"]
mod concurrent;

use notebook::smb::{Client, Credentials};
use notebook::{EditStatus, Error, Remote, Replica, SmbRemote};
use onestore::{
    CommitError, CommitState, ExGuid, Insertion, ParagraphJoin, ParagraphSplit, PreparedEdit,
    RevisionIndex, Store, TextAttribute, TreeEdit, document::Document,
};
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

const DOCUMENT_OPERATIONS: [&str; 10] = [
    "insert",
    "format",
    "text",
    "split",
    "right_text",
    "nest",
    "unnest",
    "join",
    "tail_split",
    "delete",
];

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
        Ok(bytes)
    }
    fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
        let after = view(edit.as_bytes()).map_err(|error| CommitError {
            state: CommitState::NotCommitted,
            error: io::Error::other(error.to_string()),
        })?;
        let documents = if self.documents {
            Some(document_view(edit.as_bytes()).map_err(|error| CommitError {
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
        let result = self.remote.publish(edit);
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

fn tokens(text: &str) -> Option<Vec<&str>> {
    let mut remaining = text.strip_prefix("Concurrent edits:")?;
    let mut tokens = Vec::new();
    while !remaining.is_empty() {
        let body = remaining.strip_prefix(" [w")?;
        let (token, tail) = body.split_once(']')?;
        let (actor, operation) = token.split_once(':')?;
        if actor.is_empty()
            || operation.is_empty()
            || !actor
                .bytes()
                .chain(operation.bytes())
                .all(|b| b.is_ascii_digit())
            || tokens.contains(&token)
        {
            return None;
        }
        tokens.push(token);
        remaining = tail;
    }
    Some(tokens)
}

// This owned workload explicitly resolves append conflicts after all retained tokens.
fn append_position(before: &str, current: &str, token: &str) -> Option<u32> {
    let original = tokens(before)?;
    let present = tokens(current)?;
    let mut retained = present.iter();
    for token in original {
        retained.find(|&&candidate| candidate == token)?;
    }
    if current.contains(token) {
        return None;
    }
    u32::try_from(current.encode_utf16().count()).ok()
}

fn queue_document(
    cache: &Replica,
    actor: &str,
    operation: usize,
    parent: Option<ExGuid>,
    deadline: Instant,
) -> Result<(ExGuid, [u64; DOCUMENT_OPERATIONS.len()]), Box<dyn std::error::Error>> {
    let source = cache.snapshot()?;
    let store = Store::parse(&source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let (space, page) = document.pages()?[0];
    let text = format!("Document {actor}:{operation} 🦀");
    let insertion = if operation.is_multiple_of(2) {
        let column: u32 = actor
            .strip_prefix('w')
            .ok_or("Missing writer number")?
            .parse()?;
        Insertion::outline(
            page,
            144.0 + column as f32 * 240.0,
            144.0 + operation as f32 * 72.0,
            &text,
            "Offline document writer",
        )?
    } else {
        Insertion::paragraph(
            parent.ok_or("Missing prior outline")?,
            None,
            &text,
            "Offline document writer",
        )?
    }
    .with_formatting(
        0..u32::try_from(text.encode_utf16().count())?,
        &[
            TextAttribute::FontSize(13.5),
            TextAttribute::Color(Some([0x44, 0x55, 0x66])),
        ],
    )?;
    let parent = if operation.is_multiple_of(2) {
        insertion.object()
    } else {
        parent.unwrap()
    };
    let end = u32::try_from(text.encode_utf16().count())?;
    let split = ParagraphSplit::new(insertion.text_object(), end - 2, "Offline document writer")?;
    let tail_split =
        ParagraphSplit::new(insertion.text_object(), end + 1, "Offline document writer")?;
    let join = ParagraphJoin::new(
        insertion.text_object(),
        split.text_object(),
        "Offline document writer",
    )?;
    let attributes = [
        TextAttribute::Bold(true),
        TextAttribute::FontSize(18.0 + (operation % 9) as f32),
        TextAttribute::Color(Some([0x12, 0x34, 0x56])),
    ];
    let mut ids = [0; DOCUMENT_OPERATIONS.len()];
    for (step, id) in ids.iter_mut().enumerate() {
        let kind = DOCUMENT_OPERATIONS[step];
        let tree = match kind {
            "nest" => {
                let source = cache.snapshot()?;
                let store = Store::parse(&source)?;
                let index = RevisionIndex::parse(&store)?;
                let document = Document::parse(&index)?;
                let section = &document.spaces[&space];
                let view = &section.revisions[&section.contexts[&ExGuid::default()]];
                let paragraph = view
                    .nodes
                    .iter()
                    .find(|(_, node)| node.content == [insertion.text_object()])
                    .map(|(id, _)| *id)
                    .ok_or("Missing inserted paragraph")?;
                Some(TreeEdit::move_to(
                    split.object(),
                    paragraph,
                    None,
                    "Offline document writer",
                )?)
            }
            "unnest" => Some(TreeEdit::move_to(
                split.object(),
                parent,
                None,
                "Offline document writer",
            )?),
            "delete" => Some(TreeEdit::delete(
                tail_split.object(),
                "Offline document writer",
            )?),
            _ => None,
        };
        let range = match kind {
            "text" => end - 3..end,
            "split" => end - 2..end - 2,
            "tail_split" => end + 1..end + 1,
            "right_text" => 0..2,
            _ => 1..end - 2,
        };
        let target = if kind == "right_text" {
            split.text_object()
        } else if kind == "delete" {
            tail_split.text_object()
        } else {
            insertion.text_object()
        };
        let replacement = match kind {
            "text" => Some(" e\u{301}🐈"),
            "right_text" => Some("B🦋"),
            _ => None,
        };
        loop {
            if Instant::now() >= deadline {
                return Err("Document queue timed out; cache retained".into());
            }
            let source = cache.snapshot()?;
            let started = now();
            let result = match kind {
                "insert" => cache.insert(&source, space, &insertion),
                "format" => cache.format(&source, space, target, range.clone(), &attributes),
                "text" | "right_text" => {
                    cache.edit_text(&source, space, target, range.clone(), replacement.unwrap())
                }
                "split" => cache.split(&source, space, &split),
                "tail_split" => cache.split(&source, space, &tail_split),
                "join" => cache.join(&source, space, &join),
                "nest" | "unnest" | "delete" => cache.tree(&source, space, tree.as_ref().unwrap()),
                _ => unreachable!(),
            };
            match result {
                Ok(Some(acknowledged)) => {
                    *id = acknowledged;
                    println!(
                        "{}",
                        json!({"event":"local_document_commit","id":acknowledged,"operation":operation,"kind":kind,
                        "space":space.to_string(),"object":target.to_string(),"document":insertion.text_object().to_string(),
                        "text":text,"insertion":if kind=="insert" {Some(&insertion)} else {None},
                        "split":match kind { "split" => Some(&split), "tail_split" => Some(&tail_split), _ => None },
                        "tree":tree,
                        "joined":if kind=="join" {Some(join.texts().map(|id| id.to_string()))} else {None},
                        "range":[range.start,range.end],"attributes":attributes,"replacement":replacement,
                        "started_us":started,"finished_us":now()})
                    );
                    break;
                }
                Err(Error::Io(error)) if error.kind() == io::ErrorKind::ResourceBusy => {}
                other => return Err(format!("Unexpected document queue result: {other:?}").into()),
            }
        }
    }
    Ok((parent, ids))
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
        json!({"event":"ready", "pid":std::process::id(), "actor":args[2], "offline":true, "document_operations":documents,"document_graph":documents,"document_kinds":if documents {DOCUMENT_OPERATIONS.as_slice()} else {&[]}})
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
        Ok(Traced { remote: SmbRemote::new(client, &path, 256 * 1024 * 1024), before:None, pause:pause.clone(), documents })
    }, move |result| {
        if let Err(error) = result {
            println!("{}", json!({"event":"sync_error", "error":error.to_string(), "at_us":now()}));
            if !matches!(error, Error::RemoteIo(_) | Error::Remote(_)) && !matches!(error, Error::Io(error) if error.kind() == io::ErrorKind::WouldBlock) {
                let _ = fatal_tx.send(error.to_string());
            }
        }
    })?;
    let mut ids = Vec::new();
    let mut document_ids = std::collections::BTreeSet::new();
    let mut document_parent = None;
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
                            json!({"event":if document_ids.contains(&ids[received]) {"document_receipt"} else {"remote_receipt"}, "id":ids[received], "revision":revision.to_string(), "at_us":now()})
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
            } else if documents && outage.is_some() {
                8 * (1 + DOCUMENT_OPERATIONS.len())
            } else {
                8
            };
            if generated < operations && pending.len() < capacity {
                let source = cache.snapshot()?;
                let target = view(&source)?;
                let at = u32::try_from(target.text.encode_utf16().count())?;
                let token = format!(" [{}:{}]", args[2], generated);
                let started = now();
                match cache.edit_text(&source, target.space, target.object, at..at, &token) {
                    Ok(Some(id)) => {
                        println!(
                            "{}",
                            json!({"event":"local_commit", "id":id, "operation":generated, "space":target.space.to_string(), "object":target.object.to_string(), "before":target.text, "token":token, "started_us":started, "finished_us":now()})
                        );
                        ids.push(id);
                        if documents {
                            let (parent, added) = queue_document(
                                &cache,
                                &args[2],
                                generated,
                                document_parent,
                                deadline,
                            )?;
                            document_parent = Some(parent);
                            document_ids.extend(added);
                            ids.extend(added);
                        }
                        generated += 1;
                    }
                    Err(Error::Io(error)) if error.kind() == io::ErrorKind::ResourceBusy => {}
                    other => return Err(format!("Unexpected local result: {other:?}").into()),
                }
            }
            if let Some(intent) = pending.first()
                && matches!(cache.status(intent.id)?, Some(EditStatus::Conflict(_)))
            {
                let local = cache.snapshot()?;
                let remote = cache.remote_snapshot()?;
                let current = view(&remote)?;
                let notebook::Operation::Text(edit) = &intent.operation else {
                    return Err(format!(
                        "Document intent {} requires review: {:?}",
                        intent.id,
                        cache.status(intent.id)?
                    )
                    .into());
                };
                let at = append_position(&edit.before, &current.text, &edit.replacement)
                    .ok_or("Append model disagrees with retained history")?;
                match cache.rebase_conflict(intent.id, &local, &remote, at..at) {
                    Ok(()) => println!(
                        "{}",
                        json!({"event":"reviewed_append", "id":intent.id, "before":edit.before, "remote":current.text, "token":edit.replacement, "at_us":now()})
                    ),
                    Err(Error::Io(error))
                        if [
                            io::ErrorKind::ResourceBusy,
                            io::ErrorKind::WouldBlock,
                            io::ErrorKind::InvalidInput,
                        ]
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
            json!({"event":if document_ids.contains(&id) {"reopened_document_receipt"} else {"reopened_receipt"}, "id":id, "revision":revision.to_string()})
        );
    }
    println!(
        "{}",
        json!({"event":"done", "operations":operations, "at_us":now()})
    );
    Ok(())
}

#[test]
fn append_review_requires_a_unique_ordered_history_and_an_absent_new_token() {
    assert_eq!(
        append_position(
            "Concurrent edits: [w0:0]",
            "Concurrent edits: [w1:0] [w0:0]",
            " [w0:1]"
        ),
        Some(31)
    );
    for current in [
        "Concurrent edits:",
        "Concurrent edits: [w0:0] [w0:0]",
        "Concurrent edits: [w0:1] [w0:0]",
        "Concurrent edits: changed [w0:0]",
    ] {
        assert_eq!(
            append_position("Concurrent edits: [w0:0]", current, " [w0:1]"),
            None
        );
    }
    assert_eq!(
        append_position(
            "Concurrent edits: [w0:0] [w1:0]",
            "Concurrent edits: [w1:0] [w0:0]",
            " [w0:1]"
        ),
        None
    );
}

#[cfg(test)]
#[path = "../../onestore/tests/support/disk.rs"]
mod disk;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_workload_retains_dependencies_and_receipts_across_reopen() {
        struct Server {
            disk: disk::Disk,
            lost_reply: bool,
        }
        impl Remote for Server {
            fn read(&mut self) -> io::Result<Vec<u8>> {
                Ok(self.disk.visible.clone())
            }
            fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
                edit.commit(&mut self.disk)?;
                if std::mem::take(&mut self.lost_reply) {
                    Err(CommitError {
                        state: CommitState::Unknown,
                        error: io::Error::from(io::ErrorKind::ConnectionAborted),
                    })
                } else {
                    Ok(())
                }
            }
            fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
                onestore::confirm_snapshot(&mut self.disk, snapshot)
            }
        }
        let source =
            onestore::create_section("workload.one", "Concurrent edits:", "Author").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let mut cache = Replica::create(&path, &source).unwrap();
        let mut remote = Traced {
            remote: Server {
                disk: disk::Disk {
                    visible: source.clone(),
                    durable: source.clone(),
                    operation: 0,
                    fail_at: None,
                    write_limit: 97,
                    random: 1,
                },
                lost_reply: false,
            },
            before: None,
            pause: None,
            documents: true,
        };
        println!(
            "{}",
            json!({"event":"ready", "actor":"w0", "offline":true,
            "document_operations":true, "document_graph":true, "document_kinds":DOCUMENT_OPERATIONS})
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut parent = None;
        let mut ids = Vec::new();
        for operation in 0..2 {
            let (outline, added) =
                queue_document(&cache, "w0", operation, parent, deadline).unwrap();
            parent = Some(outline);
            ids.extend(added);
            drop(cache);
            cache = Replica::open(&path).unwrap();
        }
        let local = cache.snapshot().unwrap();
        assert_eq!(
            cache.pending().unwrap().len(),
            2 * DOCUMENT_OPERATIONS.len()
        );
        for (step, id) in ids.iter().enumerate() {
            remote.remote.lost_reply = matches!(
                DOCUMENT_OPERATIONS[step % DOCUMENT_OPERATIONS.len()],
                "split" | "nest" | "unnest" | "join" | "tail_split" | "delete"
            );
            let result = cache.sync_once(&mut remote);
            let state = if result.is_err() {
                assert!(matches!(
                    result,
                    Err(Error::Remote(CommitError {
                        state: CommitState::Unknown,
                        ..
                    }))
                ));
                drop(cache);
                cache = Replica::open(&path).unwrap();
                cache.sync_once(&mut remote).unwrap().unwrap()
            } else {
                result.unwrap().unwrap()
            };
            assert_eq!(state.0, *id);
            let EditStatus::Published { revision } = state.1 else {
                panic!("{state:?}")
            };
            println!(
                "{}",
                json!({"event":"document_receipt", "id":id, "revision":revision.to_string(), "at_us":now()})
            );
            drop(cache);
            cache = Replica::open(&path).unwrap();
            assert_eq!(
                cache.status(*id).unwrap(),
                Some(EditStatus::Published { revision })
            );
            if step + 1 < ids.len() {
                assert_eq!(cache.snapshot().unwrap(), local);
            }
        }
        assert!(cache.pending().unwrap().is_empty());
        for id in ids {
            let Some(EditStatus::Published { revision }) = cache.status(id).unwrap() else {
                panic!()
            };
            println!(
                "{}",
                json!({"event":"reopened_document_receipt", "id":id, "revision":revision.to_string()})
            );
        }
        remote.read().unwrap();
        println!("{}", json!({"event":"done"}));
    }

    #[test]
    fn publication_differences_retain_removed_text_and_graph_identities() {
        let before = DocumentView {
            texts: serde_json::from_value(json!({"left":{"text":"ab"}, "right":{"text":"cd"}, "untouched":{"text":"ef"}})).unwrap(),
            graph: serde_json::from_value(json!({"outline":{"children":["a","b"]}, "a":{"content":["left"]}, "b":{"content":["right"]}})).unwrap(),
        };
        let after = DocumentView {
            texts: serde_json::from_value(
                json!({"left":{"text":"abcd"}, "untouched":{"text":"ef"}}),
            )
            .unwrap(),
            graph: serde_json::from_value(
                json!({"outline":{"children":["a"]}, "a":{"content":["left"]}}),
            )
            .unwrap(),
        };
        let changes = after.changes(&before);
        assert_eq!(
            changes.texts,
            serde_json::from_value::<serde_json::Map<_, _>>(
                json!({"left":{"text":"abcd"}, "right":null})
            )
            .unwrap()
        );
        assert_eq!(
            changes.graph,
            serde_json::from_value::<serde_json::Map<_, _>>(
                json!({"outline":{"children":["a"]}, "b":null})
            )
            .unwrap()
        );
        assert_eq!(before.changes(&after).texts["right"], before.texts["right"]);
        assert_eq!(before.changes(&before), DocumentView::default());
    }
}
