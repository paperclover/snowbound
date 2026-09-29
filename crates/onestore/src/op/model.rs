//! Ops interpreted on a page model: what reading the page back after `Section::apply`
//! yields, for the lowering's prediction and as the tests' oracle.

use super::{PageOp, TableEdit, TextProperty, content::NATIVE_INDENTS, lower::format_in};
use crate::{
    Error, ExGuid, OutlineEdit, TextAttribute,
    document::{Format, Kind},
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, Table, TextObject,
    },
};
use std::collections::BTreeSet;

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// A length in points as the store keeps it, in half inches.
fn stored(points: f32) -> f32 {
    points / 36.0 * 36.0
}

/// Where a flat paragraph list lies: a page object, a title outline, then the table cells
/// on the way down, each as (paragraph index, row, cell).
#[derive(Clone, Debug)]
pub(crate) struct Path {
    object: usize,
    title_outline: Option<usize>,
    cells: Vec<(usize, usize, usize)>,
}

/// The lists of a page, each with its path and owner (outline or cell identity).
pub(crate) fn lists(page: &Page) -> Vec<(Path, ExGuid, &Vec<PageParagraph>)> {
    fn cells<'p>(
        path: &Path,
        list: &'p [PageParagraph],
        out: &mut Vec<(Path, ExGuid, &'p Vec<PageParagraph>)>,
    ) {
        for (index, paragraph) in list.iter().enumerate() {
            if let ParagraphContent::Table(table) = &paragraph.content {
                for (r, row) in table.rows.iter().enumerate() {
                    for (c, cell) in row.cells.iter().enumerate() {
                        let mut path = path.clone();
                        path.cells.push((index, r, c));
                        out.push((path.clone(), cell.id, &cell.paragraphs));
                        cells(&path, &cell.paragraphs, out);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (object, value) in page.objects.iter().enumerate() {
        let outlines: Vec<(Option<usize>, &Outline)> = match value {
            PageObject::Outline(outline) => vec![(None, outline)],
            PageObject::Title(title) => title
                .outlines
                .iter()
                .enumerate()
                .map(|(i, o)| (Some(i), o))
                .collect(),
            _ => Vec::new(),
        };
        for (title_outline, outline) in outlines {
            let path = Path {
                object,
                title_outline,
                cells: Vec::new(),
            };
            out.push((path.clone(), outline.id, &outline.paragraphs));
            cells(&path, &outline.paragraphs, &mut out);
        }
    }
    out
}

pub(crate) fn list_at<'p>(page: &'p mut Page, path: &Path) -> &'p mut Vec<PageParagraph> {
    let outline = match (&mut page.objects[path.object], path.title_outline) {
        (PageObject::Outline(outline), None) => outline,
        (PageObject::Title(title), Some(i)) => &mut title.outlines[i],
        _ => unreachable!(),
    };
    let mut list = &mut outline.paragraphs;
    for (index, r, c) in &path.cells {
        let ParagraphContent::Table(table) = &mut list[*index].content else {
            unreachable!()
        };
        list = &mut table.rows[*r].cells[*c].paragraphs;
    }
    list
}

/// The list holding paragraph `id`, and its index there.
fn find(page: &Page, id: ExGuid) -> Option<(Path, usize)> {
    lists(page).into_iter().find_map(|(path, _, list)| {
        list.iter()
            .position(|p| p.id == id)
            .map(|index| (path, index))
    })
}

/// The list a container's children lie in: its own for an outline or cell, the one
/// holding it for a paragraph.
fn container(page: &Page, id: ExGuid) -> Option<(Path, Option<usize>)> {
    lists(page).into_iter().find_map(|(path, owner, list)| {
        if owner == id {
            Some((path, None))
        } else {
            list.iter()
                .position(|p| p.id == id)
                .map(|index| (path, Some(index)))
        }
    })
}

/// The entries of `list` in the subtree of the paragraph at `index`.
fn subtree(list: &[PageParagraph], index: usize) -> std::ops::Range<usize> {
    let mut members = BTreeSet::from([list[index].id]);
    let mut end = index + 1;
    while end < list.len() && list[end].parent.is_some_and(|p| members.contains(&p)) {
        members.insert(list[end].id);
        end += 1;
    }
    index..end
}

pub(crate) fn paragraph(page: &Page, id: ExGuid) -> Option<&PageParagraph> {
    lists(page)
        .into_iter()
        .find_map(|(_, _, list)| list.iter().find(|p| p.id == id))
}

pub(crate) fn outline(page: &Page, id: ExGuid) -> Option<&Outline> {
    page.objects.iter().find_map(|object| match object {
        PageObject::Outline(outline) if outline.id == id => Some(outline),
        _ => None,
    })
}

pub(crate) fn table(page: &Page, id: ExGuid) -> Option<&Table> {
    lists(page).into_iter().find_map(|(_, _, list)| {
        list.iter().find_map(|p| match &p.content {
            ParagraphContent::Table(table) if table.id == id => Some(table),
            _ => None,
        })
    })
}

/// Whether a paragraph of the page lists `list` among its list nodes.
pub(crate) fn owns_list(page: &Page, list: ExGuid) -> bool {
    lists(page)
        .into_iter()
        .any(|(_, _, paragraphs)| paragraphs.iter().any(|p| p.lists.contains(&list)))
}

/// Identities of paragraphs and what they hold, tables included.
pub(crate) fn identities(paragraphs: &[PageParagraph]) -> Vec<ExGuid> {
    let mut out = Vec::new();
    for paragraph in paragraphs {
        out.push(paragraph.id);
        match &paragraph.content {
            ParagraphContent::Text(text) => out.push(text.id),
            ParagraphContent::Table(table) => {
                out.push(table.id);
                for row in &table.rows {
                    out.push(row.id);
                    for cell in &row.cells {
                        out.push(cell.id);
                        out.extend(identities(&cell.paragraphs));
                    }
                }
            }
            ParagraphContent::Image(image) => out.push(image.id),
            ParagraphContent::Attachment(attachment) => out.push(attachment.id),
            ParagraphContent::Ink(ink) => {
                out.push(ink.id);
                out.extend(ink.strokes.iter().map(|s| s.id));
            }
            ParagraphContent::Unsupported(unsupported) => out.push(unsupported.id),
        }
        out.extend(paragraph.lists.iter().copied());
    }
    out
}

/// Whether the page holds `id` as an object of the model.
pub(crate) fn holds(page: &Page, id: ExGuid) -> bool {
    page.objects.iter().any(|object| {
        object.id() == id
            || match object {
                PageObject::Outline(outline) => identities(&outline.paragraphs).contains(&id),
                PageObject::Title(title) => title
                    .outlines
                    .iter()
                    .any(|o| o.id == id || identities(&o.paragraphs).contains(&id)),
                PageObject::Ink(ink) => ink.strokes.iter().any(|s| s.id == id),
                PageObject::Image(_) | PageObject::Unsupported(_) => false,
            }
    })
}

/// The paragraph holding text object `text`: its list and index.
fn text_holder(page: &Page, text: ExGuid) -> Option<(Path, usize)> {
    lists(page).into_iter().find_map(|(path, _, list)| {
        list.iter()
            .position(|p| p.text().is_some_and(|t| t.id == text))
            .map(|index| (path, index))
    })
}

/// The text of the title's date and time fields, in order.
pub(crate) fn date_fields(page: &Page) -> Vec<&TextObject> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Title(title) => {
                let date = title.date?;
                title.outlines.iter().find(|outline| outline.id == date)
            }
            _ => None,
        })
        .flat_map(|outline| outline.paragraphs.iter().filter_map(PageParagraph::text))
        .collect()
}

