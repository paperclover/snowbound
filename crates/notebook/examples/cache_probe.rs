use notebook::Replica;
use onestore::{
    ExGuid, Insertion, RevisionIndex, Store, TextAttribute,
    document::{Document, Kind},
};
use std::{
    io::{self, BufRead, Write},
    path::Path,
};

fn content(bytes: &[u8]) -> (ExGuid, ExGuid, String) {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .spaces
        .iter()
        .find_map(|(sid, space)| {
            let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
            revision
                .nodes
                .iter()
                .find_map(|(oid, node)| match &node.kind {
                    Kind::RichText { text, .. } => Some((*sid, *oid, text.clone())),
                    _ => None,
                })
        })
        .unwrap()
}

fn payload(operation: u64, size: usize) -> String {
    format!("{operation}:🦀{}", "x".repeat(size))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let mode = &args[1];
    let path = Path::new(&args[2]);
    let operation_kind =
        std::env::var("ONESTORE_CACHE_PROBE_OPERATION").unwrap_or_else(|_| "text".into());
    assert!(matches!(
        operation_kind.as_str(),
        "text" | "insert" | "format"
    ));
    let size = match std::env::var("ONESTORE_CACHE_PROBE_BYTES") {
        Ok(value) => value.parse::<usize>()?,
        Err(std::env::VarError::NotPresent) => 2 * 1024 * 1024,
        Err(error) => return Err(error.into()),
    };
    assert!(size > 0 && size <= 2 * 1024 * 1024);
    let seed = std::env::var_os("ONESTORE_CACHE_PROBE_SOURCE")
        .map(std::fs::read)
        .transpose()?;
    if mode == "init" {
        let source = match seed {
            Some(source) => source,
            None => onestore::create_section(
                "cache.one",
                &if operation_kind == "format" {
                    payload(0, size)
                } else {
                    "Base".into()
                },
                "Fixture",
            )?,
        };
        Replica::create(path, &source)?;
        return Ok(());
    }
    let cache = Replica::open(path)?;
    if mode == "read" {
        let snapshot = cache.snapshot()?;
        let base = cache.remote_snapshot()?;
        let (sid, target, mut expected) = content(&base);
        let store = Store::parse(&snapshot)?;
        assert!(store.checksum_mismatches.is_empty());
        let index = RevisionIndex::parse(&store)?;
        index.validate_current()?;
        let document = Document::parse(&index)?;
        let space = &document.spaces[&sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        let mut operations = Vec::new();
        let mut ids = Vec::new();
        let mut font_size = None;
        for pending in cache.pending()? {
            let operation = match pending.operation {
                notebook::Operation::Text(edit) => {
                    assert_eq!(operation_kind, "text");
                    assert_eq!(edit.object, target);
                    assert_eq!(edit.before, expected);
                    assert_eq!(
                        edit.range,
                        0..u32::try_from(expected.encode_utf16().count())?
                    );
                    let operation: u64 = edit.replacement.split_once(':').unwrap().0.parse()?;
                    expected = payload(operation, size);
                    assert_eq!(edit.replacement, expected);
                    operation
                }
                notebook::Operation::Insert(insertion) => {
                    assert_eq!(operation_kind, "insert");
                    let Kind::RichText { text, .. } =
                        &revision.nodes[&insertion.text_object()].kind
                    else {
                        panic!("Missing inserted text")
                    };
                    let operation: u64 = text.split_once(':').unwrap().0.parse()?;
                    assert_eq!(*text, payload(operation, size));
                    assert_eq!(serde_json::to_value(&insertion)?["text"], *text);
                    let outline = &revision.nodes[&insertion.object()];
                    assert!(matches!(outline.kind, Kind::Outline { .. }));
                    assert_eq!(
                        (outline.layout.x, outline.layout.y),
                        (Some(144.0), Some(operation as f32 * 72.0))
                    );
                    let (_, page) = document
                        .pages()?
                        .into_iter()
                        .find(|(space, _)| *space == sid)
                        .unwrap();
                    assert!(revision.nodes[&page].children.contains(&insertion.object()));
                    operation
                }
                notebook::Operation::Format(edit) => {
                    assert_eq!(operation_kind, "format");
                    assert_eq!(edit.object, target);
                    assert_eq!(edit.before, expected);
                    assert_eq!(
                        edit.range,
                        0..u32::try_from(expected.encode_utf16().count())?
                    );
                    let [TextAttribute::FontSize(value)] = edit.attributes.as_slice() else {
                        panic!("Unexpected formatting intent")
                    };
                    assert!((7.0..=130.0).contains(value) && value.fract() == 0.0);
                    font_size = Some(*value);
                    *value as u64 - 6
                }
                notebook::Operation::Split(_)
                | notebook::Operation::Page(_)
                | notebook::Operation::CreatePage(_)
                | notebook::Operation::Pages(_)
                | notebook::Operation::Join(_)
                | notebook::Operation::Outline(_)
                | notebook::Operation::Tree(_) => {
                    panic!("Unexpected operation for this fixture")
                }
            };
            assert!(operations.last().is_none_or(|last| *last < operation));
            assert!(ids.last().is_none_or(|last| *last < pending.id));
            operations.push(operation);
            ids.push(pending.id);
        }
        assert!(matches!(&revision.nodes[&target].kind,Kind::RichText{text,..} if *text==expected));
        if let Some(font_size) = font_size {
            assert!(
                revision
                    .text_runs(target)?
                    .iter()
                    .all(|run| run.format.font_size == Some(font_size))
            );
        }
        if operations.is_empty() {
            assert!(
                snapshot == base,
                "An empty local queue changed its working image"
            );
        }
        if let Some(output) = args.get(3) {
            std::fs::write(output, &snapshot)?;
        }
        println!(
            "{}",
            serde_json::json!({"operations": operations, "ids": ids, "section_bytes": snapshot.len(), "complete_payloads": true})
        );
        return Ok(());
    }
    assert_eq!(mode, "edit");
    assert!(
        matches!(Replica::open(path), Err(notebook::Error::Database(error)) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy))
    );
    println!("ready");
    io::stdout().flush()?;
    let input = io::stdin();
    let mut lines = input.lock().lines();
    let instruction = lines.next().unwrap()?;
    let (operation, acknowledgement) = instruction.split_once(' ').unwrap();
    let operation: u64 = operation.parse()?;
    let source = cache.snapshot()?;
    let (sid, oid, text) = content(&source);
    let replacement = payload(operation, size);
    println!("editing {operation}");
    io::stdout().flush()?;
    let id = match operation_kind.as_str() {
        "text" => cache.edit_text(
            &source,
            sid,
            oid,
            0..text.encode_utf16().count().try_into()?,
            &replacement,
        )?,
        "insert" => {
            let store = Store::parse(&source)?;
            let index = RevisionIndex::parse(&store)?;
            let document = Document::parse(&index)?;
            let (sid, page) = document.pages()?[0];
            let insertion = Insertion::outline(
                page,
                144.0,
                operation as f32 * 72.0,
                &replacement,
                "Fixture",
            )?;
            cache.insert(&source, sid, &insertion)?
        }
        "format" => {
            assert!((1..=124).contains(&operation));
            cache.format(
                &source,
                sid,
                oid,
                0..text.encode_utf16().count().try_into()?,
                &[TextAttribute::FontSize(6.0 + operation as f32)],
            )?
        }
        _ => unreachable!(),
    }
    .unwrap();
    if acknowledgement == "unack" {
        println!("durable {operation} {id}");
    } else {
        println!("ack {operation} {id}");
    }
    io::stdout().flush()?;
    lines.next().transpose()?;
    Ok(())
}
