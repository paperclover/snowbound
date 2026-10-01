//! The ops that turn one page model into another, computed from the models alone: each op
//! is interpreted on a prediction of the page (`model::apply`) so later ones see what earlier
//! ones leave, as the typed writers would store it.

use super::{PageOp, TableEdit, TextProperty, Values, model};
use crate::{
    Error, ExGuid, TextAttribute,
    document::{Format, Kind, Tag},
    page::{
        Definition, Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, Table,
        TableCell, TextObject, text::new_id,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// The ops that turn `before`, a page as stored, into `after`: O(page), for importing a
/// page, converting queued whole-page edits and resolving a conflict, never per keystroke.
pub fn lower_page(before: &Page, after: &Page) -> Result<Vec<PageOp>, Error> {
    let old = View::new(before, false)?;
    let new = View::new(after, false)?;
    Lowering::new(before.clone(), None).run(before, after, &old, &new)
}

/// The ops that replace `before`, consecutive paragraphs of outline or table cell
/// `container` with their parents listed first or outside the range, by `after`. `next`
/// is the paragraph following the range; `definitions` holds the lists, tags and styles
/// the paragraphs reference. Only the paragraphs given are read.
pub fn lower(
    container: ExGuid,
    before: &[PageParagraph],
    after: &[PageParagraph],
    next: Option<&PageParagraph>,
    definitions: &BTreeMap<ExGuid, Definition>,
) -> Result<Vec<PageOp>, Error> {
    let page = |paragraphs: &[PageParagraph]| Page {
        title: String::new(),
        identity: None,
        created: None,
        margin_origin: [0.0; 2],
        rtl: false,
        color: None,
        rule_lines: None,
        objects: vec![PageObject::Outline(Outline {
            id: container,
            title: false,
            min_width: None,
            layout: Default::default(),
            indents: Vec::new(),
            paragraphs: paragraphs.to_vec(),
            unsupported: Vec::new(),
        })],
        definitions: definitions.clone(),
    };
    let (before, after) = (page(before), page(after));
    let old = View::new(&before, true)?;
    let new = View::new(&after, true)?;
    let tail = next.map(|next| (next.parent.unwrap_or(container), next.id));
    Lowering::new(before.clone(), tail).run(&before, &after, &old, &new)
}

/// Direct children of every container, in model order, plus lookups by identity.
pub(crate) struct View<'a> {
    pub page: &'a Page,
    pub outlines: BTreeMap<ExGuid, &'a Outline>,
    /// Outlines owned by a title object rather than the page.
    pub title_outlines: BTreeSet<ExGuid>,
    pub paragraphs: BTreeMap<ExGuid, &'a PageParagraph>,
    pub children: BTreeMap<ExGuid, Vec<ExGuid>>,
    pub container: BTreeMap<ExGuid, ExGuid>,
    pub page_children: Vec<ExGuid>,
    /// Containers a partial model names as parents without holding them, in order.
    pub external: Vec<ExGuid>,
    partial: bool,
}

impl<'a> View<'a> {
    /// `partial` accepts parents outside the model, as a range of paragraphs has.
    pub(crate) fn new(page: &'a Page, partial: bool) -> Result<Self, Error> {
        let mut view = Self {
            page,
            outlines: BTreeMap::new(),
            title_outlines: BTreeSet::new(),
            paragraphs: BTreeMap::new(),
            children: BTreeMap::new(),
            container: BTreeMap::new(),
            page_children: Vec::new(),
            external: Vec::new(),
            partial,
        };
        for object in &page.objects {
            view.page_children.push(object.id());
            match object {
                PageObject::Outline(outline) => view.outline(outline)?,
                PageObject::Title(title) => {
                    for outline in &title.outlines {
                        view.title_outlines.insert(outline.id);
                        view.outline(outline)?;
                    }
                }
                PageObject::Image(_)
                | PageObject::Attachment(_)
                | PageObject::Ink(_)
                | PageObject::Unsupported(_) => {}
            }
        }
        Ok(view)
    }

    fn outline(&mut self, outline: &'a Outline) -> Result<(), Error> {
        if self.outlines.insert(outline.id, outline).is_some() {
            return Err(invalid("The page model repeats an outline identity"));
        }
        self.children.entry(outline.id).or_default();
        self.paragraphs_of(outline.id, &outline.paragraphs)
    }

    fn paragraphs_of(&mut self, root: ExGuid, list: &'a [PageParagraph]) -> Result<(), Error> {
        let listed: BTreeSet<ExGuid> = if self.partial {
            list.iter().map(|p| p.id).collect()
        } else {
            BTreeSet::new()
        };
        for paragraph in list {
            let container = paragraph.parent.unwrap_or(root);
            if let Some(parent) = paragraph.parent
                && !self.paragraphs.contains_key(&parent)
            {
                if !self.partial || listed.contains(&parent) {
                    return Err(invalid(
                        "A paragraph's parent must precede it in its container",
                    ));
                }
                if !self.external.contains(&parent) {
                    self.external.push(parent);
                }
            }
            if self.paragraphs.insert(paragraph.id, paragraph).is_some() {
                return Err(invalid("The page model repeats a paragraph identity"));
            }
            self.children
                .entry(container)
                .or_default()
                .push(paragraph.id);
            self.children.entry(paragraph.id).or_default();
            self.container.insert(paragraph.id, container);
            if let ParagraphContent::Table(table) = &paragraph.content {
                for row in &table.rows {
                    for cell in &row.cells {
                        if self.children.contains_key(&cell.id) {
                            return Err(invalid("The page model repeats a cell identity"));
                        }
                        self.children.entry(cell.id).or_default();
                        self.paragraphs_of(cell.id, &cell.paragraphs)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn text(&self, paragraph: ExGuid) -> Option<&'a TextObject> {
        self.paragraphs.get(&paragraph).and_then(|p| p.text())
    }

    /// Each outline's paragraphs in document order, a table's cells following its paragraph.
    pub(crate) fn documents(&self) -> impl Iterator<Item = Vec<&'a PageParagraph>> + '_ {
        fn walk<'a>(list: &'a [PageParagraph], out: &mut Vec<&'a PageParagraph>) {
            for paragraph in list {
                out.push(paragraph);
                if let ParagraphContent::Table(table) = &paragraph.content {
                    for cell in table.rows.iter().flat_map(|row| &row.cells) {
                        walk(&cell.paragraphs, out);
                    }
                }
            }
        }
        self.outlines.values().map(|outline| {
            let mut out = Vec::new();
            walk(&outline.paragraphs, &mut out);
            out
        })
    }

    /// Every container in document order: outlines, external parents, paragraphs and cells.
    fn containers(&self) -> Vec<ExGuid> {
        let mut containers = Vec::new();
        for object in &self.page.objects {
            let outlines: Vec<&Outline> = match object {
                PageObject::Outline(outline) => vec![outline],
                PageObject::Title(title) => title.outlines.iter().collect(),
                PageObject::Image(_)
                | PageObject::Attachment(_)
                | PageObject::Ink(_)
                | PageObject::Unsupported(_) => Vec::new(),
            };
            for outline in outlines {
                containers.push(outline.id);
                containers.extend(&self.external);
                collect_containers(&outline.paragraphs, &mut containers);
            }
        }
        containers
    }
}

pub(crate) fn collect_containers(list: &[PageParagraph], out: &mut Vec<ExGuid>) {
    for paragraph in list {
        out.push(paragraph.id);
        if let ParagraphContent::Table(table) = &paragraph.content {
            for row in &table.rows {
                for cell in &row.cells {
                    out.push(cell.id);
                    collect_containers(&cell.paragraphs, out);
                }
            }
        }
    }
}

/// What the writers keep as stored, checked before any op: titles, unsupported objects,
/// definitions other than lists and tags, outline roles, paragraph styles and formats,
/// fields and content types.
pub(crate) fn validate(
    before: &Page,
    after: &Page,
    old: &View<'_>,
    new: &View<'_>,
) -> Result<(), Error> {
    if after.margin_origin != before.margin_origin {
        return Err(invalid("Page margins cannot be edited"));
    }
    for (id, definition) in &after.definitions {
        if before
            .definitions
            .get(id)
            .is_some_and(|stored| stored != definition)
            && !matches!(
                definition.kind,
                Kind::List { .. } | Kind::TagDefinition { .. }
            )
        {
            return Err(invalid("Style definitions cannot be edited"));
        }
    }
    for paragraph in new.paragraphs.values() {
        if let ParagraphContent::Table(table) = &paragraph.content {
            super::table::validate_table(table)?;
        }
    }
    let fixed = |page: &Page| -> Vec<String> {
        let mut fixed: Vec<String> = page
            .objects
            .iter()
            .filter_map(|object| match object {
                PageObject::Unsupported(unsupported) => Some(format!("{unsupported:?}")),
                PageObject::Title(title) => Some(format!(
                    "{:?} {:?} {:?} {:?}",
                    title.id,
                    title.date,
                    title.layout,
                    title.outlines.iter().map(|o| o.id).collect::<Vec<_>>()
                )),
                PageObject::Outline(_)
                | PageObject::Image(_)
                | PageObject::Attachment(_)
                | PageObject::Ink(_) => None,
            })
            .collect();
        fixed.sort();
        fixed
    };
    if fixed(before) != fixed(after) {
        return Err(invalid(
            "Titles and unsupported objects cannot be edited through the page model",
        ));
    }
    for (id, outline) in &new.outlines {
        if outline.paragraphs.is_empty() {
            return Err(invalid(
                "An outline needs a paragraph; remove the outline instead",
            ));
        }
        let Some(previous) = old.outlines.get(id) else {
            continue;
        };
        let same = outline.title == previous.title
            && outline.min_width == previous.min_width
            && outline.indents == previous.indents
            && outline.unsupported == previous.unsupported
            && (!new.title_outlines.contains(id) || outline.layout == previous.layout);
        if !same {
            return Err(invalid(
                "Outline roles, indentation tables and title geometry cannot be edited",
            ));
        }
    }
    let owners: BTreeMap<ExGuid, &PageParagraph> = old
        .paragraphs
        .values()
        .filter_map(|owner| Some((owner.text()?.id, *owner)))
        .collect();
    for (id, paragraph) in &new.paragraphs {
        let Some(previous) = old.paragraphs.get(id) else {
            continue;
        };
        // A join moves a text object, with its style, into an emptied paragraph.
        let owner = paragraph.text().map(|text| owners.get(&text.id));
        let style = match owner {
            Some(Some(owner)) => owner.style,
            Some(None) => paragraph.style,
            None => previous.style,
        };
        // `Style` and `Unstyle` give a paragraph of text a style and take it away.
        let restyled = paragraph.style != style && paragraph.text().is_none();
        if restyled || paragraph.format != previous.format {
            return Err(invalid(
                "Paragraph styles and paragraph formatting cannot be edited",
            ));
        }
        match (&paragraph.content, &previous.content) {
            (ParagraphContent::Text(text), ParagraphContent::Text(previous)) => {
                if text.date_field != previous.date_field {
                    return Err(invalid("Text fields cannot be edited"));
                }
            }
            (ParagraphContent::Table(table), ParagraphContent::Table(previous)) => {
                if table.id != previous.id || table.layout != previous.layout {
                    return Err(invalid("Table identity and layout cannot be edited"));
                }
                let cells: BTreeMap<ExGuid, &TableCell> = previous
                    .rows
                    .iter()
                    .flat_map(|row| &row.cells)
                    .map(|cell| (cell.id, cell))
                    .collect();
                let unchanged_cells = table.rows.iter().flat_map(|row| &row.cells).all(|cell| {
                    cells.get(&cell.id).is_none_or(|before| {
                        cell.layout == before.layout && cell.unsupported == before.unsupported
                    })
                });
                if !unchanged_cells {
                    return Err(invalid("Cell layout cannot be edited"));
                }
            }
            (ParagraphContent::Unsupported(a), ParagraphContent::Unsupported(b)) if a == b => {}
            (ParagraphContent::Ink(a), ParagraphContent::Ink(b)) => {
                if a.id != b.id {
                    return Err(invalid("Ink identity cannot change"));
                }
            }
            (ParagraphContent::Image(a), ParagraphContent::Image(b)) => {
                super::content::picture_fixed_fields(a, b)?;
                if (a.layout.x, a.layout.y) != (b.layout.x, b.layout.y) {
                    return Err(invalid("A paragraph picture has no position of its own"));
                }
            }
            (ParagraphContent::Attachment(a), ParagraphContent::Attachment(b)) => {
                if a.id != b.id {
                    return Err(invalid("Attachment identity cannot change"));
                }
            }
            _ => return Err(invalid("Paragraph content type cannot change")),
        }
    }
    Ok(())
}

/// Identities that keep their stored position: a longest increasing run of survivors,
/// always including immovable ones. `after` may contain identities absent from `before`.
pub(crate) fn kept_set(
    before: &[ExGuid],
    after: &[ExGuid],
    movable: impl Fn(ExGuid) -> bool,
) -> Result<BTreeSet<ExGuid>, Error> {
    let position: BTreeMap<ExGuid, usize> =
        after.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let sequence: Vec<(ExGuid, usize)> = before
        .iter()
        .filter_map(|id| position.get(id).map(|at| (*id, *at)))
        .collect();
    let fixed: Vec<usize> = sequence
        .iter()
        .filter(|(id, _)| !movable(*id))
        .map(|(_, at)| *at)
        .collect();
    if fixed.windows(2).any(|w| w[0] > w[1]) {
        return Err(invalid(
            "Images and unsupported objects cannot be reordered",
        ));
    }
    if sequence.windows(2).all(|pair| pair[0].1 < pair[1].1) {
        return Ok(sequence.into_iter().map(|(id, _)| id).collect());
    }
    // Longest increasing subsequence over `after` positions, forced through immovable items.
    let n = sequence.len();
    let mut best = vec![1usize; n];
    let mut previous = vec![usize::MAX; n];
    for i in 0..n {
        let (id, at) = sequence[i];
        let mandatory_before = sequence[..i]
            .iter()
            .filter(|(other, _)| !movable(*other))
            .map(|(_, at)| *at)
            .max();
        if movable(id) && mandatory_before.is_some_and(|m| m > at) {
            best[i] = 0;
            continue;
        }
        let mandatory_after = sequence[i + 1..]
            .iter()
            .filter(|(other, _)| !movable(*other))
            .map(|(_, at)| *at)
            .min();
        if movable(id) && mandatory_after.is_some_and(|m| m < at) {
            best[i] = 0;
            continue;
        }
        for j in 0..i {
            if best[j] > 0 && sequence[j].1 < at && best[j] + 1 > best[i] {
                best[i] = best[j] + 1;
                previous[i] = j;
            }
        }
    }
    let mut kept = BTreeSet::new();
    if let Some((mut i, _)) = best
        .iter()
        .enumerate()
        .max_by_key(|(i, b)| (**b, usize::MAX - i))
        && best[i] > 0
    {
        loop {
            kept.insert(sequence[i].0);
            if previous[i] == usize::MAX {
                break;
            }
            i = previous[i];
        }
    }
    for (id, _) in &sequence {
        if !movable(*id) && !kept.contains(id) {
            return Err(invalid(
                "Images and unsupported objects cannot be reordered",
            ));
        }
    }
    Ok(kept)
}

/// The format of the span containing the UTF-16 position `at` (the following span at a
/// boundary, the last at the end), which text inserted there takes.
pub(crate) fn format_in(paragraph: &Paragraph, at: u32) -> Result<&Format, Error> {
    let byte = paragraph.byte_offset(at)?;
    let spans = paragraph.spans();
    let index = spans
        .partition_point(|span| span.end <= byte)
        .min(spans.len() - 1);
    Ok(&spans[index].format)
}

/// The smallest UTF-16 range whose replacement turns `before` into `after`.
pub(crate) fn text_edit(before: &str, after: &str) -> Result<(Range<u32>, String), Error> {
    let prefix = before
        .char_indices()
        .zip(after.chars())
        .take_while(|((_, a), b)| a == b)
        .map(|((i, a), _)| i + a.len_utf8())
        .last()
        .unwrap_or(0);
    let suffix = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    let units = |s: &str| -> Result<u32, Error> {
        u32::try_from(s.encode_utf16().count())
            .map_err(|_| invalid("Text exceeds UTF-16 offset range"))
    };
    let start = units(&before[..prefix])?;
    let end = start + units(&before[prefix..before.len() - suffix])?;
    Ok((start..end, after[prefix..after.len() - suffix].to_owned()))
}

/// A character-format change over a range: attributes to set and properties to clear.
pub(crate) type FormatEdit = (Range<u32>, Vec<TextAttribute>, Vec<TextProperty>);

/// The format edits turning `stored`'s spans into `target`'s, which has the same text. A
/// `fresh` text object was created by this edit, so an unset language keeps the writer's.
pub(crate) fn format_edits(
    stored: &Paragraph,
    target: &Paragraph,
    fresh: bool,
) -> Result<Vec<FormatEdit>, Error> {
    let mut boundaries = BTreeSet::new();
    for paragraph in [stored, target] {
        for span in paragraph.spans() {
            boundaries.insert(paragraph.utf16_offset(span.end)?);
        }
    }
    boundaries.insert(0);
    let boundaries: Vec<u32> = boundaries.into_iter().collect();
    let mut pending: Option<FormatEdit> = None;
    let mut edits = Vec::new();
    for window in boundaries.windows(2) {
        let (start, end) = (window[0], window[1]);
        if start == end {
            continue;
        }
        let (set, clear) = attributes(format_in(stored, start)?, format_in(target, start)?, fresh)?;
        match &mut pending {
            Some((range, previous_set, previous_clear))
                if *previous_set == set && *previous_clear == clear && range.end == start =>
            {
                range.end = end;
            }
            _ => {
                if let Some(edit) = pending.take() {
                    edits.push(edit);
                }
                pending = Some((start..end, set, clear));
            }
        }
    }
    edits.extend(pending);
    if target.text().is_empty() {
        let (set, clear) = attributes(format_in(stored, 0)?, format_in(target, 0)?, fresh)?;
        edits.push((0..0, set, clear));
    }
    edits.retain(|(_, set, clear)| !set.is_empty() || !clear.is_empty());
    Ok(edits)
}

/// The change turning `current` into `target`; unsupported differences are errors. A value the
/// target leaves unset is cleared from the run, as the model's formats are complete.
fn attributes(
    current: &Format,
    target: &Format,
    fresh: bool,
) -> Result<(Vec<TextAttribute>, Vec<TextProperty>), Error> {
    let mut out = Vec::new();
    let mut cleared = Vec::new();
    macro_rules! boolean {
        ($field:ident, $variant:ident) => {
            match target.$field {
                Some(value) if current.$field != Some(value) => {
                    out.push(TextAttribute::$variant(value))
                }
                None if current.$field.is_some() => cleared.push(TextProperty::$variant),
                _ => {}
            }
        };
    }
    boolean!(bold, Bold);
    boolean!(italic, Italic);
    boolean!(underline, Underline);
    boolean!(strike, Strike);
    boolean!(superscript, Superscript);
    boolean!(subscript, Subscript);
    boolean!(hidden, Hidden);
    boolean!(hyperlink, Hyperlink);
    boolean!(hyperlink_label, HyperlinkLabel);
    boolean!(math, Math);
    macro_rules! value {
        ($field:ident, $property:ident, $attribute:expr) => {
            if current.$field != target.$field {
                match &target.$field {
                    Some(value) => out.push($attribute(value)),
                    None => cleared.push(TextProperty::$property),
                }
            }
        };
    }
    let color =
        |value: u32| (value != 0xff000000).then(|| value.to_le_bytes()[..3].try_into().unwrap());
    value!(font, Font, |font: &String| TextAttribute::Font(
        font.clone()
    ));
    value!(font_size, FontSize, |size: &f32| TextAttribute::FontSize(
        *size
    ));
    value!(color, Color, |value: &u32| TextAttribute::Color(color(
        *value
    )));
    value!(highlight, Highlight, |value: &u32| {
        TextAttribute::Highlight(color(*value))
    });
    if current.language != target.language {
        match target.language {
            Some(language) => out.push(TextAttribute::Language(language)),
            None if fresh => {}
            None => {
                return Err(invalid("Inherited character formatting cannot be restored"));
            }
        }
    }
    // An absent value and its stored default are the same formatting.
    let flag = |a: Option<bool>, b: Option<bool>| a.unwrap_or(false) == b.unwrap_or(false);
    let points = |a: Option<f32>, b: Option<f32>| a.unwrap_or(0.0) == b.unwrap_or(0.0);
    let same_rest = flag(current.embedded_object, target.embedded_object)
        && current.alignment.unwrap_or(0) == target.alignment.unwrap_or(0)
        && flag(current.rtl, target.rtl)
        && points(current.space_before, target.space_before)
        && points(current.space_after, target.space_after)
        && points(current.line_spacing, target.line_spacing)
        && points(current.list_spacing, target.list_spacing);
    if !same_rest {
        return Err(invalid(
            "Fields and paragraph spacing cannot be edited through the page model",
        ));
    }
    Ok((out, cleared))
}

/// Paragraph formatting a text object stores: alignment, direction, spacing and language.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ParagraphFields {
    pub alignment: Option<u8>,
    pub rtl: Option<bool>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub line_spacing: Option<f32>,
    pub language: Option<u32>,
}

impl ParagraphFields {
    pub(crate) fn values(&self) -> Result<Values, Error> {
        super::properties::paragraph_values(
            self.alignment,
            self.rtl,
            self.space_before,
            self.space_after,
            self.line_spacing,
            self.language,
        )
    }
}

/// The paragraph formatting `target` sets throughout that `stored` lacks somewhere.
pub(crate) fn paragraph_fields(
    stored: &Paragraph,
    target: &Paragraph,
) -> Result<ParagraphFields, Error> {
    let first = format_in(target, 0)?;
    macro_rules! field {
        ($field:ident) => {{
            let value = first.$field.unwrap_or_default();
            (target
                .spans()
                .iter()
                .all(|span| span.format.$field.unwrap_or_default() == value)
                && stored
                    .spans()
                    .iter()
                    .any(|span| span.format.$field.unwrap_or_default() != value))
            .then_some(value)
        }};
    }
    Ok(ParagraphFields {
        alignment: field!(alignment),
        rtl: field!(rtl),
        space_before: field!(space_before),
        space_after: field!(space_after),
        line_spacing: field!(line_spacing),
        // An unset language is the writer's default, not LCID 0, which OneNote never stores
        // on a text object and which runs without their own language would then read.
        language: first.language.filter(|value| {
            target
                .spans()
                .iter()
                .all(|span| span.format.language == Some(*value))
                && stored
                    .spans()
                    .iter()
                    .any(|span| span.format.language != Some(*value))
        }),
    })
}

/// Whether two tag lists store the same tags.
pub(crate) fn same_tags(a: &[Tag], b: &[Tag]) -> bool {
    let key = |t: &Tag| {
        (
            t.definition,
            t.action_type,
            t.shape,
            t.property_status,
            t.status,
            t.created,
            t.completed,
            t.start,
            t.due,
            t.task_id,
        )
    };
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| key(x) == key(y))
}

/// Whether the writers can split or join this text: ordinary text without generated fields,
/// equations or embedded objects.
fn ordinary(text: &TextObject) -> bool {
    text.date_field.is_none()
        && !text.text.text().contains('\u{fffc}')
        && text
            .text
            .spans()
            .iter()
            .all(|span| span.format.math != Some(true) && span.format.embedded_object != Some(true))
}

/// Whether a split at `offset` would divide a hyperlink, which the writer refuses.
fn divides_link(text: &Paragraph, offset: u32) -> Result<bool, Error> {
    let byte = text.byte_offset(offset)?;
    let link = |span: &crate::page::text::Span| span.format.hyperlink == Some(true);
    let spans = text.spans();
    let mut start = 0;
    for (i, span) in spans.iter().enumerate() {
        if start < byte && byte < span.end {
            return Ok(link(span));
        }
        if byte == span.end && byte > 0 && i + 1 < spans.len() {
            let next = &spans[i + 1];
            return Ok(link(span) && link(next) && !text.text()[byte..].starts_with('\u{fddf}'));
        }
        start = span.end;
    }
    Ok(false)
}

/// A paragraph as `Insert` and `Add` create it: structure and content, without the
/// properties their own ops set.
fn bare(paragraph: &PageParagraph) -> PageParagraph {
    let mut stripped = paragraph.clone();
    stripped.style = None;
    stripped.lists.clear();
    stripped.tags.clear();
    stripped.collapsed = false;
    stripped.media = Default::default();
    match &mut stripped.content {
        ParagraphContent::Text(text) => text.tags.clear(),
        ParagraphContent::Image(image) => image.tags.clear(),
        ParagraphContent::Attachment(file) => file.tags.clear(),
        ParagraphContent::Table(table) => {
            table.tags.clear();
            for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                cell.paragraphs = cell.paragraphs.iter().map(bare).collect();
            }
        }
        _ => {}
    }
    stripped
}

fn bare_cells(cells: &[TableCell]) -> Vec<TableCell> {
    cells
        .iter()
        .map(|cell| TableCell {
            paragraphs: cell.paragraphs.iter().map(bare).collect(),
            ..cell.clone()
        })
        .collect()
}

struct Lowering {
    ops: Vec<PageOp>,
    /// The page as the ops so far leave it.
    current: Page,
    /// The paragraph after a partial model's range: its container and identity.
    tail: Option<(ExGuid, ExGuid)>,
    /// Paragraphs and text objects the ops so far created.
    created: BTreeSet<ExGuid>,
}

impl Lowering {
    fn new(current: Page, tail: Option<(ExGuid, ExGuid)>) -> Self {
        Self {
            ops: Vec::new(),
            current,
            tail,
            created: BTreeSet::new(),
        }
    }

    fn emit(&mut self, op: PageOp) -> Result<(), Error> {
        model::apply(&mut self.current, &op)?;
        self.ops.push(op);
        Ok(())
    }

    /// Whether the page, as the ops so far leave it, holds `id` as a paragraph, text,
    /// list node or table part.
    fn holds(&self, id: ExGuid) -> bool {
        model::holds(&self.current, id)
    }

    fn run(
        mut self,
        before: &Page,
        after: &Page,
        old: &View<'_>,
        new: &View<'_>,
    ) -> Result<Vec<PageOp>, Error> {
        validate(before, after, old, new)?;
        let mut placed: BTreeMap<ExGuid, Vec<ExGuid>> = old.children.clone();
        let mut consumed = BTreeSet::new();
        self.split_and_join(old, new, &mut placed, &mut consumed)?;
        self.place_page(old, new)?;
        self.place(old, new, &placed)?;
        self.delete(old, new, &consumed)?;
        self.levels(new)?;
        self.equations(new)?;
        self.date(before, after)?;
        self.text(new)?;
        self.styles(after, new)?;
        self.lists(after, new)?;
        self.tags(after, new)?;
        self.media(new)?;
        self.paragraph_formatting(new)?;
        self.formatting(old, new)?;
        self.layout(old, new)?;
        Ok(self.ops)
    }

    /// Splits and joins stored paragraphs as OneNote's Enter, Backspace and Delete do.
    fn split_and_join(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        placed: &mut BTreeMap<ExGuid, Vec<ExGuid>>,
        consumed: &mut BTreeSet<ExGuid>,
    ) -> Result<(), Error> {
        // A new paragraph may be split from the stored paragraph nearest before it, whatever
        // placement or pasted paragraphs the same edit adds between them.
        let mut splits = Vec::new();
        for document in new.documents() {
            let mut left = None;
            for paragraph in document {
                if old.paragraphs.contains_key(&paragraph.id) {
                    left = paragraph.text().is_some().then_some(paragraph.id);
                } else if let Some(left) = left
                    // A new cell arrives with its paragraphs.
                    && old.children.contains_key(&root(new, paragraph.id))
                {
                    let container = new.container[&paragraph.id];
                    let at = new.children[&container]
                        .iter()
                        .position(|id| *id == paragraph.id);
                    splits.push(((container, at), left, paragraph.id));
                }
            }
        }
        splits.sort();
        let mut split = BTreeSet::new();
        for (_, left, right) in splits {
            let (Some(previous), Some(after), Some(next)) =
                (old.text(left), new.text(left), new.text(right))
            else {
                continue;
            };
            if split.contains(&left)
                || after.id != previous.id
                || new.paragraphs[&right].style != old.paragraphs[&left].style
                || !ordinary(previous)
                || old.title_outlines.contains(&root(old, left))
            {
                continue;
            }
            let Ok(offset) = after.text.utf16_offset(after.text.text().len()) else {
                continue;
            };
            let length = previous.text.utf16_offset(previous.text.text().len())?;
            // Splitting at the end only differs from appending a paragraph by the copied
            // paragraph style; without one, an appended empty paragraph is an insertion.
            if offset == length && new.paragraphs[&right].style.is_none() {
                continue;
            }
            let (Ok(head), Ok(tail)) = (
                previous.text.slice(0..offset),
                previous.text.slice(offset..length),
            ) else {
                continue;
            };
            // An emptied side takes the writer's insertion style, so only its text must agree.
            let same = |expected: &Paragraph, actual: &Paragraph| {
                expected.text() == actual.text()
                    && (expected.text().is_empty() || expected == actual)
            };
            if !same(&head, &after.text)
                || !same(&tail, &next.text)
                || divides_link(&previous.text, offset)?
                || old.paragraphs[&left].lists.len() > 251
            {
                continue;
            }
            // A text object a join moved into the paragraph keeps its identity there.
            let right_text = if self.holds(next.id) {
                new_id().map_err(|_| invalid("System random source failed"))?
            } else {
                next.id
            };
            // The split copies the left paragraph's list nodes in order; a node the model
            // shares with a stored paragraph stays that paragraph's.
            let stored_lists = old.paragraphs[&left].lists.len();
            let model_lists = &new.paragraphs[&right].lists;
            let mut lists = Vec::new();
            for i in 0..stored_lists {
                lists.push(match model_lists.get(i) {
                    Some(list) if model_lists.len() == stored_lists && !self.holds(*list) => *list,
                    _ => new_id().map_err(|_| invalid("System random source failed"))?,
                });
            }
            self.emit(PageOp::Split {
                text: previous.id,
                at: offset,
                paragraph: right,
                right: right_text,
                lists,
            })?;
            self.created.extend([right, right_text]);
            split.insert(left);
            // An earlier split may have carried the left paragraph to its tail.
            let list = placed
                .values_mut()
                .find(|list| list.contains(&left))
                .unwrap();
            let at = list.iter().position(|id| *id == left).unwrap();
            list.insert(at + 1, right);
            placed.insert(right, placed[&left].clone());
            placed.insert(left, Vec::new());
        }
        for (container, children) in &old.children {
            if !new.children.contains_key(container) {
                continue;
            }
            for pair in children.windows(2) {
                let [left, right] = [pair[0], pair[1]];
                let (Some(previous), Some(after), Some(removed)) =
                    (old.text(left), new.text(left), old.text(right))
                else {
                    continue;
                };
                if new.paragraphs.contains_key(&right)
                    || !old.children[&left].is_empty()
                    || !old.children[&right].is_empty()
                    || removed.text.text().is_empty()
                    || !ordinary(previous)
                    || !ordinary(removed)
                    || old.title_outlines.contains(&root(old, left))
                {
                    continue;
                }
                let mut joined = previous.text.clone();
                if joined.append(removed.text.clone()).is_err() || joined != after.text {
                    continue;
                }
                let adopts_right = previous.text.text().is_empty();
                if after.id
                    != if adopts_right {
                        removed.id
                    } else {
                        previous.id
                    }
                {
                    continue;
                }
                self.emit(PageOp::Join {
                    left: previous.id,
                    right: removed.id,
                })?;
                consumed.insert(right);
                placed.get_mut(container).unwrap().retain(|id| *id != right);
                placed.remove(&right);
            }
        }
        Ok(())
    }

    /// Adds, changes and orders what the page holds directly: outlines, pictures and ink.
    fn place_page(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let pictures = |page: &Page| -> BTreeMap<ExGuid, crate::page::Image> {
            page.objects
                .iter()
                .filter_map(|object| match object {
                    PageObject::Image(image) => Some((image.id, image.clone())),
                    _ => None,
                })
                .collect()
        };
        let stored = pictures(old.page);
        for (id, image) in pictures(new.page) {
            let Some(previous) = stored.get(&id) else {
                continue;
            };
            super::content::picture_fixed_fields(previous, &image)?;
            if (&previous.layout, &previous.alt) != (&image.layout, &image.alt) {
                self.emit(PageOp::Picture {
                    picture: id,
                    layout: image.layout.clone(),
                    alt: image.alt.clone(),
                })?;
            }
            self.set_tags(new.page, id, &image.tags, &previous.tags)?;
        }
        let files = |page: &Page| -> BTreeMap<ExGuid, crate::page::Attachment> {
            page.objects
                .iter()
                .filter_map(|object| match object {
                    PageObject::Attachment(file) => Some((file.id, file.clone())),
                    _ => None,
                })
                .collect()
        };
        let stored = files(old.page);
        for (id, file) in files(new.page) {
            let Some(previous) = stored.get(&id) else {
                continue;
            };
            let (from, to) = (&previous.layout, &file.layout);
            if (
                from.max_width,
                from.max_height,
                from.width_set_by_user,
                from.reserved_width,
            ) != (
                to.max_width,
                to.max_height,
                to.width_set_by_user,
                to.reserved_width,
            ) {
                return Err(invalid("A file on the page keeps its extent"));
            }
            if (from.x, from.y) != (to.x, to.y) {
                let (Some(x), Some(y)) = (to.x, to.y) else {
                    return Err(invalid("A file position needs both coordinates"));
                };
                self.emit(PageOp::Outline {
                    object: id,
                    edit: crate::OutlineEdit::Position { x, y },
                })?;
            }
            if (&previous.filename, &previous.source_path, previous.size)
                != (&file.filename, &file.source_path, file.size)
            {
                self.emit(PageOp::Attachment {
                    attachment: id,
                    filename: file.filename.clone(),
                    source_path: file.source_path.clone(),
                    size: file.size,
                })?;
            }
            self.set_tags(new.page, id, &file.tags, &previous.tags)?;
        }
        let inks = |page: &Page| -> BTreeMap<ExGuid, crate::page::Ink> {
            page.objects
                .iter()
                .filter_map(|object| match object {
                    PageObject::Ink(ink) => Some((ink.id, ink.clone())),
                    _ => None,
                })
                .collect()
        };
        let stored = inks(old.page);
        for (id, ink) in inks(new.page) {
            if let Some(previous) = stored.get(&id)
                && *previous != ink
            {
                self.strokes(previous, &ink)?;
            }
        }
        let title = |page: &Page, id: ExGuid| {
            page.objects
                .iter()
                .any(|object| matches!(object, PageObject::Title(title) if title.id == id))
        };
        let survivors: Vec<ExGuid> = old
            .page_children
            .iter()
            .copied()
            .filter(|id| {
                !title(old.page, *id) && new.page.objects.iter().any(|object| object.id() == *id)
            })
            .collect();
        let after: Vec<&PageObject> = new
            .page
            .objects
            .iter()
            .filter(|object| !title(new.page, object.id()))
            .collect();
        let order: Vec<ExGuid> = after.iter().map(|object| object.id()).collect();
        let kept = kept_set(&survivors, &order, |id| {
            new.outlines.contains_key(&id) && !new.title_outlines.contains(&id)
        })?;
        let mut next = None;
        for object in after.iter().rev() {
            let id = object.id();
            let stored = old.page_children.contains(&id);
            if !stored {
                if new.title_outlines.contains(&id) {
                    return Err(invalid("Title outlines cannot be added"));
                }
                let added = match object {
                    PageObject::Outline(outline) => {
                        if outline
                            .paragraphs
                            .iter()
                            .any(|p| old.paragraphs.contains_key(&p.id))
                        {
                            return Err(invalid(
                                "A new outline holds new paragraphs; move stored ones after",
                            ));
                        }
                        self.created.extend(model::identities(&outline.paragraphs));
                        PageObject::Outline(Outline {
                            paragraphs: outline.paragraphs.iter().map(bare).collect(),
                            ..(*outline).clone()
                        })
                    }
                    PageObject::Image(image) => PageObject::Image(crate::page::Image {
                        tags: Vec::new(),
                        ..image.clone()
                    }),
                    PageObject::Attachment(file) => {
                        PageObject::Attachment(crate::page::Attachment {
                            tags: Vec::new(),
                            ..file.clone()
                        })
                    }
                    PageObject::Ink(_) => (*object).clone(),
                    PageObject::Title(_) | PageObject::Unsupported(_) => {
                        return Err(invalid(
                            "Titles and unsupported objects cannot be edited through the page model",
                        ));
                    }
                };
                self.emit(PageOp::Add {
                    object: added,
                    before: next,
                })?;
                let tags = match object {
                    PageObject::Image(image) => image.tags.as_slice(),
                    PageObject::Attachment(file) => file.tags.as_slice(),
                    _ => &[],
                };
                self.set_tags(new.page, id, tags, &[])?;
            } else if !kept.contains(&id) {
                self.emit(PageOp::Move {
                    object: id,
                    parent: None,
                    before: next,
                })?;
            }
            next = Some(id);
        }
        Ok(())
    }

    /// Inserts new paragraphs and moves stored ones to the containers and order the model
    /// gives them, then edits the stored tables and paragraph content.
    fn place(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        placed: &BTreeMap<ExGuid, Vec<ExGuid>>,
    ) -> Result<(), Error> {
        let mut inserted: BTreeSet<ExGuid> = BTreeSet::new();
        // New outlines and cells, whose paragraphs arrive with them.
        let arriving: BTreeSet<ExGuid> = new
            .children
            .keys()
            .filter(|id| {
                !old.children.contains_key(id)
                    && !new.paragraphs.contains_key(id)
                    && !new.external.contains(id)
            })
            .copied()
            .collect();
        for container in new.containers() {
            if arriving.contains(&root(new, container)) {
                continue;
            }
            let Some(children) = new.children.get(&container) else {
                continue;
            };
            let current: Vec<ExGuid> = placed
                .get(&container)
                .map(|list| {
                    list.iter()
                        .copied()
                        .filter(|id| new.paragraphs.contains_key(id))
                        .collect()
                })
                .unwrap_or_default();
            let kept = kept_set(&current, children, |_| true)?;
            let tail = self
                .tail
                .filter(|(parent, _)| *parent == container)
                .map(|(_, id)| id);
            let mut next = tail;
            let mut run: Vec<ExGuid> = Vec::new();
            for id in children.iter().rev() {
                let fresh = !old.paragraphs.contains_key(id) && !self.created.contains(id);
                if fresh && !inserted.contains(id) {
                    run.push(*id);
                    continue;
                }
                self.insert(container, next, &mut run, new, &mut inserted)?;
                if !kept.contains(id) && !inserted.contains(id) {
                    self.emit(PageOp::Move {
                        object: *id,
                        parent: Some(container),
                        before: next,
                    })?;
                }
                next = Some(*id);
            }
            self.insert(container, next, &mut run, new, &mut inserted)?;
        }
        self.tables(old, new)?;
        self.content(old, new)?;
        Ok(())
    }

    /// Inserts `run`, new siblings in reverse order, with their new descendants.
    fn insert(
        &mut self,
        container: ExGuid,
        before: Option<ExGuid>,
        run: &mut Vec<ExGuid>,
        new: &View<'_>,
        inserted: &mut BTreeSet<ExGuid>,
    ) -> Result<(), Error> {
        if run.is_empty() {
            return Ok(());
        }
        run.reverse();
        let mut paragraphs = Vec::new();
        let mut pending: Vec<ExGuid> = run.drain(..).rev().collect();
        while let Some(id) = pending.pop() {
            let paragraph = new.paragraphs[&id];
            paragraphs.push(bare(paragraph));
            inserted.insert(id);
            self.created.insert(id);
            if let Some(text) = paragraph.text() {
                self.created.insert(text.id);
            }
            if let ParagraphContent::Table(table) = &paragraph.content {
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
                    self.created.extend(model::identities(&cell.paragraphs));
                }
            }
            // New children follow their parent; stored ones move there later.
            pending.extend(
                new.children[&id]
                    .iter()
                    .rev()
                    .filter(|child| !self.holds(**child) && !inserted.contains(child)),
            );
        }
        self.emit(PageOp::Insert {
            container,
            before,
            paragraphs,
        })
    }

    /// Row, column, width, border and cell edits of stored tables.
    fn tables(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let stored: BTreeMap<ExGuid, &Table> = old
            .paragraphs
            .values()
            .filter_map(|p| match &p.content {
                ParagraphContent::Table(table) => Some((table.id, table)),
                _ => None,
            })
            .collect();
        for paragraph in new.paragraphs.values() {
            let ParagraphContent::Table(table) = &paragraph.content else {
                continue;
            };
            let Some(previous) = stored.get(&table.id) else {
                continue;
            };
            self.table(previous, table)?;
            let cells: BTreeMap<ExGuid, &TableCell> = previous
                .rows
                .iter()
                .flat_map(|row| &row.cells)
                .map(|cell| (cell.id, cell))
                .collect();
            for cell in table.rows.iter().flat_map(|row| &row.cells) {
                let Some(before) = cells.get(&cell.id) else {
                    continue;
                };
                if (cell.shading, &cell.indents) != (before.shading, &before.indents) {
                    self.emit(PageOp::Table {
                        table: table.id,
                        edit: TableEdit::Cell {
                            cell: cell.id,
                            shading: cell.shading,
                            indents: cell.indents.clone(),
                        },
                    })?;
                }
            }
        }
        Ok(())
    }

    /// The row and column edits turning stored table `before` into `after`.
    fn table(&mut self, before: &Table, after: &Table) -> Result<(), Error> {
        let id = after.id;
        let unsupported = || invalid("Table rows and cells keep their order; add or remove them");
        let old_rows: Vec<ExGuid> = before.rows.iter().map(|row| row.id).collect();
        let new_rows: Vec<ExGuid> = after.rows.iter().map(|row| row.id).collect();
        let kept_rows: Vec<ExGuid> = old_rows
            .iter()
            .copied()
            .filter(|row| new_rows.contains(row))
            .collect();
        if new_rows
            .iter()
            .filter(|row| old_rows.contains(row))
            .ne(kept_rows.iter())
        {
            return Err(unsupported());
        }
        // The column pattern every kept row shares: which stored cells stay, which are new.
        let mut deleted: Option<Vec<u32>> = None;
        let mut added: Option<Vec<u32>> = None;
        for row in &kept_rows {
            let old_cells: Vec<ExGuid> = before
                .rows
                .iter()
                .find(|r| r.id == *row)
                .unwrap()
                .cells
                .iter()
                .map(|cell| cell.id)
                .collect();
            let new_cells: Vec<ExGuid> = after
                .rows
                .iter()
                .find(|r| r.id == *row)
                .unwrap()
                .cells
                .iter()
                .map(|cell| cell.id)
                .collect();
            let gone: Vec<u32> = (0..old_cells.len() as u32)
                .filter(|i| !new_cells.contains(&old_cells[*i as usize]))
                .collect();
            let fresh: Vec<u32> = (0..new_cells.len() as u32)
                .filter(|i| !old_cells.contains(&new_cells[*i as usize]))
                .collect();
            if old_cells
                .iter()
                .filter(|cell| new_cells.contains(cell))
                .ne(new_cells.iter().filter(|cell| old_cells.contains(cell)))
                || deleted.as_ref().is_some_and(|known| *known != gone)
                || added.as_ref().is_some_and(|known| *known != fresh)
            {
                return Err(unsupported());
            }
            deleted = Some(gone);
            added = Some(fresh);
        }
        let (deleted, added) = (deleted.unwrap_or_default(), added.unwrap_or_default());
        if kept_rows.is_empty() && before.columns.len() != after.columns.len() {
            return Err(unsupported());
        }
        for row in &old_rows {
            if !new_rows.contains(row) && !kept_rows.is_empty() {
                self.emit(PageOp::Table {
                    table: id,
                    edit: TableEdit::DeleteRow(*row),
                })?;
            }
        }
        for at in deleted.iter().rev() {
            self.emit(PageOp::Table {
                table: id,
                edit: TableEdit::DeleteColumn(*at),
            })?;
        }
        for at in &added {
            let cells = kept_rows
                .iter()
                .map(|row| {
                    let row = after.rows.iter().find(|r| r.id == *row).unwrap();
                    bare_cells(&row.cells[*at as usize..=*at as usize]).remove(0)
                })
                .collect::<Vec<_>>();
            for cell in &cells {
                self.created.extend(model::identities(&cell.paragraphs));
            }
            self.emit(PageOp::Table {
                table: id,
                edit: TableEdit::Column {
                    at: *at,
                    width: after.columns[*at as usize].width,
                    cells,
                },
            })?;
        }
        // New rows, each run before the stored row that follows it.
        let mut run = Vec::new();
        let mut next = None;
        for row in after.rows.iter().rev() {
            if old_rows.contains(&row.id) {
                self.rows(id, next, &mut run)?;
                next = Some(row.id);
            } else {
                run.push(row.clone());
            }
        }
        self.rows(id, next, &mut run)?;
        if kept_rows.is_empty() {
            for row in &old_rows {
                self.emit(PageOp::Table {
                    table: id,
                    edit: TableEdit::DeleteRow(*row),
                })?;
            }
        }
        let current = model::table(&self.current, id)
            .ok_or_else(|| invalid("A table is missing after row edits"))?;
        if current.columns != after.columns {
            self.emit(PageOp::Table {
                table: id,
                edit: TableEdit::Columns(after.columns.clone()),
            })?;
        }
        let current = model::table(&self.current, id).unwrap();
        if current.borders != after.borders
            && let Some(borders) = after.borders
        {
            self.emit(PageOp::Table {
                table: id,
                edit: TableEdit::Borders(borders),
            })?;
        }
        Ok(())
    }

    fn rows(
        &mut self,
        table: ExGuid,
        before: Option<ExGuid>,
        run: &mut Vec<crate::page::TableRow>,
    ) -> Result<(), Error> {
        if run.is_empty() {
            return Ok(());
        }
        run.reverse();
        let rows: Vec<crate::page::TableRow> = run
            .drain(..)
            .map(|row| crate::page::TableRow {
                id: row.id,
                cells: bare_cells(&row.cells),
            })
            .collect();
        for cell in rows.iter().flat_map(|row| &row.cells) {
            self.created.extend(model::identities(&cell.paragraphs));
        }
        self.emit(PageOp::Table {
            table,
            edit: TableEdit::Rows { before, rows },
        })
    }

    /// Edits of stored pictures, ink and attachments that paragraphs hold.
    fn content(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let Some(previous) = old.paragraphs.get(id) else {
                continue;
            };
            match (&previous.content, &paragraph.content) {
                (ParagraphContent::Image(stored), ParagraphContent::Image(image))
                    if stored != image =>
                {
                    self.emit(PageOp::Picture {
                        picture: image.id,
                        layout: image.layout.clone(),
                        alt: image.alt.clone(),
                    })?;
                }
                (ParagraphContent::Ink(stored), ParagraphContent::Ink(ink)) if stored != ink => {
                    self.strokes(stored, ink)?;
                }
                (
                    ParagraphContent::Attachment(stored),
                    ParagraphContent::Attachment(attachment),
                ) if stored != attachment => {
                    self.emit(PageOp::Attachment {
                        attachment: attachment.id,
                        filename: attachment.filename.clone(),
                        source_path: attachment.source_path.clone(),
                        size: attachment.size,
                    })?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn strokes(&mut self, stored: &crate::page::Ink, ink: &crate::page::Ink) -> Result<(), Error> {
        let placed = |layout: &crate::document::Layout| crate::document::Layout {
            x: None,
            y: None,
            ..layout.clone()
        };
        if placed(&stored.layout) != placed(&ink.layout) || stored.groups != ink.groups {
            return Err(invalid("Ink size and groups stay as stored"));
        }
        if (stored.layout.x, stored.layout.y) != (ink.layout.x, ink.layout.y) {
            let (Some(x), Some(y)) = (ink.layout.x, ink.layout.y) else {
                return Err(invalid("An ink position needs both coordinates"));
            };
            self.emit(PageOp::Outline {
                object: ink.id,
                edit: crate::OutlineEdit::Position { x, y },
            })?;
            if stored.strokes == ink.strokes {
                return Ok(());
            }
        }
        let mut add = Vec::new();
        for stroke in &ink.strokes {
            match stored.strokes.iter().find(|s| s.id == stroke.id) {
                Some(previous) if previous == stroke => {}
                Some(_) => return Err(invalid("A stored stroke keeps its path and pen")),
                None => add.push(stroke.clone()),
            }
        }
        let remove: Vec<ExGuid> = stored
            .strokes
            .iter()
            .filter(|stroke| !ink.strokes.iter().any(|s| s.id == stroke.id))
            .map(|stroke| stroke.id)
            .collect();
        if ink.strokes.iter().map(|s| s.id).ne(stored
            .strokes
            .iter()
            .filter(|s| !remove.contains(&s.id))
            .map(|s| s.id)
            .chain(add.iter().map(|s| s.id)))
        {
            return Err(invalid("Stored strokes keep their order; new ones follow"));
        }
        self.emit(PageOp::Strokes {
            ink: ink.id,
            add,
            remove,
        })
    }

    fn delete(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        consumed: &BTreeSet<ExGuid>,
    ) -> Result<(), Error> {
        let removed_outline =
            |id: ExGuid| old.outlines.contains_key(&id) && !new.outlines.contains_key(&id);
        for id in old.paragraphs.keys() {
            if new.paragraphs.contains_key(id) || consumed.contains(id) {
                continue;
            }
            let mut ancestor = old.container[id];
            let mut covered = false;
            // A surviving ancestor is placed where the model says, carrying this paragraph along.
            while !new.paragraphs.contains_key(&ancestor) {
                if removed_outline(ancestor)
                    || (old.paragraphs.contains_key(&ancestor) && !consumed.contains(&ancestor))
                    || (old.children.contains_key(&ancestor)
                        && !old.paragraphs.contains_key(&ancestor)
                        && !old.outlines.contains_key(&ancestor)
                        && !new.children.contains_key(&ancestor)
                        && !old.external.contains(&ancestor))
                {
                    covered = true;
                    break;
                }
                match old.container.get(&ancestor) {
                    Some(parent) => ancestor = *parent,
                    None => break,
                }
            }
            if !covered {
                self.emit(PageOp::Delete { object: *id })?;
            }
        }
        let kept: BTreeSet<ExGuid> = new.page.objects.iter().map(PageObject::id).collect();
        for object in &old.page.objects {
            if matches!(
                object,
                PageObject::Image(_) | PageObject::Attachment(_) | PageObject::Ink(_)
            ) && !kept.contains(&object.id())
            {
                self.emit(PageOp::Delete {
                    object: object.id(),
                })?;
            }
        }
        for id in old.outlines.keys() {
            if removed_outline(*id) {
                if old.title_outlines.contains(id) {
                    return Err(invalid("Title outlines cannot be removed"));
                }
                self.emit(PageOp::Delete { object: *id })?;
            }
        }
        Ok(())
    }

    /// Gives paragraphs the levels the model does, parents first.
    fn levels(&mut self, new: &View<'_>) -> Result<(), Error> {
        for document in new.documents() {
            for paragraph in document {
                let current = model::paragraph(&self.current, paragraph.id)
                    .ok_or_else(|| invalid("A paragraph is missing after structural edits"))?;
                if current.level != paragraph.level {
                    self.emit(PageOp::Level {
                        paragraph: paragraph.id,
                        level: paragraph.level,
                    })?;
                }
            }
        }
        Ok(())
    }

    fn equations(&mut self, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let stored = model::paragraph(&self.current, *id).and_then(|p| p.text().cloned());
            // Deleting an equation leaves no math in the model, but its stored runs are still math.
            if !crate::page::Math::is_equation(&text.text)
                && !stored
                    .as_ref()
                    .is_some_and(|stored| crate::page::Math::is_equation(&stored.text))
            {
                continue;
            }
            let stored = stored
                .ok_or_else(|| invalid("An equation paragraph is missing after placement"))?;
            if stored.text != text.text {
                self.emit(PageOp::Equation {
                    text: stored.id,
                    math: text.text.clone(),
                })?;
            }
        }
        Ok(())
    }

    /// The page's date, with every date and time field of the title showing it.
    fn date(&mut self, before: &Page, after: &Page) -> Result<(), Error> {
        let fields: Vec<(ExGuid, String)> = model::date_fields(after)
            .into_iter()
            .map(|text| (text.id, text.text.text().to_owned()))
            .collect();
        let shown = fields.iter().all(|(id, shown)| {
            model::paragraph_text(&self.current, *id).is_some_and(|text| text.text.text() == shown)
        });
        if before.color != after.color {
            self.emit(PageOp::Color(after.color))?;
        }
        if before.rule_lines != after.rule_lines {
            self.emit(PageOp::RuleLines(after.rule_lines))?;
        }
        if before.created == after.created && shown {
            return Ok(());
        }
        let created = after
            .created
            .ok_or_else(|| invalid("A page date cannot be removed"))?;
        self.emit(PageOp::Date { created, fields })
    }

    fn text(&mut self, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let Some(stored) = model::paragraph(&self.current, *id).and_then(|p| p.text().cloned())
            else {
                return Err(invalid("A paragraph is missing after structural edits"));
            };
            if stored.id != text.id {
                return Err(invalid("Text object identities cannot change"));
            }
            if stored.text.text() == text.text.text() {
                continue;
            }
            let (range, with) = text_edit(stored.text.text(), text.text.text())?;
            self.emit(PageOp::Text {
                text: stored.id,
                range,
                with,
            })?;
        }
        Ok(())
    }

    fn styles(&mut self, after: &Page, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let current = model::paragraph(&self.current, *id)
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            let Some(style) = paragraph.style else {
                if current.style.is_some() {
                    self.emit(PageOp::Unstyle { paragraph: *id })?;
                }
                continue;
            };
            if current.style == Some(style) {
                continue;
            }
            if paragraph.text().is_none() {
                return Err(invalid("Only text paragraphs take a paragraph style"));
            }
            let definition = after
                .definitions
                .get(&style)
                .ok_or_else(|| invalid("A paragraph references a missing style definition"))?;
            self.emit(PageOp::Style {
                paragraph: *id,
                style,
                definition: definition.clone(),
            })?;
        }
        Ok(())
    }

    /// Gives each paragraph the list nodes its model references; one another paragraph
    /// owns is copied, as native list nodes belong to one paragraph.
    fn lists(&mut self, after: &Page, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let current = model::paragraph(&self.current, *id)
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            let same = paragraph.lists.len() == current.lists.len()
                && paragraph
                    .lists
                    .iter()
                    .zip(&current.lists)
                    .all(|(model, node)| {
                        model == node
                            && after.definitions.get(model) == self.current.definitions.get(node)
                    });
            if same {
                continue;
            }
            let own = current.lists.clone();
            let mut lists = Vec::new();
            for list in &paragraph.lists {
                let definition = after
                    .definitions
                    .get(list)
                    .ok_or_else(|| invalid("A paragraph references a missing list definition"))?;
                let node = if own.contains(list) || !model::owns_list(&self.current, *list) {
                    *list
                } else {
                    new_id().map_err(|_| invalid("System random source failed"))?
                };
                lists.push((node, definition.clone()));
            }
            self.emit(PageOp::List {
                paragraph: *id,
                lists,
            })?;
        }
        Ok(())
    }

    fn tags(&mut self, after: &Page, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let current = model::paragraph(&self.current, *id)
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?
                .clone();
            let mut targets = vec![(*id, &paragraph.tags, current.tags.clone())];
            match (&paragraph.content, &current.content) {
                (ParagraphContent::Text(text), ParagraphContent::Text(stored)) => {
                    targets.push((text.id, &text.tags, stored.tags.clone()))
                }
                (ParagraphContent::Table(table), ParagraphContent::Table(stored)) => {
                    targets.push((table.id, &table.tags, stored.tags.clone()))
                }
                (ParagraphContent::Image(image), ParagraphContent::Image(stored)) => {
                    targets.push((image.id, &image.tags, stored.tags.clone()))
                }
                (ParagraphContent::Attachment(file), ParagraphContent::Attachment(stored)) => {
                    targets.push((file.id, &file.tags, stored.tags.clone()))
                }
                _ => {}
            }
            for (target, tags, stored) in targets {
                self.set_tags(after, target, tags, &stored)?;
            }
        }
        Ok(())
    }

    /// Emits the op giving `target` `tags` where it stores others.
    fn set_tags(
        &mut self,
        after: &Page,
        target: ExGuid,
        tags: &[Tag],
        stored: &[Tag],
    ) -> Result<(), Error> {
        if same_tags(tags, stored) {
            return Ok(());
        }
        let mut definitions = Vec::new();
        for tag in tags {
            let Some(definition) = tag.definition else {
                continue;
            };
            if let Some(model) = after.definitions.get(&definition)
                && !definitions.iter().any(|(id, _)| *id == definition)
            {
                definitions.push((definition, model.clone()));
            }
        }
        self.emit(PageOp::Tags {
            target,
            tags: tags.to_vec(),
            definitions,
        })
    }

    fn media(&mut self, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let current = model::paragraph(&self.current, *id)
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            if current.media != paragraph.media {
                self.emit(PageOp::Media {
                    paragraph: *id,
                    media: paragraph.media.clone(),
                })?;
            }
        }
        Ok(())
    }

    fn paragraph_formatting(&mut self, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let stored = model::paragraph(&self.current, *id)
                .and_then(|p| p.text().cloned())
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            let fields = paragraph_fields(&stored.text, &text.text)?;
            if fields == ParagraphFields::default() {
                continue;
            }
            if text.date_field.is_some() {
                return Err(invalid(
                    "Generated title fields cannot be formatted as ordinary text",
                ));
            }
            self.emit(PageOp::Paragraph {
                paragraph: *id,
                alignment: fields.alignment,
                rtl: fields.rtl,
                space_before: fields.space_before,
                space_after: fields.space_after,
                line_spacing: fields.line_spacing,
                language: fields.language,
            })?;
        }
        Ok(())
    }

    fn formatting(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let stored_texts: BTreeSet<ExGuid> = old
            .paragraphs
            .values()
            .filter_map(|p| Some(p.text()?.id))
            .collect();
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let stored = model::paragraph(&self.current, *id)
                .and_then(|p| p.text().cloned())
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            if stored.text.text() != text.text.text() {
                return Err(invalid("Text edits did not converge on the model"));
            }
            let fresh = !stored_texts.contains(&text.id);
            for (range, set, clear) in format_edits(&stored.text, &text.text, fresh)? {
                self.emit(PageOp::Format {
                    text: stored.id,
                    range,
                    set,
                    clear,
                })?;
            }
        }
        Ok(())
    }

    fn layout(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let current = model::paragraph(&self.current, *id)
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            if current.collapsed != paragraph.collapsed {
                self.emit(PageOp::Outline {
                    object: *id,
                    edit: crate::OutlineEdit::Collapsed(paragraph.collapsed),
                })?;
            }
        }
        for (id, outline) in &new.outlines {
            if new.title_outlines.contains(id) || !old.outlines.contains_key(id) {
                continue;
            }
            let stored = model::outline(&self.current, *id)
                .ok_or_else(|| invalid("An outline is missing after text edits"))?;
            let (stored_position, stored_width) = (
                (stored.layout.x, stored.layout.y),
                (stored.layout.max_width, stored.layout.width_set_by_user),
            );
            if (outline.layout.x, outline.layout.y) != stored_position {
                let (Some(x), Some(y)) = (outline.layout.x, outline.layout.y) else {
                    return Err(invalid("An outline position needs both coordinates"));
                };
                self.emit(PageOp::Outline {
                    object: *id,
                    edit: crate::OutlineEdit::Position { x, y },
                })?;
            }
            if (outline.layout.max_width, outline.layout.width_set_by_user) != stored_width {
                let Some(points) = outline.layout.max_width else {
                    return Err(invalid("An outline width cannot be removed"));
                };
                self.emit(PageOp::Outline {
                    object: *id,
                    edit: crate::OutlineEdit::Width {
                        points,
                        user_set: outline.layout.width_set_by_user == Some(true),
                    },
                })?;
            }
        }
        Ok(())
    }
}

/// The outline or cell a paragraph of `view` lies in.
fn root(view: &View<'_>, paragraph: ExGuid) -> ExGuid {
    let mut at = paragraph;
    while let Some(parent) = view.container.get(&at) {
        at = *parent;
    }
    at
}
