//! Times importing a generated page into a copy of a section, then typing one character
//! into it until durable and queued: `save_probe SECTION PARAGRAPHS`.

use notebook::session::Section;
use onestore::{
    document::{Format, Layout},
    op::{Edit, Op, PageOp},
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject,
        text::new_id,
    },
};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(source), Some(count)) = (args.next(), args.next()) else {
        return Err("usage: save_probe SECTION PARAGRAPHS".into());
    };
    let count: usize = count.parse()?;
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("probe.one");
    std::fs::write(&file, std::fs::read(source)?)?;
    let section = Section::open(&file, directory.path().join("cache"), || {})?;
    let words = [
        "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog",
    ];
    let paragraph = |i: usize| -> Result<PageParagraph, Box<dyn std::error::Error>> {
        let text: Vec<&str> = (0..1 + i % 17).map(|j| words[(i * 7 + j) % 8]).collect();
        Ok(PageParagraph {
            id: new_id()?,
            parent: None,
            level: 1,
            style: None,
            format: Format::default(),
            content: ParagraphContent::Text(TextObject {
                id: new_id()?,
                date_field: None,
                text: Paragraph::new(text.join(" "), Format::default()),
                tags: Vec::new(),
            }),
            lists: Vec::new(),
            tags: Vec::new(),
            media: Default::default(),
            collapsed: false,
        })
    };
    let imported = Page {
        title: "Probe".into(),
        identity: None,
        created: None,
        margin_origin: [0.0; 2],
        color: None,
        objects: vec![PageObject::Outline(Outline {
            id: new_id()?,
            title: false,
            min_width: None,
            layout: Layout {
                x: Some(36.0),
                y: Some(86.4),
                ..Layout::default()
            },
            indents: Vec::new(),
            paragraphs: (0..count).map(paragraph).collect::<Result<_, _>>()?,
            unsupported: Vec::new(),
        })],
        definitions: Default::default(),
    };
    let start = Instant::now();
    let space = section.import_page(&imported, "Probe")?;
    println!("import {count}: {:?}", start.elapsed());
    let text = section
        .page(space)?
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if outline.paragraphs.len() == count => {
                outline.paragraphs[count / 2].text().map(|text| text.id)
            }
            _ => None,
        })
        .ok_or("the imported outline is missing")?;
    let typed = |word: &str| Edit {
        at: 133_000_000_000_000_000,
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text,
                range: 0..0,
                with: word.into(),
            },
        }],
    };
    let start = Instant::now();
    section.replica().apply("Probe", typed("x"))?;
    println!("typed, durable: {:?}", start.elapsed());
    let start = Instant::now();
    section.apply("Probe", typed("y"))?;
    println!("typed, queued: {:?}", start.elapsed());
    let deadline = start + Duration::from_secs(600);
    while Instant::now() < deadline {
        if section.replica().pending()?.len() == 2 {
            println!("queued edit durable: {:?}", start.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    section.close()?;
    Ok(())
}
