//! Publishes an edited page model by lowering the difference from the stored page onto
//! the typed writers, whose revisions accumulate in memory, then squashing them into one
//! revision per space. `PreparedEdit::page` keeps it until the op path replaces it.

use super::{Image, Ink, Outline, Page, PageObject, PageParagraph, ParagraphContent};
use crate::{
    Error, ExGuid, Insertion, ObjectData, OutlineEdit, ParagraphJoin, ParagraphSplit, PropertySets,
    RevisionIndex, Store, TreeEdit, Value,
    active::{ActivePage, Changes},
    document::Kind,
    op::{
        content::{self, AttachmentIds},
        levels,
        lower::{View, collect_containers, kept_set, text_edit, validate},
        properties,
        table::{self, Structure},
    },
    write::{PropertyObject, RevisionEdit},
};
use bumpalo::Bump;
use std::collections::{BTreeMap, BTreeSet};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

fn page_images(page: &Page) -> impl Iterator<Item = (ExGuid, &Image)> {
    page.objects.iter().filter_map(|object| match object {
        PageObject::Image(image) => Some((image.id, image)),
        _ => None,
    })
}

fn page_ink(page: &Page) -> impl Iterator<Item = (ExGuid, &Ink)> {
    page.objects.iter().filter_map(|object| match object {
        PageObject::Ink(ink) => Some((ink.id, ink)),
        _ => None,
    })
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
    let arena = Bump::new();
    let active = ActivePage::parse(&index, space)?;
    let [page] = active.pages[..] else {
        return Err(invalid("Choose an object space containing one active page"));
    };
    let before = Page::from_revision(&active.view, page)?;
    let mut lowering = Lowering {
        active,
        arena: &arena,
        current: Some(before.clone()),
        edited: false,
        page,
        author,
        alias: BTreeMap::new(),
        built: BTreeSet::new(),
    };
    lowering.run(&before, after)?;
    if !lowering.edited {
        return Ok(source.to_vec());
    }
    let active = &lowering.active;
    squash(
        &index,
        &[(space, &active.live.revision)],
        &active.payloads,
        &lowering.alias,
        None,
    )
}

struct Lowering<'a> {
    active: ActivePage<'a>,
    /// Holds the objects the typed writers store for as long as `active` reads them.
    arena: &'a Bump,
    /// The page as `active` stores it, until a writer changes it.
    current: Option<Page>,
    /// Whether a writer stored anything.
    edited: bool,
    page: ExGuid,
    author: &'a str,
    /// Model identities of new objects mapped to the identities the typed writers allocated.
    alias: BTreeMap<ExGuid, ExGuid>,
    /// Tables whose structure this run has written.
    built: BTreeSet<ExGuid>,
}

impl<'a> Lowering<'a> {
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

