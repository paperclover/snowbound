use notebook::fs;
use onestore::{
    Arena, ExGuid, Section, TextAttribute,
    document::{Format, Layout},
    op::{Op, PageOp},
    page::{
        Outline, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject, text::new_id,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env,
    io::{self, Write},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    space: ExGuid,
    object: ExGuid,
    action: Action,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Action {
    Text {
        start: u32,
        end: u32,
        replacement: String,
    },
    Format {
        start: u32,
        end: u32,
        attributes: Vec<TextAttribute>,
    },
    Paragraph {
        before: Option<ExGuid>,
        text: String,
        author: String,
    },
    Outline {
        x: f32,
        y: f32,
        text: String,
        author: String,
    },
}

/// A new paragraph holding `text` in OneNote's default text style.
fn paragraph(text: String) -> Result<PageParagraph, Box<dyn std::error::Error>> {
    let format = Format {
        font: Some("Calibri".into()),
        font_size: Some(11.0),
        language: Some(0x409),
        ..Default::default()
    };
    Ok(PageParagraph {
        id: new_id()?,
        parent: None,
        level: 1,
        style: None,
        format: Format::default(),
        content: ParagraphContent::Text(TextObject {
            id: new_id()?,
            date_field: None,
            text: Paragraph::new(text, format),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    })
}

fn run(args: &[std::ffi::OsString]) -> Result<Value, Box<dyn std::error::Error>> {
    if let [mode, root] = args
        && mode == "catalog"
    {
        let catalog = notebook::discover::discover(
            &mut notebook::discover::Local::open(root)?,
            notebook::discover::Limits {
                entries: 100_000,
                bytes_per_file: 256 * 1024 * 1024,
                depth: 64,
            },
        )?;
        return Ok(json!({"ok": true, "catalog": catalog}));
    }
    if args.len() != 3
        || !["snapshot", "check", "commit"]
            .iter()
            .any(|mode| args[0] == *mode)
    {
        return Err("Usage: onestore-diagnostic catalog ROOT | snapshot FILE NEW_SNAPSHOT | check SNAPSHOT - | commit FILE SNAPSHOT; edits arrive as JSON on stdin".into());
    }
    if args[0] == "snapshot" {
        let bytes = fs::read_file(&args[1])?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[2])?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        return Ok(json!({"ok": true, "bytes": bytes.len()}));
    }
    let check = args[0] == "check";
    if check && args[2] != "-" {
        return Err("The check command requires '-' as its final argument".into());
    }
    let bytes = fs::read(if check { &args[1] } else { &args[2] })?;
    let edit: Edit = serde_json::from_reader(io::stdin().lock())?;
    let sid = edit.space;
    let oid = edit.object;
    let (op, author) = match edit.action {
        Action::Text {
            start,
            end,
            replacement,
        } => (
            PageOp::Text {
                text: oid,
                range: start..end,
                with: replacement,
            },
            String::new(),
        ),
        Action::Format {
            start,
            end,
            attributes,
        } => (
            PageOp::Format {
                text: oid,
                range: start..end,
                set: attributes,
                clear: Vec::new(),
            },
            String::new(),
        ),
        Action::Paragraph {
            before,
            text,
            author,
        } => (
            PageOp::Insert {
                container: oid,
                before,
                paragraphs: vec![paragraph(text)?],
            },
            author,
        ),
        Action::Outline { x, y, text, author } => {
            let store = onestore::Store::parse(&bytes)?;
            let index = onestore::RevisionIndex::parse(&store)?;
            if !onestore::document::Document::parse(&index)?
                .pages()?
                .contains(&(sid, oid))
            {
                return Err("Choose the page to hold the outline".into());
            }
            (
                PageOp::Add {
                    object: PageObject::Outline(Outline {
                        id: new_id()?,
                        title: false,
                        min_width: None,
                        layout: Layout {
                            x: Some(x),
                            y: Some(y),
                            ..Default::default()
                        },
                        indents: Vec::new(),
                        paragraphs: vec![paragraph(text)?],
                        unsupported: Vec::new(),
                    }),
                    before: None,
                },
                author,
            )
        }
    };
    let arena = Arena::default();
    let mut section = Section::open(&arena, bytes)?;
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let at = (unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(unix.subsec_nanos() / 100);
    section.apply(
        &author,
        &onestore::op::Edit {
            at,
            ops: vec![Op::Page { space: sid, op }],
        },
    )?;
    let transaction = section.seal()?;
    if check {
        return Ok(json!({"ok": true}));
    }
    let Some(transaction) = transaction else {
        return Ok(json!({"ok": true, "state": "Unchanged"}));
    };
    Ok(match fs::commit_file(&transaction, &args[1]) {
        Ok(()) => json!({"ok": true, "state": "Committed"}),
        Err(error) => json!({"ok": false, "state": format!("{:?}", error.state),
            "kind": format!("{:?}", error.error.kind()), "error": error.error.to_string()}),
    })
}

fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let result = run(&args).unwrap_or_else(|error| {
        let mut result = json!({"ok": false, "kind": "Input", "error": error.to_string()});
        if args.first().is_some_and(|mode| mode == "commit") {
            result["state"] = json!("NotCommitted");
        }
        result
    });
    println!("{result}");
}
