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

#[derive(Debug, Default, PartialEq)]
pub struct DocumentView {
    pub texts: serde_json::Map<String, serde_json::Value>,
    pub graph: serde_json::Map<String, serde_json::Value>,
}

pub fn document_view(bytes: &[u8]) -> Result<DocumentView, onestore::Error> {
    let store = Store::parse(bytes)?;
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let mut observed = DocumentView::default();
    for (sid, page) in document.pages()? {
        let space = &document.spaces[&sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        for outline in &revision.nodes[&page].children {
            if !matches!(revision.nodes[outline].kind, Kind::Outline { .. }) {
                continue;
            }
            let mut pending = vec![(*outline, page)];
            let mut parents = std::collections::BTreeMap::new();
            while let Some((id, parent)) = pending.pop() {
                if parents.insert(id, parent).is_some() {
                    return Err(onestore::Error {
                        offset: 0,
                        message: "Document observation contains a repeated object",
                    });
                }
                let node = &revision.nodes[&id];
                pending.extend(
                    node.children
                        .iter()
                        .chain(&node.content)
                        .map(|child| (*child, id)),
                );
            }
            // The workload keeps its marker in the left paragraph through boundary edits.
            if !parents.keys().any(|id| {
                matches!(&revision.nodes[id].kind,
                Kind::RichText { text, .. } if text.starts_with("Document w"))
            }) {
                continue;
            }
            for (id, parent) in parents {
                let key = id.to_string();
                if observed.texts.contains_key(&key) || observed.graph.contains_key(&key) {
                    return Err(onestore::Error {
                        offset: 0,
                        message: "Document observation contains a repeated object",
                    });
                }
                let node = &revision.nodes[&id];
                if let Kind::RichText { text, .. } = &node.kind {
                    let runs = revision.text_runs(id)?.into_iter().map(|run| json!({
                        "text": run.text, "bold": run.format.bold.unwrap_or(false),
                        "size": run.format.font_size, "color": run.format.color.unwrap_or(0xff000000)
                    })).collect::<Vec<_>>();
                    observed.texts.insert(key, json!({"text":text,"runs":runs}));
                } else {
                    observed.graph.insert(key, json!({
                        "parent": parent.to_string(), "children": node.children.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        "content": node.content.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        "child_level": node.child_level,
                        "position": if id == *outline { Some(json!({"x":node.layout.x,"y":node.layout.y})) } else { None }
                    }));
                }
            }
        }
    }
    Ok(observed)
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
    let documents = std::env::var_os("ONESTORE_OFFLINE_DOCUMENTS").is_some();
    log(
        json!({"event": "ready", "pid": std::process::id(), "actor": args[2], "document_graph": documents}),
    )?;
    while !Path::new(&args[4]).exists() {
        if Instant::now() > deadline {
            return Err("Start barrier timed out.".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
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
        let observed = if documents {
            Some(document_view(&source).map_err(preserve)?)
        } else {
            None
        };
        let read_finished = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
        log(
            json!({"event": "read", "attempt": attempts, "started_us": started, "finished_us": read_finished,
            "transaction": store.header.transaction_count, "text": text, "documents":observed.as_ref().map(|view| &view.texts), "document_graph":observed.as_ref().map(|view| &view.graph)}),
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

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::{
        TextAttribute,
        document::{Format, Layout},
        op::{Edit, Op, PageOp},
        page::{
            Outline, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject,
            text::new_id,
        },
    };

    fn paragraph(text: &str, level: u32) -> PageParagraph {
        PageParagraph {
            id: new_id().unwrap(),
            parent: None,
            level,
            style: None,
            format: Format::default(),
            content: ParagraphContent::Text(TextObject {
                id: new_id().unwrap(),
                date_field: None,
                text: Paragraph::new(
                    text.into(),
                    Format {
                        font: Some("Calibri".into()),
                        font_size: Some(11.0),
                        language: Some(0x409),
                        ..Format::default()
                    },
                ),
                tags: Vec::new(),
            }),
            lists: Vec::new(),
            tags: Vec::new(),
            media: Default::default(),
            collapsed: false,
        }
    }

    fn edited(image: &[u8], space: ExGuid, ops: Vec<PageOp>) -> Vec<u8> {
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, image.to_vec()).unwrap();
        let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
        section
            .apply(
                "Author",
                &Edit {
                    at: 134_000_000_000_000_000,
                    ops,
                },
            )
            .unwrap();
        section.seal().unwrap();
        section.image()
    }

    #[test]
    fn observation_follows_split_suffixes_and_moved_children_through_active_ancestry() {
        let source = onestore::create_section("observation.one", "Original", "Author").unwrap();
        assert_eq!(document_view(&source).unwrap(), DocumentView::default());
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (space, page) = document.pages().unwrap()[0];
        let first = paragraph("Document w0:0 🦀", 1);
        let (paragraph_id, text) = (first.id, first.text().unwrap().id);
        let outline_id = new_id().unwrap();
        let inserted = edited(
            &source,
            space,
            vec![
                PageOp::Add {
                    object: PageObject::Outline(Outline {
                        id: outline_id,
                        title: false,
                        min_width: None,
                        layout: Layout {
                            x: Some(144.0),
                            y: Some(216.0),
                            ..Default::default()
                        },
                        indents: Vec::new(),
                        paragraphs: vec![first],
                        unsupported: Vec::new(),
                    }),
                    before: None,
                },
                PageOp::Format {
                    text,
                    range: 0..16,
                    set: vec![TextAttribute::Bold(true)],
                    clear: Vec::new(),
                },
            ],
        );
        let before = document_view(&inserted).unwrap();
        assert_eq!(before.texts.len(), 1);
        assert_eq!(before.graph.len(), 2);
        let outline = outline_id.to_string();
        let paragraph_name = paragraph_id.to_string();
        assert_eq!(before.graph[&outline]["children"], json!([paragraph_name]));
        let child = paragraph("Unmarked child", 2);
        let child_id = child.id;
        let with_child = edited(
            &inserted,
            space,
            vec![PageOp::Insert {
                container: paragraph_id,
                before: None,
                paragraphs: vec![child],
            }],
        );
        let before = document_view(&with_child).unwrap();
        assert_eq!(before.texts.len(), 2);
        assert_eq!(before.graph.len(), 3);
        assert_eq!(
            before.graph[&outline],
            json!({"parent":page.to_string(), "children":[paragraph_name], "content":[], "child_level":1, "position":{"x":144.0,"y":216.0}})
        );
        for offset in [14, 16] {
            let (split, right) = (new_id().unwrap(), new_id().unwrap());
            let split_edit = edited(
                &with_child,
                space,
                vec![PageOp::Split {
                    text,
                    at: offset,
                    paragraph: split,
                    right,
                    lists: Vec::new(),
                }],
            );
            let observed = document_view(&split_edit).unwrap();
            assert_eq!(observed.texts.len(), 3);
            assert_eq!(observed.graph.len(), 4);
            assert_eq!(
                observed.texts[&right.to_string()]["text"],
                if offset == 14 { "🦀" } else { "" }
            );
            assert_eq!(
                observed.graph[&outline]["children"],
                json!([paragraph_name, split.to_string()])
            );
            assert_eq!(observed.graph[&paragraph_name]["children"], json!([]));
            assert_eq!(
                observed.graph[&split.to_string()]["children"],
                json!([child_id.to_string()])
            );
            assert_eq!(
                observed.graph[&child_id.to_string()]["parent"],
                split.to_string()
            );
            let joined = edited(&split_edit, space, vec![PageOp::Join { left: text, right }]);
            let joined = document_view(&joined).unwrap();
            assert_eq!(joined.graph, before.graph);
            let characters = |view: &DocumentView| {
                view.texts
                    .iter()
                    .map(|(id, text)| {
                        let runs = text["runs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .flat_map(|run| {
                                run["text"]
                                    .as_str()
                                    .unwrap()
                                    .chars()
                                    .map(|c| json!([c, run["bold"], run["size"], run["color"]]))
                            })
                            .collect::<Vec<_>>();
                        (id.clone(), (text["text"].clone(), runs))
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            assert_eq!(characters(&joined), characters(&before));
        }
    }
}