    /// Stores the next revision a typed writer computes from the page as stored so far.
    fn write(
        &mut self,
        changes: impl FnOnce(&ActivePage<'a>) -> Result<Changes, Error>,
    ) -> Result<(), Error> {
        self.write_with(&[], changes)
    }

    fn write_with(
        &mut self,
        payloads: &[([u8; 16], &[u8])],
        changes: impl FnOnce(&ActivePage<'a>) -> Result<Changes, Error>,
    ) -> Result<(), Error> {
        let changes = changes(&self.active)?;
        if self.active.write(self.arena, payloads, changes)? {
            self.current = None;
            self.edited = true;
        }
        Ok(())
    }

    fn current(&mut self) -> Result<Page, Error> {
        if self.current.is_none() {
            self.current = Some(Page::from_revision(&self.active.view, self.page)?);
        }
        Ok(self.current.clone().unwrap())
    }

    fn run(&mut self, before: &Page, after: &Page) -> Result<(), Error> {
        let old = View::new(before, false)?;
        let new = View::new(after, false)?;
        validate(before, after, &old, &new)?;
        for id in new.outlines.keys().chain(new.paragraphs.keys()) {
            if !old.outlines.contains_key(id)
                && !old.paragraphs.contains_key(id)
                && self.active.live.revision.objects.contains_key(id)
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
        self.ungroup(&old, &new)?;
        self.split_and_join(&old, &new, &mut placed, &mut consumed)?;
        self.place(&old, &new, &placed, &page_order)?;
        self.delete(&old, &new, &consumed)?;
        self.edit_levels(&new)?;
        self.edit_text_identities(&new)?;
        self.edit_equations(&new)?;
        self.edit_text(&new)?;
        self.edit_paragraph_styles(&old, &new, after)?;
        self.edit_lists(after, &new)?;
        self.edit_tags(after, &new)?;
        self.edit_paragraph_formatting(&new)?;
        self.edit_formatting(&new)?;
        self.edit_layout(&old, &new)?;
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
            // A new outline starts as one text paragraph; other content replaces it in
            // its own pass, as for a paragraph inserted into an existing outline.
            let (text, text_id) = match &new.paragraphs[&first].content {
                ParagraphContent::Text(text) if text.date_field.is_none() => {
                    (text.text.text(), Some(text.id))
                }
                ParagraphContent::Table(_)
                | ParagraphContent::Image(_)
                | ParagraphContent::Attachment(_)
                | ParagraphContent::Ink(_) => ("", None),
                _ => {
                    return Err(invalid(
                        "A new outline starts with a paragraph the writer builds",
                    ));
                }
            };
            let insertion = Insertion::outline(self.page, x, y, text, self.author)?;
            self.write(|active| insertion.changes(active))?;
            let outline_id = insertion.object();
            self.alias.insert(*id, outline_id);
            self.alias.insert(
                first,
                ExGuid {
                    guid: outline_id.guid,
                    n: 3,
                },
            );
            if let Some(text_id) = text_id {
                self.alias.insert(text_id, insertion.text_object());
            }
            placed.insert(*id, vec![first]);
            placed.insert(first, Vec::new());
            page_order.push(*id);
        }
        Ok(())
    }

    /// Splits and joins stored paragraphs as OneNote's Enter, Backspace and Delete do. A new
    /// styled paragraph that is no exact split starts as Enter at the end of the nearest stored
    /// paragraph that allows one, taking its properties as OneNote's new paragraphs do; later
    /// passes place it and give it its text.
    fn split_and_join(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        placed: &mut BTreeMap<ExGuid, Vec<ExGuid>>,
        consumed: &mut BTreeSet<ExGuid>,
    ) -> Result<(), Error> {
        // A new paragraph may be split from the stored paragraph nearest before it, whatever
        // placement or pasted paragraphs the same save adds between them.
        let mut splits = Vec::new();
        for document in new.documents() {
            let mut left = None;
            for paragraph in document {
                if old.paragraphs.contains_key(&paragraph.id) {
                    left = paragraph.text().is_some().then_some(paragraph.id);
                } else if let Some(left) = left {
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
            let edit = ParagraphSplit::new(previous.id, offset, self.author)?;
            if self.split(&edit, left, right, old, new, placed)? {
                split.insert(left);
            }
        }
        for document in new.documents() {
            for (at, paragraph) in document.iter().enumerate() {
                if old.paragraphs.contains_key(&paragraph.id)
                    || self.alias.contains_key(&paragraph.id)
                    || paragraph.style.is_none()
                    || paragraph.text().is_none()
                {
                    continue;
                }
                let stored = |candidate: &&&PageParagraph| {
                    old.paragraphs.contains_key(&candidate.id) && candidate.text().is_some()
                };
                let before = document[..at].iter().rev().filter(stored);
                for template in before.chain(document[at + 1..].iter().filter(stored)) {
                    let text = self.active.view.nodes[&template.id].content[0];
                    let Kind::RichText { text: content, .. } = &self.active.view.nodes[&text].kind
                    else {
                        continue;
                    };
                    let end = u32::try_from(content.encode_utf16().count())
                        .map_err(|_| invalid("Paragraph exceeds the UTF-16 offset range"))?;
                    let edit = ParagraphSplit::new(text, end, self.author)?;
                    if self.split(&edit, template.id, paragraph.id, old, new, placed)? {
                        break;
                    }
                }
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
                    continue;
                }
                let join = ParagraphJoin::new(previous.id, removed.id, self.author)?;
                let Ok(changes) = join.changes(&self.active) else {
                    continue;
                };
                self.write(|_| Ok(changes))?;
                consumed.insert(right);
                placed.get_mut(container).unwrap().retain(|id| *id != right);
                placed.remove(&right);
            }
        }
        Ok(())
    }

    /// Stores `edit` of model paragraph `left` as model paragraph `right`, unless the split
    /// cannot be made there.
    fn split(
        &mut self,
        edit: &ParagraphSplit,
        left: ExGuid,
        right: ExGuid,
        old: &View<'_>,
        new: &View<'_>,
        placed: &mut BTreeMap<ExGuid, Vec<ExGuid>>,
    ) -> Result<bool, Error> {
        let Ok(changes) = edit.changes(&self.active) else {
            return Ok(false);
        };
        self.write(|_| Ok(changes))?;
        self.alias.insert(right, edit.object());
        // A text object a join moved into the paragraph is taken over after placement.
        if let Some(text) = new.text(right)
            && !self.active.live.revision.objects.contains_key(&text.id)
        {
            self.alias.insert(text.id, edit.text_object());
        }
        // The split copies the left paragraph's list nodes in order; a list the model shares
        // with a stored paragraph stays that paragraph's.
        if new.paragraphs[&right].lists.len() == old.paragraphs[&left].lists.len() {
            for (n, list) in (5..).zip(&new.paragraphs[&right].lists) {
                if !self.active.live.revision.objects.contains_key(list) {
                    let guid = edit.object().guid;
                    self.alias.insert(*list, ExGuid { guid, n });
                }
            }
        }
        // An earlier split may have carried the left paragraph to its tail.
        let list = placed
            .values_mut()
            .find(|list| list.contains(&left))
            .unwrap();
        let at = list.iter().position(|id| *id == left).unwrap();
        list.insert(at + 1, right);
        placed.insert(right, placed[&left].clone());
        placed.insert(left, Vec::new());
        Ok(true)
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
        self.edit_page_images(old, new)?;
        self.edit_page_ink(old, new)?;
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
                self.write(|active| edit.changes(active))?;
            }
            next = Some(*id);
        }
        let mut containers: Vec<ExGuid> = Vec::new();
        for object in &new.page.objects {
            let outlines: Vec<&Outline> = match object {
                PageObject::Outline(outline) => vec![outline],
                PageObject::Title(title) => title.outlines.iter().collect(),
                PageObject::Image(_) | PageObject::Ink(_) | PageObject::Unsupported(_) => {
                    Vec::new()
                }
            };
            for outline in outlines {
                containers.push(outline.id);
                collect_containers(&outline.paragraphs, &mut containers);
            }
        }
        // Cells that do not exist yet, and containers inside them, receive their
        // paragraphs once the table structure does.
        let new_cell = |id: &ExGuid| {
            !(old.children.contains_key(id)
                || new.paragraphs.contains_key(id)
                || new.outlines.contains_key(id))
        };
        let (existing, deferred): (Vec<ExGuid>, Vec<ExGuid>) =
            containers.into_iter().partition(|container| {
                let mut at = *container;
                loop {
                    if new_cell(&at) {
                        return false;
                    }
                    match new.container.get(&at) {
                        Some(parent) => at = *parent,
                        None => return true,
                    }
                }
            });
        self.place_containers(old, new, placed, &existing)?;
        // Each round builds the tables whose holders exist and fills their cells, which
        // may hold further new tables.
        let mut pending = deferred;
        loop {
            self.edit_table_structure(old, new)?;
            if pending.is_empty() {
                break;
            }
            let (ready, waiting): (Vec<ExGuid>, Vec<ExGuid>) =
                pending.into_iter().partition(|container| {
                    self.alias.contains_key(container) || old.children.contains_key(container)
                });
            if ready.is_empty() {
                return Err(invalid("Table content has no table to hold it"));
            }
            self.place_containers(old, new, placed, &ready)?;
            pending = waiting;
        }
        self.edit_cells(old, new)?;
        self.edit_images(old, new)?;
        self.edit_ink_paragraphs(old, new)?;
        self.edit_attachments(old, new)?;
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
                let ParagraphContent::Attachment(stored) = &previous.content else {
                    return Err(invalid("Paragraph content type cannot change"));
                };
                if stored != attachment {
                    self.write(|active| content::attachment_edit_changes(active, stored, attachment))?;
                }
                continue;
            }
            if attachment.recording.is_some() {
                return Err(invalid("Recordings are captured by OneNote, not inserted"));
            }
            let Some(bytes) = &attachment.bytes else {
                return Err(invalid("A new attachment needs its payload"));
            };
            let ids = AttachmentIds {
                object: self.allocate(attachment.id)?,
                file: ExGuid {
                    guid: crate::write::fresh_guid()?,
                    n: 1,
                },
                payload: crate::write::fresh_guid()?,
                preview: match &attachment.preview {
                    Some(_) => {
                        let payload = crate::write::fresh_guid()?;
                        let icon = ExGuid {
                            guid: crate::write::fresh_guid()?,
                            n: 1,
                        };
                        Some((payload, icon))
                    }
                    None => None,
                },
            };
            let mut payloads: Vec<([u8; 16], &[u8])> = vec![(ids.payload, bytes)];
            if let (Some((payload, _)), Some(icon)) = (ids.preview, &attachment.preview) {
                payloads.push((payload, icon));
            }
            let holder = self.id(*paragraph_id);
            self.write_with(&payloads, |active| {
                content::attachment_changes(active, attachment, &ids, holder)
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
                let ParagraphContent::Image(stored) = &previous.content else {
                    return Err(invalid("Paragraph content type cannot change"));
                };
                if stored != image {
                    self.edit_image(stored, image)?;
                }
                continue;
            }
            if image.layout.x.is_some() || image.layout.y.is_some() {
                return Err(invalid("A paragraph picture has no position of its own"));
            }
            let holder = self.id(*paragraph_id);
            self.insert_image(image, Some(holder))?;
        }
        Ok(())
    }

