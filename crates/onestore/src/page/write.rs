//! Publishes an edited page model by lowering the difference from the stored page onto
//! the typed writers, then squashing their transactions into one revision per space.

use super::{Outline, Page, PageObject, PageParagraph, ParagraphContent, Table};
use crate::{
    Error, ExGuid, Insertion, ObjectData, OutlineEdit, ParagraphJoin, ParagraphSplit, PropertySets,
    RevisionIndex, Store, TextAttribute, TreeEdit, Value,
    document::{Document, Format, Kind, Tag},
    write::{PropertyObject, RevisionEdit},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// Property identifiers with their encoded values.
type Values = Vec<(u32, Vec<u8>)>;

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// A native measurement array: a count byte, `header - 1` reserved bytes, then each value
/// in inches.
/// The `<ifndf>{GUID}` form a file-data declaration uses to name a payload in the file-data store.
fn payload_reference(guid: [u8; 16]) -> String {
    let id = ExGuid { guid, n: 0 }.to_string();
    format!("<ifndf>{}", id.split(',').next().unwrap())
}

fn measurement_bytes(values: &[f32], header: usize) -> Result<Vec<u8>, Error> {
    let count = u8::try_from(values.len())
        .map_err(|_| invalid("A measurement array exceeds the document range"))?;
    let mut bytes = vec![0u8; header];
    bytes[0] = count;
    for value in values {
        if !value.is_finite() {
            return Err(invalid("A measurement must be finite"));
        }
        bytes.extend_from_slice(&(value / 36.0).to_le_bytes());
    }
    Ok(bytes)
}

/// Every row carries one cell per column and widths are usable.
fn validate_table(table: &Table) -> Result<(), Error> {
    if table.rows.is_empty() || table.columns.is_empty() {
        return Err(invalid("A table needs at least one row and one column"));
    }
    if table
        .columns
        .iter()
        .any(|c| !c.width.is_finite() || c.width < 36.0)
    {
        return Err(invalid("Table columns are at least 36 points wide"));
    }
    for row in &table.rows {
        if row.cells.len() != table.columns.len() {
            return Err(invalid("Every table row has one cell per column"));
        }
        for cell in &row.cells {
            // An emptied cell keeps a replacement paragraph, as the tree writer provides.
            for paragraph in &cell.paragraphs {
                if let ParagraphContent::Table(nested) = &paragraph.content {
                    validate_table(nested)?;
                }
            }
        }
    }
    Ok(())
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

    /// A writer identity for a new object; squash renames it to the model's.
    fn allocate(&mut self, model: ExGuid) -> Result<ExGuid, Error> {
        let written = ExGuid {
            guid: crate::write::fresh_guid()?,
            n: 1,
        };
        self.alias.insert(model, written);
        Ok(written)
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
            if before.definitions.get(id) != Some(definition)
                && !matches!(
                    definition.kind,
                    Kind::List { .. } | Kind::TagDefinition { .. }
                )
            {
                return Err(invalid("Style definitions cannot be edited"));
            }
        }
        let old = View::new(before)?;
        let new = View::new(after)?;
        for paragraph in new.paragraphs.values() {
            if let ParagraphContent::Table(table) = &paragraph.content {
                validate_table(table)?;
            }
        }
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
        self.edit_lists(after, &new)?;
        self.edit_tags(after, &new)?;
        self.edit_paragraph_formatting(&new)?;
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
            let same = paragraph.style == previous.style && paragraph.format == previous.format;
            if !same {
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
                    if table.id != previous.id
                        || table.layout != previous.layout
                        || table.tags != previous.tags
                    {
                        return Err(invalid("Table identity, layout and tags cannot be edited"));
                    }
                    let unchanged_cells =
                        table.rows.iter().flat_map(|row| &row.cells).all(|cell| {
                            previous
                                .rows
                                .iter()
                                .flat_map(|row| &row.cells)
                                .find(|before| before.id == cell.id)
                                .is_none_or(|before| {
                                    cell.layout == before.layout
                                        && cell.indents == before.indents
                                        && cell.shading == before.shading
                                        && cell.unsupported == before.unsupported
                                })
                        });
                    if !unchanged_cells {
                        return Err(invalid("Cell layout, indents and shading cannot be edited"));
                    }
                }
                (ParagraphContent::Unsupported(a), ParagraphContent::Unsupported(b)) if a == b => {}
                (ParagraphContent::Image(a), ParagraphContent::Image(b)) => {
                    if a != b {
                        return Err(invalid("A stored picture cannot be edited"));
                    }
                }
                (ParagraphContent::Attachment(a), ParagraphContent::Attachment(b)) => {
                    if a != b {
                        return Err(invalid("A stored attachment cannot be edited"));
                    }
                }
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
        // Cells that do not exist yet receive their paragraphs once the table structure does.
        let (existing, deferred): (Vec<ExGuid>, Vec<ExGuid>) =
            containers.into_iter().partition(|container| {
                old.children.contains_key(container)
                    || new.paragraphs.contains_key(container)
                    || new.outlines.contains_key(container)
            });
        self.place_containers(old, new, placed, &existing)?;
        self.edit_table_structure(old, new)?;
        self.edit_images(old, new)?;
        self.edit_attachments(old, new)?;
        self.place_containers(old, new, placed, &deferred)?;
        Ok(())
    }

    /// Gives each new attachment paragraph what OneNote stores for an inserted file: the
    /// payload embedded in the file-data store, an embedded-file container declaring it,
    /// and an attachment object naming the file that the paragraph holds as content.
    fn edit_attachments(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        for (paragraph_id, paragraph) in &new.paragraphs {
            let ParagraphContent::Attachment(attachment) = &paragraph.content else {
                continue;
            };
            if let Some(previous) = old.paragraphs.get(paragraph_id) {
                if previous.content != paragraph.content {
                    return Err(invalid("A stored attachment cannot be edited"));
                }
                continue;
            }
            let Some(bytes) = &attachment.bytes else {
                return Err(invalid("A new attachment needs its payload"));
            };
            let name = attachment.filename.as_str();
            if name.is_empty() || name.contains(['\0', '/', '\\']) {
                return Err(invalid(
                    "An attachment needs a file name without path separators",
                ));
            }
            let extension = name
                .rfind('.')
                .filter(|dot| *dot > 0)
                .map(|dot| &name[dot..])
                .unwrap_or("");
            let attachment_id = self.allocate(attachment.id)?;
            let file_id = ExGuid {
                guid: crate::write::fresh_guid()?,
                n: 1,
            };
            let payload_guid = crate::write::fresh_guid()?;
            let reference = payload_reference(payload_guid);
            let modified = crate::create::current_timestamps()?.0.to_le_bytes();
            let mut values: Values = vec![(0x14001d7a, modified.to_vec())];
            if let Some([width, height]) = attachment.size {
                if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
                    return Err(invalid("Attachment icon size must be positive"));
                }
                values.push((0x140034cd, (width / 36.0).to_le_bytes().to_vec()));
                values.push((0x140034ce, (height / 36.0).to_le_bytes().to_vec()));
            }
            values.push((0x14001c3b, 0x409_u32.to_le_bytes().to_vec()));
            values.push((0x10001cfe, 0x409_u16.to_le_bytes().to_vec()));
            values.push((0x1c001dcf, vec![0; 32]));
            values.push((
                0x1c001d61,
                [16u32, 1, 0, 0, 0]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
            ));
            values.push((0x14001c3e, 1u32.to_le_bytes().to_vec()));
            values.push((0x14001c84, 1u32.to_le_bytes().to_vec()));
            values.push((0x1c001c22, crate::create::string(name)));
            values.push((0x1c001d9c, crate::create::string(name)));
            if let Some(path) = &attachment.source_path {
                values.push((0x1c001d9d, crate::create::string(path)));
            }
            let holder = self.id(*paragraph_id);
            let space = self.space;
            let mut payloads: Vec<([u8; 16], &[u8])> = vec![(payload_guid, bytes)];
            let mut preview = None;
            if let Some(icon) = &attachment.preview {
                if !icon.starts_with(&[0x89, b'P', b'N', b'G']) {
                    return Err(invalid("An attachment preview is a PNG icon"));
                }
                let guid = crate::write::fresh_guid()?;
                payloads.push((guid, icon));
                preview = Some((
                    ExGuid {
                        guid: crate::write::fresh_guid()?,
                        n: 1,
                    },
                    payload_reference(guid),
                ));
            }
            self.apply(|current| {
                crate::write::write_revision_with_payloads(current, space, &payloads, |raw| {
                    let mut changed = BTreeMap::new();
                    let mut file = PropertyObject::file(file_id, &reference, extension)?;
                    file.jcid = 0x80036;
                    changed.insert(file_id, file);
                    let mut node = PropertyObject {
                        jcid: 0x60035,
                        bytes: crate::create::properties(&values)?,
                        global_ids: std::sync::Arc::new(BTreeMap::from([(0, attachment_id.guid)])),
                    };
                    node.reference(attachment_id)?;
                    let container = node.reference(file_id)?;
                    node.set(&[(0x20001d9b, &container)])?;
                    if let Some((icon_id, icon_reference)) = &preview {
                        changed.insert(
                            *icon_id,
                            PropertyObject::file(*icon_id, icon_reference, ".png")?,
                        );
                        let icon = node.reference(*icon_id)?;
                        node.set(&[(0x20001c3f, &icon)])?;
                    }
                    changed.insert(attachment_id, node);
                    let mut object = PropertyObject::from_object(&raw.objects[&holder])?;
                    let content = object.reference(attachment_id)?;
                    object.set(&[(0x24001c1f, &content), (0x14001d7a, &modified)])?;
                    changed.insert(holder, object);
                    Ok(changed)
                })
            })?;
        }
        Ok(())
    }

    /// Gives each new picture paragraph what OneNote stores for an inserted picture: the
    /// payload embedded in the section's file-data store, a file-data object declaring it
    /// by identity and extension, and a picture object the paragraph holds as content.
    fn edit_images(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        for (paragraph_id, paragraph) in &new.paragraphs {
            let ParagraphContent::Image(image) = &paragraph.content else {
                continue;
            };
            if let Some(previous) = old.paragraphs.get(paragraph_id) {
                if previous.content != paragraph.content {
                    return Err(invalid("A stored picture cannot be edited"));
                }
                continue;
            }
            let Some(bytes) = &image.bytes else {
                return Err(invalid("A new picture needs its payload"));
            };
            let extension = match bytes.as_ref() {
                [0x89, b'P', b'N', b'G', ..] => ".png",
                [0xff, 0xd8, 0xff, ..] => ".jpg",
                [b'G', b'I', b'F', b'8', ..] => ".gif",
                [b'B', b'M', ..] => ".bmp",
                _ => return Err(invalid("Choose a PNG, JPEG, GIF or BMP picture")),
            };
            let image_id = self.allocate(image.id)?;
            let file_id = ExGuid {
                guid: crate::write::fresh_guid()?,
                n: 1,
            };
            let payload_guid = crate::write::fresh_guid()?;
            let reference = payload_reference(payload_guid);
            let modified = crate::create::current_timestamps()?.0.to_le_bytes();
            let mut values: Values = vec![(0x14001d7a, modified.to_vec())];
            if let Some([width, height]) = image.size {
                if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
                    return Err(invalid("Picture size must be positive"));
                }
                values.push((0x140034cd, (width / 36.0).to_le_bytes().to_vec()));
                values.push((0x140034ce, (height / 36.0).to_le_bytes().to_vec()));
            }
            if let Some(alt) = &image.alt {
                values.push((0x1c001e58, crate::create::string(alt)));
            }
            if image.background {
                values.push((0x08001d13 | (1 << 31), Vec::new()));
            }
            values.push((0x08001d85, Vec::new()));
            let holder = self.id(*paragraph_id);
            let space = self.space;
            let payload: &[u8] = bytes;
            self.apply(|current| {
                crate::write::write_revision_with_payloads(
                    current,
                    space,
                    &[(payload_guid, payload)],
                    |raw| {
                        let mut changed = BTreeMap::new();
                        let file = PropertyObject::file(file_id, &reference, extension)?;
                        changed.insert(file_id, file);
                        let mut picture = PropertyObject {
                            jcid: 0x60011,
                            bytes: crate::create::properties(&values)?,
                            global_ids: std::sync::Arc::new(BTreeMap::from([(0, image_id.guid)])),
                        };
                        picture.reference(image_id)?;
                        let container = picture.reference(file_id)?;
                        picture.set(&[(0x20001c3f, &container)])?;
                        changed.insert(image_id, picture);
                        let mut object = PropertyObject::from_object(&raw.objects[&holder])?;
                        let content = object.reference(image_id)?;
                        object.set(&[(0x24001c1f, &content), (0x14001d7a, &modified)])?;
                        changed.insert(holder, object);
                        Ok(changed)
                    },
                )
            })?;
        }
        Ok(())
    }

    fn place_containers(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        placed: &BTreeMap<ExGuid, Vec<ExGuid>>,
        containers: &[ExGuid],
    ) -> Result<(), Error> {
        for container in containers {
            let after = &new.children[container];
            let current: Vec<ExGuid> = placed
                .get(container)
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
                    if paragraph.style.is_some() {
                        return Err(invalid(
                            "New paragraphs contain plain text without fields or styles",
                        ));
                    }
                    // A new table starts as an empty text paragraph whose content the
                    // structure pass replaces with the table.
                    let (text, text_id) = match &paragraph.content {
                        ParagraphContent::Text(text) if text.date_field.is_none() => {
                            (text.text.text(), Some(text.id))
                        }
                        ParagraphContent::Table(_)
                        | ParagraphContent::Image(_)
                        | ParagraphContent::Attachment(_) => ("", None),
                        _ => {
                            return Err(invalid(
                                "New paragraphs contain plain text without fields or styles",
                            ));
                        }
                    };
                    let insertion =
                        Insertion::paragraph(self.id(*container), anchor, text, self.author)?;
                    let space = self.space;
                    self.apply(|image| insertion.apply(image, space))?;
                    self.alias.insert(*id, insertion.object());
                    if let Some(text_id) = text_id {
                        self.alias.insert(text_id, insertion.text_object());
                    }
                } else if !kept.contains(id) {
                    let edit =
                        TreeEdit::move_to(self.id(*id), self.id(*container), anchor, self.author)?;
                    let space = self.space;
                    self.apply(|image| edit.apply(image, space))?;
                }
                next = Some(*id);
            }
        }
        Ok(())
    }

    /// Creates tables, rows and cells the model added, rebuilds every table's row and cell
    /// order to the model's, and writes the column widths, locks and border flag. Native
    /// rows and cells are plain containers; a cell carries its indent array and the flags
    /// every native cell has.
    fn edit_table_structure(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let old_tables: BTreeMap<ExGuid, &Table> = old
            .paragraphs
            .values()
            .filter_map(|p| match &p.content {
                ParagraphContent::Table(table) => Some((table.id, table)),
                _ => None,
            })
            .collect();
        for (paragraph_id, paragraph) in &new.paragraphs {
            let ParagraphContent::Table(table) = &paragraph.content else {
                continue;
            };
            let previous = old_tables.get(&table.id).copied();
            let unchanged = previous.is_some_and(|previous| {
                previous.columns == table.columns
                    && previous.borders == table.borders
                    && previous.rows.len() == table.rows.len()
                    && previous.rows.iter().zip(&table.rows).all(|(a, b)| {
                        a.id == b.id
                            && a.cells.len() == b.cells.len()
                            && a.cells.iter().zip(&b.cells).all(|(x, y)| x.id == y.id)
                    })
            });
            if unchanged {
                continue;
            }
            let modified = crate::create::current_timestamps()?.0.to_le_bytes();
            let mut created: Vec<(ExGuid, u32, Values)> = Vec::new();
            let table_id = if previous.is_some() {
                self.id(table.id)
            } else {
                self.allocate(table.id)?
            };
            let existing_rows: BTreeSet<ExGuid> = previous
                .map(|p| p.rows.iter().map(|r| r.id).collect())
                .unwrap_or_default();
            let existing_cells: BTreeSet<ExGuid> = previous
                .map(|p| {
                    p.rows
                        .iter()
                        .flat_map(|r| r.cells.iter().map(|c| c.id))
                        .collect()
                })
                .unwrap_or_default();
            let template = previous
                .and_then(|p| p.rows.first())
                .and_then(|r| r.cells.first());
            let mut rows = Vec::new();
            for row in &table.rows {
                let row_id = if existing_rows.contains(&row.id) {
                    self.id(row.id)
                } else {
                    self.allocate(row.id)?
                };
                let mut cells = Vec::new();
                for cell in &row.cells {
                    let cell_id = if existing_cells.contains(&cell.id) {
                        self.id(cell.id)
                    } else {
                        let written = self.allocate(cell.id)?;
                        let indents = if cell.indents.is_empty() {
                            template.map(|t| t.indents.clone()).unwrap_or_default()
                        } else {
                            cell.indents.clone()
                        };
                        let mut values: Values = vec![
                            (0x14001d7a, modified.to_vec()),
                            (0x0c001c13, vec![0]),
                            (0x0c001c03, vec![1]),
                            (0x88001c91, Vec::new()),
                        ];
                        if !indents.is_empty() {
                            values.push((0x1c001c12, measurement_bytes(&indents, 4)?));
                        }
                        if let Some(shading) = cell.shading {
                            values.push((0x14001e26, shading.to_le_bytes().to_vec()));
                        }
                        created.push((written, 0x60024, values));
                        written
                    };
                    cells.push(cell_id);
                }
                if !existing_rows.contains(&row.id) {
                    created.push((row_id, 0x60023, vec![(0x14001d7a, modified.to_vec())]));
                }
                rows.push((row_id, cells));
            }
            let mut table_values: Values = vec![
                (0x14001d7a, modified.to_vec()),
                (0x14001d57, (table.rows.len() as u32).to_le_bytes().to_vec()),
                (
                    0x14001d58,
                    (table.columns.len() as u32).to_le_bytes().to_vec(),
                ),
                (
                    0x1c001d66,
                    measurement_bytes(
                        &table.columns.iter().map(|c| c.width).collect::<Vec<_>>(),
                        1,
                    )?,
                ),
            ];
            let mut locks = vec![table.columns.len() as u8];
            locks.extend(vec![0u8; table.columns.len().div_ceil(8)]);
            for (i, column) in table.columns.iter().enumerate() {
                if column.locked {
                    locks[1 + i / 8] |= 1 << (i % 8);
                }
            }
            table_values.push((0x1c001d7d, locks));
            table_values.push((
                0x08001d5e | (u32::from(table.borders.unwrap_or(true)) << 31),
                Vec::new(),
            ));
            if previous.is_none() {
                table_values.push((0x14001c3e, 1u32.to_le_bytes().to_vec()));
                table_values.push((0x14001c84, 1u32.to_le_bytes().to_vec()));
                created.push((table_id, 0x60022, Vec::new()));
            }
            let holder = self.id(*paragraph_id);
            let is_new = previous.is_none();
            let space = self.space;
            self.apply(|image| {
                crate::write::write_revision(image, space, |raw| {
                    let mut changed: BTreeMap<ExGuid, PropertyObject> = BTreeMap::new();
                    for (id, jcid, values) in &created {
                        let mut node = PropertyObject {
                            jcid: *jcid,
                            bytes: crate::create::properties(values)?,
                            global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
                        };
                        node.reference(*id)?;
                        changed.insert(*id, node);
                    }
                    let mut row_refs = Vec::new();
                    for (row_id, cells) in &rows {
                        let mut row = match changed.remove(row_id) {
                            Some(row) => row,
                            None => PropertyObject::from_object(&raw.objects[row_id])?,
                        };
                        let mut cell_refs = Vec::new();
                        for cell in cells {
                            cell_refs.extend_from_slice(&row.reference(*cell)?);
                        }
                        row.set(&[(0x24001c20, &cell_refs), (0x14001d7a, &modified)])?;
                        changed.insert(*row_id, row);
                    }
                    let mut table_object = match changed.remove(&table_id) {
                        Some(object) => object,
                        None => PropertyObject::from_object(&raw.objects[&table_id])?,
                    };
                    for (row_id, _) in &rows {
                        row_refs.extend_from_slice(&table_object.reference(*row_id)?);
                    }
                    table_object.remove(&[0x1c001d7d, 0x08001d5e])?;
                    table_object.set(
                        &table_values
                            .iter()
                            .map(|(id, bytes)| (*id, bytes.as_slice()))
                            .collect::<Vec<_>>(),
                    )?;
                    table_object.set(&[(0x24001c20, &row_refs)])?;
                    changed.insert(table_id, table_object);
                    if is_new {
                        let mut object = PropertyObject::from_object(&raw.objects[&holder])?;
                        let reference = object.reference(table_id)?;
                        object.set(&[(0x24001c1f, &reference), (0x14001d7a, &modified)])?;
                        changed.insert(holder, object);
                    }
                    Ok(changed)
                })
            })?;
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
                    || (old.children.contains_key(&ancestor)
                        && !old.paragraphs.contains_key(&ancestor)
                        && !old.outlines.contains_key(&ancestor)
                        && !new.children.contains_key(&ancestor))
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

    /// Gives each paragraph the list nodes its model references: a definition new to the
    /// section becomes a list node carrying the model's identity, a changed one is rewritten
    /// in place, a definition another paragraph already owns is copied because native list
    /// nodes belong to one paragraph, and a dropped reference leaves the node unreferenced.
    fn edit_lists(&mut self, after: &Page, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let stored = View::new(&current)?;
        let mut owners: BTreeMap<ExGuid, ExGuid> = BTreeMap::new();
        for (id, paragraph) in &stored.paragraphs {
            for list in &paragraph.lists {
                owners.insert(*list, *id);
            }
        }
        for (id, paragraph) in &new.paragraphs {
            let image_id = self.id(*id);
            let previous = stored
                .paragraphs
                .get(&image_id)
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            let same = paragraph.lists.len() == previous.lists.len()
                && paragraph
                    .lists
                    .iter()
                    .zip(&previous.lists)
                    .all(|(model, node)| {
                        after.definitions.get(model) == current.definitions.get(node)
                    });
            if same {
                continue;
            }
            let mut nodes = Vec::new();
            for list in &paragraph.lists {
                // Writer-allocated identities keep a nonzero sequence number so the node's
                // compact identity is never the null identity; squash renames them to the
                // model's. A copy for a second owner keeps its allocated identity.
                let node_id = match owners.get(list) {
                    Some(owner) if *owner == image_id => *list,
                    Some(_) => ExGuid {
                        guid: crate::write::fresh_guid()?,
                        n: 1,
                    },
                    None => {
                        let written = ExGuid {
                            guid: crate::write::fresh_guid()?,
                            n: 1,
                        };
                        self.alias.insert(*list, written);
                        written
                    }
                };
                let definition = after
                    .definitions
                    .get(list)
                    .ok_or_else(|| invalid("A paragraph references a missing list definition"))?;
                let Kind::List {
                    font,
                    format,
                    restart,
                    bullet,
                } = &definition.kind
                else {
                    return Err(invalid("A paragraph list must reference a list definition"));
                };
                if bullet.is_some() && (format.is_none() || font.is_none()) {
                    return Err(invalid("A bullet definition names its glyph and font"));
                }
                let mut values: Values = Vec::new();
                if let Some(format) = format {
                    let units: Vec<u16> = format.encode_utf16().collect();
                    let count = u16::try_from(units.len())
                        .map_err(|_| invalid("List format exceeds the document range"))?;
                    let mut bytes = count.to_le_bytes().to_vec();
                    bytes.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
                    values.push((0x1c001c1a, bytes));
                }
                if let Some(font) = font {
                    values.push((0x1c001c52, crate::create::string(font)));
                }
                if let Some(restart) = restart {
                    values.push((0x14001cb7, restart.to_le_bytes().to_vec()));
                }
                if let Some(bullet) = bullet {
                    values.push((0x10001d0e, bullet.to_le_bytes().to_vec()));
                    // Every native bullet node carries this cleared flag alongside its index.
                    values.push((0x0c001cc0, vec![0]));
                }
                let style = &definition.format;
                if let Some(font) = &style.font {
                    values.push((0x1c001c0a, crate::create::string(font)));
                }
                if let Some(size) = style.font_size {
                    let half = (size * 2.0).round();
                    if !(0.0..=f32::from(u16::MAX)).contains(&half) {
                        return Err(invalid("List font size is outside the document range"));
                    }
                    values.push((0x10001c0b, (half as u16).to_le_bytes().to_vec()));
                }
                if let Some(color) = style.color {
                    values.push((0x14001c0c, color.to_le_bytes().to_vec()));
                }
                if let Some(language) = style.language {
                    values.push((0x14001c3b, language.to_le_bytes().to_vec()));
                }
                for (flag, id) in [(style.bold, 0x08001c04), (style.italic, 0x08001c05)] {
                    if let Some(flag) = flag {
                        values.push((id | (u32::from(flag) << 31), Vec::new()));
                    }
                }
                nodes.push((node_id, values));
            }
            let (space, object) = (self.space, self.id(*id));
            self.apply(|image| {
                let store = Store::parse(image)?;
                let index = RevisionIndex::parse(&store)?;
                let document = Document::parse(&index)?;
                let parents = crate::edit::editable_parents(
                    document.active(space)?,
                    &document.pages_in(space)?,
                    object,
                )?;
                let modified = crate::create::current_timestamps()?.0.to_le_bytes();
                crate::write::write_revision(image, space, |raw| {
                    let mut changed = BTreeMap::new();
                    let mut target = PropertyObject::from_object(&raw.objects[&object])?;
                    let mut references = Vec::new();
                    for (list, values) in &nodes {
                        let mut node = match raw.objects.get(list) {
                            Some(existing) => {
                                if existing.jcid != 0x60012 {
                                    return Err(invalid(
                                        "A list definition identity belongs to another object",
                                    ));
                                }
                                let mut node = PropertyObject::from_object(existing)?;
                                node.remove(&[
                                    0x1c001c1a, 0x1c001c52, 0x14001cb7, 0x10001d0e, 0x0c001cc0,
                                    0x1c001c0a, 0x10001c0b, 0x14001c0c, 0x14001c3b, 0x08001c04,
                                    0x08001c05,
                                ])?;
                                node
                            }
                            None => PropertyObject {
                                jcid: 0x60012,
                                bytes: crate::create::properties(&[])?,
                                global_ids: std::sync::Arc::new(BTreeMap::from([(0, list.guid)])),
                            },
                        };
                        node.set(
                            &values
                                .iter()
                                .map(|(id, bytes)| (*id, bytes.as_slice()))
                                .collect::<Vec<_>>(),
                        )?;
                        node.set(&[(0x14001d7a, &modified)])?;
                        node.reference(*list)?;
                        references.extend_from_slice(&target.reference(*list)?);
                        changed.insert(*list, node);
                    }
                    if references.is_empty() {
                        target.remove(&[0x24001c26])?;
                    } else {
                        target.set(&[(0x24001c26, &references)])?;
                    }
                    target.set(&[(0x14001d7a, &modified)])?;
                    changed.insert(object, target);
                    crate::formatting::touch_ancestors(
                        raw,
                        &parents,
                        object,
                        &modified,
                        &mut changed,
                    )?;
                    Ok(changed)
                })
            })?;
        }
        Ok(())
    }

    /// Rewrites the note tags of paragraphs and text objects whose model tags differ from
    /// the stored ones. A tag definition new to the section becomes a definition object
    /// carrying the model's identity after squash.
    fn edit_tags(&mut self, after: &Page, new: &View<'_>) -> Result<(), Error> {
        fn same(a: &[Tag], b: &[Tag]) -> bool {
            let key = |t: &Tag| {
                (
                    t.definition,
                    t.action_type,
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
        let current = self.current()?;
        let stored = View::new(&current)?;
        for (id, paragraph) in &new.paragraphs {
            let image_id = self.id(*id);
            let previous = stored
                .paragraphs
                .get(&image_id)
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            let mut targets = vec![(image_id, &paragraph.tags, &previous.tags)];
            if let (Some(text), Some(before)) = (paragraph.text(), previous.text()) {
                targets.push((self.id(text.id), &text.tags, &before.tags));
            }
            for (object, tags, stored_tags) in targets {
                if same(tags, stored_tags) {
                    continue;
                }
                let mut definitions: Vec<(ExGuid, Option<Values>)> = Vec::new();
                let mut sets: Vec<(usize, Values)> = Vec::new();
                let mut action_types = BTreeSet::new();
                for tag in tags {
                    let definition = tag
                        .definition
                        .ok_or_else(|| invalid("A note tag names its definition"))?;
                    let action_type = if tag.status & 4 != 0 {
                        tag.action_type
                    } else {
                        match after.definitions.get(&definition).map(|d| &d.kind) {
                            Some(Kind::TagDefinition { action_type, .. }) => *action_type,
                            _ => return Err(invalid("A note tag must reference a tag definition")),
                        }
                    };
                    if !action_types.insert(action_type.unwrap_or(0)) {
                        return Err(invalid("An element holds one note tag per action type"));
                    }
                    let known = current.definitions.contains_key(&definition)
                        || self.alias.contains_key(&definition);
                    let written = if known {
                        self.id(definition)
                    } else {
                        let allocated = ExGuid {
                            guid: crate::write::fresh_guid()?,
                            n: 1,
                        };
                        self.alias.insert(definition, allocated);
                        allocated
                    };
                    let index = match definitions.iter().position(|(id, _)| *id == written) {
                        Some(index) => index,
                        None => {
                            let values = if known {
                                None
                            } else {
                                let Some(model) = after.definitions.get(&definition) else {
                                    return Err(invalid(
                                        "A note tag references a missing tag definition",
                                    ));
                                };
                                let Kind::TagDefinition {
                                    label,
                                    action_type,
                                    shape,
                                    color,
                                    highlight,
                                } = &model.kind
                                else {
                                    return Err(invalid(
                                        "A note tag must reference a tag definition",
                                    ));
                                };
                                let mut values: Values = vec![
                                    (0x0c003473, vec![0]),
                                    (0x10003463, action_type.unwrap_or(0).to_le_bytes().to_vec()),
                                    (0x10003464, shape.unwrap_or(0).to_le_bytes().to_vec()),
                                    (0x14003467, 0u32.to_le_bytes().to_vec()),
                                ];
                                if let Some(label) = label {
                                    values.push((0x1c003468, crate::create::string(label)));
                                }
                                if let Some(color) = color {
                                    values.push((0x14003466, color.to_le_bytes().to_vec()));
                                }
                                if let Some(highlight) = highlight {
                                    values.push((0x14003465, highlight.to_le_bytes().to_vec()));
                                }
                                Some(values)
                            };
                            definitions.push((written, values));
                            definitions.len() - 1
                        }
                    };
                    let mut fields: Values = Vec::new();
                    if let Some(action_type) = tag.action_type {
                        fields.push((0x10003463, action_type.to_le_bytes().to_vec()));
                    }
                    for (id, value) in [
                        (0x1400346e, tag.created),
                        (0x1400346f, tag.completed),
                        (0x1400346a, tag.start),
                        (0x1400346b, tag.due),
                    ] {
                        if let Some(value) = value {
                            fields.push((id, value.to_le_bytes().to_vec()));
                        }
                    }
                    fields.push((0x10003470, tag.status.to_le_bytes().to_vec()));
                    if let Some(task) = tag.task_id {
                        fields.push((0x1c003469, task.to_vec()));
                    }
                    sets.push((index, fields));
                }
                let space = self.space;
                self.apply(|image| {
                    let store = Store::parse(image)?;
                    let index = RevisionIndex::parse(&store)?;
                    let document = Document::parse(&index)?;
                    let parents = crate::edit::editable_parents(
                        document.active(space)?,
                        &document.pages_in(space)?,
                        object,
                    )?;
                    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
                    crate::write::write_revision(image, space, |raw| {
                        let mut changed = BTreeMap::new();
                        for (id, values) in &definitions {
                            let Some(values) = values else {
                                continue;
                            };
                            let mut node = PropertyObject {
                                jcid: 0x120043,
                                bytes: crate::create::properties(values)?,
                                global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
                            };
                            node.reference(*id)?;
                            changed.insert(*id, node);
                        }
                        let mut target = PropertyObject::from_object(&raw.objects[&object])?;
                        let mut encoded = Vec::new();
                        for (index, fields) in &sets {
                            let reference = target.reference(definitions[*index].0)?;
                            let mut set = vec![(0x20003488, reference.to_vec())];
                            set.extend(fields.iter().cloned());
                            encoded.push(set);
                        }
                        target.set_sets(0x40003489, 0x44000811, &encoded)?;
                        target.set(&[(0x14001d7a, &modified)])?;
                        changed.insert(object, target);
                        crate::formatting::touch_ancestors(
                            raw,
                            &parents,
                            object,
                            &modified,
                            &mut changed,
                        )?;
                        Ok(changed)
                    })
                })?;
            }
        }
        Ok(())
    }

    fn edit_paragraph_formatting(&mut self, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current)?;
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let stored = current
                .text(self.id(*id))
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            let mut values: Vec<(u32, Vec<u8>)> = Vec::new();
            macro_rules! field {
                ($field:ident, $value:ident, $encode:block) => {
                    let $value = format_in(&text.text, 0)?.$field.unwrap_or_default();
                    if text
                        .text
                        .spans()
                        .iter()
                        .all(|span| span.format.$field.unwrap_or_default() == $value)
                        && stored
                            .text
                            .spans()
                            .iter()
                            .any(|span| span.format.$field.unwrap_or_default() != $value)
                    {
                        values.push($encode);
                    }
                };
            }
            field!(alignment, value, {
                if value > 2 {
                    return Err(invalid("Paragraph alignment must be left, center or right"));
                }
                (0x0c003477, vec![value])
            });
            field!(rtl, value, {
                (0x08003476 | (u32::from(value) << 31), Vec::new())
            });
            macro_rules! spacing {
                ($field:ident, $property:expr) => {
                    field!($field, value, {
                        let stored = value / 36.0;
                        if !stored.is_finite() || !(0.0..=27777.777).contains(&stored) {
                            return Err(invalid("Paragraph spacing is outside the document range"));
                        }
                        ($property, stored.to_le_bytes().to_vec())
                    });
                };
            }
            spacing!(space_before, 0x1400342e);
            spacing!(space_after, 0x1400342f);
            spacing!(line_spacing, 0x14003430);
            if values.is_empty() {
                continue;
            }
            if text.date_field.is_some() {
                return Err(invalid(
                    "Generated title fields cannot be formatted as ordinary text",
                ));
            }
            let (space, object) = (self.space, stored.id);
            self.apply(|image| {
                let store = Store::parse(image)?;
                let index = RevisionIndex::parse(&store)?;
                let document = Document::parse(&index)?;
                let parents = crate::edit::editable_parents(
                    document.active(space)?,
                    &document.pages_in(space)?,
                    object,
                )?;
                let modified = crate::create::current_timestamps()?.0.to_le_bytes();
                crate::write::write_revision(image, space, |raw| {
                    let mut target = PropertyObject::from_object(&raw.objects[&object])?;
                    target.set(
                        &values
                            .iter()
                            .map(|(id, bytes)| (*id, bytes.as_slice()))
                            .collect::<Vec<_>>(),
                    )?;
                    if let Some((_, alignment)) = values.iter().find(|(id, _)| *id == 0x0c003477) {
                        for property in [0x14001c3e, 0x14001c84] {
                            let fields = PropertySets::parse(&target.bytes)?;
                            let previous = fields.sets[0]
                                .iter()
                                .find(|field| field.id == property)
                                .map(|field| match field.value {
                                    Value::Bytes(bytes) => bytes
                                        .try_into()
                                        .map(u32::from_le_bytes)
                                        .map_err(|_| invalid("Invalid paragraph layout alignment")),
                                    _ => Err(invalid("Invalid paragraph layout alignment")),
                                })
                                .transpose()?
                                .unwrap_or(0);
                            let value = (previous & !7) | (u32::from(alignment[0]) + 1);
                            target.set(&[(property, &value.to_le_bytes())])?;
                        }
                    }
                    target.set(&[(0x14001d7a, &modified)])?;
                    let mut changed = BTreeMap::from([(object, target)]);
                    crate::formatting::touch_ancestors(
                        raw,
                        &parents,
                        object,
                        &modified,
                        &mut changed,
                    )?;
                    Ok(changed)
                })
            })?;
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
    // An absent flag and an explicit false are the same formatting.
    macro_rules! boolean {
        ($field:ident, $variant:ident) => {
            if current.$field.unwrap_or(false) != target.$field.unwrap_or(false) {
                out.push(TextAttribute::$variant(target.$field.unwrap_or(false)));
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
    let same_rest = flag(current.math, target.math)
        && flag(current.embedded_object, target.embedded_object)
        && current.alignment.unwrap_or(0) == target.alignment.unwrap_or(0)
        && flag(current.rtl, target.rtl)
        && points(current.space_before, target.space_before)
        && points(current.space_after, target.space_after)
        && points(current.line_spacing, target.line_spacing)
        && points(current.list_spacing, target.list_spacing);
    if !same_rest {
        return Err(invalid(
            "Fields, language and paragraph spacing cannot be edited through the page model",
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
    // Payloads the typed edits embedded travel into the squashed transaction as well.
    let source_store = Store::parse(source)?;
    let declared = |store: &Store<'_>| -> Vec<[u8; 16]> {
        store
            .lists
            .values()
            .flat_map(|list| &list.nodes)
            .filter(|node| node.id == 0x94)
            .filter_map(|node| node.payload.get(..16).and_then(|g| g.try_into().ok()))
            .collect()
    };
    let existing = declared(&source_store);
    let mut payloads = Vec::new();
    for guid in declared(&applied_store) {
        if !existing.contains(&guid) {
            payloads.push((guid, applied_store.file_data(guid)?));
        }
    }
    crate::write::write_revisions_with_payloads(source, &payloads, |index| {
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
                let id = rename.get(id).copied().unwrap_or(*id);
                let mut replacement = match object.data {
                    ObjectData::Properties(_) => {
                        let mut replacement = PropertyObject::from_object(object)?;
                        remap(&mut replacement, &rename)?;
                        replacement
                    }
                    ObjectData::File {
                        reference,
                        extension,
                    } => {
                        let text = |bytes: &[u8]| {
                            String::from_utf16(
                                &bytes
                                    .chunks_exact(2)
                                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                                    .collect::<Vec<_>>(),
                            )
                            .map_err(|_| invalid("Invalid UTF-16 file-data declaration"))
                        };
                        let mut replacement =
                            PropertyObject::file(id, &text(reference)?, &text(extension)?)?;
                        replacement.jcid = object.jcid;
                        replacement
                    }
                    ObjectData::Encrypted(_) => {
                        return Err(invalid("Page edits only produce property objects"));
                    }
                };
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