/// Text object `text` wherever the page holds it.
pub(crate) fn paragraph_text(page: &Page, text: ExGuid) -> Option<&TextObject> {
    lists(page).into_iter().find_map(|(_, _, list)| {
        list.iter()
            .find_map(|paragraph| paragraph.text().filter(|object| object.id == text))
    })
}

fn text_mut(page: &mut Page, text: ExGuid) -> Result<&mut TextObject, Error> {
    let (path, index) =
        text_holder(page, text).ok_or_else(|| invalid("Text is not on the page"))?;
    Ok(list_at(page, &path)[index].text_mut().unwrap())
}

fn paragraph_mut(page: &mut Page, id: ExGuid) -> Result<&mut PageParagraph, Error> {
    let (path, index) = find(page, id).ok_or_else(|| invalid("Paragraph is not on the page"))?;
    Ok(&mut list_at(page, &path)[index])
}

fn table_mut(page: &mut Page, id: ExGuid) -> Result<&mut Table, Error> {
    let (path, index) = lists(page)
        .into_iter()
        .find_map(|(path, _, list)| {
            list.iter()
                .position(|p| matches!(&p.content, ParagraphContent::Table(t) if t.id == id))
                .map(|index| (path, index))
        })
        .ok_or_else(|| invalid("Table is not on the page"))?;
    let ParagraphContent::Table(table) = &mut list_at(page, &path)[index].content else {
        unreachable!()
    };
    Ok(table)
}

/// `text` with a range replaced as the text writer does: inserted text takes the format of
/// the run it lands in, and emptied text keeps its last run's.
pub(crate) fn replace_text(
    text: &Paragraph,
    range: std::ops::Range<u32>,
    with: &str,
) -> Result<Paragraph, Error> {
    let length = text.utf16_offset(text.text().len())?;
    if range.start > range.end || range.end > length {
        return Err(invalid("The edit range exceeds the text"));
    }
    // Replacing text with itself leaves its formatting as it is.
    if text.text()[text.byte_offset(range.start)?..text.byte_offset(range.end)?] == *with {
        return Ok(text.clone());
    }
    let format = format_in(text, range.start)?.clone();
    let removed = range.end - range.start;
    if length - removed == 0 && with.is_empty() {
        return Ok(Paragraph::new(
            String::new(),
            text.spans().last().unwrap().format.clone(),
        ));
    }
    let mut result = text.clone();
    result.apply(crate::page::text::Edit {
        range,
        replacement: Paragraph::new(with.to_owned(), format),
    })?;
    Ok(result)
}

/// `text` with every span taking `from`'s paragraph formatting, which the text object
/// stores for all its runs.
fn paragraph_formatted(text: &Paragraph, from: &Format) -> Paragraph {
    let mut previous = 0;
    Paragraph::from_runs(text.spans().iter().map(|span| {
        let mut format = span.format.clone();
        format.alignment = from.alignment;
        format.rtl = from.rtl;
        format.space_before = from.space_before;
        format.space_after = from.space_after;
        format.line_spacing = from.line_spacing;
        let run = (text.text()[previous..span.end].to_owned(), format);
        previous = span.end;
        run
    }))
}

