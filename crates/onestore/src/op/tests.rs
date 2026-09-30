//! Ops against the model: for random edits of corpus pages, lowering the edit and applying
//! the ops through a `Section` stores the edited page, as the model oracle predicts and as
//! the sealed image reads back.

use super::*;
use crate::{
    Arena, RevisionIndex, Section, Store,
    document::{Document, Format, Kind, Layout},
    page::{
        Attachment, Definition, Image, Ink, InkStroke, Outline, PageObject, PageParagraph,
        ParagraphContent, Table, TableCell, TableColumn, TableRow, TextObject, VerticalRule,
        text::new_id,
    },
    write::GUIDS,
};
use std::collections::{BTreeMap, BTreeSet};

const SOURCES: &[(&str, &[u8])] = &[
    (
        "outline-edit",
        include_bytes!("../../../../corpus/outline-edit/before/notebook/synthetic.one"),
    ),
    (
        "paragraph-edit",
        include_bytes!("../../../../corpus/paragraph-edit/before/notebook/synthetic.one"),
    ),
    (
        "outline-tree",
        include_bytes!("../../../../corpus/outline-edit/tree/before/notebook/synthetic.one"),
    ),
    (
        "features",
        include_bytes!("../../../../corpus/m6/native-features-01/notebook/Features.one"),
    ),
    (
        "tables",
        include_bytes!("../../../../corpus/table-edit/nested/cold/notebook/synthetic.one"),
    ),
    (
        "table-controls",
        include_bytes!("../../../../corpus/m6/native-table-controls-01/notebook/synthetic.one"),
    ),
    (
        "lists",
        include_bytes!("../../../../corpus/list-edit/cold/notebook/lists.one"),
    ),
    (
        "tags",
        include_bytes!("../../../../corpus/tag-edit/cold/notebook/tags.one"),
    ),
    (
        "pictures",
        include_bytes!("../../../../corpus/picture-edit/native-page-level/notebook/pictures.one"),
    ),
    (
        "attachments",
        include_bytes!("../../../../corpus/attachment-edit/plain/cold/notebook/files.one"),
    ),
    (
        "ink",
        include_bytes!("../../../../corpus/ink-edit/drawing/cold/notebook/ink.one"),
    ),
    (
        "math",
        include_bytes!("../../../../corpus/math-edit/native-editor/notebook/links.one"),
    ),
    (
        "links",
        include_bytes!("../../../../corpus/link-edit/native-links/notebook/links.one"),
    ),
    (
        "paragraph-format",
        include_bytes!("../../../../corpus/paragraph-format/cold/notebook/synthetic.one"),
    ),
    (
        "enter-probe",
        include_bytes!("../../../../corpus/structural-probe/probe.one"),
    ),
];

/// FILETIME both writers take as the edit's time.
const AT: u64 = 133_700_000_000_000_000;

pub(super) struct Rng(pub(super) u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn coin(&mut self) -> bool {
        self.next().is_multiple_of(2)
    }

    fn pick(&mut self, count: usize) -> Option<usize> {
        (count > 0).then(|| self.next() as usize % count)
    }

    fn text(&mut self) -> String {
        let words = [
            "",
            "a",
            "Two words",
            "東京 🦀",
            "é\u{301}",
            "longer text here",
            " ",
        ];
        words[self.next() as usize % words.len()].to_owned()
    }
}

pub(super) fn seeded<T>(seed: u64, f: impl FnOnce() -> T) -> T {
    let outer = GUIDS.replace(Some(seed));
    let result = f();
    GUIDS.set(outer);
    result
}

fn pages(image: &[u8]) -> Vec<ExGuid> {
    let store = Store::parse(image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| space)
        .collect()
}

fn read(image: &[u8], space: ExGuid) -> Page {
    let store = Store::parse(image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    Page::from_space(&Document::parse(&index).unwrap(), space).unwrap()
}

/// `page` with writer-chosen identities replaced in document order: list nodes, which a
/// copy for a second owner gets (so equal definitions become one), tag field indices, and ink
/// coordinates at the resolution they are stored in.
pub(super) fn normalize(page: &Page) -> Page {
    let mut page = page.clone();
    let mut lists = BTreeMap::new();
    let mut canonical = BTreeMap::new();
    let mut paths: Vec<_> = model::lists(&page)
        .into_iter()
        .map(|(path, _, _)| path)
        .collect();
    paths.sort_by_key(|path| format!("{path:?}"));
    let definitions = page.definitions.clone();
    for path in &paths {
        for paragraph in model::list_at(&mut page, path).iter_mut() {
            for list in &mut paragraph.lists {
                let next = ExGuid {
                    guid: [0xee; 16],
                    n: 1 + canonical.len() as u32,
                };
                let content = definitions
                    .get(list)
                    .map_or(format!("{list:?}"), |d| format!("{d:?}"));
                let id = *canonical.entry(content).or_insert(next);
                *list = *lists.entry(*list).or_insert(id);
            }
            for tag in paragraph.tags.iter_mut() {
                tag.extra_set = 0;
            }
            if let Some(text) = paragraph.text_mut() {
                for tag in &mut text.tags {
                    tag.extra_set = 0;
                }
                // An absent paragraph value and its default are the same formatting.
                let mut previous = 0;
                let runs: Vec<(String, Format)> = text
                    .text
                    .spans()
                    .iter()
                    .map(|span| {
                        let mut format = span.format.clone();
                        format.alignment = format.alignment.filter(|v| *v != 0);
                        format.rtl = format.rtl.filter(|v| *v);
                        format.space_before = format.space_before.filter(|v| *v != 0.0);
                        format.space_after = format.space_after.filter(|v| *v != 0.0);
                        format.line_spacing = format.line_spacing.filter(|v| *v != 0.0);
                        let run = (text.text.text()[previous..span.end].to_owned(), format);
                        previous = span.end;
                        run
                    })
                    .collect();
                text.text = Paragraph::from_runs(runs);
            }
        }
    }
    page.definitions = std::mem::take(&mut page.definitions)
        .into_iter()
        .filter_map(|(id, definition)| match definition.kind {
            Kind::List { .. } => lists.get(&id).map(|id| (*id, definition)),
            _ => Some((id, definition)),
        })
        .collect();
    fn quantize(ink: &mut Ink) {
        for stroke in &mut ink.strokes {
            for point in &mut stroke.points {
                *point = point.map(|value| (value * 10.0).round() / 10.0);
            }
        }
        ink.groups.iter_mut().for_each(quantize);
    }
    for object in &mut page.objects {
        if let PageObject::Ink(ink) = object {
            quantize(ink);
        }
    }
    for path in &paths {
        for paragraph in model::list_at(&mut page, path).iter_mut() {
            if let ParagraphContent::Ink(ink) = &mut paragraph.content {
                quantize(ink);
            }
        }
    }
    page
}

/// `after` with what the writers derive rather than store as given: the title from the
/// title text, the values they give new objects where the model leaves them unset, and only
/// the definitions something uses.
fn as_written(after: &Page, before: &Page, stored: &Page) -> Page {
    use serde_json::Value;
    fn ids(value: &Value, out: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(id)) = map.get("id") {
                    out.insert(id.clone());
                }
                map.values().for_each(|value| ids(value, out));
            }
            Value::Array(values) => values.iter().for_each(|value| ids(value, out)),
            _ => {}
        }
    }
    fn adopt(after: &mut Value, stored: &Value, old: &BTreeSet<String>, new: bool) {
        match (after, stored) {
            (Value::Object(after), Value::Object(stored)) => {
                let new =
                    new || matches!(after.get("id"), Some(Value::String(id)) if !old.contains(id));
                for (key, value) in after.iter_mut() {
                    let Some(written) = stored.get(key) else {
                        continue;
                    };
                    // Ink coordinates are stored at a coarser resolution.
                    let near = |a: &Value, b: &Value| {
                        a.as_f64()
                            .zip(b.as_f64())
                            .is_some_and(|(a, b)| (a - b).abs() < 0.05)
                    };
                    let close_points = key == "points"
                        && value
                            .as_array()
                            .zip(written.as_array())
                            .is_some_and(|(a, b)| {
                                a.len() == b.len()
                                    && a.iter().zip(b).all(|(a, b)| {
                                        a.as_array().zip(b.as_array()).is_some_and(|(a, b)| {
                                            a.len() == b.len()
                                                && a.iter().zip(b).all(|(a, b)| near(a, b))
                                        })
                                    })
                            });
                    let unset =
                        new && (value.is_null() || value.as_array().is_some_and(Vec::is_empty));
                    if close_points || unset {
                        *value = written.clone();
                    } else {
                        adopt(value, written, old, new);
                    }
                }
            }
            (Value::Array(after), Value::Array(stored)) if after.len() == stored.len() => {
                for (value, written) in after.iter_mut().zip(stored) {
                    adopt(value, written, old, new);
                }
            }
            _ => {}
        }
    }
    let mut old = BTreeSet::new();
    ids(&serde_json::to_value(before).unwrap(), &mut old);
    let mut value = serde_json::to_value(after).unwrap();
    adopt(
        &mut value,
        &serde_json::to_value(stored).unwrap(),
        &old,
        false,
    );
    let mut page: Page = serde_json::from_value(value).unwrap();
    page.title = stored.title.clone();
    unused_dropped(&mut page);
    page
}

/// Drops the definitions nothing on `page` uses, as reading a page leaves them out.
fn unused_dropped(page: &mut Page) {
    let mut used = BTreeSet::new();
    for (_, _, list) in model::lists(page) {
        for paragraph in list {
            used.extend(paragraph.style);
            used.extend(paragraph.lists.iter().copied());
            used.extend(paragraph.tags.iter().filter_map(|tag| tag.definition));
            if let ParagraphContent::Table(table) = &paragraph.content {
                used.extend(table.tags.iter().filter_map(|tag| tag.definition));
            }
            if let Some(text) = paragraph.text() {
                used.extend(text.tags.iter().filter_map(|tag| tag.definition));
            }
        }
    }
    page.definitions.retain(|id, _| used.contains(id));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Family {
    Text,
    Format,
    Insert,
    Paste,
    Delete,
    Move,
    Collapse,
    Position,
    Width,
    NewOutline,
    List,
    Tag,
    Split,
    Join,
    Indent,
    Outdent,
    Alignment,
    Style,
    TableRow,
    TableColumn,
    TableDelete,
    TableWidth,
    TableShading,
    NewTable,
    PagePicture,
    PictureEdit,
    ParagraphPicture,
    Attachment,
    AttachmentRename,
    NewInk,
    InkStrokes,
    InkPosition,
    Link,
    Equation,
}

pub(super) const FAMILIES: [Family; 34] = [
    Family::Text,
    Family::Format,
    Family::Insert,
    Family::Paste,
    Family::Delete,
    Family::Move,
    Family::Collapse,
    Family::Position,
    Family::Width,
    Family::NewOutline,
    Family::List,
    Family::Tag,
    Family::Split,
    Family::Join,
    Family::Indent,
    Family::Outdent,
    Family::Alignment,
    Family::Style,
    Family::TableRow,
    Family::TableColumn,
    Family::TableDelete,
    Family::TableWidth,
    Family::TableShading,
    Family::NewTable,
    Family::PagePicture,
    Family::PictureEdit,
    Family::ParagraphPicture,
    Family::Attachment,
    Family::AttachmentRename,
    Family::NewInk,
    Family::InkStrokes,
    Family::InkPosition,
    Family::Link,
    Family::Equation,
];

const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn id() -> ExGuid {
    new_id().unwrap()
}

/// `ops` on page `space` of `source` as one edit, sealed: the image they leave.
pub(crate) fn edited(source: &[u8], space: ExGuid, ops: Vec<PageOp>) -> Result<Vec<u8>, OpError> {
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.to_vec()).map_err(OpError::Failed)?;
    let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
    section.apply("Author", &Edit { at: AT, ops })?;
    section.seal().map_err(OpError::Failed)?;
    Ok(section.image())
}

