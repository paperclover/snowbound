//! Drives a section the way the application does: open it through a session, apply
//! edits, wait for their publication and report the page texts.
//! `session_client SECTION CACHE_DIR LABEL` edits the first body paragraph of every
//! page whose title starts with "Move" and adds one outline per page; each launch
//! reports the pending queue it found and the receipts it obtained.

use notebook::{
    EditStatus,
    session::{Event, Section},
};
use onestore::{
    ExGuid,
    op::PageOp,
    page::{Outline, Page, PageObject, PageParagraph, ParagraphContent, TextObject, text::new_id},
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

fn body_text(page: &Page) -> Option<ExGuid> {
    page.objects.iter().find_map(|object| match object {
        PageObject::Outline(outline) => outline
            .paragraphs
            .iter()
            .find_map(|p| p.text().map(|t| t.id)),
        _ => None,
    })
}

fn texts(page: &Page) -> Vec<String> {
    let mut out = Vec::new();
    for object in &page.objects {
        let outlines: Vec<&Outline> = match object {
            PageObject::Outline(outline) => vec![outline],
            PageObject::Title(title) => title.outlines.iter().collect(),
            _ => Vec::new(),
        };
        for outline in outlines {
            for paragraph in &outline.paragraphs {
                if let Some(text) = paragraph.text() {
                    out.push(text.text.text().to_owned());
                }
            }
        }
    }
    out
}

/// Types `label` before the first body paragraph and adds an outline below the body.
fn edit(page: &Page, label: &str) -> Vec<PageOp> {
    let text = body_text(page).expect("a body paragraph");
    let template = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find(|p| p.text().is_some())
                .cloned(),
            _ => None,
        })
        .unwrap();
    let mut paragraph: PageParagraph = template.clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    paragraph.content = ParagraphContent::Text(TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: onestore::page::Paragraph::new(
            format!("{label} outline 🦀 é"),
            template.text().unwrap().text.format_at(0).unwrap().clone(),
        ),
        tags: Vec::new(),
    });
    let outline = Outline {
        id: new_id().unwrap(),
        title: false,
        min_width: None,
        layout: onestore::document::Layout {
            x: Some(72.0),
            y: Some(520.0),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: vec![paragraph],
        unsupported: Vec::new(),
    };
    let title = page.objects.iter().find_map(|object| match object {
        PageObject::Title(title) => Some(title.id),
        _ => None,
    });
    vec![
        PageOp::Text {
            text,
            range: 0..0,
            with: format!("{label} "),
        },
        PageOp::Add {
            object: PageObject::Outline(outline),
            before: title,
        },
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, file, cache, label] = args.as_slice() else {
        return Err("Usage: session_client SECTION CACHE_DIR LABEL".into());
    };
    let (notify, notified) = mpsc::channel();
    let section = Section::open(file, cache, move || {
        let _ = notify.send(());
    })?;
    let pending = section.pending()?;
    println!(
        "{}",
        serde_json::json!({"event": "opened", "pending": pending.len(), "file": section.file()})
    );
    let pages = section.pages()?;
    let mut queued = Vec::new();
    for (space, title, _) in pages
        .iter()
        .filter(|(_, title, _)| title.starts_with("Move"))
    {
        let mut page = section.page(*space)?;
        let ops = edit(&page, label);
        for op in &ops {
            onestore::op::predict(&mut page, op)?;
        }
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
        let edit = onestore::op::Edit {
            at: (unix.as_secs() + 11_644_473_600) * 10_000_000,
            ops: ops
                .into_iter()
                .map(|op| onestore::op::Op::Page { space: *space, op })
                .collect(),
        };
        let id = section.replica().apply("session client", edit)?;
        queued.push((id, *space, title.clone(), texts(&page)));
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut receipts = Vec::new();
    while queued
        .iter()
        .any(|(id, ..)| !receipts.iter().any(|(n, _)| n == id))
    {
        if Instant::now() > deadline {
            return Err("Publication timed out".into());
        }
        let _ = notified.recv_timeout(Duration::from_millis(200));
        for event in section.events() {
            match event {
                Event::Attempt {
                    status: EditStatus::Published { .. },
                    ..
                } => {}
                Event::Attempt { id, status } => {
                    println!(
                        "{}",
                        serde_json::json!({"event": "attempt", "id": id, "status": format!("{status:?}")})
                    );
                }
                Event::Unreachable(error) => {
                    println!(
                        "{}",
                        serde_json::json!({"event": "unreachable", "error": error.to_string()})
                    );
                }
                Event::Failed(error) => return Err(error.into()),
                Event::Rejected { error, .. } => return Err(error.into()),
                Event::Changed(_) => {}
            }
        }
        // A batch reports its newest edit; each edit's receipt is its own.
        for (id, ..) in &queued {
            if !receipts.iter().any(|(n, _)| n == id)
                && let Some(EditStatus::Published { revision }) = section.status(*id)?
            {
                receipts.push((*id, revision));
            }
        }
    }
    for (id, space, title, expected) in &queued {
        let revision = receipts
            .iter()
            .find(|(n, _)| n == id)
            .map(|(_, r)| r.to_string());
        let stored = texts(&section.page(*space)?);
        println!(
            "{}",
            serde_json::json!({"event": "published", "id": id, "space": space.to_string(), "title": title,
            "revision": revision, "texts": expected, "stored": stored})
        );
    }
    assert!(section.pending()?.is_empty());
    section.close()?;
    println!("{}", serde_json::json!({"event": "closed"}));
    Ok(())
}