fn set(format: &mut Format, attribute: &TextAttribute) {
    let color = |value: &Option<[u8; 3]>| {
        Some(value.map_or(0xff000000, |[r, g, b]| u32::from_le_bytes([r, g, b, 0])))
    };
    match attribute {
        TextAttribute::Bold(v) => format.bold = Some(*v),
        TextAttribute::Italic(v) => format.italic = Some(*v),
        TextAttribute::Underline(v) => format.underline = Some(*v),
        TextAttribute::Strike(v) => format.strike = Some(*v),
        TextAttribute::Superscript(v) => {
            format.superscript = Some(*v);
            if *v {
                format.subscript = Some(false);
            }
        }
        TextAttribute::Subscript(v) => {
            format.subscript = Some(*v);
            if *v {
                format.superscript = Some(false);
            }
        }
        TextAttribute::Font(font) => format.font = Some(font.clone()),
        TextAttribute::FontSize(size) => format.font_size = Some(*size),
        TextAttribute::Color(value) => format.color = color(value),
        TextAttribute::Highlight(value) => format.highlight = color(value),
        TextAttribute::Language(language) => format.language = Some(*language),
        TextAttribute::Hidden(v) => format.hidden = Some(*v),
        TextAttribute::Hyperlink(v) => format.hyperlink = Some(*v),
        TextAttribute::HyperlinkLabel(v) => format.hyperlink_label = Some(*v),
        TextAttribute::Math(v) => format.math = Some(*v),
    }
}

fn clear(format: &mut Format, property: TextProperty) {
    match property {
        TextProperty::Bold => format.bold = None,
        TextProperty::Italic => format.italic = None,
        TextProperty::Underline => format.underline = None,
        TextProperty::Strike => format.strike = None,
        TextProperty::Superscript => format.superscript = None,
        TextProperty::Subscript => format.subscript = None,
        TextProperty::Hidden => format.hidden = None,
        TextProperty::Hyperlink => format.hyperlink = None,
        TextProperty::HyperlinkLabel => format.hyperlink_label = None,
        TextProperty::Math => format.math = None,
        TextProperty::Font => format.font = None,
        TextProperty::FontSize => format.font_size = None,
        TextProperty::Color => format.color = None,
        TextProperty::Highlight => format.highlight = None,
    }
}

/// `text` with a range reformatted as the format writer does; a reformatted run states its
/// language.
pub(crate) fn format_text(
    text: &Paragraph,
    range: std::ops::Range<u32>,
    attributes: &[TextAttribute],
    cleared: &[TextProperty],
    style: &Format,
) -> Result<Paragraph, Error> {
    let apply = |format: &Format| {
        let mut format = format.clone();
        for attribute in attributes {
            set(&mut format, attribute);
        }
        for property in cleared {
            clear(&mut format, *property);
        }
        // A cleared property shows the paragraph style's value.
        format = format.inherit(style);
        if format.language.is_none() {
            format.language = Some(0x409);
        }
        format
    };
    if text.text().is_empty() {
        return Ok(Paragraph::new(
            String::new(),
            apply(&text.spans()[0].format),
        ));
    }
    let (start, end) = (text.byte_offset(range.start)?, text.byte_offset(range.end)?);
    let mut runs = Vec::new();
    let mut previous = 0;
    for span in text.spans() {
        for (from, to, selected) in [
            (previous, start.clamp(previous, span.end), false),
            (
                start.clamp(previous, span.end),
                end.clamp(previous, span.end),
                true,
            ),
            (end.clamp(previous, span.end), span.end, false),
        ] {
            if from < to {
                let format = if selected {
                    apply(&span.format)
                } else {
                    span.format.clone()
                };
                runs.push((text.text()[from..to].to_owned(), format));
            }
        }
        previous = span.end;
    }
    Ok(Paragraph::from_runs(runs))
}

/// A text as `Insert` stores it: an unset language is the writer's insertion language, and
/// paragraph formatting is stored only where every span shares a value other than the default.
fn inserted(text: &Paragraph) -> Paragraph {
    let spans = text.spans();
    macro_rules! shared {
        ($field:ident, $type:ty, $store:expr) => {{
            let first = spans[0].format.$field.unwrap_or_default();
            (spans
                .iter()
                .all(|s| s.format.$field.unwrap_or_default() == first)
                && first != <$type>::default())
            .then(|| $store(first))
        }};
    }
    let alignment = shared!(alignment, u8, |v| v);
    let rtl = shared!(rtl, bool, |v| v);
    let space_before = shared!(space_before, f32, stored);
    let space_after = shared!(space_after, f32, stored);
    let line_spacing = shared!(line_spacing, f32, stored);
    let mut previous = 0;
    Paragraph::from_runs(spans.iter().map(|span| {
        let mut format = span.format.clone();
        format.language.get_or_insert(0x409);
        format.alignment = alignment;
        format.rtl = rtl;
        format.space_before = space_before;
        format.space_after = space_after;
        format.line_spacing = line_spacing;
        let run = (text.text()[previous..span.end].to_owned(), format);
        previous = span.end;
        run
    }))
}

/// A stroke as the store keeps it: points on the HIMETRIC grid, pen size in HIMETRIC.
fn stroked(stroke: &crate::page::InkStroke) -> crate::page::InkStroke {
    let himetric = |points: f32| points * 2540.0 / 72.0 * 72.0 / 2540.0;
    crate::page::InkStroke {
        points: stroke
            .points
            .iter()
            .map(|[x, y]| [crate::page::ink::snap(*x), crate::page::ink::snap(*y)])
            .collect(),
        width: himetric(stroke.width),
        height: himetric(stroke.height),
        ..stroke.clone()
    }
}

fn inked(ink: &crate::page::Ink) -> crate::page::Ink {
    crate::page::Ink {
        strokes: ink.strokes.iter().map(stroked).collect(),
        ..ink.clone()
    }
}

/// Paragraphs as `Insert` stores them.
fn insertion(paragraphs: &[PageParagraph]) -> Vec<PageParagraph> {
    paragraphs
        .iter()
        .map(|paragraph| {
            let mut paragraph = paragraph.clone();
            match &mut paragraph.content {
                ParagraphContent::Text(text) if !crate::page::Math::is_equation(&text.text) => {
                    text.text = inserted(&text.text);
                }
                // An equation is written whole, without paragraph formatting.
                ParagraphContent::Text(text) => {
                    text.text = paragraph_formatted(&text.text, &Format::default());
                }
                ParagraphContent::Table(table) => {
                    for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                        if cell.indents.is_empty() {
                            cell.indents = NATIVE_INDENTS.to_vec();
                        }
                        cell.paragraphs = insertion(&cell.paragraphs);
                    }
                }
                ParagraphContent::Ink(ink) => *ink = inked(ink),
                _ => {}
            }
            paragraph
        })
        .collect()
}