    /// Page-level pictures are direct page children, as OneNote stores a picture placed
    /// outside any outline: new ones are appended for the placement pass to order, changed
    /// ones are moved, resized or described, and removed ones are deleted in the delete pass.
    fn edit_page_images(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let stored: BTreeMap<ExGuid, &Image> = page_images(old.page).collect();
        for (id, image) in page_images(new.page) {
            match stored.get(&id) {
                Some(previous) => {
                    if *previous != image {
                        self.edit_image(previous, image)?;
                    }
                }
                None => {
                    if image.layout.x.is_none() || image.layout.y.is_none() {
                        return Err(invalid("A new page-level picture needs a position"));
                    }
                    self.insert_image(image, None)?;
                }
            }
        }
        Ok(())
    }

    /// Ink placed on the page: new drawings are appended for the placement pass to order,
    /// changed ones have their stroke list rewritten, removed ones go in the delete pass.
    fn edit_page_ink(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let stored: BTreeMap<ExGuid, &Ink> = page_ink(old.page).collect();
        for (id, ink) in page_ink(new.page) {
            match stored.get(&id) {
                Some(previous) => {
                    if *previous != ink {
                        self.edit_ink(previous, ink)?;
                    }
                }
                None => self.insert_ink(ink, None)?,
            }
        }
        Ok(())
    }

