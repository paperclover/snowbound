//! Drives random page-model mutations through `op::lower_page` and a `Section` and checks
//! the contract: the page the section holds is what the model oracle predicts from the ops,
//! the sealed image reads back as the edited model, and objects outside the edit keep their
//! bytes.

use onestore::{
    Arena, ExGuid, RevisionIndex, Section, Store,
    document::{Document, Format, Layout},
    op::{self, Op},
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent,
        text::{Edit, new_id},
    },
};
use std::{collections::BTreeSet, sync::LazyLock};

#[path = "current.rs"]
pub(crate) mod current;

static SOURCES: LazyLock<[Vec<u8>; 4]> = LazyLock::new(|| {
    [
        onestore::create_section("model.one", "Original 🦀 é 東京", "Author").unwrap(),
        include_bytes!("../../../../corpus/outline-edit/before/notebook/synthetic.one").to_vec(),
        include_bytes!("../../../../corpus/paragraph-edit/before/notebook/synthetic.one").to_vec(),
        include_bytes!("../../../../corpus/outline-edit/tree/before/notebook/synthetic.one")
            .to_vec(),
    ]
});

struct Bytes<'a> {
    input: &'a [u8],
    at: usize,
}

impl Bytes<'_> {
    fn next(&mut self) -> Option<u8> {
        let byte = *self.input.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    fn pick(&mut self, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        Some(usize::from(self.next().unwrap_or(0)) % count)
    }

    fn text(&mut self) -> String {
        let length = usize::from(self.next().unwrap_or(0)) % 12;
        let mut text = String::new();
        for _ in 0..length {
            text.push(match self.next().unwrap_or(b' ') % 12 {
                0 => '🦀',
                1 => 'é',
                2 => '\u{301}',
                3 => '東',
                4 => ' ',
                5 => '\u{05e9}',
                6 => '\u{000b}',
                n => (b'a' + n) as char,
            });
        }
        text
    }
}

fn model(bytes: &[u8], space: ExGuid) -> Option<Page> {
    let store = Store::parse(bytes).ok()?;
    let index = RevisionIndex::parse(&store).ok()?;
    let document = Document::parse(&index).ok()?;
    Page::from_space(&document, space).ok()
}

/// What the writer promises to reproduce exactly; other fields are writer- or reader-owned.
fn projection(page: &Page) -> String {
    let mut out = String::new();
    for object in &page.objects {
        match object {
            PageObject::Outline(outline) => {
                // A new outline without a width takes the writer's 468 pt default.
                out.push_str(&format!(
                    "outline {} {:?} {:?} {:?}\n",
                    outline.id,
                    outline.layout.x,
                    outline.layout.y,
                    outline.layout.max_width.unwrap_or(468.0)
                ));
                for paragraph in &outline.paragraphs {
                    out.push_str(&format!(
                        "  {} parent {:?} collapsed {}",
                        paragraph.id, paragraph.parent, paragraph.collapsed
                    ));
                    match &paragraph.content {
                        ParagraphContent::Text(text) => {
                            out.push_str(&format!(" text {} {:?}", text.id, text.text.text()));
                            // Runs that differ only in attributes the writer leaves as stored,
                            // such as the language tag, project as one span.
                            let mut runs: Vec<(usize, String)> = Vec::new();
                            for span in text.text.spans() {
                                let f = &span.format;
                                let attributes = format!(
                                    "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                                    f.bold.unwrap_or(false),
                                    f.italic.unwrap_or(false),
                                    f.underline.unwrap_or(false),
                                    f.strike.unwrap_or(false),
                                    f.superscript.unwrap_or(false),
                                    f.subscript.unwrap_or(false),
                                    f.font,
                                    f.font_size,
                                    f.color,
                                    f.highlight
                                );
                                match runs.last_mut() {
                                    Some((end, previous)) if *previous == attributes => {
                                        *end = span.end
                                    }
                                    _ => runs.push((span.end, attributes)),
                                }
                            }
                            for (end, attributes) in runs {
                                out.push_str(&format!(" [{end} {attributes}]"));
                            }
                        }
                        ParagraphContent::Table(table) => {
                            out.push_str(&format!(" table {}", table.id))
                        }
                        ParagraphContent::Image(image) => {
                            out.push_str(&format!(" image {}", image.id))
                        }
                        ParagraphContent::Attachment(attachment) => {
                            out.push_str(&format!(" attachment {}", attachment.id))
                        }
                        ParagraphContent::Ink(ink) => out.push_str(&format!(" ink {}", ink.id)),
                        ParagraphContent::Unsupported(u) => {
                            out.push_str(&format!(" unsupported {}", u.id))
                        }
                    }
                    out.push('\n');
                }
            }
            PageObject::Title(title) => out.push_str(&format!("title {}\n", title.id)),
            PageObject::Image(image) => out.push_str(&format!("image {}\n", image.id)),
            PageObject::Attachment(file) => out.push_str(&format!("file {}\n", file.id)),
            PageObject::Ink(ink) => out.push_str(&format!("ink {}\n", ink.id)),
            PageObject::Unsupported(u) => out.push_str(&format!("unsupported {}\n", u.id)),
        }
    }
    out
}

