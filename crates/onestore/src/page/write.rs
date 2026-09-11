//! Publishes an edited page model by lowering the difference from the stored page onto
//! the typed writers, then squashing their transactions into one revision per space.

use super::{Outline, Page, PageObject, PageParagraph, ParagraphContent};
use crate::{
    Error, ExGuid, Insertion, ObjectData, OutlineEdit, ParagraphJoin, ParagraphSplit, PropertySets,
    RevisionIndex, Store, TextAttribute, TreeEdit, Value,
    document::{Document, Format},
    write::{PropertyObject, RevisionEdit, write_revisions},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

pub(crate) fn write_page(
    source: &[u8],
    space: ExGuid,
    after: &Page,
    author: &str,
) -> Result<Vec<u8>, Error> {
    if author.contains('\0') {
        return Err(invalid("Choose an author name without NUL"));
    }
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let pages = document.pages_in(space)?;
    let [page] = pages.as_slice() else {
        return Err(invalid("Choose an object space containing one active page"));
    };
    let before = Page::from_space(&document, space)?;
    let raw = index.resolve_active(space)?;
    let mut lowering = Lowering {
        image: source.to_vec(),
        space,
        page: *page,
        author,
        alias: BTreeMap::new(),
    };
    lowering.run(&before, after, &raw.objects.keys().copied().collect())?;
    if lowering.image == source {
        return Ok(lowering.image);
    }
    squash(source, &lowering.image, &lowering.alias)
}

/// Direct children of every container, in model order, plus lookups by identity.
struct View<'a> {
    page: &'a Page,
    outlines: BTreeMap<ExGuid, &'a Outline>,
    /// Outlines owned by a title object rather than the page.
    title_outlines: BTreeSet<ExGuid>,
    paragraphs: BTreeMap<ExGuid, &'a PageParagraph>,
    children: BTreeMap<ExGuid, Vec<ExGuid>>,
    container: BTreeMap<ExGuid, ExGuid>,
    page_children: Vec<ExGuid>,
}