    fn edit_ink_paragraphs(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        for (paragraph_id, paragraph) in &new.paragraphs {
            let ParagraphContent::Ink(ink) = &paragraph.content else {
                continue;
            };
            match old.paragraphs.get(paragraph_id) {
                Some(previous) => {
                    let ParagraphContent::Ink(stored) = &previous.content else {
                        return Err(invalid("Paragraph content type cannot change"));
                    };
                    if stored != ink {
                        self.edit_ink(stored, ink)?;
                    }
                }
                None => {
                    let holder = self.id(*paragraph_id);
                    self.insert_ink(ink, Some(holder))?;
                }
            }
        }
        Ok(())
    }

    /// Writes new ink the way OneNote 2010 stores a drawing: a container the page lists as a
    /// child (or a paragraph holds as content), its data node listing stroke objects, each
    /// stroke's packet and half-inch origin, and one drawing-attribute object per distinct pen.
    fn insert_ink(&mut self, ink: &Ink, holder: Option<ExGuid>) -> Result<(), Error> {
        let container = self.allocate(ink.id)?;
        let data = ExGuid {
            guid: crate::write::fresh_guid()?,
            n: 1,
        };
        let mut strokes = Vec::new();
        for stroke in &ink.strokes {
            strokes.push((self.allocate(stroke.id)?, stroke));
        }
        self.write(|active| content::ink_changes(active, ink, container, data, &strokes, holder))
    }

    /// Rewrites a drawing's stroke list: stored strokes stay as they are (OneNote erases whole
    /// strokes rather than editing them), removed ones leave the list, new ones are created.
    fn edit_ink(&mut self, stored: &Ink, ink: &Ink) -> Result<(), Error> {
        if stored.id != ink.id {
            return Err(invalid("Ink identity cannot change"));
        }
        if stored.layout != ink.layout || stored.groups != ink.groups {
            return Err(invalid("Ink position and groups stay as stored"));
        }
        let mut kept = Vec::new();
        let mut added = Vec::new();
        for stroke in &ink.strokes {
            match stored.strokes.iter().find(|s| s.id == stroke.id) {
                Some(previous) if previous == stroke => kept.push(self.id(stroke.id)),
                Some(_) => return Err(invalid("A stored stroke keeps its path and pen")),
                None => added.push((self.allocate(stroke.id)?, stroke)),
            }
        }
        let container = self.id(ink.id);
        self.write(|active| content::strokes_changes(active, container, &kept, &added))
    }

    /// Gives a new picture what OneNote stores for an inserted one: the payload embedded
    /// in the section's file-data store, a file-data object declaring it by identity and
    /// extension, and a picture object that a paragraph holds as content or the page
    /// lists as a child.
    fn insert_image(&mut self, image: &Image, holder: Option<ExGuid>) -> Result<(), Error> {
        let Some(bytes) = &image.bytes else {
            return Err(invalid("A new picture needs its payload"));
        };
        let image_id = self.allocate(image.id)?;
        let file_id = ExGuid {
            guid: crate::write::fresh_guid()?,
            n: 1,
        };
        let payload_guid = crate::write::fresh_guid()?;
        let payload: &[u8] = bytes;
        self.write_with(&[(payload_guid, payload)], |active| {
            content::picture_changes(active, image, image_id, file_id, payload_guid, holder)
        })
    }