fn text_paragraphs(outline: &mut Outline) -> Vec<usize> {
    outline
        .paragraphs
        .iter()
        .enumerate()
        .filter(|(_, p)| p.text().is_some())
        .map(|(i, _)| i)
        .collect()
}

fn fresh_paragraph(template: &PageParagraph, text: String) -> PageParagraph {
    let mut paragraph = template.clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    let format = template
        .text()
        .map(|t| t.text.format_at(0).unwrap().clone())
        .unwrap_or_default();
    paragraph.content = ParagraphContent::Text(onestore::page::TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: Paragraph::new(text, format),
        tags: Vec::new(),
    });
    paragraph
}

/// Removes a paragraph and every descendant, returning the removed identities.
fn remove_subtree(outline: &mut Outline, index: usize) -> BTreeSet<ExGuid> {
    let mut removed = BTreeSet::from([outline.paragraphs[index].id]);
    loop {
        let before = removed.len();
        for paragraph in &outline.paragraphs {
            if paragraph.parent.is_some_and(|p| removed.contains(&p)) {
                removed.insert(paragraph.id);
            }
        }
        if removed.len() == before {
            break;
        }
    }
    outline.paragraphs.retain(|p| !removed.contains(&p.id));
    removed
}

fn mutate(page: &mut Page, bytes: &mut Bytes<'_>) {
    let steps = usize::from(bytes.next().unwrap_or(0)) % 6 + 1;
    for _ in 0..steps {
        let Some(kind) = bytes.next() else { return };
        let outlines: Vec<usize> = page
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| matches!(o, PageObject::Outline(_)))
            .map(|(i, _)| i)
            .collect();
        match kind % 11 {
            10 => {
                let Some(outline) = outlines.first().map(|i| match &mut page.objects[*i] {
                    PageObject::Outline(outline) => outline,
                    _ => unreachable!(),
                }) else {
                    continue;
                };
                let at = usize::from(bytes.next().unwrap_or(0)) % outline.paragraphs.len().max(1);
                let Some(text) = outline.paragraphs.get_mut(at).and_then(|p| p.text_mut()) else {
                    continue;
                };
                if text.tags.is_empty() {
                    let definition = page
                        .definitions
                        .iter()
                        .find(|(_, d)| {
                            matches!(&d.kind, onestore::document::Kind::TagDefinition { label: Some(label), .. } if label == "Fuzz task")
                        })
                        .map(|(id, _)| *id)
                        .unwrap_or_else(|| {
                            let id = new_id().unwrap();
                            page.definitions.insert(
                                id,
                                onestore::page::Definition {
                                    kind: onestore::document::Kind::TagDefinition {
                                        label: Some("Fuzz task".into()),
                                        action_type: Some(0),
                                        shape: Some(3),
                                        color: None,
                                        highlight: None,
                                    },
                                    format: Default::default(),
                                },
                            );
                            id
                        });
                    let outline = outlines.first().map(|i| match &mut page.objects[*i] {
                        PageObject::Outline(outline) => outline,
                        _ => unreachable!(),
                    });
                    let text = outline.unwrap().paragraphs[at].text_mut().unwrap();
                    text.tags.push(onestore::document::Tag {
                        definition: Some(definition),
                        action_type: None,
                        status: u16::from(bytes.next().unwrap_or(0) % 2),
                        created: Some(1_262_401_445),
                        completed: None,
                        start: None,
                        due: None,
                        task_id: None,
                        extra_set: 0,
                    });
                } else {
                    text.tags.clear();
                }
            }
            8 => {
                let Some(outline) = outlines.first().map(|i| match &mut page.objects[*i] {
                    PageObject::Outline(outline) => outline,
                    _ => unreachable!(),
                }) else {
                    continue;
                };
                let at = usize::from(bytes.next().unwrap_or(0)) % outline.paragraphs.len().max(1);
                let Some(paragraph) = outline.paragraphs.get_mut(at) else {
                    continue;
                };
                if paragraph.text().is_none() {
                    continue;
                }
                if paragraph.lists.is_empty() {
                    let id = new_id().unwrap();
                    page.definitions.insert(
                        id,
                        onestore::page::Definition {
                            kind: onestore::document::Kind::List {
                                font: Some("Courier New".into()),
                                format: Some("\u{25cb}".into()),
                                restart: None,
                                bullet: Some(4),
                            },
                            format: onestore::document::Format {
                                font_size: Some(11.0),
                                color: Some(0xff000000),
                                ..Default::default()
                            },
                        },
                    );
                    paragraph.lists = vec![id];
                } else {
                    paragraph.lists.clear();
                }
            }
            0 | 1 => {
                let Some(o) = bytes.pick(outlines.len()) else {
                    continue;
                };
                let PageObject::Outline(outline) = &mut page.objects[outlines[o]] else {
                    continue;
                };
                let texts = text_paragraphs(outline);
                let Some(p) = bytes.pick(texts.len()) else {
                    continue;
                };
                let text = outline.paragraphs[texts[p]].text_mut().unwrap();
                let end = text.text.utf16_offset(text.text.text().len()).unwrap();
                let start = bytes.next().map_or(0, |b| u32::from(b) % (end + 1));
                let stop = bytes
                    .next()
                    .map_or(end, |b| start + u32::from(b) % (end - start + 1));
                let format = text.text.format_at(start).cloned().unwrap_or_default();
                let replacement = bytes.text();
                let _ = text.text.apply(Edit {
                    range: start..stop,
                    replacement: Paragraph::new(replacement, format),
                });
            }
            2 => {
                let Some(o) = bytes.pick(outlines.len()) else {
                    continue;
                };
                let PageObject::Outline(outline) = &mut page.objects[outlines[o]] else {
                    continue;
                };
                let texts = text_paragraphs(outline);
                let Some(p) = bytes.pick(texts.len()) else {
                    continue;
                };
                let text = outline.paragraphs[texts[p]].text_mut().unwrap();
                let end = text.text.utf16_offset(text.text.text().len()).unwrap();
                let start = bytes.next().map_or(0, |b| u32::from(b) % (end + 1));
                let stop = bytes
                    .next()
                    .map_or(end, |b| start + u32::from(b) % (end - start + 1));
                let Ok(mut slice) = text.text.slice(start..stop) else {
                    continue;
                };
                let attribute = bytes.next().unwrap_or(0) % 8;
                let runs: Vec<(String, Format)> = {
                    let mut runs = Vec::new();
                    let mut from = 0;
                    for span in slice.spans() {
                        let mut format = span.format.clone();
                        match attribute {
                            0 => format.bold = Some(!format.bold.unwrap_or(false)),
                            1 => format.italic = Some(!format.italic.unwrap_or(false)),
                            2 => format.underline = Some(!format.underline.unwrap_or(false)),
                            3 => format.strike = Some(!format.strike.unwrap_or(false)),
                            4 => {
                                format.font_size = Some(if format.font_size == Some(14.0) {
                                    11.0
                                } else {
                                    14.0
                                })
                            }
                            5 => {
                                format.color = Some(if format.color == Some(0x00ff) {
                                    0xff000000
                                } else {
                                    0x00ff
                                })
                            }
                            6 => format.highlight = Some(0x00ffff),
                            _ => format.font = Some("Consolas".into()),
                        }
                        runs.push((slice.text()[from..span.end].to_owned(), format));
                        from = span.end;
                    }
                    runs
                };
                slice = Paragraph::from_runs(runs);
                let _ = text.text.apply(Edit {
                    range: start..stop,
                    replacement: slice,
                });
            }
            3 => {
                let Some(o) = bytes.pick(outlines.len()) else {
                    continue;
                };
                let PageObject::Outline(outline) = &mut page.objects[outlines[o]] else {
                    continue;
                };
                let Some(template) = outline
                    .paragraphs
                    .iter()
                    .find(|p| p.text().is_some())
                    .cloned()
                else {
                    continue;
                };
                let mut fresh = fresh_paragraph(&template, bytes.text());
                // Only subtree boundaries keep the pre-order flattening valid.
                let boundaries: Vec<usize> = (0..=outline.paragraphs.len())
                    .filter(|i| {
                        outline
                            .paragraphs
                            .get(*i)
                            .is_none_or(|p| p.parent.is_none())
                    })
                    .collect();
                let at = boundaries[bytes.pick(boundaries.len()).unwrap_or(0)];
                if bytes.next().unwrap_or(0).is_multiple_of(3)
                    && let Some(parent) = at.checked_sub(1).map(|i| &outline.paragraphs[i])
                {
                    fresh.parent = Some(parent.id);
                    fresh.level = parent.level + 1;
                }
                outline.paragraphs.insert(at, fresh);
            }
            4 => {
                let Some(o) = bytes.pick(outlines.len()) else {
                    continue;
                };
                let PageObject::Outline(outline) = &mut page.objects[outlines[o]] else {
                    continue;
                };
                let Some(index) = bytes.pick(outline.paragraphs.len()) else {
                    continue;
                };
                remove_subtree(outline, index);
            }
            5 => {
                let Some(o) = bytes.pick(outlines.len()) else {
                    continue;
                };
                let PageObject::Outline(outline) = &mut page.objects[outlines[o]] else {
                    continue;
                };
                let top: Vec<usize> = outline
                    .paragraphs
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.parent.is_none())
                    .map(|(i, _)| i)
                    .collect();
                let (Some(from), Some(to)) = (bytes.pick(top.len()), bytes.pick(top.len())) else {
                    continue;
                };
                let moving = outline.paragraphs[top[from]].clone();
                let mut subtree = remove_subtree(outline, top[from]);
                subtree.remove(&moving.id);
                let mut descendants: Vec<PageParagraph> = Vec::new();
                let mut rest = Vec::new();
                for paragraph in outline.paragraphs.drain(..) {
                    if subtree.contains(&paragraph.id) {
                        descendants.push(paragraph);
                    } else {
                        rest.push(paragraph);
                    }
                }
                outline.paragraphs = rest;
                let anchor = outline
                    .paragraphs
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.parent.is_none())
                    .nth(to.min(top.len().saturating_sub(1)))
                    .map(|(i, _)| i)
                    .unwrap_or(outline.paragraphs.len());
                let mut block = vec![moving];
                block.extend(descendants);
                for (offset, paragraph) in block.into_iter().enumerate() {
                    outline.paragraphs.insert(anchor + offset, paragraph);
                }
            }
            6 => {
                let Some(o) = bytes.pick(outlines.len()) else {
                    continue;
                };
                let PageObject::Outline(outline) = &mut page.objects[outlines[o]] else {
                    continue;
                };
                let Some(index) = bytes.pick(outline.paragraphs.len()) else {
                    continue;
                };
                outline.paragraphs[index].collapsed ^= true;
            }
            7 => {
                let Some(o) = bytes.pick(outlines.len()) else {
                    continue;
                };
                let PageObject::Outline(outline) = &mut page.objects[outlines[o]] else {
                    continue;
                };
                if outline.title {
                    continue;
                }
                match bytes.next().unwrap_or(0) % 3 {
                    0 => {
                        outline.layout.x = Some(f32::from(bytes.next().unwrap_or(0)) * 1.5);
                        outline.layout.y = Some(f32::from(bytes.next().unwrap_or(0)) * 2.25);
                    }
                    1 => {
                        outline.layout.max_width =
                            Some(36.0 + f32::from(bytes.next().unwrap_or(0)) * 2.0);
                        outline.layout.width_set_by_user = Some(true);
                    }
                    _ => {
                        outline.layout.max_width =
                            Some(36.0 + f32::from(bytes.next().unwrap_or(0)) * 2.0);
                        outline.layout.width_set_by_user = None;
                    }
                }
            }
            _ => {
                let Some(template) = page.objects.iter().find_map(|o| match o {
                    PageObject::Outline(outline) => outline
                        .paragraphs
                        .iter()
                        .find(|p| p.text().is_some())
                        .cloned(),
                    _ => None,
                }) else {
                    continue;
                };
                let outline = Outline {
                    id: new_id().unwrap(),
                    title: false,
                    min_width: None,
                    layout: Layout {
                        x: Some(f32::from(bytes.next().unwrap_or(0)) * 1.5),
                        y: Some(f32::from(bytes.next().unwrap_or(0)) * 2.25),
                        ..Default::default()
                    },
                    indents: Vec::new(),
                    paragraphs: vec![fresh_paragraph(&template, bytes.text())],
                    unsupported: Vec::new(),
                };
                // Titles live in the page's structure list and always follow its children.
                let children = page
                    .objects
                    .iter()
                    .position(|o| matches!(o, PageObject::Title(_)))
                    .unwrap_or(page.objects.len());
                let at = bytes.pick(children + 1).unwrap_or(children);
                page.objects.insert(at, PageObject::Outline(outline));
            }
        }
    }
}

