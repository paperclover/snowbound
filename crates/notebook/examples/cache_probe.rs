use notebook::Replica;
use onestore::op::{Op, PageOp};
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
    page::{PageObject, Paragraph},
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
            let [Op::Page { op, .. }] = &pending.edit.ops[..] else {
                panic!("Unexpected edit for this fixture")
            };
            let operation = match (operation_kind.as_str(), op) {
                ("text", PageOp::Text { text, range, with }) => {
                    assert_eq!(*text, target);
                    assert_eq!(*range, 0..u32::try_from(expected.encode_utf16().count())?);
                    let operation: u64 = with.split_once(':').unwrap().0.parse()?;
                    expected = payload(operation, size);
                    assert_eq!(*with, expected);
                    operation
                }
                (
                    "insert",
                    PageOp::Add {
                        object: PageObject::Outline(outline),
                        ..
                    },
                ) => {
                    let text = outline.paragraphs[0].text().unwrap().text.text();
                    let operation: u64 = text.split_once(':').unwrap().0.parse()?;
                    assert_eq!(text, payload(operation, size));
                    assert_eq!(
                        (outline.layout.x, outline.layout.y),
                        (Some(144.0), Some(operation as f32 * 72.0))
                    );
                    assert!(revision.nodes.contains_key(&outline.id));
                    operation
                }
                ("format", PageOp::Format { text, set, .. }) => {
                    assert_eq!(*text, target);
                    let [onestore::TextAttribute::FontSize(value)] = set[..] else {
                        panic!("Expected a font size")
                    };
                    assert!((7.0..=130.0).contains(&value) && value.fract() == 0.0);
                    font_size = Some(value);
                    value as u64 - 6
                }
                other => panic!("Unexpected op {other:?}"),
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
            assert!(snapshot == base, "An empty local queue changed its image");
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
    let end = u32::try_from(text.encode_utf16().count())?;
    let op = match operation_kind.as_str() {
        "text" => PageOp::Text {
            text: oid,
            range: 0..end,
            with: replacement.clone(),
        },
        "insert" => {
            let page = cache.page(sid)?;
            let template = page
                .objects
                .iter()
                .find_map(|object| match object {
                    PageObject::Outline(outline) => outline
                        .paragraphs
                        .iter()
                        .find(|p| p.text().is_some_and(|t| t.id == oid)),
                    _ => None,
                })
                .expect("target paragraph")
                .clone();
            let mut fresh = template.clone();
            fresh.id = onestore::page::text::new_id()?;
            fresh.parent = None;
            fresh.level = 1;
            fresh.lists.clear();
            fresh.tags.clear();
            fresh.style = None;
            let format = template.text().unwrap().text.format_at(0)?.clone();
            fresh.content = onestore::page::ParagraphContent::Text(onestore::page::TextObject {
                id: onestore::page::text::new_id()?,
                date_field: None,
                text: Paragraph::new(replacement.clone(), format),
                tags: Vec::new(),
            });
            PageOp::Add {
                object: PageObject::Outline(onestore::page::Outline {
                    id: onestore::page::text::new_id()?,
                    title: false,
                    min_width: None,
                    layout: onestore::document::Layout {
                        x: Some(144.0),
                        y: Some(operation as f32 * 72.0),
                        ..Default::default()
                    },
                    indents: Vec::new(),
                    paragraphs: vec![fresh],
                    unsupported: Vec::new(),
                }),
                before: page
                    .objects
                    .iter()
                    .find(|o| matches!(o, PageObject::Title(_)))
                    .map(PageObject::id),
            }
        }
        "format" => {
            assert!((1..=124).contains(&operation));
            PageOp::Format {
                text: oid,
                range: 0..end,
                set: vec![onestore::TextAttribute::FontSize(6.0 + operation as f32)],
                clear: Vec::new(),
            }
        }
        _ => unreachable!(),
    };
    let id = cache.apply(
        "Fixture",
        onestore::op::Edit {
            at: 133_000_000_000_000_000,
            ops: vec![Op::Page { space: sid, op }],
        },
    )?;
    if acknowledgement == "unack" {
        println!("durable {operation} {id}");
    } else {
        println!("ack {operation} {id}");
    }
    io::stdout().flush()?;
    lines.next().transpose()?;
    Ok(())
}