    fn edit_image(&mut self, stored: &Image, image: &Image) -> Result<(), Error> {
        content::picture_fixed_fields(stored, image)?;
        let object = self.id(image.id);
        self.write(|active| {
            content::picture_edit_changes(
                active,
                object,
                (&stored.layout, &stored.alt),
                &image.layout,
                &image.alt,
            )
        })
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
                    // A styled paragraph appended to a stored outline is a split whose
                    // precondition failed under a merge; it stays for review. A new outline
                    // takes styled paragraphs as built (a copied page).
                    let mut root = *container;
                    while let Some(parent) = new.container.get(&root) {
                        root = *parent;
                    }
                    if paragraph.style.is_some() && old.outlines.contains_key(&root) {
                        return Err(invalid(
                            "New paragraphs in a stored outline contain plain text without styles",
                        ));
                    }
                    if paragraph.media != Default::default() {
                        return Err(invalid(
                            "Recording annotations are made by OneNote while it records",
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
                        | ParagraphContent::Attachment(_)
                        | ParagraphContent::Ink(_) => ("", None),
                        _ => {
                            return Err(invalid(
                                "New paragraphs contain plain text without fields or styles",
                            ));
                        }
                    };
                    let insertion =
                        Insertion::paragraph(self.id(*container), anchor, text, self.author)?;
                    self.write(|active| insertion.changes(active))?;
                    self.alias.insert(*id, insertion.object());
                    if let Some(text_id) =
                        text_id.filter(|id| !self.active.live.revision.objects.contains_key(id))
                    {
                        self.alias.insert(text_id, insertion.text_object());
                    }
                } else if !kept.contains(id) {
                    let edit =
                        TreeEdit::move_to(self.id(*id), self.id(*container), anchor, self.author)?;
                    self.write(|active| edit.changes(active))?;
                }
                next = Some(*id);
            }
        }
        Ok(())
    }

    /// Changes the shading and indents of stored cells in place.
    fn edit_cells(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let stored: BTreeMap<ExGuid, &super::TableCell> = old
            .paragraphs
            .values()
            .filter_map(|paragraph| match &paragraph.content {
                ParagraphContent::Table(table) => Some(table),
                _ => None,
            })
            .flat_map(|table| table.rows.iter().flat_map(|row| &row.cells))
            .map(|cell| (cell.id, cell))
            .collect();
        for paragraph in new.paragraphs.values() {
            let ParagraphContent::Table(table) = &paragraph.content else {
                continue;
            };
            for cell in table.rows.iter().flat_map(|row| &row.cells) {
                let Some(previous) = stored.get(&cell.id) else {
                    continue;
                };
                if (cell.shading, &cell.indents) == (previous.shading, &previous.indents) {
                    continue;
                }
                let object = self.id(cell.id);
                self.write(|active| {
                    table::cell_changes(
                        active,
                        object,
                        (previous.shading, &previous.indents),
                        cell.shading,
                        &cell.indents,
                    )
                })?;
            }
        }
        Ok(())
    }

    /// Creates tables, rows and cells the model added, rebuilds every table's row and cell
    /// order to the model's, and writes the column widths, locks and border flag.
    fn edit_table_structure(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let old_tables: BTreeMap<ExGuid, &super::Table> = old
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
            // A table is built once its holder is placed, and only once.
            if !(old.paragraphs.contains_key(paragraph_id) || self.alias.contains_key(paragraph_id))
                || self.built.contains(&table.id)
            {
                continue;
            }
            self.built.insert(table.id);
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
                .and_then(|r| r.cells.first())
                .map(|cell| cell.indents.as_slice());
            let mut structure = Structure {
                rows: Vec::new(),
                new_rows: BTreeSet::new(),
                new_cells: BTreeMap::new(),
                columns: &table.columns,
                borders: table.borders,
            };
            for row in &table.rows {
                let row_id = if existing_rows.contains(&row.id) {
                    self.id(row.id)
                } else {
                    let written = self.allocate(row.id)?;
                    structure.new_rows.insert(written);
                    written
                };
                let mut cells = Vec::new();
                for cell in &row.cells {
                    let cell_id = if existing_cells.contains(&cell.id) {
                        self.id(cell.id)
                    } else {
                        let written = self.allocate(cell.id)?;
                        structure.new_cells.insert(
                            written,
                            (table::cell_indents(&cell.indents, template), cell.shading),
                        );
                        written
                    };
                    cells.push(cell_id);
                }
                structure.rows.push((row_id, cells));
            }
            let holder = previous.is_none().then(|| self.id(*paragraph_id));
            self.write(|active| table::table_changes(active, table_id, holder, &structure))?;
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
            // A surviving ancestor is placed where the model says, carrying this paragraph along.
            while !new.paragraphs.contains_key(&ancestor) {
                if removed_outline(ancestor)
                    || (old.paragraphs.contains_key(&ancestor) && !consumed.contains(&ancestor))
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
            self.write(|active| edit.changes(active))?;
        }
        let kept: BTreeSet<ExGuid> = page_images(new.page)
            .map(|(id, _)| id)
            .chain(page_ink(new.page).map(|(id, _)| id))
            .collect();
        for id in page_images(old.page)
            .map(|(id, _)| id)
            .chain(page_ink(old.page).map(|(id, _)| id))
        {
            if !kept.contains(&id) {
                let edit = TreeEdit::delete(id, self.author)?;
                self.write(|active| edit.changes(active))?;
            }
        }
        for id in old.outlines.keys() {
            if removed_outline(*id) {
                if old.title_outlines.contains(id) {
                    return Err(invalid("Title outlines cannot be removed"));
                }
                let edit = TreeEdit::delete(*id, self.author)?;
                self.write(|active| edit.changes(active))?;
            }
        }
        Ok(())
    }

    /// Rewrites every equation paragraph whose stored text object differs from the model.
    fn edit_equations(&mut self, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current, false)?;
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let stored = current.text(self.id(*id));
            // Deleting an equation leaves no math in the model, but its stored runs are still math.
            if !super::Math::is_equation(&text.text)
                && !stored.is_some_and(|stored| super::Math::is_equation(&stored.text))
            {
                continue;
            }
            let stored = stored
                .ok_or_else(|| invalid("An equation paragraph is missing after placement"))?;
            if stored.text == text.text {
                continue;
            }
            let object = stored.id;
            self.write(|active| content::equation_changes(active, object, &text.text))?;
        }
        Ok(())
    }

    /// Lifts the paragraphs of outline groups into their container wherever this save changes
    /// the container's children or their depths, for placement to see them; `edit_levels`
    /// groups them again.
    fn ungroup(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        for (container, children) in &old.children {
            let Some(after) = new.children.get(container) else {
                continue;
            };
            let grouped = self.active.view.nodes[container]
                .children
                .iter()
                .any(|id| matches!(self.active.view.nodes[id].kind, Kind::OutlineGroup));
            if !grouped
                || (after == children
                    && old.depths(*container, children) == new.depths(*container, after))
            {
                continue;
            }
            let object = *container;
            self.write(|active| levels::ungroup_changes(active, object))?;
        }
        Ok(())
    }

    /// Indents each container's children to the depths the model gives them.
    fn edit_levels(&mut self, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let stored = View::new(&current, false)?;
        for (container, children) in &new.children {
            let depths = new
                .depths(*container, children)
                .ok_or_else(|| invalid("A paragraph lies no deeper than its parent"))?;
            let object = self.id(*container);
            let written: Vec<ExGuid> = children.iter().map(|id| self.id(*id)).collect();
            if children.is_empty() || stored.depths(object, &written) == Some(depths.clone()) {
                continue;
            }
            let is_cell =
                !new.paragraphs.contains_key(container) && !new.outlines.contains_key(container);
            let (level, runs) = levels::runs(&written, &depths, is_cell)?;
            self.write(|active| levels::regroup_changes(active, object, level, &runs))?;
        }
        Ok(())
    }

    /// Gives a paragraph the text object its model names when a split and a join in one save
    /// moved text between paragraphs: a stored object is taken over, as OneNote moves a lower
    /// paragraph's text into an emptied upper one; a new one starts as a copy of the text
    /// the paragraph holds.
    fn edit_text_identities(&mut self, new: &View<'_>) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let object = self.id(*id);
            let stored = self.active.view.nodes[&object].content[0];
            if stored == self.id(text.id) {
                continue;
            }
            let target = if self.active.live.revision.objects.contains_key(&text.id) {
                text.id
            } else {
                self.allocate(text.id)?
            };
            let unstyled = paragraph.style.is_none();
            self.write(|active| {
                let parents = active.editable_parents(object)?;
                let modified = crate::create::current_timestamps()?.0.to_le_bytes();
                let raw = &active.live.revision;
                let mut changed = BTreeMap::new();
                if !raw.objects.contains_key(&target) {
                    let mut copy = PropertyObject::from_object(&raw.objects[&stored])?;
                    copy.remove(&[0x1c001c98, 0x14001c99])?;
                    if unstyled {
                        copy.remove(&[0x2000342c])?;
                    }
                    copy.reference(target)?;
                    changed.insert(target, copy);
                }
                let mut holder = PropertyObject::from_object(&raw.objects[&object])?;
                let reference = holder.reference(target)?;
                holder.set(&[(0x24001c1f, &reference), (0x14001d7a, &modified)])?;
                changed.insert(object, holder);
                crate::formatting::touch_ancestors(raw, parents, object, &modified, &mut changed)?;
                Ok(changed)
            })?;
        }
        Ok(())
    }

    fn edit_text(&mut self, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current, false)?;
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
            let object = stored.id;
            self.write(|active| crate::edit::text_changes(active, object, range, &replacement))?;
        }
        Ok(())
    }

    /// Gives each paragraph the list nodes its model references: a definition new to the
    /// section becomes a list node carrying the model's identity, a changed one is rewritten
    /// in place, a definition another paragraph already owns is copied because native list
    /// nodes belong to one paragraph, and a dropped reference leaves the node unreferenced.
    fn edit_lists(&mut self, after: &Page, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let stored = View::new(&current, false)?;
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
                        self.id(*model) == *node
                            && after.definitions.get(model) == current.definitions.get(node)
                    });
            if same {
                continue;
            }
            let mut nodes = Vec::new();
            for list in &paragraph.lists {
                // Writer-allocated identities keep a nonzero sequence number so the node's
                // compact identity is never the null identity; squash renames them to the
                // model's. A copy for a second owner keeps its allocated identity.
                let node_id = match owners.get(&self.id(*list)) {
                    Some(owner) if *owner == image_id => self.id(*list),
                    Some(_) => ExGuid {
                        guid: crate::write::fresh_guid()?,
                        n: 1,
                    },
                    // A node its paragraph let go of in this save serves the model again.
                    None if self.active.live.revision.objects.contains_key(list) => *list,
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
                nodes.push((node_id, properties::list_values(definition)?));
            }
            let object = self.id(*id);
            self.write(|active| properties::list_changes(active, object, &nodes))?;
        }
        Ok(())
    }

    /// Rewrites the note tags of paragraphs and text objects whose model tags differ from
    /// the stored ones. A tag definition new to the section becomes a definition object
    /// carrying the model's identity after squash.
    fn edit_tags(&mut self, after: &Page, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let stored = View::new(&current, false)?;
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
                if crate::op::lower::same_tags(tags, stored_tags) {
                    continue;
                }
                let mut entries = Vec::new();
                for tag in tags {
                    let definition = tag
                        .definition
                        .ok_or_else(|| invalid("A note tag names its definition"))?;
                    let known = self.active.live.revision.objects.contains_key(&definition)
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
                    entries.push((written, tag, after.definitions.get(&definition)));
                }
                self.write(|active| properties::tag_changes(active, object, &entries))?;
            }
        }
        Ok(())
    }

    /// A new paragraph, or one whose style changed, references its paragraph style from
    /// its text object; a style the page has not stored yet is created from its
    /// definition, as OneNote keeps quick styles.
    fn edit_paragraph_styles(
        &mut self,
        old: &View<'_>,
        new: &View<'_>,
        after: &Page,
    ) -> Result<(), Error> {
        for (id, paragraph) in &new.paragraphs {
            let Some(definition) = paragraph.style else {
                continue;
            };
            if old
                .paragraphs
                .get(id)
                .is_some_and(|previous| previous.style == paragraph.style)
            {
                continue;
            }
            let Some(text) = paragraph.text() else {
                return Err(invalid("Only text paragraphs take a paragraph style"));
            };
            let known = self.active.live.revision.objects.contains_key(&definition)
                || self.alias.contains_key(&definition);
            let style_id = if known {
                self.id(definition)
            } else {
                let allocated = ExGuid {
                    guid: crate::write::fresh_guid()?,
                    n: 1,
                };
                self.alias.insert(definition, allocated);
                allocated
            };
            let object = self.id(text.id);
            let definition = after.definitions.get(&definition);
            self.write(|active| properties::style_changes(active, object, style_id, definition))?;
        }
        Ok(())
    }

    fn edit_paragraph_formatting(&mut self, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current, false)?;
        for (id, paragraph) in &new.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            let stored = current
                .text(self.id(*id))
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            let values = crate::op::lower::paragraph_change(&stored.text, &text.text)?;
            if values.is_empty() {
                continue;
            }
            if text.date_field.is_some() {
                return Err(invalid(
                    "Generated title fields cannot be formatted as ordinary text",
                ));
            }
            let object = stored.id;
            self.write(|active| properties::paragraph_format_changes(active, object, &values))?;
        }
        Ok(())
    }

    fn edit_formatting(&mut self, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current, false)?;
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
            for (range, set, clear) in crate::op::lower::format_edits(&stored.text, &text.text, fresh)? {
                let object = stored.id;
                self.write(|active| {
                    crate::formatting::format_changes(active, object, range, &set, &clear)
                })?;
            }
        }
        Ok(())
    }

    fn edit_layout(&mut self, old: &View<'_>, new: &View<'_>) -> Result<(), Error> {
        let current = self.current()?;
        let current = View::new(&current, false)?;
        for (id, paragraph) in &new.paragraphs {
            let stored = current
                .paragraphs
                .get(&self.id(*id))
                .ok_or_else(|| invalid("A paragraph is missing after text edits"))?;
            if stored.collapsed != paragraph.collapsed {
                let object = self.id(*id);
                let edit = OutlineEdit::Collapsed(paragraph.collapsed);
                self.write(|active| edit.changes(active, object))?;
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
            let object = self.id(*id);
            // A new outline takes the model's indentation table (a copied outline keeps
            // its levels' offsets); a stored table stays as it is.
            if !old.outlines.contains_key(id)
                && !outline.indents.is_empty()
                && outline.indents != stored.indents
            {
                let indents = content::measurement_bytes(&outline.indents, 4)?;
                self.write(|active| {
                    let raw = &active.live.revision;
                    let mut node = PropertyObject::from_object(&raw.objects[&object])?;
                    node.set(&[(0x1c001c12, &indents)])?;
                    Ok(BTreeMap::from([(object, node)]))
                })?;
            }
            if (outline.layout.x, outline.layout.y) != (stored.layout.x, stored.layout.y) {
                let (Some(x), Some(y)) = (outline.layout.x, outline.layout.y) else {
                    return Err(invalid("An outline position needs both coordinates"));
                };
                let edit = OutlineEdit::Position { x, y };
                self.write(|active| edit.changes(active, object))?;
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
                self.write(|active| edit.changes(active, object))?;
            }
        }
        Ok(())
    }
}

