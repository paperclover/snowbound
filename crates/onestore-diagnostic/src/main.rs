use onestore::{ExGuid, Insertion, PreparedEdit, TextAttribute};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env, fs,
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

fn run(args: &[std::ffi::OsString]) -> Result<Value, Box<dyn std::error::Error>> {
    if let [mode, root] = args
        && mode == "catalog"
    {
        let catalog = onestore_notebook::discover(
            &mut onestore_notebook::Local::open(root)?,
            onestore_notebook::Limits {
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
        let bytes = onestore::read_file(&args[1])?;
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
    let prepared = match edit.action {
        Action::Text {
            start,
            end,
            replacement,
        } => PreparedEdit::text(&bytes, sid, oid, start..end, &replacement)?,
        Action::Format {
            start,
            end,
            attributes,
        } => PreparedEdit::format(&bytes, sid, oid, start..end, &attributes)?,
        Action::Paragraph {
            before,
            text,
            author,
        } => PreparedEdit::insert(
            &bytes,
            sid,
            &Insertion::paragraph(oid, before, &text, &author)?,
        )?,
        Action::Outline { x, y, text, author } => {
            PreparedEdit::insert(&bytes, sid, &Insertion::outline(oid, x, y, &text, &author)?)?
        }
    };
    if check {
        return Ok(json!({"ok": true}));
    }
    Ok(match prepared.commit_file(&args[1]) {
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