pub(crate) fn text_paragraph(text: &str, format: Format) -> PageParagraph {
    PageParagraph {
        id: id(),
        parent: None,
        level: 1,
        style: None,
        format: Format::default(),
        content: ParagraphContent::Text(TextObject {
            id: id(),
            date_field: None,
            text: Paragraph::new(text.to_owned(), format),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    }
}

/// Paths of the page's editable lists: ordinary outlines and their cells.
fn editable(page: &Page) -> Vec<model::Path> {
    let titles: Vec<ExGuid> = page
        .objects
        .iter()
        .filter_map(|o| match o {
            PageObject::Title(title) => {
                Some(title.outlines.iter().map(|o| o.id).collect::<Vec<_>>())
            }
            _ => None,
        })
        .flatten()
        .collect();
    model::lists(page)
        .into_iter()
        .filter(|(_, owner, list)| !titles.contains(owner) && !list.is_empty())
        .map(|(path, _, _)| path)
        .collect()
}

/// A text paragraph of an editable list: its list and index.
fn pick_text(page: &Page, rng: &mut Rng) -> Option<(model::Path, usize)> {
    let candidates: Vec<(model::Path, usize)> = editable(page)
        .into_iter()
        .flat_map(|path| {
            let list = model::lists(page)
                .into_iter()
                .find(|(p, _, _)| format!("{p:?}") == format!("{path:?}"))
                .unwrap()
                .2
                .clone();
            list.iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.text().is_some_and(|t| {
                        t.date_field.is_none() && !crate::page::Math::is_equation(&t.text)
                    })
                })
                .map(|(i, _)| (path.clone(), i))
                .collect::<Vec<_>>()
        })
        .collect();
    candidates.get(rng.pick(candidates.len())?).cloned()
}