pub fn run(input: &[u8]) {
    let mut bytes = Bytes { input, at: 0 };
    let source = &SOURCES[usize::from(bytes.next().unwrap_or(0)) % SOURCES.len()];
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let pages = document.pages().unwrap();
    let Some(page_index) = bytes.pick(pages.len()) else {
        return;
    };
    let (space, page_object) = pages[page_index];
    let Some(mut after) = model(source, space) else {
        return;
    };
    let before = after.clone();
    mutate(&mut after, &mut bytes);
    let Ok(ops) = onestore::op::lower_page(&before, &after) else {
        return;
    };
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.clone()).unwrap();
    let edit = op::Edit {
        at: 134_000_000_000_000_000,
        ops: ops
            .iter()
            .map(|op| Op::Page {
                space,
                op: op.clone(),
            })
            .collect(),
    };
    if section.apply("Fuzz author", &edit).is_err() {
        assert_eq!(
            section.page(space).unwrap(),
            before,
            "a refused edit changes nothing"
        );
        return;
    }
    let mut predicted = before.clone();
    for op in &ops {
        onestore::op::predict(&mut predicted, op).unwrap();
    }
    assert_eq!(
        projection(&section.page(space).unwrap()),
        projection(&predicted),
        "the model oracle"
    );
    if section.seal().unwrap().is_none() {
        assert_eq!(
            projection(&before),
            projection(&after),
            "an unchanged model publishes nothing"
        );
        return;
    }
    let written = &section.image();
    current::current(written);
    let stored = model(written, space).expect("the written page reads back");
    let (stored, expected) = (projection(&stored), projection(&after));
    if stored != expected {
        let differences: Vec<String> = stored
            .lines()
            .zip(expected.lines())
            .filter(|(a, b)| a != b)
            .map(|(a, b)| format!("stored:   {a}\nexpected: {b}"))
            .collect();
        panic!(
            "model round trip ({} vs {} lines):\n{}",
            stored.lines().count(),
            expected.lines().count(),
            differences.join("\n")
        );
    }
    let raw_before = index.resolve_active(space).unwrap();
    let store = Store::parse(written).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let raw_after = index.resolve_active(space).unwrap();
    let mut modeled = BTreeSet::from([page_object]);
    for object in &after.objects {
        modeled.insert(object.id());
        let outlines: Vec<&Outline> = match object {
            PageObject::Outline(outline) => vec![outline],
            PageObject::Title(title) => title.outlines.iter().collect(),
            _ => Vec::new(),
        };
        for outline in outlines {
            modeled.insert(outline.id);
            for paragraph in &outline.paragraphs {
                modeled.insert(paragraph.id);
                if let Some(text) = paragraph.text() {
                    modeled.insert(text.id);
                }
            }
        }
    }
    // Revision roots carry derived metadata such as the automatic navigation title, and
    // outline groups (0x60019) are containers the model flattens but the writers normalize.
    let roots: BTreeSet<ExGuid> = raw_before.roots.values().copied().collect();
    for (id, object) in &raw_before.objects {
        if modeled.contains(id)
            || roots.contains(id)
            || matches!(object.jcid, 0x12004d | 0x120001 | 0x60019)
        {
            continue;
        }
        let Some(after) = raw_after.objects.get(id) else {
            panic!("{id} disappeared from the active revision");
        };
        assert_eq!(
            (object.jcid, object.data),
            (after.jcid, after.data),
            "{id} changed outside the model"
        );
    }
}