/// Inserts `paragraphs` (roots then descendants) into `container` before `before`.
fn insert(
    page: &mut Page,
    container: ExGuid,
    before: Option<ExGuid>,
    paragraphs: Vec<PageParagraph>,
) -> Result<(), Error> {
    let (path, owner) = self::container(page, container)
        .ok_or_else(|| invalid("The container is not on the page"))?;
    let list = list_at(page, &path);
    let at = match before {
        Some(before) => list
            .iter()
            .position(|p| p.id == before)
            .ok_or_else(|| invalid("The anchor is not on the page"))?,
        None => match owner {
            Some(index) => subtree(list, index).end,
            None => list.len(),
        },
    };
    list.splice(at..at, paragraphs);
    Ok(())
}

/// The absolute level of the children of `container`.
fn child_level(page: &Page, container: ExGuid) -> u32 {
    paragraph(page, container).map_or(0, |p| p.level)
}

/// Shifts the levels of the subtree at `index` so its root lies at `level`.
fn relevel(list: &mut [PageParagraph], index: usize, level: u32) {
    let range = subtree(list, index);
    let delta = i64::from(level) - i64::from(list[index].level);
    for paragraph in &mut list[range] {
        paragraph.level = (i64::from(paragraph.level) + delta) as u32;
    }
}

/// The title a page's writers store after changing it: the title text's first line when
/// it has one, which only an edit of that text changes, else the first line of body text.
fn retitle(page: &mut Page, edited: Option<ExGuid>) {
    let line = |text: &str| {
        crate::edit::without_fields(text.trim_start().split('\r').next().unwrap_or_default())
    };
    match find_title_text(page) {
        Some(title) => {
            let text = text_of(page, title);
            if line(&text).is_empty() {
                automatic(page);
            } else if edited == Some(title) {
                page.title = line(&text);
            }
        }
        None => automatic(page),
    }
}

fn automatic(page: &mut Page) {
    let mut roots: Vec<&PageObject> = page
        .objects
        .iter()
        .filter(|object| !matches!(object, PageObject::Title(_)))
        .collect();
    roots.sort_by(|a, b| {
        let (a, b) = (a.layout(), b.layout());
        a.y.unwrap_or(0.0)
            .total_cmp(&b.y.unwrap_or(0.0))
            .then_with(|| a.x.unwrap_or(0.0).total_cmp(&b.x.unwrap_or(0.0)))
    });
    fn first(list: &[PageParagraph]) -> Option<String> {
        list.iter().find_map(|paragraph| match &paragraph.content {
            ParagraphContent::Text(text) => {
                Some(crate::edit::automatic_title(text.text.text())).filter(|t| !t.is_empty())
            }
            ParagraphContent::Table(table) => table
                .rows
                .iter()
                .flat_map(|row| &row.cells)
                .find_map(|cell| first(&cell.paragraphs)),
            _ => None,
        })
    }
    let title = roots
        .into_iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => first(&outline.paragraphs),
            _ => None,
        })
        .unwrap_or_default();
    page.title = title;
}

/// Applies `op` to `page` as reading the page back after `Section::apply` would show it.
pub fn apply(page: &mut Page, op: &PageOp) -> Result<(), Error> {
    interpret(page, op)?;
    // A page lists the definitions its paragraphs reference.
    let mut referenced = BTreeSet::new();
    for (_, _, list) in lists(page) {
        for paragraph in list {
            referenced.extend(paragraph.lists.iter().copied());
            referenced.extend(paragraph.style);
            let tags = paragraph.tags.iter().chain(match &paragraph.content {
                ParagraphContent::Text(text) => text.tags.as_slice(),
                ParagraphContent::Table(table) => table.tags.as_slice(),
                _ => &[],
            });
            referenced.extend(tags.filter_map(|tag| tag.definition));
        }
    }
    page.definitions.retain(|id, _| referenced.contains(id));
    Ok(())
}

