//! OneNote 2010's Informal Meeting Notes content (Business templates;
//! `corpus/notebook-management/native/meeting-template`): four outlines of bold headings over
//! numbered, to-do and arrow-bulleted lines, placed where the template places them.

use onestore::{
    ExGuid,
    document::{Format, Kind, Layout, Tag},
    page::{
        Definition, MediaIndex, Outline, Page, PageObject, PageParagraph, ParagraphContent,
        TextObject, text::new_id,
    },
};
use std::error::Error;

/// OneNote's automatic colour.
const AUTOMATIC: u32 = 0xff00_0000;

/// A list line's marker.
enum Marker {
    Numbered,
    Arrow,
    ToDo,
    None,
}

/// A heading and the lines under it.
type Heading = (&'static str, &'static [(&'static str, Marker)]);

/// An outline of the template: position, width, indents, then each heading's lines.
type Block = ([f32; 2], f32, [f32; 4], &'static [Heading]);

const OUTLINES: [Block; 4] = [
    (
        [36.0, 77.4],
        141.75,
        [18.0, 0.0, 21.6, 36.0],
        &[("Agenda", &[("", Marker::Numbered)])],
    ),
    (
        [234.0, 77.4],
        137.25,
        [18.0, 0.0, 18.0, 36.0],
        &[("Action Items", &[("", Marker::ToDo)])],
    ),
    (
        [423.0, 77.4],
        136.5,
        [18.0, 0.0, 21.6, 36.0],
        &[("Important Dates", &[("", Marker::Arrow)])],
    ),
    (
        [36.0, 221.4],
        529.5,
        [18.0, 0.0, 36.0, 36.0],
        &[
            (
                "Meeting Details",
                &[
                    ("Date and Time:", Marker::Arrow),
                    ("Location:", Marker::Arrow),
                    ("Attendees:", Marker::Arrow),
                ],
            ),
            ("Announcements", &[("", Marker::Arrow)]),
            ("Discussion", &[("", Marker::Arrow)]),
            ("Summary", &[("", Marker::Arrow), ("", Marker::None)]),
            (
                "Next Meeting",
                &[
                    ("Date and Time:", Marker::Arrow),
                    ("Location:", Marker::Arrow),
                    ("Agenda:", Marker::Arrow),
                    ("Notes:", Marker::Arrow),
                ],
            ),
        ],
    ),
];

/// `page` with the template's outlines added after what it holds.
pub fn content(page: &Page) -> Result<Page, Box<dyn Error>> {
    let mut page = page.clone();
    let text = |bold: bool| Format {
        bold: Some(bold),
        italic: Some(false),
        underline: Some(false),
        strike: Some(false),
        superscript: Some(false),
        subscript: Some(false),
        font: Some("Calibri".into()),
        font_size: Some(11.0),
        color: Some(AUTOMATIC),
        highlight: Some(AUTOMATIC),
        language: Some(0x409),
        space_before: Some(0.0),
        space_after: Some(0.0),
        ..Format::default()
    };
    let style = new_id()?;
    page.definitions.insert(
        style,
        Definition {
            kind: Kind::Style {
                name: Some("p".into()),
                next: None,
            },
            format: Format {
                language: None,
                ..text(false)
            },
        },
    );
    // Tag dates count seconds since 1980.
    let created = u32::try_from(crate::filetime() / 10_000_000 - 11_644_473_600 - 315_532_800)?;
    let to_do = new_id()?;
    page.definitions.insert(
        to_do,
        Definition {
            kind: Kind::TagDefinition {
                label: Some("To Do".into()),
                action_type: Some(0),
                shape: Some(3),
                color: None,
                highlight: None,
            },
            format: Format::default(),
        },
    );
    let paragraph = |parent: Option<ExGuid>,
                     line: &str,
                     bold: bool,
                     lists: Vec<ExGuid>,
                     tags: Vec<Tag>|
     -> Result<PageParagraph, Box<dyn Error>> {
        Ok(PageParagraph {
            id: new_id()?,
            parent,
            level: if parent.is_some() { 2 } else { 1 },
            style: Some(style),
            format: Format::default(),
            content: ParagraphContent::Text(TextObject {
                id: new_id()?,
                date_field: None,
                text: onestore::page::Paragraph::new(line.to_owned(), text(bold)),
                // OneNote tags the line's text, not its paragraph.
                tags,
            }),
            lists,
            tags: Vec::new(),
            media: MediaIndex::default(),
            collapsed: false,
        })
    };
    for (position, width, indents, headings) in OUTLINES {
        let mut paragraphs = Vec::new();
        for (heading, lines) in headings {
            let parent = paragraph(None, heading, true, Vec::new(), Vec::new())?;
            let id = parent.id;
            paragraphs.push(parent);
            for (line, marker) in *lines {
                let (lists, tags) = match marker {
                    Marker::None => (Vec::new(), Vec::new()),
                    Marker::ToDo => (
                        Vec::new(),
                        vec![Tag {
                            definition: Some(to_do),
                            action_type: None,
                            shape: None,
                            property_status: None,
                            status: 0,
                            created: Some(created),
                            completed: Some(0),
                            start: None,
                            due: None,
                            task_id: None,
                            extra_set: 0,
                        }],
                    ),
                    Marker::Numbered | Marker::Arrow => {
                        let list = new_id()?;
                        page.definitions.insert(list, list_definition(marker));
                        (vec![list], Vec::new())
                    }
                };
                paragraphs.push(paragraph(Some(id), line, false, lists, tags)?);
            }
        }
        page.objects.push(PageObject::Outline(Outline {
            id: new_id()?,
            title: false,
            min_width: None,
            layout: Layout {
                x: Some(position[0]),
                y: Some(position[1]),
                max_width: Some(width),
                width_set_by_user: Some(true),
                ..Layout::default()
            },
            indents: indents.to_vec(),
            paragraphs,
            unsupported: Vec::new(),
        }));
    }
    Ok(page)
}

/// The template's `##.` numbering, or its Wingdings 3 arrow bullet.
fn list_definition(marker: &Marker) -> Definition {
    let format = Format {
        font_size: Some(11.0),
        color: Some(AUTOMATIC),
        ..Format::default()
    };
    match marker {
        Marker::Numbered => Definition {
            kind: Kind::List {
                font: None,
                format: Some("\u{fffd}\u{0}.".into()),
                restart: None,
                bullet: None,
            },
            format: Format {
                bold: Some(false),
                italic: Some(false),
                font: Some("Calibri".into()),
                language: Some(0x409),
                ..format
            },
        },
        _ => Definition {
            kind: Kind::List {
                font: Some("Wingdings 3".into()),
                format: Some("}".into()),
                restart: None,
                bullet: Some(11),
            },
            format,
        },
    }
}