/// Payload identities the file-data store of `store` declares, in order.
pub(crate) fn declared_payloads(store: &Store<'_>) -> Vec<[u8; 16]> {
    store
        .lists
        .values()
        .flat_map(|list| &list.nodes)
        .filter(|node| node.id == 0x94)
        .filter_map(|node| node.payload.get(..16).and_then(|g| g.try_into().ok()))
        .collect()
}

/// Writes the spaces the typed writers `edited` as one transaction on the validated
/// `source`, renaming writer-allocated identities to the model's and embedding the
/// `payloads` it lacks. A protected `source` takes the revisions its plaintext twin gained.
pub(crate) fn squash(
    source: &RevisionIndex<'_>,
    edited: &[(ExGuid, &crate::ResolvedRevision<'_>)],
    payloads: &[([u8; 16], &[u8])],
    alias: &BTreeMap<ExGuid, ExGuid>,
    protection: Option<&dyn crate::write::Protection>,
) -> Result<Vec<u8>, Error> {
    let rename: BTreeMap<ExGuid, ExGuid> = alias
        .iter()
        .map(|(model, image)| (*image, *model))
        .collect();
    let existing = declared_payloads(source.store);
    let payloads: Vec<_> = payloads
        .iter()
        .filter(|(guid, _)| !existing.contains(guid))
        .copied()
        .collect();
    let edit = |index: &RevisionIndex<'_>| {
        let mut changes = BTreeMap::new();
        for (sid, after) in edited {
            let before = match protection {
                Some(protection) => protection.resolve(*sid, index.active(*sid)?)?,
                None => index.resolve_active(*sid)?,
            };
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
                // Stored tables keep the entries an object names, so those decide.
                if let Some(previous) = before.objects.get(id)
                    && previous.jcid == object.jcid
                    && previous.data == object.data
                {
                    let entries = match object.data {
                        ObjectData::Properties(bytes) => crate::write::table_entries(bytes)?,
                        _ => Vec::new(),
                    };
                    if entries
                        .iter()
                        .all(|entry| previous.global_ids.get(entry) == object.global_ids.get(entry))
                    {
                        continue;
                    }
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
    };
    let output = crate::write::build_on(source, &payloads, protection, edit)?;
    crate::write::check(&output, protection.is_none())?;
    Ok(output)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        document::Format,
        page::{PageParagraph, Paragraph, TextObject, text::new_id},
    };

    /// A new page in a new section and the page with `count` paragraphs added in an outline.
    fn generated(count: usize) -> (Vec<u8>, ExGuid, Page) {
        let section = crate::create_section("big.one", "First", "Author").unwrap();
        let creation = crate::PageCreation::new(None, Some("Big"), "Author").unwrap();
        let source = crate::PreparedEdit::create_page(&section, &creation)
            .unwrap()
            .as_bytes()
            .to_vec();
        let space = creation.space();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let mut page =
            Page::from_space(&crate::document::Document::parse(&index).unwrap(), space).unwrap();
        let paragraph = |i: usize| PageParagraph {
            id: new_id().unwrap(),
            parent: None,
            level: 1,
            style: None,
            format: Format::default(),
            content: ParagraphContent::Text(TextObject {
                id: new_id().unwrap(),
                date_field: None,
                text: Paragraph::new(format!("Paragraph {i}"), Format::default()),
                tags: Vec::new(),
            }),
            lists: Vec::new(),
            tags: Vec::new(),
            media: Default::default(),
            collapsed: false,
        };
        page.objects.push(PageObject::Outline(Outline {
            id: new_id().unwrap(),
            title: false,
            min_width: None,
            layout: crate::document::Layout {
                x: Some(36.0),
                y: Some(86.4),
                ..Default::default()
            },
            indents: Vec::new(),
            paragraphs: (0..count).map(paragraph).collect(),
            unsupported: Vec::new(),
        }));
        (source, space, page)
    }

    #[test]
    fn a_page_write_builds_one_revision_however_many_paragraphs_it_adds() {
        for count in [10, 40] {
            let (source, space, after) = generated(count);
            let builds = crate::write::BUILDS.with(std::cell::Cell::get);
            let written = write_page(&source, space, &after, "Author").unwrap();
            assert_eq!(crate::write::BUILDS.with(std::cell::Cell::get) - builds, 1);
            let store = Store::parse(&written).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let read = Page::from_space(&crate::document::Document::parse(&index).unwrap(), space)
                .unwrap();
            let texts = |page: &Page| -> Vec<String> {
                page.objects
                    .iter()
                    .filter_map(|object| match object {
                        PageObject::Outline(outline) => Some(outline),
                        _ => None,
                    })
                    .flat_map(|outline| &outline.paragraphs)
                    .filter_map(|paragraph| Some(paragraph.text()?.text.text().to_owned()))
                    .collect()
            };
            assert_eq!(texts(&read), texts(&after));
        }
    }
}