fn interpret(page: &mut Page, op: &PageOp) -> Result<(), Error> {
    match op {
        PageOp::Date { created, fields } => {
            for (text, shown) in fields {
                if !date_fields(page).iter().any(|field| field.id == *text) {
                    return Err(invalid("Select the title's date or time"));
                }
                let object = text_mut(page, *text)?;
                let format = object.text.spans()[0].format.clone();
                object.text = Paragraph::new(shown.clone(), format);
            }
            page.created = Some(*created);
        }
        PageOp::Color(color) => page.color = *color,
        PageOp::RuleLines(lines) => page.rule_lines = *lines,
        PageOp::Text { text, range, with } => {
            let object = text_mut(page, *text)?;
            object.text = replace_text(&object.text, range.clone(), with)?;
            retitle(page, Some(*text));
        }
        PageOp::Format {
            text,
            range,
            set,
            clear,
        } => {
            let (path, index) =
                text_holder(page, *text).ok_or_else(|| invalid("Text is not on the page"))?;
            let style = list_at(page, &path)[index]
                .style
                .and_then(|style| page.definitions.get(&style))
                .map(|definition| definition.format.clone())
                .unwrap_or_default();
            let object = text_mut(page, *text)?;
            object.text = format_text(&object.text, range.clone(), set, clear, &style)?;
        }
        PageOp::Link {
            text,
            range,
            target,
        } => {
            let current = text_mut(page, *text)?.text.clone();
            for op in super::apply::link_ops(&current, *text, range.clone(), target.as_deref())? {
                interpret(page, &op)?;
            }
        }
        PageOp::Equation { text, math } => {
            let (path, index) =
                text_holder(page, *text).ok_or_else(|| invalid("Text is not on the page"))?;
            let style = list_at(page, &path)[index]
                .style
                .and_then(|style| page.definitions.get(&style))
                .map(|definition| definition.format.clone())
                .unwrap_or_default();
            let paragraph = &mut list_at(page, &path)[index];
            // Runs read back through the paragraph style and format, which fill what they
            // leave unset.
            let inherited = style.inherit(&paragraph.format);
            let object = paragraph.text_mut().unwrap();
            let previous = object.text.spans()[0].format.clone();
            let math = Paragraph::from_runs(math.spans().iter().scan(0, |start, span| {
                let run = (
                    math.text()[*start..span.end].to_owned(),
                    span.format.inherit(&inherited),
                );
                *start = span.end;
                Some(run)
            }));
            object.text = paragraph_formatted(&math, &previous);
        }
        PageOp::Insert {
            container,
            before,
            paragraphs,
        } => {
            insert(page, *container, *before, insertion(paragraphs))?;
            retitle(page, None);
        }
        PageOp::Split {
            text,
            at,
            paragraph,
            right,
            lists,
        } => {
            let (path, index) =
                text_holder(page, *text).ok_or_else(|| invalid("Text is not on the page"))?;
            let definitions: Vec<_> = {
                let list = list_at(page, &path);
                list[index].lists.clone()
            };
            let copies: Vec<_> = definitions
                .iter()
                .zip(lists)
                .map(|(old, new)| {
                    let mut definition = page.definitions.get(old).cloned();
                    if let Some(crate::page::Definition {
                        kind: Kind::List { restart, .. },
                        ..
                    }) = &mut definition
                    {
                        *restart = None;
                    }
                    (*new, definition)
                })
                .collect();
            for (id, definition) in copies {
                if let Some(definition) = definition {
                    page.definitions.insert(id, definition);
                }
            }
            let list = list_at(page, &path);
            let left = &mut list[index];
            let source = left.text().unwrap().text.clone();
            let length = source.utf16_offset(source.text().len())?;
            let (head, tail) = (source.slice(0..*at)?, source.slice(*at..length)?);
            let mut new = left.clone();
            left.text_mut().unwrap().text = head;
            new.id = *paragraph;
            new.lists = lists.clone();
            new.media = Default::default();
            new.content = ParagraphContent::Text(TextObject {
                id: *right,
                date_field: None,
                text: tail,
                tags: Vec::new(),
            });
            let left_id = list[index].id;
            for child in &mut list[index + 1..] {
                if child.parent == Some(left_id) {
                    child.parent = Some(*paragraph);
                }
            }
            list.insert(index + 1, new);
            retitle(page, None);
        }
        PageOp::Join { left, right } => {
            let (path, index) =
                text_holder(page, *right).ok_or_else(|| invalid("Text is not on the page"))?;
            let (left_path, left_index) =
                text_holder(page, *left).ok_or_else(|| invalid("Text is not on the page"))?;
            let list = list_at(page, &path);
            let removed = list[index].clone();
            let right_text = removed.text().unwrap().clone();
            let definitions = page.definitions.clone();
            let left_list = list_at(page, &left_path);
            let left_style = left_list[left_index].style;
            let adopts = left_list[left_index].text().unwrap().text.text().is_empty();
            if adopts {
                // The text object brings its paragraph style and recording link.
                left_list[left_index].style = removed.style;
                left_list[left_index].media = removed.media.clone();
            }
            let upper = left_list[left_index].text_mut().unwrap();
            if adopts {
                upper.id = right_text.id;
                upper.text = right_text.text;
            } else {
                // The lower text keeps its look under the upper paragraph's style: a flag or
                // colour only that style states becomes false or automatic.
                let differs = left_style != removed.style;
                let left = left_style
                    .and_then(|style| definitions.get(&style))
                    .map(|definition| definition.format.clone())
                    .unwrap_or_default();
                let mut previous = 0;
                let lower = Paragraph::from_runs(right_text.text.spans().iter().map(|span| {
                    let mut format = span.format.clone();
                    if differs {
                        for (field, base) in [
                            (&mut format.bold, left.bold),
                            (&mut format.italic, left.italic),
                            (&mut format.underline, left.underline),
                            (&mut format.strike, left.strike),
                            (&mut format.superscript, left.superscript),
                            (&mut format.subscript, left.subscript),
                            (&mut format.hidden, left.hidden),
                            (&mut format.hyperlink, left.hyperlink),
                            (&mut format.hyperlink_label, left.hyperlink_label),
                            (&mut format.math, left.math),
                        ] {
                            if field.is_none() && base.is_some() {
                                *field = Some(false);
                            }
                        }
                        for (field, base) in [
                            (&mut format.color, left.color),
                            (&mut format.highlight, left.highlight),
                        ] {
                            if field.is_none() && base.is_some() {
                                *field = Some(0xff000000);
                            }
                        }
                    }
                    let run = (
                        right_text.text.text()[previous..span.end].to_owned(),
                        format,
                    );
                    previous = span.end;
                    run
                }));
                let paragraph = upper.text.spans()[0].format.clone();
                upper.text.append(lower)?;
                upper.text = paragraph_formatted(&upper.text, &paragraph);
            }
            let left_id = left_list[left_index].id;
            let list = list_at(page, &path);
            // The right paragraph's children join the left paragraph's ancestor beside it,
            // which is the outline group holding it where it lies deeper.
            let mut stem = left_id;
            while let Some(parent) = list.iter().find(|p| p.id == stem).and_then(|p| p.parent) {
                if Some(parent) == removed.parent {
                    break;
                }
                stem = parent;
            }
            let range = subtree(list, index);
            let stem_at = list.iter().position(|p| p.id == stem).unwrap();
            let stem_level = list[stem_at].level;
            if range.len() > 1 && stem_level > removed.level {
                // The group takes the right paragraph's child level for all its members.
                let level = list[range.start + 1].level;
                let mut member = stem_at;
                loop {
                    relevel(list, member, level);
                    let Some(previous) = list[..member]
                        .iter()
                        .rposition(|p| p.parent == removed.parent)
                        .filter(|at| list[*at].level == stem_level)
                    else {
                        break;
                    };
                    member = previous;
                }
                for child in &mut list[range.start + 1..range.end] {
                    if child.parent == Some(removed.id) {
                        child.parent = removed.parent;
                    }
                }
            } else {
                let delta = i64::from(stem_level) - i64::from(removed.level);
                for child in &mut list[range.start + 1..range.end] {
                    child.level = (i64::from(child.level) + delta) as u32;
                    if child.parent == Some(removed.id) {
                        child.parent = Some(stem);
                    }
                }
            }
            list.remove(index);
            retitle(page, None);
        }
        PageOp::Move {
            object,
            parent,
            before,
        } => {
            let Some(parent) = parent else {
                let from = page
                    .objects
                    .iter()
                    .position(|o| o.id() == *object)
                    .ok_or_else(|| invalid("The object is not on the page"))?;
                let moved = page.objects.remove(from);
                let at = page_position(page, *before)?;
                page.objects.insert(at, moved);
                retitle(page, None);
                return Ok(());
            };
            let (path, index) =
                find(page, *object).ok_or_else(|| invalid("The paragraph is not on the page"))?;
            let list = list_at(page, &path);
            let mut moved: Vec<PageParagraph> = list.drain(subtree(list, index)).collect();
            let into_cell = table_cells(page).contains(parent);
            let base = child_level(page, *parent);
            let level = if into_cell {
                let (path, _) = self::container(page, *parent).unwrap();
                list_at(page, &path)
                    .iter()
                    .find(|p| p.parent.is_none())
                    .map_or(1, |p| p.level)
            } else if moved[0].level > base {
                moved[0].level
            } else {
                base + 1
            };
            moved[0].parent = paragraph(page, *parent).map(|_| *parent);
            relevel(&mut moved, 0, level);
            insert(page, *parent, *before, moved)?;
            drop_emptied(page);
            retitle(page, None);
        }
        PageOp::Delete { object } => {
            if let Some(at) = page.objects.iter().position(|o| o.id() == *object) {
                page.objects.remove(at);
            } else {
                let (path, index) =
                    find(page, *object).ok_or_else(|| invalid("The object is not on the page"))?;
                let list = list_at(page, &path);
                list.drain(subtree(list, index));
            }
            drop_emptied(page);
            retitle(page, None);
        }
        PageOp::Level { paragraph, level } => {
            let (path, index) = find(page, *paragraph)
                .ok_or_else(|| invalid("The paragraph is not on the page"))?;
            relevel(list_at(page, &path), index, *level);
        }
        PageOp::Outline { object, edit } => match edit {
            OutlineEdit::Collapsed(value) => paragraph_mut(page, *object)?.collapsed = *value,
            OutlineEdit::Position { x, y } => {
                let layout = match page.objects.iter_mut().find(|o| o.id() == *object) {
                    Some(PageObject::Ink(ink)) => &mut ink.layout,
                    _ => &mut outline_mut(page, *object)?.layout,
                };
                layout.x = Some(stored(*x));
                layout.y = Some(stored(*y));
                retitle(page, None);
            }
            OutlineEdit::Width { points, user_set } => {
                let outline = outline_mut(page, *object)?;
                outline.layout.max_width = Some(stored(*points));
                outline.layout.width_set_by_user = Some(*user_set);
                outline.layout.reserved_width = None;
            }
        },
        PageOp::Paragraph {
            paragraph,
            alignment,
            rtl,
            space_before,
            space_after,
            line_spacing,
            language,
        } => {
            let text = &mut paragraph_mut(page, *paragraph)?
                .text_mut()
                .ok_or_else(|| invalid("Paragraph formatting belongs to text"))?
                .text;
            let mut previous = 0;
            let runs: Vec<(String, Format)> = text
                .spans()
                .iter()
                .map(|span| {
                    let mut format = span.format.clone();
                    format.alignment = alignment.or(format.alignment);
                    format.rtl = rtl.or(format.rtl);
                    format.space_before = space_before.map(stored).or(format.space_before);
                    format.space_after = space_after.map(stored).or(format.space_after);
                    format.line_spacing = line_spacing.map(stored).or(format.line_spacing);
                    // A run's own language outranks the text's.
                    format.language = format.language.or(*language);
                    let run = (text.text()[previous..span.end].to_owned(), format);
                    previous = span.end;
                    run
                })
                .collect();
            *text = Paragraph::from_runs(runs);
        }
        PageOp::Style {
            paragraph,
            style,
            definition,
        } => {
            let old = paragraph_mut(page, *paragraph)?
                .style
                .and_then(|style| page.definitions.get(&style))
                .map(|definition| definition.format.clone())
                .unwrap_or_default();
            let style = define(page, *style, definition);
            let new = page.definitions[&style].format.clone();
            let paragraph = paragraph_mut(page, *paragraph)?;
            paragraph.style = Some(style);
            if let Some(text) = paragraph.text_mut() {
                let mut previous = 0;
                let runs: Vec<(String, Format)> = text
                    .text
                    .spans()
                    .iter()
                    .map(|span| {
                        // What the old style gave, the new one gives instead.
                        let mut format = span.format.clone();
                        macro_rules! restyle {
                            ($($field:ident),*) => {$(
                                if old.$field.is_some() && format.$field == old.$field {
                                    format.$field = new.$field.clone();
                                } else if format.$field.is_none() {
                                    format.$field = new.$field.clone();
                                }
                            )*};
                        }
                        restyle!(
                            bold,
                            italic,
                            underline,
                            strike,
                            superscript,
                            subscript,
                            hidden,
                            hyperlink,
                            hyperlink_label,
                            math,
                            embedded_object,
                            font,
                            font_size,
                            color,
                            highlight,
                            language,
                            alignment,
                            rtl,
                            space_before,
                            space_after,
                            line_spacing,
                            list_spacing,
                            math_object
                        );
                        let run = (text.text.text()[previous..span.end].to_owned(), format);
                        previous = span.end;
                        run
                    })
                    .collect();
                text.text = Paragraph::from_runs(runs);
            }
        }
        PageOp::List { paragraph, lists } => {
            paragraph_mut(page, *paragraph)?.lists = lists.iter().map(|(id, _)| *id).collect();
            for (id, definition) in lists {
                page.definitions.insert(*id, definition.clone());
            }
        }
        PageOp::Tags {
            target,
            tags,
            definitions,
        } => {
            let mut tags = tags.clone();
            for tag in &mut tags {
                if let Some(id) = &mut tag.definition
                    && let Some((_, definition)) = definitions.iter().find(|(d, _)| d == id)
                {
                    *id = define(page, *id, definition);
                }
            }
            if let Ok(paragraph) = paragraph_mut(page, *target) {
                paragraph.tags = tags;
            } else if let Ok(text) = text_mut(page, *target) {
                text.tags = tags;
            } else {
                table_mut(page, *target)?.tags = tags;
            }
        }
        PageOp::Add { object, before } => {
            let object = match object {
                PageObject::Outline(outline) => {
                    let mut outline = outline.clone();
                    outline.paragraphs = insertion(&outline.paragraphs);
                    if outline.indents.is_empty() {
                        outline.indents = NATIVE_INDENTS.to_vec();
                    }
                    let layout = &mut outline.layout;
                    layout.x = layout.x.map(stored);
                    layout.y = layout.y.map(stored);
                    match layout.max_width {
                        Some(points) if (points, layout.width_set_by_user) != (468.0, None) => {
                            layout.max_width = Some(stored(points));
                            layout.width_set_by_user = Some(layout.width_set_by_user == Some(true));
                        }
                        _ => {
                            layout.max_width = Some(468.0);
                            layout.width_set_by_user = None;
                        }
                    }
                    layout.reserved_width = None;
                    layout.max_height = Some(0.6_f32 * 36.0);
                    PageObject::Outline(outline)
                }
                PageObject::Ink(ink) => PageObject::Ink(inked(ink)),
                object => object.clone(),
            };
            let outline = matches!(object, PageObject::Outline(_));
            let at = page_position(page, *before)?;
            page.objects.insert(at, object);
            if outline {
                retitle(page, None);
            }
        }
        PageOp::Picture {
            picture,
            layout,
            alt,
        } => {
            let image = image_mut(page, *picture)?;
            image.layout = crate::document::Layout {
                x: layout.x.map(stored),
                y: layout.y.map(stored),
                max_width: layout.max_width.map(stored),
                max_height: layout.max_height.map(stored),
                width_set_by_user: layout
                    .max_width
                    .map(|_| layout.width_set_by_user == Some(true)),
                reserved_width: image.layout.reserved_width,
            };
            image.alt = alt.clone();
        }
        PageOp::Attachment {
            attachment,
            filename,
            source_path,
            size,
        } => {
            let (path, index) = lists(page)
                .into_iter()
                .find_map(|(path, _, list)| {
                    list.iter()
                        .position(|p| matches!(&p.content, ParagraphContent::Attachment(a) if a.id == *attachment))
                        .map(|index| (path, index))
                })
                .ok_or_else(|| invalid("The attachment is not on the page"))?;
            let ParagraphContent::Attachment(stored_attachment) =
                &mut list_at(page, &path)[index].content
            else {
                unreachable!()
            };
            stored_attachment.filename = filename.clone();
            stored_attachment.source_path = source_path.clone();
            stored_attachment.size = size.map(|[w, h]| [stored(w), stored(h)]);
        }
        PageOp::Strokes { ink, add, remove } => {
            let ink = ink_mut(page, *ink)?;
            ink.strokes.retain(|stroke| !remove.contains(&stroke.id));
            ink.strokes.extend(add.iter().map(stroked));
        }
        PageOp::Table { table, edit } => {
            let table = table_mut(page, *table)?;
            match edit {
                TableEdit::Rows { before, rows } => {
                    let at = match before {
                        Some(before) => table
                            .rows
                            .iter()
                            .position(|r| r.id == *before)
                            .ok_or_else(|| invalid("The anchor row is not in the table"))?,
                        None => table.rows.len(),
                    };
                    let mut rows = rows.clone();
                    for cell in rows.iter_mut().flat_map(|row| &mut row.cells) {
                        if cell.indents.is_empty() {
                            cell.indents = table
                                .rows
                                .first()
                                .and_then(|r| r.cells.first())
                                .map_or(NATIVE_INDENTS.to_vec(), |c| c.indents.clone());
                        }
                        cell.paragraphs = insertion(&cell.paragraphs);
                    }
                    table.rows.splice(at..at, rows);
                }
                TableEdit::Column { at, width, cells } => {
                    let template = table
                        .rows
                        .first()
                        .and_then(|r| r.cells.first())
                        .map(|c| c.indents.clone());
                    for (row, cell) in table.rows.iter_mut().zip(cells) {
                        let mut cell = cell.clone();
                        if cell.indents.is_empty() {
                            cell.indents = template.clone().unwrap_or(NATIVE_INDENTS.to_vec());
                        }
                        cell.paragraphs = insertion(&cell.paragraphs);
                        row.cells.insert(*at as usize, cell);
                    }
                    table.columns.insert(
                        *at as usize,
                        crate::page::TableColumn {
                            width: stored(*width),
                            locked: false,
                        },
                    );
                }
                TableEdit::DeleteRow(row) => table.rows.retain(|r| r.id != *row),
                TableEdit::DeleteColumn(at) => {
                    table.columns.remove(*at as usize);
                    for row in &mut table.rows {
                        row.cells.remove(*at as usize);
                    }
                }
                TableEdit::Columns(columns) => {
                    table.columns = columns
                        .iter()
                        .map(|c| crate::page::TableColumn {
                            width: stored(c.width),
                            locked: c.locked,
                        })
                        .collect();
                }
                TableEdit::Borders(borders) => table.borders = Some(*borders),
                TableEdit::Cell {
                    cell,
                    shading,
                    indents,
                } => {
                    let cell = table
                        .rows
                        .iter_mut()
                        .flat_map(|row| &mut row.cells)
                        .find(|c| c.id == *cell)
                        .ok_or_else(|| invalid("The cell is not in the table"))?;
                    cell.shading = *shading;
                    cell.indents = indents.iter().map(|v| stored(*v)).collect();
                }
            }
        }
    }
    Ok(())
}

