#[path = "../src/flush.rs"]
mod flush;
#[path = "support/typing.rs"]
mod typing;

use onestore::{
    document::{Format, Layout},
    op::{Edit, Op, PageOp},
    page::{
        Outline, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject, text::new_id,
    },
};
use std::{fs, io::Write};

/// A paragraph holding `text` in OneNote's default text style.
fn paragraph(text: &str) -> Result<PageParagraph, Box<dyn std::error::Error>> {
    let format = Format {
        font: Some("Calibri".into()),
        font_size: Some(11.0),
        language: Some(0x409),
        ..Format::default()
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
            text: Paragraph::new(text.into(), format),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let usage = "Usage: insert paragraph INPUT OUTPUT|--in-place SPACE CONTAINER BEFORE|- TEXT AUTHOR\n       insert outline INPUT OUTPUT|--in-place SPACE X Y TEXT AUTHOR";
    let (op, object, text, author) = match args.first().map(String::as_str) {
        Some("paragraph") if args.len() == 8 => {
            let paragraph = paragraph(&args[6])?;
            let (object, text) = (paragraph.id, paragraph.text().unwrap().id);
            let op = PageOp::Insert {
                container: args[4].parse()?,
                before: if args[5] == "-" {
                    None
                } else {
                    Some(args[5].parse()?)
                },
                paragraphs: vec![paragraph],
            };
            (op, object, text, &args[7])
        }
        Some("outline") if args.len() == 8 => {
            let paragraph = paragraph(&args[6])?;
            let text = paragraph.text().unwrap().id;
            let object = new_id()?;
            let op = PageOp::Add {
                object: PageObject::Outline(Outline {
                    id: object,
                    title: false,
                    min_width: None,
                    layout: Layout {
                        x: Some(args[4].parse()?),
                        y: Some(args[5].parse()?),
                        ..Layout::default()
                    },
                    indents: Vec::new(),
                    paragraphs: vec![paragraph],
                    unsupported: Vec::new(),
                }),
                before: None,
            };
            (op, object, text, &args[7])
        }
        _ => return Err(usage.into()),
    };
    let source = onestore::read_file(&args[1])?;
    let edit = Edit {
        at: typing::now(),
        ops: vec![Op::Page {
            space: args[3].parse()?,
            op,
        }],
    };
    let transaction =
        typing::sealed(&source, author, &edit)?.ok_or("The insertion stores nothing")?;
    let mut record = serde_json::json!({"edit": edit, "object": object, "text_object": text});
    if args[2] == "--in-place" {
        if let Err(error) = transaction.commit_file(&args[1]) {
            record["state"] = format!("{:?}", error.state).into();
            record["error"] = error.error.to_string().into();
            println!("{record}");
            std::process::exit(2);
        }
        record["state"] = "Committed".into();
    } else {
        let mut written = source;
        transaction.apply(&mut written)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[2])?;
        file.write_all(&written)?;
        flush::flush(&file)?;
        record["state"] = "Created".into();
    }
    println!("{record}");
    Ok(())
}