fn outlines(page: &mut Page) -> Vec<&mut Outline> {
    page.objects
        .iter_mut()
        .filter_map(|o| match o {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .collect()
}

fn subtree_end(list: &[PageParagraph], index: usize) -> usize {
    let mut members = vec![list[index].id];
    let mut end = index + 1;
    while end < list.len() && list[end].parent.is_some_and(|p| members.contains(&p)) {
        members.push(list[end].id);
        end += 1;
    }
    end
}

fn tables(page: &mut Page) -> Vec<&mut Table> {
    let mut out = Vec::new();
    for outline in outlines(page) {
        for paragraph in &mut outline.paragraphs {
            if let ParagraphContent::Table(table) = &mut paragraph.content {
                out.push(table);
            }
        }
    }
    out
}

fn new_cell(indents: &[f32]) -> TableCell {
    TableCell {
        id: id(),
        layout: Layout::default(),
        indents: indents.to_vec(),
        shading: None,
        paragraphs: vec![text_paragraph("cell", Format::default())],
        unsupported: Vec::new(),
    }
}

/// Applies one random edit of `family` to `page`; false when the page offers none.
pub(super) fn mutate(page: &mut Page, rng: &mut Rng, family: Family) -> bool {
    match family {
        Family::Text | Family::Format | Family::Link => {
            let Some((path, index)) = pick_text(page, rng) else {
                return false;
            };
            let text = &mut model::list_at(page, &path)[index].text_mut().unwrap().text;
            let end = text.utf16_offset(text.text().len()).unwrap();
            let chars: Vec<u32> = text
                .text()
                .char_indices()
                .map(|(i, _)| text.utf16_offset(i).unwrap())
                .chain([end])
                .collect();
            let a = chars[rng.pick(chars.len()).unwrap()];
            let b = chars[rng.pick(chars.len()).unwrap()];
            let range = a.min(b)..a.max(b);
            match family {
                Family::Text => {
                    let format = text.format_at(range.start).unwrap().clone();
                    text.apply(crate::page::text::Edit {
                        range,
                        replacement: Paragraph::new(rng.text(), format),
                    })
                    .is_ok()
                }
                Family::Format => {
                    if range.is_empty() {
                        return false;
                    }
                    let slice = text.slice(range.clone()).unwrap();
                    let attribute = rng.next() % 6;
                    let mut from = 0;
                    let runs: Vec<(String, Format)> = slice
                        .spans()
                        .iter()
                        .map(|span| {
                            let mut format = span.format.clone();
                            match attribute {
                                0 => format.bold = Some(!format.bold.unwrap_or(false)),
                                1 => format.italic = Some(!format.italic.unwrap_or(false)),
                                2 => format.underline = Some(!format.underline.unwrap_or(false)),
                                3 => {
                                    format.font_size = Some(if format.font_size == Some(14.0) {
                                        11.0
                                    } else {
                                        14.0
                                    })
                                }
                                4 => format.color = Some(0x00ff),
                                _ => format.font = Some("Consolas".into()),
                            }
                            let run = (slice.text()[from..span.end].to_owned(), format);
                            from = span.end;
                            run
                        })
                        .collect();
                    text.apply(crate::page::text::Edit {
                        range,
                        replacement: Paragraph::from_runs(runs),
                    })
                    .is_ok()
                }
                _ => {
                    if range.is_empty()
                        || text
                            .spans()
                            .iter()
                            .any(|s| s.format.hyperlink == Some(true))
                    {
                        return false;
                    }
                    let base = text.format_at(range.start).unwrap().clone();
                    let label = text.slice(range.clone()).unwrap();
                    let mut code = base.clone();
                    code.hyperlink = Some(true);
                    code.hyperlink_label = Some(true);
                    code.hidden = Some(true);
                    let mut from = 0;
                    let mut runs = vec![(
                        "\u{fddf}HYPERLINK \"https://example.invalid/op\"".to_owned(),
                        code,
                    )];
                    for span in label.spans() {
                        let mut format = span.format.clone();
                        format.hyperlink = Some(true);
                        format.hyperlink_label = Some(true);
                        runs.push((label.text()[from..span.end].to_owned(), format));
                        from = span.end;
                    }
                    text.apply(crate::page::text::Edit {
                        range,
                        replacement: Paragraph::from_runs(runs),
                    })
                    .is_ok()
                }
            }
        }
        Family::Insert | Family::Paste | Family::Style => {
            let paths = editable(page);
            let Some(path) = paths.get(rng.pick(paths.len()).unwrap_or(0)).cloned() else {
                return false;
            };
            let style = (family == Family::Style).then(|| {
                let existing = page.definitions.iter().find_map(|(id, d)| {
                    matches!(&d.kind, Kind::Style { name: Some(name) } if name == "Heading 2")
                        .then_some(*id)
                });
                existing.unwrap_or_else(|| {
                    let id = id();
                    page.definitions.insert(
                        id,
                        Definition {
                            kind: Kind::Style {
                                name: Some("Heading 2".into()),
                            },
                            format: Format {
                                font: Some("Calibri".into()),
                                font_size: Some(13.0),
                                bold: Some(true),
                                color: Some(0x7d4f1e),
                                ..Default::default()
                            },
                        },
                    );
                    id
                })
            });
            let list = model::list_at(page, &path);
            let template = list
                .iter()
                .find_map(|p| p.text().map(|t| t.text.format_at(0).unwrap().clone()))
                .unwrap_or_default();
            let boundaries: Vec<usize> = (0..=list.len())
                .filter(|i| list.get(*i).is_none_or(|p| p.parent.is_none()))
                .collect();
            let at = boundaries[rng.pick(boundaries.len()).unwrap()];
            let count = if family == Family::Paste {
                2 + rng.next() as usize % 3
            } else {
                1
            };
            let mut inserted = Vec::new();
            for i in 0..count {
                let mut paragraph = text_paragraph(&rng.text(), template.clone());
                if let Some(style) = style {
                    paragraph.style = Some(style);
                }
                if i > 0 && rng.coin() {
                    let parent: &PageParagraph = &inserted[0];
                    paragraph.parent = Some(parent.id);
                    paragraph.level = parent.level + 1;
                }
                inserted.push(paragraph);
            }
            // Children follow their parent, the first inserted paragraph, directly.
            let (children, mut inserted): (Vec<_>, Vec<_>) =
                inserted.into_iter().partition(|p| p.parent.is_some());
            inserted.splice(1..1, children);
            let cell_level = list.first().map_or(1, |p| p.level);
            for paragraph in &mut inserted {
                if paragraph.parent.is_none() {
                    paragraph.level = cell_level;
                }
            }
            list.splice(at..at, inserted);
            true
        }
        Family::Delete | Family::Move | Family::Collapse => {
            let paths = editable(page);
            let Some(path) = paths.get(rng.pick(paths.len()).unwrap_or(0)).cloned() else {
                return false;
            };
            let list = model::list_at(page, &path);
            let top: Vec<usize> = (0..list.len())
                .filter(|i| list[*i].parent.is_none())
                .collect();
            match family {
                Family::Delete => {
                    if top.len() < 2 {
                        return false;
                    }
                    let at = top[rng.pick(top.len()).unwrap()];
                    let end = subtree_end(list, at);
                    list.drain(at..end);
                    true
                }
                Family::Move => {
                    if top.len() < 2 {
                        return false;
                    }
                    let from = top[rng.pick(top.len()).unwrap()];
                    let end = subtree_end(list, from);
                    let block: Vec<_> = list.drain(from..end).collect();
                    let boundaries: Vec<usize> = (0..=list.len())
                        .filter(|i| list.get(*i).is_none_or(|p| p.parent.is_none()))
                        .collect();
                    let to = boundaries[rng.pick(boundaries.len()).unwrap()];
                    list.splice(to..to, block);
                    true
                }
                _ => {
                    let at = rng.pick(list.len()).unwrap();
                    list[at].collapsed ^= true;
                    true
                }
            }
        }
        Family::Position | Family::Width => {
            let mut outlines: Vec<&mut Outline> =
                outlines(page).into_iter().filter(|o| !o.title).collect();
            let Some(at) = rng.pick(outlines.len()) else {
                return false;
            };
            let outline = &mut outlines[at];
            if family == Family::Position {
                outline.layout.x = Some((rng.next() % 200) as f32 * 1.5);
                outline.layout.y = Some((rng.next() % 200) as f32 * 2.25);
            } else {
                outline.layout.max_width = Some(36.0 + (rng.next() % 200) as f32 * 2.0);
                outline.layout.width_set_by_user = Some(rng.coin());
            }
            true
        }
        Family::NewOutline => {
            let at = page
                .objects
                .iter()
                .position(|o| matches!(o, PageObject::Title(_)))
                .unwrap_or(page.objects.len());
            let at = rng.pick(at + 1).unwrap();
            page.objects.insert(
                at,
                PageObject::Outline(Outline {
                    id: id(),
                    title: false,
                    min_width: None,
                    layout: Layout {
                        x: Some((rng.next() % 100) as f32 * 1.5),
                        y: Some((rng.next() % 100) as f32 * 2.25),
                        ..Default::default()
                    },
                    indents: Vec::new(),
                    paragraphs: vec![text_paragraph(&rng.text(), Format::default())],
                    unsupported: Vec::new(),
                }),
            );
            true
        }
        Family::List | Family::Tag | Family::Alignment => {
            let Some((path, index)) = pick_text(page, rng) else {
                return false;
            };
            match family {
                Family::List => {
                    let paragraph = &model::list_at(page, &path)[index];
                    if paragraph.lists.is_empty() {
                        let list = id();
                        page.definitions.insert(
                            list,
                            Definition {
                                kind: Kind::List {
                                    font: Some("Courier New".into()),
                                    format: Some("\u{25cb}".into()),
                                    restart: None,
                                    bullet: Some(4),
                                },
                                format: Format {
                                    font_size: Some(11.0),
                                    color: Some(0xff000000),
                                    ..Default::default()
                                },
                            },
                        );
                        model::list_at(page, &path)[index].lists = vec![list];
                    } else {
                        model::list_at(page, &path)[index].lists.clear();
                    }
                }
                Family::Tag => {
                    let definition = page
                        .definitions
                        .iter()
                        .find(|(_, d)| matches!(&d.kind, Kind::TagDefinition { label: Some(l), .. } if l == "Op task"))
                        .map(|(id, _)| *id)
                        .unwrap_or_else(|| {
                            let id = id();
                            page.definitions.insert(
                                id,
                                Definition {
                                    kind: Kind::TagDefinition {
                                        label: Some("Op task".into()),
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
                    let status = (rng.next() % 2) as u16;
                    let text = model::list_at(page, &path)[index].text_mut().unwrap();
                    if text.tags.is_empty() {
                        text.tags.push(crate::document::Tag {
                            definition: Some(definition),
                            action_type: None,
                            shape: None,
                            property_status: None,
                            status,
                            created: Some(1_262_401_445),
                            completed: (status == 1).then_some(1_262_402_000),
                            start: None,
                            due: None,
                            task_id: None,
                            extra_set: 0,
                        });
                    } else {
                        text.tags.clear();
                    }
                }
                _ => {
                    let alignment = (rng.next() % 3) as u8;
                    let text = &mut model::list_at(page, &path)[index].text_mut().unwrap().text;
                    let mut from = 0;
                    let runs: Vec<(String, Format)> = text
                        .spans()
                        .iter()
                        .map(|span| {
                            let mut format = span.format.clone();
                            format.alignment = Some(alignment);
                            let run = (text.text()[from..span.end].to_owned(), format);
                            from = span.end;
                            run
                        })
                        .collect();
                    *text = Paragraph::from_runs(runs);
                }
            }
            true
        }
        Family::Split | Family::Join => {
            let Some((path, index)) = pick_text(page, rng) else {
                return false;
            };
            let list = model::list_at(page, &path);
            if family == Family::Split {
                let paragraph = &list[index];
                let text = &paragraph.text().unwrap().text;
                let end = text.utf16_offset(text.text().len()).unwrap();
                if text
                    .spans()
                    .iter()
                    .any(|s| s.format.hyperlink == Some(true))
                {
                    return false;
                }
                let offsets: Vec<u32> = text
                    .text()
                    .char_indices()
                    .map(|(i, _)| text.utf16_offset(i).unwrap())
                    .chain([end])
                    .collect();
                let at = offsets[rng.pick(offsets.len()).unwrap()];
                let (head, tail) = (text.slice(0..at).unwrap(), text.slice(at..end).unwrap());
                let mut right = paragraph.clone();
                right.id = id();
                right.tags.clear();
                right.media = Default::default();
                right.content = ParagraphContent::Text(TextObject {
                    id: id(),
                    date_field: None,
                    text: tail,
                    tags: Vec::new(),
                });
                let left_id = paragraph.id;
                list[index].text_mut().unwrap().text = head;
                for child in &mut list[index + 1..] {
                    if child.parent == Some(left_id) {
                        child.parent = Some(right.id);
                    }
                }
                list.insert(index + 1, right);
                true
            } else {
                let next = index + 1;
                let Some(lower) = list.get(next) else {
                    return false;
                };
                let upper = &list[index];
                if lower.parent != upper.parent
                    || lower.level != upper.level
                    || list
                        .iter()
                        .any(|p| p.parent == Some(upper.id) || p.parent == Some(lower.id))
                {
                    return false;
                }
                let Some(lower_text) = lower.text().cloned() else {
                    return false;
                };
                if lower_text.text.text().is_empty()
                    || crate::page::Math::is_equation(&lower_text.text)
                {
                    return false;
                }
                let upper = list[index].text_mut().unwrap();
                if upper.text.text().is_empty() {
                    upper.id = lower_text.id;
                    upper.text = lower_text.text;
                } else {
                    upper.text.append(lower_text.text).unwrap();
                }
                list.remove(next);
                true
            }
        }
        Family::Indent | Family::Outdent => {
            let paths = editable(page);
            let Some(path) = paths.get(rng.pick(paths.len()).unwrap_or(0)).cloned() else {
                return false;
            };
            if !format!("{path:?}").contains("cells: []") {
                return false;
            }
            let list = model::list_at(page, &path);
            let Some(index) = rng.pick(list.len()) else {
                return false;
            };
            let end = subtree_end(list, index);
            if family == Family::Indent {
                // Tab: the paragraph becomes the last child of its previous sibling.
                let parent = list[index].parent;
                let Some(sibling) = list[..index]
                    .iter()
                    .rev()
                    .find(|p| p.parent == parent)
                    .map(|p| (p.id, p.level))
                else {
                    return false;
                };
                list[index].parent = Some(sibling.0);
                let delta = sibling.1 + 1 - list[index].level;
                if delta > 1 || list[index].level + 1 > 31 {
                    return false;
                }
                for paragraph in &mut list[index..end] {
                    paragraph.level += 1;
                }
                list[index].level = list[index].level.max(sibling.1 + 1);
            } else {
                let Some(parent) = list[index].parent else {
                    return false;
                };
                // Shift-Tab of a last child: it follows its parent.
                if list[end..].iter().any(|p| p.parent == Some(parent)) {
                    return false;
                }
                let grandparent = list.iter().find(|p| p.id == parent).unwrap().parent;
                list[index].parent = grandparent;
                for paragraph in &mut list[index..end] {
                    paragraph.level -= 1;
                }
                if list[index].level == 0 {
                    return false;
                }
            }
            true
        }
        Family::TableRow
        | Family::TableColumn
        | Family::TableDelete
        | Family::TableWidth
        | Family::TableShading => {
            let mut tables = tables(page);
            let Some(at) = rng.pick(tables.len()) else {
                return false;
            };
            let table = &mut tables[at];
            let indents = table.rows[0].cells[0].indents.clone();
            match family {
                Family::TableRow => {
                    let row = TableRow {
                        id: id(),
                        cells: table.columns.iter().map(|_| new_cell(&indents)).collect(),
                    };
                    let at = rng.pick(table.rows.len() + 1).unwrap();
                    table.rows.insert(at, row);
                }
                Family::TableColumn => {
                    let at = rng.pick(table.columns.len() + 1).unwrap();
                    table.columns.insert(
                        at,
                        TableColumn {
                            width: 72.0,
                            locked: false,
                        },
                    );
                    for row in &mut table.rows {
                        row.cells.insert(at, new_cell(&indents));
                    }
                }
                Family::TableDelete => {
                    if rng.coin() && table.rows.len() > 1 {
                        let at = rng.pick(table.rows.len()).unwrap();
                        table.rows.remove(at);
                    } else if table.columns.len() > 1 {
                        let at = rng.pick(table.columns.len()).unwrap();
                        table.columns.remove(at);
                        for row in &mut table.rows {
                            row.cells.remove(at);
                        }
                    } else {
                        return false;
                    }
                }
                Family::TableWidth => {
                    let at = rng.pick(table.columns.len()).unwrap();
                    table.columns[at].width = 40.0 + (rng.next() % 100) as f32;
                    table.columns[at].locked = rng.coin();
                }
                _ => {
                    let cells: Vec<_> = table.rows.iter_mut().flat_map(|r| &mut r.cells).collect();
                    let len = cells.len();
                    let cell = cells.into_iter().nth(rng.pick(len).unwrap()).unwrap();
                    cell.shading = if cell.shading.is_some() {
                        None
                    } else {
                        Some(0x00ccff)
                    };
                }
            }
            true
        }
        Family::NewTable | Family::ParagraphPicture | Family::Attachment | Family::Equation => {
            let equation = page.objects.iter().find_map(|o| match o {
                PageObject::Outline(outline) => outline.paragraphs.iter().find_map(|p| {
                    p.text()
                        .filter(|t| crate::page::Math::is_equation(&t.text))
                        .map(|t| t.text.clone())
                }),
                _ => None,
            });
            let Some(outline) = outlines(page).into_iter().find(|o| !o.title) else {
                return false;
            };
            let mut paragraph = text_paragraph("", Format::default());
            paragraph.content = match family {
                Family::NewTable => ParagraphContent::Table(Table {
                    id: id(),
                    columns: vec![
                        TableColumn {
                            width: 72.0,
                            locked: false
                        };
                        2
                    ],
                    rows: (0..2)
                        .map(|_| TableRow {
                            id: id(),
                            cells: (0..2).map(|_| new_cell(&[])).collect(),
                        })
                        .collect(),
                    borders: Some(true),
                    layout: Layout::default(),
                    tags: Vec::new(),
                }),
                Family::ParagraphPicture => ParagraphContent::Image(Image {
                    id: id(),
                    layout: Layout::default(),
                    size: Some([0.75, 0.75]),
                    bytes: Some(PNG.into()),
                    display: None,
                    alt: Some("dot".into()),
                    background: false,
                    printout: None,
                }),
                Family::Attachment => ParagraphContent::Attachment(Attachment {
                    id: id(),
                    filename: "notes.txt".into(),
                    source_path: Some(r"C:\notes.txt".into()),
                    size: None,
                    layout: Layout::default(),
                    bytes: Some(b"attached bytes".as_slice().into()),
                    preview: None,
                    recording: None,
                }),
                _ => {
                    let Some(equation) = equation else {
                        return false;
                    };
                    ParagraphContent::Text(TextObject {
                        id: id(),
                        date_field: None,
                        text: equation,
                        tags: Vec::new(),
                    })
                }
            };
            let boundaries: Vec<usize> = (0..=outline.paragraphs.len())
                .filter(|i| {
                    outline
                        .paragraphs
                        .get(*i)
                        .is_none_or(|p| p.parent.is_none())
                })
                .collect();
            let at = boundaries[rng.pick(boundaries.len()).unwrap()];
            outline.paragraphs.insert(at, paragraph);
            true
        }
        Family::PagePicture | Family::NewInk => {
            let at = page
                .objects
                .iter()
                .position(|o| matches!(o, PageObject::Title(_)))
                .unwrap_or(page.objects.len());
            let object = if family == Family::PagePicture {
                PageObject::Image(Image {
                    id: id(),
                    layout: Layout {
                        x: Some(100.0),
                        y: Some(200.0),
                        ..Default::default()
                    },
                    size: Some([0.75, 0.75]),
                    bytes: Some(PNG.into()),
                    display: None,
                    alt: None,
                    background: false,
                    printout: None,
                })
            } else {
                PageObject::Ink(Ink {
                    id: id(),
                    layout: Layout::default(),
                    strokes: vec![stroke(rng)],
                    groups: Vec::new(),
                    shape: None,
                })
            };
            page.objects.insert(at, object);
            true
        }
        Family::PictureEdit => {
            let images: Vec<&mut Image> = page
                .objects
                .iter_mut()
                .filter_map(|o| match o {
                    PageObject::Image(image) => Some(image),
                    _ => None,
                })
                .collect();
            let len = images.len();
            let Some(image) = images.into_iter().nth(rng.pick(len).unwrap_or(0)) else {
                return false;
            };
            match rng.next() % 3 {
                0 => {
                    image.layout.x = Some((rng.next() % 300) as f32);
                    image.layout.y = Some((rng.next() % 300) as f32);
                }
                1 => {
                    image.layout.max_width = Some(20.0 + (rng.next() % 100) as f32);
                    image.layout.max_height = Some(20.0 + (rng.next() % 100) as f32);
                    image.layout.width_set_by_user = Some(true);
                }
                _ => image.alt = Some(rng.text()),
            }
            true
        }
        Family::AttachmentRename => {
            let mut found = false;
            for outline in outlines(page) {
                for paragraph in &mut outline.paragraphs {
                    if let ParagraphContent::Attachment(attachment) = &mut paragraph.content
                        && !found
                        && attachment.recording.is_none()
                    {
                        attachment.filename = format!("renamed {}.txt", rng.next() % 100);
                        found = true;
                    }
                }
            }
            found
        }
        Family::InkStrokes => {
            let inks: Vec<&mut Ink> = page
                .objects
                .iter_mut()
                .filter_map(|o| match o {
                    PageObject::Ink(ink) if ink.groups.is_empty() => Some(ink),
                    _ => None,
                })
                .collect();
            let len = inks.len();
            let Some(ink) = inks.into_iter().nth(rng.pick(len).unwrap_or(0)) else {
                return false;
            };
            if rng.coin() || ink.strokes.len() < 2 {
                ink.strokes.push(stroke(rng));
            } else {
                let at = rng.pick(ink.strokes.len()).unwrap();
                ink.strokes.remove(at);
            }
            true
        }
        Family::InkPosition => {
            let Some(ink) = page.objects.iter_mut().find_map(|o| match o {
                PageObject::Ink(ink) => Some(ink),
                _ => None,
            }) else {
                return false;
            };
            ink.layout.x = Some((rng.next() % 40) as f32 - 20.0);
            ink.layout.y = Some((rng.next() % 90) as f32);
            true
        }
    }
}

fn stroke(rng: &mut Rng) -> InkStroke {
    let x = (rng.next() % 300) as f32;
    InkStroke {
        id: id(),
        points: vec![[x, 100.0], [x + 10.0, 110.0], [x + 20.0, 105.0]],
        width: 1.5,
        height: 1.5,
        color: Some(0x0000ff),
        transparency: None,
        pen_tip: None,
        // Every other stroke a highlighter's, and every third a pressure pen's.
        raster_operation: (x as u32).is_multiple_of(2).then_some(9),
        pressure: if (x as u32).is_multiple_of(3) {
            [0.0, 0.3, 1.0].map(crate::page::ink::level).to_vec()
        } else {
            Vec::new()
        },
    }
}

/// How one lowered edit came out.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(super) enum Outcome {
    /// The edit made no change the writers store.
    Unchanged,
    /// Lowering or the section refused the edit; the section is as it was.
    Refused,
    /// The sealed image reads back as the edited page.
    Stored,
    Mismatch(String),
}

/// Lowers `after` over page `space` of `source`, applies and seals the ops, and compares
/// the page the section holds with the edited page, with the model oracle's prediction and
/// with the page the sealed image reads back as.
pub(super) fn stored(source: &[u8], space: ExGuid, after: &Page, seed: u64) -> Outcome {
    let before = read(source, space);
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.to_vec()).unwrap();
    let Ok(ops) = lower_page(&before, after) else {
        return Outcome::Refused;
    };
    let edit = Edit {
        at: AT,
        ops: ops
            .iter()
            .map(|op| Op::Page {
                space,
                op: op.clone(),
            })
            .collect(),
    };
    if seeded(seed, || section.apply("Author", &edit)).is_err() {
        if section.page(space).unwrap() != before {
            return Outcome::Mismatch("a refused edit changed the page".into());
        }
        return Outcome::Refused;
    }
    let mut predicted = before.clone();
    for op in &ops {
        model::apply(&mut predicted, op).unwrap();
    }
    let live = section.page(space).unwrap();
    if normalize(&predicted) != normalize(&live) {
        return Outcome::Mismatch(format!(
            "the model predicts otherwise: {}",
            first_difference(&normalize(&live), &normalize(&predicted))
        ));
    }
    let Some(_) = seeded(seed, || section.seal().unwrap()) else {
        return if live == before {
            Outcome::Unchanged
        } else {
            Outcome::Mismatch("the page changed and the seal stored nothing".into())
        };
    };
    let image = section.image();
    let store = Store::parse(&image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    if let Err(error) = index.validate_current() {
        return Outcome::Mismatch(format!("invalid image: {error:?}"));
    }
    let reread = read(&image, space);
    if reread != live {
        return Outcome::Mismatch(format!(
            "reads back otherwise: {}",
            first_difference(&live, &reread)
        ));
    }
    let written = as_written(after, &before, &reread);
    let (stored, wanted) = (normalize(&reread), normalize(&written));
    if stored != wanted {
        return Outcome::Mismatch(format!(
            "stores another page: {}",
            first_difference(&wanted, &stored)
        ));
    }
    Outcome::Stored
}

fn spans(text: &Paragraph) -> String {
    let mut out = format!("{:?}:", text.text());
    for span in text.spans() {
        let f = &span.format;
        out.push_str(&format!(
            " [{} b{:?} h{:?} l{:?}/{:?} c{:?} a{:?} s{:?} lang{:?} f{:?}]",
            span.end,
            f.bold,
            f.hidden,
            f.hyperlink,
            f.hyperlink_label,
            f.color,
            f.alignment,
            f.space_before,
            f.language,
            f.font
        ));
    }
    out
}

pub(super) fn first_difference(expected: &Page, actual: &Page) -> String {
    let texts = |page: &Page| -> Vec<(ExGuid, Paragraph)> {
        model::lists(page)
            .into_iter()
            .flat_map(|(_, _, list)| list.iter())
            .filter_map(|p| p.text().map(|t| (p.id, t.text.clone())))
            .collect()
    };
    for ((a_id, a), (b_id, b)) in texts(expected).iter().zip(texts(actual).iter()) {
        if a_id == b_id && a != b {
            return format!("\n  expected {}\n  actual   {}", spans(a), spans(b));
        }
    }
    let (a, b) = (format!("{expected:?}"), format!("{actual:?}"));
    let at = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let window = |s: &str| {
        let start = s.floor_char_boundary(at.saturating_sub(300));
        let end = s.floor_char_boundary((at + 200).min(s.len()));
        s[start..end].to_owned()
    };
    format!("\n  expected …{}\n  actual   …{}", window(&a), window(&b))
}

/// Every family on every corpus page it applies to, several seeds each.
#[test]
fn lowered_edits_store_the_edited_page() {
    let mut tally: BTreeMap<(Family, String), usize> = BTreeMap::new();
    let mut mismatches = Vec::new();
    for (name, source) in SOURCES {
        for (p, space) in pages(source).into_iter().enumerate().take(4) {
            for family in FAMILIES {
                for seed in crate::sweep::seeds(0..3, 1) {
                    let mut rng = Rng(seed * 7919 + p as u64 * 131 + family as u64);
                    let mut after = read(source, space);
                    GUIDS.set(Some(1 << 60 | seed << 32 | (family as u64) << 20));
                    if !mutate(&mut after, &mut rng, family) {
                        continue;
                    }
                    GUIDS.set(None);
                    let outcome = stored(source, space, &after, 1 << 40 | seed);
                    let key = match &outcome {
                        Outcome::Mismatch(_) => "Mismatch".to_owned(),
                        other => format!("{other:?}"),
                    };
                    *tally.entry((family, key)).or_default() += 1;
                    if let Outcome::Mismatch(detail) = outcome {
                        mismatches
                            .push(format!("{name} page {p} {family:?} seed {seed}: {detail}"));
                    }
                }
            }
        }
    }
    let mut report = String::new();
    for ((family, outcome), count) in &tally {
        report.push_str(&format!("{family:?} {outcome} {count}\n"));
    }
    println!("{report}");
    for mismatch in mismatches.iter().take(8) {
        println!("{mismatch}\n");
    }
    assert!(mismatches.is_empty(), "{} mismatches", mismatches.len());
}

#[test]
#[ignore]
fn debug_case() {
    let (_, source) = SOURCES[5];
    let space = pages(source)[1];
    let family = Family::NewOutline;
    let (seed, p) = (1u64, 1u64);
    let mut rng = Rng(seed * 7919 + p * 131 + family as u64);
    let mut after = read(source, space);
    GUIDS.set(Some(1 << 60 | seed << 32 | (family as u64) << 20));
    assert!(mutate(&mut after, &mut rng, family));
    GUIDS.set(None);
    let before = read(source, space);
    let ops = lower_page(&before, &after).unwrap();
    let mut page = before.clone();
    for op in &ops {
        model::apply(&mut page, op).unwrap();
    }
    for object in &page.objects {
        println!(
            "{:?} {:?} {:?}",
            object.id(),
            object.layout(),
            match object {
                PageObject::Outline(o) => o
                    .paragraphs
                    .iter()
                    .filter_map(|p| p.text().map(|t| t.text.text().to_owned()))
                    .collect::<Vec<_>>(),
                PageObject::Title(t) => t
                    .outlines
                    .iter()
                    .flat_map(|o| &o.paragraphs)
                    .filter_map(|p| p.text().map(|t| format!("title {:?}", t.text.text())))
                    .collect(),
                _ => vec![],
            }
        );
    }
    println!("{:?} -> {:?}", before.title, page.title);
}

/// Random ops drawn from a page model: targets the page holds, new identities fresh.
fn random_op(page: &Page, rng: &mut Rng) -> Option<PageOp> {
    let texts: Vec<(ExGuid, ExGuid, Paragraph)> = model::lists(page)
        .into_iter()
        .flat_map(|(_, _, list)| list.iter())
        .filter_map(|p| {
            let text = p.text()?;
            (text.date_field.is_none() && !crate::page::Math::is_equation(&text.text))
                .then(|| (p.id, text.id, text.text.clone()))
        })
        .collect();
    let (paragraph, text, content) = texts.get(rng.pick(texts.len())?)?.clone();
    let length = content.utf16_offset(content.text().len()).unwrap();
    let offsets: Vec<u32> = content
        .text()
        .char_indices()
        .map(|(i, _)| content.utf16_offset(i).unwrap())
        .chain([length])
        .collect();
    let a = offsets[rng.pick(offsets.len()).unwrap()];
    let b = offsets[rng.pick(offsets.len()).unwrap()];
    let range = a.min(b)..a.max(b);
    let holder = model::paragraph(page, paragraph).unwrap();
    let container = holder.parent.or_else(|| {
        model::lists(page)
            .into_iter()
            .find(|(_, _, list)| list.iter().any(|p| p.id == paragraph))
            .map(|(_, owner, _)| owner)
    })?;
    let siblings: Vec<ExGuid> = model::lists(page)
        .into_iter()
        .flat_map(|(_, _, list)| list.iter())
        .filter(|p| p.parent == holder.parent && p.id != paragraph)
        .map(|p| p.id)
        .collect();
    if rng.next().is_multiple_of(4)
        && let Some(op) = random_object_op(page, rng)
    {
        return Some(op);
    }
    Some(match rng.next() % 16 {
        0 | 1 => PageOp::Text {
            text,
            range,
            with: rng.text(),
        },
        2 => PageOp::Format {
            text,
            range,
            set: vec![TextAttribute::Bold(rng.coin())],
            clear: if rng.next().is_multiple_of(3) {
                vec![TextProperty::Italic]
            } else {
                Vec::new()
            },
        },
        3 => PageOp::Split {
            text,
            at: a,
            paragraph: id(),
            right: id(),
            lists: holder.lists.iter().map(|_| id()).collect(),
        },
        4 => {
            let mut new = text_paragraph(&rng.text(), content.format_at(0).unwrap().clone());
            new.level = holder.level;
            new.parent = holder.parent;
            PageOp::Insert {
                container,
                before: (rng.coin()).then_some(paragraph),
                paragraphs: vec![new],
            }
        }
        5 => PageOp::Delete { object: paragraph },
        6 => PageOp::Move {
            object: paragraph,
            parent: Some(container),
            before: siblings
                .get(rng.pick(siblings.len().max(1)).unwrap_or(0))
                .copied(),
        },
        7 => PageOp::Level {
            paragraph,
            level: (holder.level + 1).clamp(1, 4) - (rng.next() % 2) as u32,
        },
        8 => PageOp::Outline {
            object: paragraph,
            edit: crate::OutlineEdit::Collapsed(rng.coin()),
        },
        9 => PageOp::Paragraph {
            paragraph,
            alignment: Some((rng.next() % 3) as u8),
            rtl: None,
            space_before: Some(4.5),
            space_after: None,
            line_spacing: None,
            language: rng.coin().then_some(0x40c),
        },
        10 => {
            let right = texts
                .iter()
                .skip_while(|(p, _, _)| *p != paragraph)
                .nth(1)?;
            PageOp::Join {
                left: text,
                right: right.1,
            }
        }
        11 => PageOp::List {
            paragraph,
            lists: vec![(
                id(),
                Definition {
                    kind: Kind::List {
                        font: Some("Calibri".into()),
                        format: Some("\u{fffd}1.".into()),
                        restart: None,
                        bullet: None,
                    },
                    format: Format::default(),
                },
            )],
        },
        12 => {
            let definition = id();
            PageOp::Tags {
                target: if rng.coin() { paragraph } else { text },
                tags: vec![crate::document::Tag {
                    definition: Some(definition),
                    action_type: None,
                    shape: None,
                    property_status: None,
                    status: 0,
                    created: Some(1_262_401_445),
                    completed: None,
                    start: None,
                    due: None,
                    task_id: None,
                    extra_set: 0,
                }],
                definitions: vec![(
                    definition,
                    Definition {
                        kind: Kind::TagDefinition {
                            label: Some("Random".into()),
                            action_type: Some(0),
                            shape: Some(3),
                            color: None,
                            highlight: None,
                        },
                        format: Format::default(),
                    },
                )],
            }
        }
        13 => PageOp::Link {
            text,
            range,
            target: rng.coin().then(|| "https://example.invalid/random".into()),
        },
        14 => PageOp::Add {
            object: PageObject::Outline(Outline {
                id: id(),
                title: false,
                min_width: None,
                layout: Layout {
                    x: Some(36.0),
                    y: Some(300.0 + (rng.next() % 100) as f32),
                    ..Default::default()
                },
                indents: Vec::new(),
                paragraphs: vec![text_paragraph(&rng.text(), Format::default())],
                unsupported: Vec::new(),
            }),
            before: None,
        },
        _ => {
            let stroke = stroke(rng);
            let ends = [stroke.points[0], stroke.points[stroke.points.len() - 1]];
            PageOp::Add {
                object: PageObject::Ink(Ink {
                    id: id(),
                    layout: Layout::default(),
                    strokes: vec![stroke],
                    groups: Vec::new(),
                    shape: Some(crate::page::ink::InkShape::Line(ends)),
                }),
                before: None,
            }
        }
    })
}

/// Random edits of tables, pictures, ink, outlines and styles the page holds.
fn random_object_op(page: &Page, rng: &mut Rng) -> Option<PageOp> {
    let tables: Vec<Table> = model::lists(page)
        .into_iter()
        .flat_map(|(_, _, list)| list.iter())
        .filter_map(|p| match &p.content {
            ParagraphContent::Table(table) => Some(table.clone()),
            _ => None,
        })
        .collect();
    let outlines: Vec<&Outline> = page
        .objects
        .iter()
        .filter_map(|o| match o {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .collect();
    let pictures: Vec<&Image> = page
        .objects
        .iter()
        .filter_map(|o| match o {
            PageObject::Image(image) => Some(image),
            _ => None,
        })
        .collect();
    let inks: Vec<&Ink> = page
        .objects
        .iter()
        .filter_map(|o| match o {
            PageObject::Ink(ink) if ink.groups.is_empty() => Some(ink),
            _ => None,
        })
        .collect();
    match rng.next() % 6 {
        0 | 1 => {
            let table = tables.get(rng.pick(tables.len())?)?;
            let indents = table.rows[0].cells[0].indents.clone();
            let cell = || TableCell {
                paragraphs: vec![text_paragraph("cell", Format::default())],
                ..new_cell(&indents)
            };
            let edit = match rng.next() % 7 {
                0 => TableEdit::Rows {
                    before: table
                        .rows
                        .get(rng.pick(table.rows.len() + 1)?)
                        .map(|r| r.id),
                    rows: vec![TableRow {
                        id: id(),
                        cells: table.columns.iter().map(|_| cell()).collect(),
                    }],
                },
                1 => TableEdit::Column {
                    at: rng.pick(table.columns.len() + 1)? as u32,
                    width: 60.0,
                    cells: table.rows.iter().map(|_| cell()).collect(),
                },
                2 => TableEdit::DeleteRow(table.rows[rng.pick(table.rows.len())?].id),
                3 => TableEdit::DeleteColumn(rng.pick(table.columns.len())? as u32),
                4 => TableEdit::Columns(
                    table
                        .columns
                        .iter()
                        .map(|c| TableColumn {
                            width: c.width + 9.0,
                            locked: !c.locked,
                        })
                        .collect(),
                ),
                5 => TableEdit::Borders(rng.coin()),
                _ => {
                    let cells: Vec<&TableCell> = table.rows.iter().flat_map(|r| &r.cells).collect();
                    let cell = cells[rng.pick(cells.len())?];
                    TableEdit::Cell {
                        cell: cell.id,
                        shading: Some(0x00ccff),
                        indents: vec![18.0, 0.0, 27.0, 27.0],
                    }
                }
            };
            Some(PageOp::Table {
                table: table.id,
                edit,
            })
        }
        2 => {
            let outline = outlines.get(rng.pick(outlines.len())?)?;
            Some(PageOp::Outline {
                object: outline.id,
                edit: if rng.coin() {
                    crate::OutlineEdit::Position {
                        x: (rng.next() % 300) as f32,
                        y: (rng.next() % 300) as f32,
                    }
                } else {
                    crate::OutlineEdit::Width {
                        points: 100.0 + (rng.next() % 300) as f32,
                        user_set: rng.coin(),
                    }
                },
            })
        }
        3 => {
            let picture = pictures.get(rng.pick(pictures.len())?)?;
            Some(PageOp::Picture {
                picture: picture.id,
                layout: Layout {
                    x: Some((rng.next() % 300) as f32),
                    y: Some((rng.next() % 300) as f32),
                    max_width: Some(40.0),
                    max_height: Some(30.0),
                    width_set_by_user: Some(true),
                    reserved_width: picture.layout.reserved_width,
                },
                alt: Some(rng.text()),
            })
        }
        4 => {
            let ink = inks.get(rng.pick(inks.len())?)?;
            let remove = if ink.strokes.len() > 1 && rng.coin() {
                vec![ink.strokes[rng.pick(ink.strokes.len())?].id]
            } else {
                Vec::new()
            };
            Some(PageOp::Strokes {
                ink: ink.id,
                add: vec![stroke(rng)],
                remove,
            })
        }
        _ => {
            let paragraphs: Vec<&PageParagraph> = model::lists(page)
                .into_iter()
                .flat_map(|(_, _, list)| list.iter())
                .filter(|p| p.text().is_some())
                .collect();
            let paragraph = paragraphs.get(rng.pick(paragraphs.len())?)?;
            Some(PageOp::Style {
                paragraph: paragraph.id,
                style: id(),
                definition: Definition {
                    kind: Kind::Style {
                        name: Some("Heading 3".into()),
                    },
                    format: Format {
                        bold: Some(true),
                        font_size: Some(12.0),
                        color: Some(0x7d4f1e),
                        ..Default::default()
                    },
                },
            })
        }
    }
}

/// Random edits through a section, sealed now and then: each applied edit reads back as the
/// model predicts, a refused one changes nothing, and reopening every sealed image gives
/// the same pages.
#[test]
fn random_ops_read_back_as_the_model_predicts() {
    let steps = 60;
    let (mut applied, mut refused, mut unseen) = (0, 0, 0);
    let mut mismatches: Vec<String> = Vec::new();
    for (name, source) in SOURCES {
        let spaces = pages(source);
        for seed in crate::sweep::seeds(0..2, 2) {
            let arena = Arena::default();
            let mut section = Section::open(&arena, source.to_vec()).unwrap();
            let mut rng = Rng(seed * 104_729 + name.len() as u64);
            for step in 0..steps {
                let space = spaces[step % spaces.len().min(3)];
                let before = section.page(space).unwrap();
                let ops: Vec<Op> = (0..1 + rng.next() % 2)
                    .filter_map(|_| random_op(&before, &mut rng))
                    .map(|op| Op::Page { space, op })
                    .collect();
                if ops.is_empty() {
                    continue;
                }
                let edit = Edit {
                    at: AT + step as u64,
                    ops,
                };
                // Text whose emptied final run keeps an insertion style the model cannot show.
                let hidden: Vec<ExGuid> = section
                    .active(space)
                    .unwrap()
                    .view
                    .nodes
                    .iter()
                    .filter(|(_, node)| {
                        matches!(&node.kind, Kind::RichText { text, runs, .. }
                            if !text.is_empty() && runs.len() > 1 && runs.last().is_some_and(|r| r.start == r.end))
                    })
                    .map(|(id, _)| *id)
                    .collect();
                match section.apply("Author", &edit) {
                    Ok(()) => {
                        applied += 1;
                        let mut predicted = before.clone();
                        for op in &edit.ops {
                            let Op::Page { op, .. } = op else {
                                unreachable!()
                            };
                            model::apply(&mut predicted, op).unwrap();
                        }
                        let stored = section.page(space).unwrap();
                        let touches_hidden = edit.ops.iter().any(|op| {
                            matches!(op, Op::Page { op: PageOp::Text { text, .. } | PageOp::Link { text, .. } | PageOp::Split { text, .. }, .. } if hidden.contains(text))
                        });
                        // A restyled run may state what its old style did, which the model
                        // cannot tell from inheriting it.
                        let restyles = edit.ops.iter().any(|op| {
                            matches!(op, Op::Page { op: PageOp::Style { paragraph, .. }, .. }
                                if model::paragraph(&before, *paragraph).is_some_and(|p| p.style.is_some()))
                        });
                        if (touches_hidden || restyles)
                            && normalize(&predicted) != normalize(&stored)
                        {
                            unseen += 1;
                            continue;
                        }
                        if normalize(&predicted) != normalize(&stored) {
                            let kinds: Vec<String> = edit
                                .ops
                                .iter()
                                .map(|op| match op {
                                    Op::Page { op, .. } => format!("{op:?}")
                                        .split([' ', '{'])
                                        .next()
                                        .unwrap()
                                        .to_owned(),
                                    Op::Section(_) => "Section".into(),
                                })
                                .collect();
                            if std::env::var("OP_DEBUG").is_ok() {
                                let tree = |page: &Page| -> String {
                                    model::lists(page)
                                        .into_iter()
                                        .map(|(_, owner, list)| {
                                            format!(
                                                "{}: {}",
                                                owner.n,
                                                list.iter()
                                                    .map(|p| format!(
                                                        "{}<{:?}>@{}",
                                                        p.id.n,
                                                        p.parent.map(|p| p.n),
                                                        p.level
                                                    ))
                                                    .collect::<Vec<_>>()
                                                    .join(" ")
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                };
                                println!(
                                    "ops {:?}\nbefore\n{}\nstored\n{}\npredicted\n{}",
                                    edit.ops,
                                    tree(&before),
                                    tree(&stored),
                                    tree(&predicted)
                                );
                            }
                            if mismatches.len() < 6 {
                                println!(
                                    "{name} seed {seed} step {step} {kinds:?}: {}",
                                    first_difference(&normalize(&stored), &normalize(&predicted))
                                );
                            }
                            mismatches.push(kinds.join("+"));
                        }
                    }
                    Err(OpError::Failed(error)) => panic!("{name} step {step}: {error:?}"),
                    Err(_) => {
                        refused += 1;
                        assert!(
                            section.page(space).unwrap() == before,
                            "{name} step {step}: a refused edit changed the page"
                        );
                    }
                }
                if step % 7 == 6 || step + 1 == steps {
                    section.seal().unwrap();
                    let image = section.image();
                    let reopened = Section::open(&arena, image).unwrap();
                    for space in &spaces {
                        assert!(
                            reopened.page(*space).unwrap() == section.page(*space).unwrap(),
                            "{name} step {step}: reopening changed a page"
                        );
                    }
                }
            }
        }
    }
    println!(
        "{applied} edits applied, {refused} refused, {unseen} restyled or typed into a hidden insertion style"
    );
    let mut kinds = BTreeMap::new();
    for kind in &mismatches {
        *kinds.entry(kind.clone()).or_insert(0) += 1;
    }
    println!("model mispredictions: {kinds:?}");
    assert!(mismatches.is_empty());
    assert!(applied > refused);
}

/// Page creation, moves, indentation and removal through a section leave the page list
/// they describe, and the sealed image lists the pages the section does.
#[test]
fn section_ops_leave_the_page_list_they_describe() {
    let sources: Vec<Vec<u8>> = vec![
        crate::create_section("pages.one", "First", "Author").unwrap(),
        include_bytes!("../../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one")
            .to_vec(),
        SOURCES[3].1.to_vec(),
    ];
    let mut applied = 0;
    for (n, source) in sources.iter().enumerate() {
        let arena = Arena::default();
        let mut section = Section::open(&arena, source.to_vec()).unwrap();
        let mut rng = Rng(n as u64 + 17);
        for step in 0..12u64 {
            let listed = section.pages().unwrap();
            let spaces: Vec<ExGuid> = listed.iter().map(|(space, ..)| *space).collect();
            let anchor = spaces
                .get(rng.pick(spaces.len()).unwrap_or(0))
                .copied()
                .filter(|_| rng.coin());
            let op = match step % 4 {
                0 => SectionOp::Create(
                    crate::PageCreation::new(anchor, Some("Created"), "Author").unwrap(),
                ),
                1 => {
                    let Some(space) = spaces.get(rng.pick(spaces.len()).unwrap_or(0)).copied()
                    else {
                        continue;
                    };
                    let level = if spaces[0] == space {
                        1
                    } else {
                        1 + (rng.next() % 2) as u32
                    };
                    SectionOp::Pages(vec![crate::PageEdit::set_level(space, level).unwrap()])
                }
                2 => {
                    let Some(space) = spaces.get(rng.pick(spaces.len()).unwrap_or(0)).copied()
                    else {
                        continue;
                    };
                    SectionOp::Pages(vec![crate::PageEdit::move_to(space, None, 1).unwrap()])
                }
                _ => {
                    if spaces.len() < 2 {
                        continue;
                    }
                    SectionOp::Delete(vec![spaces[rng.pick(spaces.len()).unwrap()]])
                }
            };
            let edit = Edit {
                at: AT,
                ops: vec![Op::Section(op.clone())],
            };
            if section.apply("Author", &edit).is_err() {
                assert_eq!(section.pages().unwrap(), listed, "{n} step {step} {op:?}");
                continue;
            }
            section.seal().unwrap();
            applied += 1;
            let after = section.pages().unwrap();
            let order = |list: &[(ExGuid, String, u32)], without: ExGuid| -> Vec<ExGuid> {
                list.iter()
                    .map(|(space, ..)| *space)
                    .filter(|space| *space != without)
                    .collect()
            };
            match &op {
                SectionOp::Create(creation) => {
                    let at = after
                        .iter()
                        .position(|(space, ..)| *space == creation.space())
                        .unwrap();
                    assert_eq!(after[at].1, "Created");
                    assert_eq!(order(&after, creation.space()), spaces, "{n} step {step}");
                    match anchor {
                        Some(before) => assert_eq!(after[at + 1].0, before),
                        None => assert_eq!(at + 1, after.len()),
                    }
                }
                SectionOp::Pages(edits) => {
                    let space = edits[0].space();
                    let at = after
                        .iter()
                        .position(|(listed, ..)| *listed == space)
                        .unwrap();
                    assert_eq!(after[at].2, edits[0].level(), "{n} step {step} {op:?}");
                    if edits[0].position() == crate::PagePosition::Keep {
                        assert_eq!(order(&after, space), order(&listed, space));
                        assert_eq!(after.iter().map(|(s, ..)| *s).collect::<Vec<_>>(), spaces);
                    } else {
                        assert_eq!(after.last().unwrap().0, space, "{n} step {step}");
                    }
                }
                SectionOp::Delete(deleted) => {
                    assert_eq!(
                        after.iter().map(|(s, ..)| *s).collect::<Vec<_>>(),
                        order(&listed, deleted[0])
                    );
                }
                SectionOp::Import { .. }
                | SectionOp::Conflict { .. }
                | SectionOp::Color(_)
                | SectionOp::RestoreVersion { .. }
                | SectionOp::DeleteVersions { .. } => unreachable!(),
            }
            let image = section.image();
            let store = Store::parse(&image).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            index.validate_current().unwrap();
            let document = Document::parse(&index).unwrap();
            let reread: Vec<(ExGuid, String, u32)> = document
                .pages()
                .unwrap()
                .into_iter()
                .map(|(space, page)| {
                    let (title, level) = Page::heading(document.active(space).unwrap(), page);
                    (space, title, level)
                })
                .collect();
            assert_eq!(reread, after, "{n} step {step}");
            for (space, ..) in &after {
                assert_eq!(
                    read(&image, *space),
                    section.page(*space).unwrap(),
                    "{n} step {step}"
                );
            }
        }
    }
    assert!(applied > 12, "{applied} section edits applied");
}

/// An imported page reads back as its copy under the created page's title.
#[test]
fn an_imported_page_reads_back_as_its_copy() {
    let target = crate::create_section("import.one", "First", "Author").unwrap();
    let mut imported = 0;
    for (name, source) in SOURCES {
        for space in pages(source).into_iter().take(2) {
            let Ok(copy) = read(source, space).copy() else {
                continue;
            };
            let creation = crate::PageCreation::new(None, Some(&copy.title), "Author").unwrap();
            let arena = Arena::default();
            let mut section = Section::open(&arena, target.clone()).unwrap();
            let import = |section: &mut Section<'_>, op: SectionOp| {
                section.apply(
                    "Author",
                    &Edit {
                        at: AT,
                        ops: vec![Op::Section(op)],
                    },
                )
            };
            import(&mut section, SectionOp::Create(creation.clone())).unwrap();
            let created = section.page(creation.space()).unwrap();
            let mut expected = created.clone();
            let arena = Arena::default();
            let mut section = Section::open(&arena, target.clone()).unwrap();
            if import(
                &mut section,
                SectionOp::Import {
                    creation: creation.clone(),
                    page: copy.clone(),
                },
            )
            .is_err()
            {
                continue;
            }
            section.seal().unwrap();
            let image = section.image();
            let store = Store::parse(&image).unwrap();
            RevisionIndex::parse(&store)
                .unwrap()
                .validate_current()
                .unwrap();
            expected
                .objects
                .retain(|object| matches!(object, PageObject::Title(_)));
            expected.objects.extend(
                copy.objects
                    .iter()
                    .filter(|object| !matches!(object, PageObject::Title(_)))
                    .cloned(),
            );
            expected.definitions.extend(copy.definitions.clone());
            let stored = read(&image, creation.space());
            // New page children stack as the writers add them.
            let order: Vec<ExGuid> = stored.objects.iter().map(PageObject::id).collect();
            expected
                .objects
                .sort_by_key(|object| order.iter().position(|id| *id == object.id()));
            let mut expected = as_written(&expected, &created, &stored);
            // An outline's height follows its content; no op carries it.
            for (object, stored) in expected.objects.iter_mut().zip(&stored.objects) {
                if let (PageObject::Outline(outline), PageObject::Outline(stored)) =
                    (object, stored)
                {
                    outline.layout.max_height = stored.layout.max_height;
                }
            }
            let (ours, theirs) = (normalize(&stored), normalize(&expected));
            assert!(
                ours == theirs,
                "{name}: {}",
                first_difference(&theirs, &ours)
            );
            imported += 1;
        }
    }
    assert!(imported > 10, "{imported} pages imported");
}

/// The ops for the changed paragraphs of one outline, lowered from them alone.
fn lower_range(before: &Page, after: &Page) -> Option<Result<Vec<PageOp>, crate::Error>> {
    if before.objects.len() != after.objects.len() {
        return None;
    }
    let mut changed = None;
    for (a, b) in before.objects.iter().zip(&after.objects) {
        match (a, b) {
            (PageObject::Outline(a), PageObject::Outline(b)) if a.id == b.id => {
                if a.paragraphs != b.paragraphs {
                    if changed.is_some()
                        || (Outline {
                            paragraphs: Vec::new(),
                            ..a.clone()
                        }) != (Outline {
                            paragraphs: Vec::new(),
                            ..b.clone()
                        })
                    {
                        return None;
                    }
                    changed = Some((a, b));
                }
            }
            (a, b) if a == b => {}
            _ => return None,
        }
    }
    let (a, b) = changed?;
    let (a, b) = (&a.paragraphs, &b.paragraphs);
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (mut end_a, mut end_b) = (a.len() - suffix, b.len() - suffix);
    // A range holds the children of what it changes.
    while end_a < a.len() {
        let ids: Vec<ExGuid> = a[prefix..end_a]
            .iter()
            .chain(&b[prefix..end_b])
            .map(|p| p.id)
            .collect();
        if a[end_a].parent.is_some_and(|parent| ids.contains(&parent)) {
            end_a += 1;
            end_b += 1;
        } else {
            break;
        }
    }
    let outline = match before
        .objects
        .iter()
        .find(|o| matches!(o, PageObject::Outline(o) if o.paragraphs == *a))
    {
        Some(PageObject::Outline(outline)) => outline.id,
        _ => return None,
    };
    let mut definitions = before.definitions.clone();
    definitions.extend(after.definitions.clone());
    Some(lower(
        outline,
        &a[prefix..end_a],
        &b[prefix..end_b],
        b.get(end_b),
        &definitions,
    ))
}

/// Lowering the changed range of an outline stores what lowering the whole page does.
#[test]
fn a_range_lowers_as_its_page_does() {
    let families = [
        Family::Text,
        Family::Format,
        Family::Insert,
        Family::Paste,
        Family::Delete,
        Family::Move,
        Family::Collapse,
        Family::List,
        Family::Tag,
        Family::Split,
        Family::Join,
        Family::Indent,
        Family::Outdent,
        Family::Alignment,
        Family::Style,
        Family::Link,
        Family::NewTable,
        Family::ParagraphPicture,
        Family::Attachment,
        Family::Equation,
    ];
    let (mut same, mut total, mut smaller) = (0, 0, 0);
    for (name, source) in SOURCES {
        for (p, space) in pages(source).into_iter().enumerate().take(4) {
            for family in families {
                for seed in crate::sweep::seeds(0..3, 1) {
                    let mut rng = Rng(seed * 7919 + p as u64 * 131 + family as u64);
                    let before = read(source, space);
                    let mut after = before.clone();
                    if !mutate(&mut after, &mut rng, family) {
                        continue;
                    }
                    let Some(ranged) = lower_range(&before, &after) else {
                        continue;
                    };
                    let whole = lower_page(&before, &after);
                    let (Ok(ranged), Ok(whole)) = (ranged, whole) else {
                        continue;
                    };
                    total += 1;
                    let write = |ops: &[PageOp]| {
                        let arena = Arena::default();
                        let mut section = Section::open(&arena, source.to_vec()).unwrap();
                        let edit = Edit {
                            at: AT,
                            ops: ops
                                .iter()
                                .map(|op| Op::Page {
                                    space,
                                    op: op.clone(),
                                })
                                .collect(),
                        };
                        section
                            .apply("Author", &edit)
                            .map(|()| normalize(&section.page(space).unwrap()))
                    };
                    let (ours, theirs) = (write(&ranged), write(&whole));
                    match (ours, theirs) {
                        (Ok(ours), Ok(theirs)) => {
                            assert!(
                                ours == theirs,
                                "{name} page {p} {family:?} seed {seed}: {}",
                                first_difference(&theirs, &ours)
                            );
                            same += 1;
                        }
                        (Err(_), Err(_)) => same += 1,
                        (ours, theirs) => {
                            panic!("{name} page {p} {family:?} seed {seed}: {ours:?} vs {theirs:?}")
                        }
                    }
                    let length = |ops: &[PageOp]| format!("{ops:?}").len();
                    smaller += usize::from(length(&ranged) <= length(&whole));
                }
            }
        }
    }
    println!("{same} of {total} ranges store what their pages do; {smaller} lowered no larger");
    assert_eq!(same, total);
    assert!(total > 100);
}

/// One keystroke as ops on the 3000-paragraph probe page:
/// `SECTION_PROBE=path cargo test --release -p onestore --lib keystroke_ops -- --ignored --nocapture`.
#[test]
#[ignore]
fn keystroke_ops() {
    use std::time::{Duration, Instant};
    fn median(mut samples: Vec<Duration>) -> Duration {
        samples.sort();
        samples[samples.len() / 2]
    }
    let path = std::env::var("SECTION_PROBE").unwrap_or("/tmp/probe3000.one".into());
    let image = std::fs::read(&path).unwrap();
    let space = pages(&image)
        .into_iter()
        .max_by_key(|space| {
            read(&image, *space).objects.len() + format!("{:?}", read(&image, *space)).len()
        })
        .unwrap();
    let page = read(&image, space);
    let outline = page
        .objects
        .iter()
        .filter_map(|o| match o {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .max_by_key(|o| o.paragraphs.len())
        .unwrap();
    let middle = outline.paragraphs.len() / 2;
    let paragraph = &outline.paragraphs[middle];
    let text = paragraph.text().unwrap().id;
    println!(
        "{path}: {} bytes, {} paragraphs in the outline",
        image.len(),
        outline.paragraphs.len()
    );

    // Emitting: a keystroke is one op; a generic range replacement lowers its paragraphs.
    let mut edited = paragraph.clone();
    let format = edited.text().unwrap().text.format_at(0).unwrap().clone();
    edited
        .text_mut()
        .unwrap()
        .text
        .apply(crate::page::text::Edit {
            range: 0..0,
            replacement: Paragraph::new("x".into(), format),
        })
        .unwrap();
    let mut samples = Vec::new();
    for _ in 0..200 {
        let start = Instant::now();
        let ops = lower(
            outline.id,
            std::slice::from_ref(paragraph),
            std::slice::from_ref(&edited),
            outline.paragraphs.get(middle + 1),
            &page.definitions,
        )
        .unwrap();
        samples.push(start.elapsed());
        assert_eq!(ops.len(), 1);
    }
    println!("lower one paragraph: {:?}", median(samples));

    let arena = Arena::default();
    let start = Instant::now();
    let mut section = Section::open(&arena, image.clone()).unwrap();
    println!("Section::open: {:?}", start.elapsed());
    let start = Instant::now();
    section.active(space).unwrap();
    println!("first edit opens the page: {:?}", start.elapsed());
    // Five keystrokes a second, each sealed: the outline's modification time ticks once a
    // second, which re-declares its child list.
    let (mut applies, mut seals, mut sizes) = (Vec::new(), Vec::new(), Vec::new());
    for keystroke in 0..300u32 {
        let edit = Edit {
            at: AT + u64::from(keystroke) * 2_000_000,
            ops: vec![Op::Page {
                space,
                op: PageOp::Text {
                    text,
                    range: keystroke..keystroke,
                    with: "x".into(),
                },
            }],
        };
        let start = Instant::now();
        section.apply("Probe", &edit).unwrap();
        applies.push(start.elapsed());
        let start = Instant::now();
        let transaction = section.seal().unwrap().unwrap();
        seals.push(start.elapsed());
        sizes.push((
            transaction.append.len(),
            transaction
                .patches
                .iter()
                .map(|(_, bytes)| bytes.len())
                .sum::<usize>(),
        ));
    }
    sizes.sort();
    println!(
        "Section::apply(Text) {:?}, seal {:?}; appended {} bytes (max {}), patched {} bytes (median of 300)",
        median(applies),
        median(seals),
        sizes[sizes.len() / 2].0,
        sizes.last().unwrap().0,
        sizes[sizes.len() / 2].1,
    );
    let start = Instant::now();
    let reopened = Section::open(&arena, section.image()).unwrap();
    println!("reopen after 300 seals: {:?}", start.elapsed());
    assert_eq!(reopened.page(space).unwrap(), section.page(space).unwrap());
}

/// Writes a notebook for the native cold-open gate of the op path to `OP_GATE_EXPORT`:
/// corpus pages imported as ops, then edited by lowered page edits of every family, new
/// pages, an outline move on a page without a margin origin, page moves and a removal.
#[test]
#[ignore]
fn op_gate_candidate() {
    let directory = std::path::PathBuf::from(std::env::var("OP_GATE_EXPORT").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    let source = crate::create_section("ops.one", "Ops gate", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source).unwrap();
    let at = std::cell::Cell::new(AT);
    let next = || {
        at.set(at.get() + 20_000_000);
        at.get()
    };
    let apply = |section: &mut Section<'_>, op: Op| {
        let result = section.apply(
            "Gate author",
            &Edit {
                at: next(),
                ops: vec![op],
            },
        );
        section.seal().unwrap();
        result
    };
    let mut imported = Vec::new();
    for (name, bytes) in SOURCES.iter().filter(|(name, _)| *name != "enter-probe") {
        let Some(space) = pages(bytes).first().copied() else {
            continue;
        };
        let page = read(bytes, space);
        if page
            .objects
            .iter()
            .any(|o| matches!(o, PageObject::Unsupported(_)))
        {
            continue;
        }
        let Ok(copy) = page.copy() else { continue };
        let creation =
            crate::PageCreation::new(None, Some(&format!("Imported {name}")), "Gate author")
                .unwrap();
        if apply(
            &mut section,
            Op::Section(SectionOp::Import {
                creation: creation.clone(),
                page: copy,
            }),
        )
        .is_ok()
        {
            imported.push(creation.space());
        }
    }
    let mut edits = 0;
    for (n, space) in imported.iter().enumerate() {
        for (k, family) in FAMILIES.iter().enumerate() {
            if (k + n) % 4 != 0 {
                continue;
            }
            let before = section.page(*space).unwrap();
            let mut after = before.clone();
            let mut rng = Rng((n * 100 + k) as u64);
            if !mutate(&mut after, &mut rng, *family) {
                continue;
            }
            // Edits inside a field code leave a field OneNote and this reader resolve apart,
            // whichever writer stores them.
            let fields = |page: &Page| -> Vec<(ExGuid, String)> {
                model::lists(page)
                    .into_iter()
                    .flat_map(|(_, _, list)| list.iter())
                    .filter_map(|p| p.text())
                    .filter(|t| t.text.text().contains('\u{fddf}'))
                    .map(|t| (t.id, t.text.text().to_owned()))
                    .collect()
            };
            let edited = fields(&before);
            if fields(&after)
                .iter()
                .any(|(id, text)| edited.iter().any(|(e, old)| e == id && old != text))
            {
                continue;
            }
            if section
                .apply_page("Gate author", next(), *space, &after)
                .is_ok()
            {
                section.seal().unwrap();
                edits += 1;
            }
        }
    }
    // A created page: margins, an outline, and its first move adding the margin origin.
    let creation = crate::PageCreation::new(None, Some("Created by ops"), "Gate author").unwrap();
    apply(
        &mut section,
        Op::Section(SectionOp::Create(creation.clone())),
    )
    .unwrap();
    let outline = Outline {
        id: id(),
        title: false,
        min_width: None,
        layout: Layout {
            x: Some(36.0),
            y: Some(86.4),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: vec![text_paragraph("Typed through ops", Format::default())],
        unsupported: Vec::new(),
    };
    let outline_id = outline.id;
    let text = outline.paragraphs[0].text().unwrap().id;
    let space = creation.space();
    let page = |op| Op::Page { space, op };
    apply(
        &mut section,
        page(PageOp::Add {
            object: PageObject::Outline(outline),
            before: None,
        }),
    )
    .unwrap();
    apply(
        &mut section,
        page(PageOp::Outline {
            object: outline_id,
            edit: crate::OutlineEdit::Position { x: 72.0, y: 108.0 },
        }),
    )
    .unwrap();
    apply(
        &mut section,
        page(PageOp::Text {
            text,
            range: 5..5,
            with: " and sealed".into(),
        }),
    )
    .unwrap();
    apply(
        &mut section,
        page(PageOp::Link {
            text,
            range: 0..5,
            target: Some("https://example.invalid/ops".into()),
        }),
    )
    .unwrap();
    let spaces: Vec<ExGuid> = section
        .pages()
        .unwrap()
        .into_iter()
        .map(|(s, _, _)| s)
        .collect();
    apply(
        &mut section,
        Op::Section(SectionOp::Pages(vec![
            crate::PageEdit::move_to(space, Some(spaces[1]), 1).unwrap(),
        ])),
    )
    .unwrap();
    apply(
        &mut section,
        Op::Section(SectionOp::Pages(vec![
            crate::PageEdit::set_level(spaces[2], 2).unwrap(),
        ])),
    )
    .unwrap();
    apply(
        &mut section,
        Op::Section(SectionOp::Delete(vec![*spaces.last().unwrap()])),
    )
    .unwrap();
    let image = section.image();
    let store = Store::parse(&image).unwrap();
    RevisionIndex::parse(&store)
        .unwrap()
        .validate_current()
        .unwrap();
    let count = section.pages().unwrap().len();
    std::fs::write(directory.join("ops.one"), &image).unwrap();
    std::fs::write(
        directory.join("Open Notebook.onetoc2"),
        crate::create_table_of_contents(
            "Open Notebook.onetoc2",
            &[("ops.one", store.header.file_id)],
        )
        .unwrap(),
    )
    .unwrap();
    std::fs::write(directory.join("expected-count.txt"), count.to_string()).unwrap();
    println!(
        "{} imported pages, {edits} lowered edits, {count} pages",
        imported.len()
    );
}

/// OneNote 2010's own date change (`corpus/page-date/native`, COM `UpdatePageContent`
/// with a new `dateTime`): the page's second page, "Changed date", took it; "Kept date"
/// did not.
const DATED: &[u8] = include_bytes!("../../../../corpus/page-date/native/notebook/pictures.one");

/// The objects of `space`'s active revision that differ from its previous one, by jcid.
fn changed_jcids(image: &[u8], space: ExGuid) -> Vec<u32> {
    let store = Store::parse(image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let active = index.active(space).unwrap();
    let previous = index.spaces[&space].revisions[&active].dependency.unwrap();
    let (old, new) = (
        index.resolve(space, previous).unwrap(),
        index.resolve(space, active).unwrap(),
    );
    let mut jcids: Vec<u32> = new
        .objects
        .iter()
        .filter(|(id, object)| {
            old.objects
                .get(id)
                .is_none_or(|old| format!("{:?}", old.data) != format!("{:?}", object.data))
        })
        .map(|(_, object)| object.jcid)
        .collect();
    jcids.sort();
    jcids
}

/// The title's date and time fields of `page`, by text object.
fn date_fields(page: &Page) -> Vec<(ExGuid, String)> {
    model::date_fields(page)
        .into_iter()
        .map(|text| (text.id, text.text.text().to_owned()))
        .collect()
}

/// FILETIME of 2025-07-04 16:45 UTC, which a Pacific clock shows as 9:45 AM.
const DATED_AT: u64 = 133_961_211_000_000_000;

/// The `Date` op writes what OneNote writes for a new page date: the metadata's creation
/// time, the date and time field text, and the modification times above them.
#[test]
fn a_page_date_stores_what_onenote_stores() {
    let spaces = pages(DATED);
    let title = |space: &ExGuid| read(DATED, *space).title;
    let kept = *spaces
        .iter()
        .find(|space| title(space) == "Kept date")
        .unwrap();
    let changed = *spaces
        .iter()
        .find(|space| title(space) == "Changed date")
        .unwrap();
    let native = changed_jcids(DATED, changed);
    assert_eq!(
        native,
        [0x20030, 0x6000c, 0x6000d, 0x6000d, 0x6000e, 0x6000e]
    );

    let arena = Arena::default();
    let mut section = Section::open(&arena, DATED.to_vec()).unwrap();
    let before = section.page(kept).unwrap();
    // The date field precedes the time field.
    let fields: Vec<(ExGuid, String)> = date_fields(&before)
        .into_iter()
        .zip(["Friday, July 04, 2025", "9:45 AM"])
        .map(|((id, _), shown)| (id, shown.to_owned()))
        .collect();
    assert_eq!(fields.len(), 2);
    let op = PageOp::Date {
        created: DATED_AT,
        fields: fields.clone(),
    };
    let mut predicted = before.clone();
    model::apply(&mut predicted, &op).unwrap();
    section
        .apply(
            "Author",
            &Edit {
                at: AT,
                ops: vec![Op::Page { space: kept, op }],
            },
        )
        .unwrap();
    section.seal().unwrap();
    let image = section.image();
    let stored = read(&image, kept);
    assert_eq!(stored.created, Some(DATED_AT));
    assert_eq!(date_fields(&stored), fields);
    assert_eq!(stored, predicted);
    assert_eq!(changed_jcids(&image, kept), native);
    let store = Store::parse(&image).unwrap();
    RevisionIndex::parse(&store)
        .unwrap()
        .validate_current()
        .unwrap();

    // The whole-page lowering reaches the same page through the same op.
    let mut after = before.clone();
    after.created = Some(DATED_AT);
    model::apply(
        &mut after,
        &PageOp::Date {
            created: DATED_AT,
            fields,
        },
    )
    .unwrap();
    assert!(matches!(
        &lower_page(&before, &after).unwrap()[..],
        [PageOp::Date { .. }]
    ));

    if let Some(directory) = std::env::var_os("ONESTORE_PAGE_DATE_EXPORT") {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join("pictures.one"), &image).unwrap();
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../corpus/page-date/native/notebook/Open Notebook.onetoc2"
            ),
            directory.join("Open Notebook.onetoc2"),
        )
        .unwrap();
    }
}

/// OneNote 2010's View, Rule Lines (`corpus/rule-lines/native`): each page is titled for
/// what its menu gave it, and "Narrow" was ruled, then set back to None.
const RULED: &[u8] = include_bytes!("../../../../corpus/rule-lines/native/notebook/Rules.one");

/// The rule-line properties on `space`'s page node, in stored order.
fn rule_properties(image: &[u8], space: ExGuid) -> Vec<(u32, Vec<u8>)> {
    let store = Store::parse(image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let revision = index.resolve(space, index.active(space).unwrap()).unwrap();
    let document = Document::parse(&index).unwrap();
    let [node] = crate::active::manifest_pages(document.active(space).unwrap())[..] else {
        panic!("one page node")
    };
    let crate::ObjectData::Properties(bytes) = revision.objects[&node].data else {
        panic!("a page node's properties")
    };
    crate::PropertySets::parse(bytes).unwrap().sets[0]
        .iter()
        .filter(|property| crate::page::RULE_LINES.contains(&property.id))
        .map(|property| match property.value {
            crate::Value::Bytes(bytes) => (property.id, bytes.to_vec()),
            _ => panic!("rule lines are scalars"),
        })
        .collect()
}

/// Rule lines read as OneNote's page XML reports them, and the `RuleLines` op stores the
/// properties OneNote stores, in its order, or removes them for None.
#[test]
fn rule_lines_store_what_onenote_stores() {
    let apply = |section: &mut Section, op| {
        section.apply(
            "Author",
            &Edit {
                at: AT,
                ops: vec![op],
            },
        )
    };
    let titled = |image: &[u8], title: &str| {
        *pages(image)
            .iter()
            .find(|space| read(image, **space).title == title)
            .unwrap()
    };
    let lines = |title| read(RULED, titled(RULED, title)).rule_lines;
    let blue = 0xfdebca;
    assert_eq!(lines("None"), None);
    assert_eq!(lines("Narrow"), None);
    // The page XML reports these spacings in points.
    for (title, spacing, vertical) in [
        ("College", 23.76, None),
        ("Standard", 33.12, None),
        ("Wide", 46.8, None),
        ("SmallGrid", 12.0, Some(blue)),
        ("MediumGrid", 28.3464, Some(0xffc9e0)),
        ("LargeGrid", 42.5196, Some(blue)),
        ("VeryLargeGrid", 56.6928, Some(blue)),
    ] {
        let lines = lines(title).unwrap();
        assert!((lines.spacing * 36.0 - spacing).abs() < 1e-3, "{title}");
        assert_eq!(lines.color, vertical.unwrap_or(blue), "{title}");
        assert_eq!(
            lines.vertical,
            match vertical {
                Some(color) => VerticalRule::Grid {
                    spacing: lines.spacing,
                    color,
                },
                None => VerticalRule::Margin(0x5050ff),
            },
            "{title}"
        );
    }
    assert_eq!(lines("ColorRed").unwrap().color, 0xd6d4ff);
    // "Hidden" is Standard in the menu's "<none>" colour, white, on a Teal page.
    assert_eq!(lines("Hidden").unwrap().color, 0xffffff);

    // Each page's lines given to the unruled page store what OneNote stored for them.
    let arena = Arena::default();
    let bare = titled(RULED, "None");
    for title in [
        "College",
        "Standard",
        "Wide",
        "SmallGrid",
        "MediumGrid",
        "LargeGrid",
        "VeryLargeGrid",
        "ColorRed",
        "Hidden",
    ] {
        let mut section = Section::open(&arena, RULED.to_vec()).unwrap();
        let op = PageOp::RuleLines(lines(title));
        let mut predicted = section.page(bare).unwrap();
        model::apply(&mut predicted, &op).unwrap();
        apply(&mut section, Op::Page { space: bare, op }).unwrap();
        section.seal().unwrap();
        let image = section.image();
        assert_eq!(read(&image, bare), predicted, "{title}");
        assert_eq!(
            rule_properties(&image, bare),
            rule_properties(RULED, titled(RULED, title)),
            "{title}"
        );
    }

    // The candidate: rules added, changed between ruled and grid, and removed on OneNote's
    // pages, and a created page ruled.
    let mut section = Section::open(&arena, RULED.to_vec()).unwrap();
    let creation = crate::PageCreation::new(None, Some("Created"), "Author").unwrap();
    apply(
        &mut section,
        Op::Section(SectionOp::Create(creation.clone())),
    )
    .unwrap();
    let wide = lines("Wide");
    let red_narrow = crate::page::RuleLines {
        spacing: f32::from_bits(0x3ebe_f8d2),
        color: 0xd6d4ff,
        vertical: VerticalRule::Margin(0x5050ff),
    };
    let edits = [
        (bare, wide),
        (titled(RULED, "College"), None),
        (titled(RULED, "Standard"), lines("SmallGrid")),
        (titled(RULED, "MediumGrid"), Some(red_narrow)),
        (creation.space(), lines("VeryLargeGrid")),
    ];
    for (space, lines) in edits {
        apply(
            &mut section,
            Op::Page {
                space,
                op: PageOp::RuleLines(lines),
            },
        )
        .unwrap();
        section.seal().unwrap();
    }
    let image = section.image();
    RevisionIndex::parse(&Store::parse(&image).unwrap())
        .unwrap()
        .validate_current()
        .unwrap();
    for (space, lines) in edits {
        assert_eq!(read(&image, space).rule_lines, lines);
    }
    assert!(rule_properties(&image, titled(&image, "College")).is_empty());

    // The whole-page lowering reaches the same pages through the same op.
    let before = read(RULED, bare);
    let mut after = before.clone();
    after.rule_lines = wide;
    assert_eq!(
        lower_page(&before, &after).unwrap(),
        [PageOp::RuleLines(wide)]
    );

    if let Some(directory) = std::env::var_os("ONESTORE_RULE_LINES_EXPORT") {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join("Rules.one"), &image).unwrap();
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../corpus/rule-lines/native/notebook/Open Notebook.onetoc2"
            ),
            directory.join("Open Notebook.onetoc2"),
        )
        .unwrap();
    }
}

/// Text added without a language stores the writer's, and lowering it back must not write
/// LCID 0 on the text object, which runs later written without a language would read.
#[test]
fn an_unset_language_is_not_paragraph_formatting() {
    let language = |language| Format {
        language,
        ..Default::default()
    };
    let stored = Paragraph::new(String::new(), language(Some(0x409)));
    let fields = |target| lower::paragraph_fields(&stored, &target).unwrap().language;
    assert_eq!(fields(Paragraph::new("Fresh".into(), language(None))), None);
    assert_eq!(
        fields(Paragraph::new("Frais".into(), language(Some(0x40c)))),
        Some(0x40c)
    );
}

/// A paragraph given a language, then an equation whose runs name none: the model predicts
/// what the section stores.
#[test]
fn runs_written_after_a_paragraph_language_read_as_predicted() {
    let (_, source) = SOURCES[1];
    let space = pages(source)[0];
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.to_vec()).unwrap();
    let before = section.page(space).unwrap();
    let paragraph = model::lists(&before)
        .into_iter()
        .flat_map(|(_, _, list)| list.iter())
        .find(|paragraph| {
            paragraph
                .text()
                .is_some_and(|text| !text.text.text().is_empty())
        })
        .unwrap()
        .clone();
    let text = paragraph.text().unwrap().id;
    let math = Format {
        math: Some(true),
        ..Default::default()
    };
    let ops = [
        PageOp::Paragraph {
            paragraph: paragraph.id,
            alignment: None,
            rtl: None,
            space_before: None,
            space_after: None,
            line_spacing: None,
            language: Some(0x40c),
        },
        PageOp::Equation {
            text,
            math: Paragraph::new("x".into(), math),
        },
    ];
    let mut predicted = before;
    for op in ops {
        model::apply(&mut predicted, &op).unwrap();
        section
            .apply(
                "Author",
                &Edit {
                    at: AT,
                    ops: vec![Op::Page { space, op }],
                },
            )
            .unwrap();
        let stored = section.page(space).unwrap();
        assert_eq!(
            normalize(&predicted),
            normalize(&stored),
            "{}",
            first_difference(&normalize(&stored), &normalize(&predicted))
        );
    }
}

/// Undoing a deletion adds the object back whole: an outline whose first paragraph OneNote
/// tabbed into a group, and a TIFF picture OneNote imported, store as they were.
#[test]
fn deleted_objects_added_back_store_as_they_were() {
    let tree = SOURCES[2].1;
    let features = SOURCES[3].1;
    for (source, title) in [
        (tree, "Outdent first group"),
        (tree, "Delete unindented sibling after group"),
        (features, "Image tiff"),
    ] {
        let space = *pages(source)
            .iter()
            .find(|space| read(source, **space).title == title)
            .unwrap();
        let arena = Arena::default();
        let mut section = Section::open(&arena, source.to_vec()).unwrap();
        let before = section.page(space).unwrap();
        let shape = |section: &mut Section, id: ExGuid| {
            let view = &section.active(space).unwrap().view;
            let mut shape = Vec::new();
            let mut pending = vec![(id, 0)];
            while let Some((id, depth)) = pending.pop() {
                let node = &view.nodes[&id];
                shape.push((
                    depth,
                    node.child_level,
                    matches!(node.kind, Kind::OutlineGroup),
                ));
                pending.extend(node.children.iter().rev().map(|child| (*child, depth + 1)));
            }
            shape
        };
        let at = before
            .objects
            .iter()
            .position(|object| matches!(object, PageObject::Outline(_) | PageObject::Image(_)))
            .unwrap();
        let object = before.objects[at].id();
        let stored = shape(&mut section, object);
        let edit = |ops| Edit { at: AT, ops };
        section
            .apply(
                "Author",
                &edit(vec![Op::Page {
                    space,
                    op: PageOp::Delete { object },
                }]),
            )
            .unwrap();
        let gone = section.page(space).unwrap();
        let ops = lower_page(&gone, &before)
            .unwrap()
            .into_iter()
            .map(|op| Op::Page { space, op })
            .collect();
        section
            .apply("Author", &edit(ops))
            .unwrap_or_else(|error| panic!("{title}: {error:?}"));
        let after = section.page(space).unwrap();
        assert_eq!(normalize(&after), normalize(&before), "{title}");
        assert_eq!(shape(&mut section, object), stored, "{title}");
    }
}