/// Removes outlines a tree edit left without paragraphs, as the tree writer does.
fn drop_emptied(page: &mut Page) {
    page.objects.retain(
        |object| !matches!(object, PageObject::Outline(outline) if outline.paragraphs.is_empty()),
    );
}

/// The identity a read-only definition is stored under: an equal one the page holds, as
/// the writers share identical read-only objects, else `id`.
fn define(page: &mut Page, id: ExGuid, definition: &crate::page::Definition) -> ExGuid {
    if page.definitions.contains_key(&id) {
        return id;
    }
    if let Some((existing, _)) = page.definitions.iter().find(|(_, d)| *d == definition) {
        return *existing;
    }
    page.definitions.insert(id, definition.clone());
    id
}

/// Where a page object goes before `before`, or after the other children; titles, which
/// the page lists apart, read after them.
fn page_position(page: &Page, before: Option<ExGuid>) -> Result<usize, Error> {
    match before {
        Some(before) => page
            .objects
            .iter()
            .position(|o| o.id() == before)
            .ok_or_else(|| invalid("The anchor is not on the page")),
        None => Ok(page
            .objects
            .iter()
            .position(|o| matches!(o, PageObject::Title(_)))
            .unwrap_or(page.objects.len())),
    }
}

fn table_cells(page: &Page) -> BTreeSet<ExGuid> {
    lists(page)
        .into_iter()
        .filter(|(path, _, _)| !path.cells.is_empty())
        .map(|(_, owner, _)| owner)
        .collect()
}