impl<'a> View<'a> {
    fn new(page: &'a Page) -> Result<Self, Error> {
        let mut view = Self {
            page,
            outlines: BTreeMap::new(),
            title_outlines: BTreeSet::new(),
            paragraphs: BTreeMap::new(),
            children: BTreeMap::new(),
            container: BTreeMap::new(),
            page_children: Vec::new(),
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
                PageObject::Image(_) | PageObject::Unsupported(_) => {}
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
        for paragraph in list {
            let container = paragraph.parent.unwrap_or(root);
            if paragraph
                .parent
                .is_some_and(|parent| !self.paragraphs.contains_key(&parent))
            {
                return Err(invalid(
                    "A paragraph's parent must precede it in its container",
                ));
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

    fn text(&self, paragraph: ExGuid) -> Option<&'a super::TextObject> {
        self.paragraphs.get(&paragraph).and_then(|p| p.text())
    }
}

struct Lowering<'a> {
    image: Vec<u8>,
    space: ExGuid,
    page: ExGuid,
    author: &'a str,
    /// Model identities of new objects mapped to the identities the typed writers allocated.
    alias: BTreeMap<ExGuid, ExGuid>,
}

impl Lowering<'_> {
    fn id(&self, model: ExGuid) -> ExGuid {
        self.alias.get(&model).copied().unwrap_or(model)
    }

    fn apply(&mut self, edit: impl FnOnce(&[u8]) -> Result<Vec<u8>, Error>) -> Result<(), Error> {
        self.image = edit(&self.image)?;
        Ok(())
    }

    fn current(&self) -> Result<Page, Error> {
        let store = Store::parse(&self.image)?;
        let index = RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        Page::from_space(&document, self.space)
    }

    fn run(
        &mut self,
        before: &Page,
        after: &Page,
        existing: &BTreeSet<ExGuid>,
    ) -> Result<(), Error> {
        if after.created != before.created || after.margin_origin != before.margin_origin {
            return Err(invalid("Page creation time and margins cannot be edited"));
        }
        for (id, definition) in &after.definitions {
            if before.definitions.get(id) != Some(definition) {
                return Err(invalid("List, tag and style definitions cannot be edited"));
            }
        }
        let old = View::new(before)?;
        let new = View::new(after)?;
        self.check_fixed_objects(before, after, &old, &new)?;
        for id in new.outlines.keys().chain(new.paragraphs.keys()) {
            if !old.outlines.contains_key(id)
                && !old.paragraphs.contains_key(id)
                && existing.contains(id)
            {
                return Err(invalid(
                    "A new model identity already exists in the section",
                ));
            }
        }
        let mut placed: BTreeMap<ExGuid, Vec<ExGuid>> = BTreeMap::new();
        for (container, children) in &old.children {
            placed.insert(*container, children.clone());
        }
        let mut page_order: Vec<ExGuid> = old
            .page_children
            .iter()
            .copied()
            .filter(|id| !old.outlines.contains_key(id) || new.outlines.contains_key(id))
            .collect();
        self.insert_outlines(&old, &new, &mut placed, &mut page_order)?;
        let mut consumed = BTreeSet::new();
        self.split_and_join(&old, &new, &mut placed, &mut consumed)?;
        self.place(&old, &new, &placed, &page_order)?;
        self.delete(&old, &new, &consumed)?;
        self.edit_text(&new)?;
        self.edit_formatting(&new)?;
        self.edit_layout(&old, &new)?;
        Ok(())
    }

    fn check_fixed_objects(
        &self,
        before: &Page,
        after: &Page,
        old: &View<'_>,
        new: &View<'_>,
    ) -> Result<(), Error> {
        let fixed = |page: &Page| -> Vec<String> {
            page.objects
                .iter()
                .filter_map(|object| match object {
                    PageObject::Image(image) => Some(format!(
                        "{:?} {:?} {:?} {:?}",
                        image.id, image.layout, image.alt, image.background
                    )),
                    PageObject::Unsupported(unsupported) => Some(format!("{unsupported:?}")),
                    PageObject::Title(title) => Some(format!(
                        "{:?} {:?} {:?} {:?}",
                        title.id,
                        title.date,
                        title.layout,
                        title.outlines.iter().map(|o| o.id).collect::<Vec<_>>()
                    )),
                    PageObject::Outline(_) => None,
                })
                .collect()
        };
        let mut before_fixed = fixed(before);
        let mut after_fixed = fixed(after);
        before_fixed.sort();
        after_fixed.sort();
        if before_fixed != after_fixed {
            return Err(invalid(
                "Images, titles and unsupported objects cannot be edited through the page model",
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
        for (id, paragraph) in &new.paragraphs {
            let Some(previous) = old.paragraphs.get(id) else {
                continue;
            };
            let same = paragraph.lists == previous.lists
                && paragraph.tags == previous.tags
                && paragraph.style == previous.style
                && paragraph.format == previous.format;
            if !same {
                return Err(invalid(
                    "Paragraph lists, tags, styles and paragraph formatting cannot be edited",
                ));
            }
            match (&paragraph.content, &previous.content) {
                (ParagraphContent::Text(text), ParagraphContent::Text(previous)) => {
                    if text.date_field != previous.date_field || text.tags != previous.tags {
                        return Err(invalid("Text fields and tags cannot be edited"));
                    }
                }
                (ParagraphContent::Table(table), ParagraphContent::Table(previous)) => {
                    let same = table.id == previous.id
                        && table.columns == previous.columns
                        && table.borders == previous.borders
                        && table.layout == previous.layout
                        && table.tags == previous.tags
                        && table.rows.len() == previous.rows.len()
                        && table
                            .rows
                            .iter()
                            .zip(&previous.rows)
                            .all(|(row, previous)| {
                                row.id == previous.id
                                    && row.cells.len() == previous.cells.len()
                                    && row.cells.iter().zip(&previous.cells).all(
                                        |(cell, previous)| {
                                            cell.id == previous.id
                                                && cell.layout == previous.layout
                                                && cell.indents == previous.indents
                                                && cell.shading == previous.shading
                                                && cell.unsupported == previous.unsupported
                                        },
                                    )
                            });
                    if !same {
                        return Err(invalid("Table structure cannot be edited"));
                    }
                }
                (ParagraphContent::Unsupported(a), ParagraphContent::Unsupported(b)) if a == b => {}
                _ => return Err(invalid("Paragraph content type cannot change")),
            }
        }
        Ok(())
    }

    fn insert_outlines(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        placed: &mut BTreeMap<ExGuid, Vec<ExGuid>>,
        page_order: &mut Vec<ExGuid>,
    ) -> Result<(), Error> {
        for id in &new.page_children {
            let Some(outline) = new.outlines.get(id) else {
                continue;
            };
            if old.outlines.contains_key(id) {
                continue;
            }
            if new.title_outlines.contains(id) {
                return Err(invalid("Title outlines cannot be added"));
            }
            let (Some(x), Some(y)) = (outline.layout.x, outline.layout.y) else {
                return Err(invalid("A new outline needs a position"));
            };
            let Some(first) = new.children[id].first().copied() else {
                return Err(invalid("A new outline needs a paragraph"));
            };
            let text = new
                .text(first)
                .ok_or_else(|| invalid("A new outline starts with a text paragraph"))?;
            let insertion = Insertion::outline(self.page, x, y, text.text.text(), self.author)?;
            let space = self.space;
            self.apply(|image| insertion.apply(image, space))?;
            let outline_id = insertion.object();
            self.alias.insert(*id, outline_id);
            self.alias.insert(
                first,
                ExGuid {
                    guid: outline_id.guid,
                    n: 3,
                },
            );
            self.alias.insert(text.id, insertion.text_object());
            placed.insert(*id, vec![first]);
            placed.insert(first, Vec::new());
            page_order.push(*id);
        }
        Ok(())
    }

    fn split_and_join(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        placed: &mut BTreeMap<ExGuid, Vec<ExGuid>>,
        consumed: &mut BTreeSet<ExGuid>,
    ) -> Result<(), Error> {
        for (container, children) in &new.children {
            if !old.children.contains_key(container) {
                continue;
            }
            for pair in children.windows(2) {
                let [left, right] = [pair[0], pair[1]];
                let (Some(previous), Some(after), Some(next)) =
                    (old.text(left), new.text(left), new.text(right))
                else {
                    continue;
                };
                if old.paragraphs.contains_key(&right)
                    || old.container.get(&left) != Some(container)
                    || !new.children[&left].is_empty()
                    || new.children[&right] != old.children[&left]
                    || after.id != previous.id
                    || new.paragraphs[&right].style != old.paragraphs[&left].style
                {
                    continue;
                }
                let Ok(offset) = after.text.utf16_offset(after.text.text().len()) else {
                    continue;
                };
                // Splitting at the end only differs from appending a paragraph by the copied
                // paragraph style; without one, an appended empty paragraph is an insertion.
                if offset == previous.text.utf16_offset(previous.text.text().len())?
                    && new.paragraphs[&right].style.is_none()
                {
                    continue;
                }
                let Ok(head) = previous.text.slice(0..offset) else {
                    continue;
                };
                let Ok(tail) = previous.text.slice(
                    offset
                        ..previous
                            .text
                            .utf16_offset(previous.text.text().len())
                            .unwrap_or(0),
                ) else {
                    continue;
                };
                // An emptied side takes the writer's insertion style, so only its text must agree.
                let same = |expected: &super::Paragraph, actual: &super::Paragraph| {
                    expected.text() == actual.text()
                        && (expected.text().is_empty() || expected == actual)
                };
                if !same(&head, &after.text) || !same(&tail, &next.text) {
                    continue;
                }
                let split = ParagraphSplit::new(previous.id, offset, self.author)?;
                let space = self.space;
                self.apply(|image| split.apply(image, space))?;
                self.alias.insert(right, split.object());
                self.alias.insert(next.id, split.text_object());
                let list = placed.get_mut(container).unwrap();
                let at = list.iter().position(|id| *id == left).unwrap();
                list.insert(at + 1, right);
                placed.insert(right, placed[&left].clone());
                placed.insert(left, Vec::new());
            }
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
                    return Err(invalid(
                        "A joined paragraph keeps the left text identity unless the left text was empty",
                    ));
                }
                let join = ParagraphJoin::new(previous.id, removed.id, self.author)?;
                let space = self.space;
                self.apply(|image| join.apply(image, space))?;
                consumed.insert(right);
                placed.get_mut(container).unwrap().retain(|id| *id != right);
                placed.remove(&right);
            }
        }
        Ok(())
    }

    fn place(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        placed: &BTreeMap<ExGuid, Vec<ExGuid>>,
        page_order: &[ExGuid],
    ) -> Result<(), Error> {
        let title = |page: &Page, id: ExGuid| {
            page.objects
                .iter()
                .any(|object| matches!(object, PageObject::Title(title) if title.id == id))
        };
        let survivors: Vec<ExGuid> = page_order
            .iter()
            .copied()
            .filter(|id| {
                (new.outlines.contains_key(id) || !old.outlines.contains_key(id))
                    && !title(old.page, *id)
            })
            .collect();
        let after: Vec<ExGuid> = new
            .page_children
            .iter()
            .copied()
            .filter(|id| !title(new.page, *id))
            .collect();
        let kept = kept_set(&survivors, &after, |id| {
            new.outlines.contains_key(&id) && !new.title_outlines.contains(&id)
        })?;
        let mut next = None;
        for id in after.iter().rev() {
            if !kept.contains(id) {
                let edit = TreeEdit::move_to(
                    self.id(*id),
                    self.page,
                    next.map(|n| self.id(n)),
                    self.author,
                )?;
                let space = self.space;
                self.apply(|image| edit.apply(image, space))?;
            }
            next = Some(*id);
        }
        let mut containers: Vec<ExGuid> = Vec::new();
        for object in &new.page.objects {
            let outlines: Vec<&Outline> = match object {
                PageObject::Outline(outline) => vec![outline],
                PageObject::Title(title) => title.outlines.iter().collect(),
                PageObject::Image(_) | PageObject::Unsupported(_) => Vec::new(),
            };
            for outline in outlines {
                containers.push(outline.id);
                collect_containers(&outline.paragraphs, &mut containers);
            }
        }
        for container in containers {
            let after = &new.children[&container];
            let current: Vec<ExGuid> = placed
                .get(&container)
                .map(|list| {
                    list.iter()
                        .copied()
                        .filter(|id| new.paragraphs.contains_key(id))
                        .collect()
                })
                .unwrap_or_default();
            let kept = kept_set(&current, after, |_| true)?;
            let mut next = None;
            for id in after.iter().rev() {
                let anchor = next.map(|n| self.id(n));
                if !old.paragraphs.contains_key(id) && !self.alias.contains_key(id) {
                    let paragraph = new.paragraphs[id];
                    let text = paragraph
                        .text()
                        .filter(|text| text.date_field.is_none() && text.tags.is_empty())
                        .filter(|_| {
                            paragraph.lists.is_empty()
                                && paragraph.tags.is_empty()
                                && paragraph.style.is_none()
                        })
                        .ok_or_else(|| {
                            invalid("New paragraphs contain plain text without lists, tags, fields or styles")
                        })?;
                    let insertion = Insertion::paragraph(
                        self.id(container),
                        anchor,
                        text.text.text(),
                        self.author,
                    )?;
                    let space = self.space;
                    self.apply(|image| insertion.apply(image, space))?;
                    self.alias.insert(*id, insertion.object());
                    self.alias.insert(text.id, insertion.text_object());
                } else if !kept.contains(id) {
                    let edit =
                        TreeEdit::move_to(self.id(*id), self.id(container), anchor, self.author)?;
                    let space = self.space;
                    self.apply(|image| edit.apply(image, space))?;
                }
                next = Some(*id);
            }
        }
        Ok(())
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
            loop {
                if removed_outline(ancestor)
                    || (old.paragraphs.contains_key(&ancestor)
                        && !new.paragraphs.contains_key(&ancestor)
                        && !consumed.contains(&ancestor))
                {
                    covered = true;
                    break;
                }
                match old.container.get(&ancestor) {
                    Some(parent) => ancestor = *parent,
                    None => break,
                }
            }
            if covered {
                continue;
            }
            let edit = TreeEdit::delete(*id, self.author)?;
            let space = self.space;
            self.apply(|image| edit.apply(image, space))?;
        }
        for id in old.outlines.keys() {
            if removed_outline(*id) {
                if old.title_outlines.contains(id) {
                    return Err(invalid("Title outlines cannot be removed"));
                }
                let edit = TreeEdit::delete(*id, self.author)?;
                let space = self.space;
                self.apply(|image| edit.apply(image, space))?;
            }
        }
        Ok(())
    }

    fn edit_text(&mut self, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current)?;
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let image_id = self.id(*id);
            let Some(stored) = current.text(image_id) else {
                return Err(invalid("A paragraph is missing after structural edits"));
            };
            if stored.id != self.id(text.id) {
                return Err(invalid("Text object identities cannot change"));
            }
            if stored.text.text() == text.text.text() {
                continue;
            }
            let (range, replacement) = text_edit(stored.text.text(), text.text.text())?;
            let (space, object) = (self.space, stored.id);
            self.apply(|image| crate::replace_text(image, space, object, range, &replacement))?;
        }
        Ok(())
    }

    fn edit_formatting(&mut self, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current)?;
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let stored = current
                .text(self.id(*id))
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            if stored.text.text() != text.text.text() {
                return Err(invalid("Text edits did not converge on the model"));
            }
            let fresh = self.alias.contains_key(&text.id);
            let mut boundaries = BTreeSet::new();
            for paragraph in [&stored.text, &text.text] {
                for span in paragraph.spans() {
                    boundaries.insert(paragraph.utf16_offset(span.end)?);
                }
            }
            boundaries.insert(0);
            let boundaries: Vec<u32> = boundaries.into_iter().collect();
            let mut pending: Option<(Range<u32>, Vec<TextAttribute>)> = None;
            let mut edits = Vec::new();
            for window in boundaries.windows(2) {
                let (start, end) = (window[0], window[1]);
                if start == end {
                    continue;
                }
                let attributes = attributes(
                    format_in(&stored.text, start)?,
                    format_in(&text.text, start)?,
                    fresh,
                )?;
                match &mut pending {
                    Some((range, previous)) if *previous == attributes && range.end == start => {
                        range.end = end;
                    }
                    _ => {
                        if let Some(edit) = pending.take() {
                            edits.push(edit);
                        }
                        pending = Some((start..end, attributes));
                    }
                }
            }
            edits.extend(pending);
            if text.text.text().is_empty() {
                let attributes = attributes(
                    format_in(&stored.text, 0)?,
                    format_in(&text.text, 0)?,
                    fresh,
                )?;
                if !attributes.is_empty() {
                    edits.push((0..0, attributes));
                }
            }
            for (range, attributes) in edits {
                if attributes.is_empty() {
                    continue;
                }
                let (space, object) = (self.space, stored.id);
                self.apply(|image| {
                    crate::formatting::format_text(image, space, object, range, &attributes)
                })?;
            }
        }
        Ok(())
    }

    fn edit_layout(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current)?;
        for (id, paragraph) in &new.paragraphs {
            let stored = current
                .paragraphs
                .get(&self.id(*id))
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            if stored.collapsed != paragraph.collapsed {
                let (space, object) = (self.space, self.id(*id));
                let edit = OutlineEdit::Collapsed(paragraph.collapsed);
                self.apply(|image| edit.apply(image, space, object))?;
            }
        }
        for (id, outline) in &new.outlines {
            if new.title_outlines.contains(id) {
                continue;
            }
            let stored = current
                .outlines
                .get(&self.id(*id))
                .ok_or_else(|| invalid("An outline is missing after text edits"))?;
            let (space, object) = (self.space, self.id(*id));
            if (outline.layout.x, outline.layout.y) != (stored.layout.x, stored.layout.y) {
                let (Some(x), Some(y)) = (outline.layout.x, outline.layout.y) else {
                    return Err(invalid("An outline position needs both coordinates"));
                };
                let edit = OutlineEdit::Position { x, y };
                self.apply(|image| edit.apply(image, space, object))?;
            }
            if (outline.layout.max_width, outline.layout.width_set_by_user)
                != (stored.layout.max_width, stored.layout.width_set_by_user)
                && (old.outlines.contains_key(id) || outline.layout.max_width.is_some())
            {
                let Some(points) = outline.layout.max_width else {
                    return Err(invalid("An outline width cannot be removed"));
                };
                let edit = OutlineEdit::Width {
                    points,
                    user_set: outline.layout.width_set_by_user == Some(true),
                };
                self.apply(|image| edit.apply(image, space, object))?;
            }
        }
        Ok(())
    }
}