fn outline_mut(page: &mut Page, id: ExGuid) -> Result<&mut Outline, Error> {
    page.objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if outline.id == id => Some(outline),
            _ => None,
        })
        .ok_or_else(|| invalid("The outline is not on the page"))
}

fn image_mut(page: &mut Page, id: ExGuid) -> Result<&mut crate::page::Image, Error> {
    if let Some(at) = page.objects.iter().position(|o| o.id() == id) {
        let PageObject::Image(image) = &mut page.objects[at] else {
            return Err(invalid("Select a picture"));
        };
        return Ok(image);
    }
    let (path, index) = lists(page)
        .into_iter()
        .find_map(|(path, _, list)| {
            list.iter()
                .position(|p| matches!(&p.content, ParagraphContent::Image(i) if i.id == id))
                .map(|index| (path, index))
        })
        .ok_or_else(|| invalid("The picture is not on the page"))?;
    let ParagraphContent::Image(image) = &mut list_at(page, &path)[index].content else {
        unreachable!()
    };
    Ok(image)
}

fn ink_mut(page: &mut Page, id: ExGuid) -> Result<&mut crate::page::Ink, Error> {
    if let Some(at) = page.objects.iter().position(|o| o.id() == id) {
        let PageObject::Ink(ink) = &mut page.objects[at] else {
            return Err(invalid("Select ink"));
        };
        return Ok(ink);
    }
    let (path, index) = lists(page)
        .into_iter()
        .find_map(|(path, _, list)| {
            list.iter()
                .position(|p| matches!(&p.content, ParagraphContent::Ink(i) if i.id == id))
                .map(|index| (path, index))
        })
        .ok_or_else(|| invalid("The ink is not on the page"))?;
    let ParagraphContent::Ink(ink) = &mut list_at(page, &path)[index].content else {
        unreachable!()
    };
    Ok(ink)
}

/// The text object of the page title, when it has one.
fn find_title_text(page: &Page) -> Option<ExGuid> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Title(title) => Some(title),
            _ => None,
        })
        .flat_map(|title| title.outlines.iter().filter(|o| Some(o.id) != title.date))
        .flat_map(|outline| &outline.paragraphs)
        .filter_map(|p| p.text().filter(|t| t.date_field.is_none()))
        .map(|t| t.id)
        .next_back()
}

fn text_of(page: &Page, text: ExGuid) -> String {
    lists(page)
        .into_iter()
        .find_map(|(_, _, list)| {
            list.iter().find_map(|p| {
                p.text()
                    .filter(|t| t.id == text)
                    .map(|t| t.text.text().to_owned())
            })
        })
        .unwrap_or_default()
}