fn collect_containers(list: &[PageParagraph], out: &mut Vec<ExGuid>) {
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

/// Identities that keep their stored position: a longest increasing run of survivors,
/// always including immovable ones. `after` may contain identities absent from `before`.
fn kept_set(
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

/// The format of the span containing the UTF-16 position `at` (the last span at the end).
fn format_in(paragraph: &super::Paragraph, at: u32) -> Result<&Format, Error> {
    let byte = paragraph.byte_offset(at)?;
    let spans = paragraph.spans();
    let index = spans
        .partition_point(|span| span.end <= byte)
        .min(spans.len() - 1);
    Ok(&spans[index].format)
}

/// The smallest UTF-16 range whose replacement turns `before` into `after`.
fn text_edit(before: &str, after: &str) -> Result<(Range<u32>, String), Error> {
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

/// Explicit attributes turning `current` into `target`; unsupported differences are errors.
/// A `fresh` text object was inserted by this edit, so an unspecified target value keeps
/// the insertion's default instead of demanding an inherited value the image cannot restore.
/// Language tags are retained as stored because the model has no way to author them.
fn attributes(current: &Format, target: &Format, fresh: bool) -> Result<Vec<TextAttribute>, Error> {
    let mut out = Vec::new();
    let inherited = || invalid("Inherited character formatting cannot be restored");
    macro_rules! boolean {
        ($field:ident, $variant:ident) => {
            if current.$field != target.$field {
                match target.$field {
                    Some(value) => out.push(TextAttribute::$variant(value)),
                    None if fresh => {}
                    None => return Err(inherited()),
                }
            }
        };
    }
    boolean!(bold, Bold);
    boolean!(italic, Italic);
    boolean!(underline, Underline);
    boolean!(strike, Strike);
    boolean!(superscript, Superscript);
    boolean!(subscript, Subscript);
    if current.font != target.font {
        match &target.font {
            Some(font) => out.push(TextAttribute::Font(font.clone())),
            None if fresh => {}
            None => return Err(inherited()),
        }
    }
    if current.font_size != target.font_size {
        match target.font_size {
            Some(size) => out.push(TextAttribute::FontSize(size)),
            None if fresh => {}
            None => return Err(inherited()),
        }
    }
    let color =
        |value: u32| (value != 0xff000000).then(|| value.to_le_bytes()[..3].try_into().unwrap());
    if current.color != target.color {
        match target.color {
            Some(value) => out.push(TextAttribute::Color(color(value))),
            None if fresh => {}
            None => return Err(inherited()),
        }
    }
    if current.highlight != target.highlight {
        match target.highlight {
            Some(value) => out.push(TextAttribute::Highlight(color(value))),
            None if fresh => {}
            None => return Err(inherited()),
        }
    }
    // An absent value and its stored default are the same formatting.
    let flag = |a: Option<bool>, b: Option<bool>| a.unwrap_or(false) == b.unwrap_or(false);
    let points = |a: Option<f32>, b: Option<f32>| a.unwrap_or(0.0) == b.unwrap_or(0.0);
    let same_rest = flag(current.hidden, target.hidden)
        && flag(current.hyperlink, target.hyperlink)
        && flag(current.hyperlink_label, target.hyperlink_label)
        && flag(current.math, target.math)
        && flag(current.embedded_object, target.embedded_object)
        && current.alignment.unwrap_or(0) == target.alignment.unwrap_or(0)
        && flag(current.rtl, target.rtl)
        && points(current.space_before, target.space_before)
        && points(current.space_after, target.space_after)
        && points(current.line_spacing, target.line_spacing)
        && points(current.list_spacing, target.list_spacing);
    if !same_rest {
        return Err(invalid(
            "Fields, links, language and paragraph spacing cannot be edited through the page model",
        ));
    }
    Ok(out)
}

/// Rewrites every revision the typed writers appended as one transaction on `source`,
/// renaming writer-allocated identities to the model's.
fn squash(
    source: &[u8],
    applied: &[u8],
    alias: &BTreeMap<ExGuid, ExGuid>,
) -> Result<Vec<u8>, Error> {
    let rename: BTreeMap<ExGuid, ExGuid> = alias
        .iter()
        .map(|(model, image)| (*image, *model))
        .collect();
    let applied_store = Store::parse(applied)?;
    let applied_index = RevisionIndex::parse(&applied_store)?;
    write_revisions(source, |index| {
        let mut changes = BTreeMap::new();
        for sid in applied_index.spaces.keys() {
            let Some(space) = index.spaces.get(sid) else {
                return Err(invalid("Page edits cannot create object spaces"));
            };
            let after_rid = applied_index.active(*sid)?;
            if space.labels.get(&(ExGuid::default(), 1)) == Some(&after_rid) {
                continue;
            }
            let before = index.resolve_active(*sid)?;
            let after = applied_index.resolve(*sid, after_rid)?;
            if before.roots != after.roots {
                return Err(invalid("Page edits cannot change revision roots"));
            }
            // Retired styles and deleted content stay in history; only live objects are written.
            let live = after.reachable()?;
            let mut changed = BTreeMap::new();
            for (id, object) in &after.objects {
                if !live.contains(id) {
                    continue;
                }
                if before.objects.get(id).is_some_and(|previous| {
                    previous.jcid == object.jcid
                        && previous.data == object.data
                        && previous.global_ids == object.global_ids
                }) {
                    continue;
                }
                let ObjectData::Properties(_) = object.data else {
                    return Err(invalid("Page edits only produce property objects"));
                };
                let mut replacement = PropertyObject::from_object(object)?;
                remap(&mut replacement, &rename)?;
                let id = rename.get(id).copied().unwrap_or(*id);
                replacement.reference(id)?;
                if before.objects.contains_key(&id) && rename.values().any(|model| *model == id) {
                    return Err(invalid(
                        "A new model identity already exists in the section",
                    ));
                }
                changed.insert(id, replacement);
            }
            changes.insert(*sid, RevisionEdit::Update(changed));
        }
        Ok(changes)
    })
}

fn remap(object: &mut PropertyObject, rename: &BTreeMap<ExGuid, ExGuid>) -> Result<(), Error> {
    let mut remapped = Vec::new();
    for property in PropertySets::parse(&object.bytes)?.sets.iter().flatten() {
        if let Value::References {
            stream: crate::IdStream::Objects,
            compact_ids,
        } = property.value
        {
            for bytes in compact_ids.chunks_exact(4) {
                let offset = bytes.as_ptr().addr() - object.bytes.as_ptr().addr();
                let id = crate::bytes::Cursor { bytes, offset }.compact(&object.global_ids)?;
                if let Some(model) = rename.get(&id) {
                    remapped.push((offset, *model));
                }
            }
        }
    }
    for (offset, id) in remapped {
        let reference = object.reference(id)?;
        object.bytes[offset..offset + 4].copy_from_slice(&reference);
    }
    Ok(())
}
