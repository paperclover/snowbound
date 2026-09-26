//! `Section::apply`: each op checked against the page it names, then written through the
//! typed writers onto the section's copy of that page.

use super::{
    Edit, Op, OpError, PageOp, SectionOp, TableEdit, TextProperty,
    content::{self, AttachmentIds},
    levels,
    lower::{ParagraphFields, format_edits, paragraph_fields},
    properties,
    table::{self, Structure},
};
use crate::{
    Error, ExGuid, Insertion, OutlineEdit, ParagraphJoin, ParagraphSplit, Section, TextAttribute,
    TreeEdit,
    active::{ActivePage, Changes},
    document::Kind,
    page::{
        Attachment, Image, Ink, Page, PageObject, PageParagraph, Paragraph, ParagraphContent,
        TableCell, TableColumn,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

impl<'a> Section<'a> {
    /// Applies an edit whole or not at all, `author` naming who made it. Its modification
    /// times are `edit.at`.
    pub fn apply(&mut self, author: &str, edit: &Edit) -> Result<(), OpError> {
        if author.contains('\0') {
            return Err(OpError::Unsupported("Choose an author name without NUL"));
        }
        let undo = edit.ops.len() > 1 || edit.ops.iter().any(stores_more_than_once);
        crate::create::at(edit.at, || {
            self.atomically(undo, |section| {
                for op in &edit.ops {
                    match op {
                        Op::Page { space, op } => Writer {
                            section: &mut *section,
                            space: *space,
                            author,
                        }
                        .apply(op)?,
                        Op::Section(op) => section.apply_section(author, op)?,
                    }
                }
                Ok(())
            })
        })
        .map_err(|error| self.classify(error))
    }

    /// Lowers `after` against the page `space` holds and applies the result as one edit;
    /// returns the ops, which reach `after` where the writers can store it.
    pub fn apply_page(
        &mut self,
        author: &str,
        at: u64,
        space: ExGuid,
        after: &Page,
    ) -> Result<Vec<PageOp>, OpError> {
        let before = self.page(space).map_err(|error| self.classify(Failure::Rejected(error)))?;
        let ops = super::lower_page(&before, after)
            .map_err(|error| OpError::Unsupported(error.message))?;
        let edit = Edit {
            at,
            ops: ops
                .iter()
                .map(|op| Op::Page {
                    space,
                    op: op.clone(),
                })
                .collect(),
        };
        self.apply(author, &edit)?;
        Ok(ops)
    }

    fn classify(&self, failure: Failure) -> OpError {
        match failure {
            Failure::Refused(error) => error,
            Failure::Rejected(error) if self.broken() => OpError::Failed(error),
            Failure::Rejected(error) => OpError::Unsupported(error.message),
        }
    }

    fn apply_section(&mut self, author: &str, op: &SectionOp) -> Result<(), Failure> {
        match op {
            SectionOp::Create(creation) => {
                let (root, roots, objects) = creation.changes(self)?;
                self.apply_changes(self.root(), &[], root)?;
                self.create_space(creation.space(), roots, objects)?;
            }
            SectionOp::Import { creation, page } => {
                self.apply_section(author, &SectionOp::Create(creation.clone()))?;
                let space = creation.space();
                let before = self.page(space)?;
                let mut after = page.clone();
                // The created page's title stays; the imported page brings the rest.
                after.objects.retain(|object| !matches!(object, PageObject::Title(_)));
                for object in before.objects.iter().rev() {
                    if let PageObject::Title(_) = object {
                        after.objects.insert(0, object.clone());
                    }
                }
                after.title = before.title.clone();
                after.identity = before.identity;
                after.created = before.created;
                after.margin_origin = before.margin_origin;
                for op in super::lower_page(&before, &after)? {
                    Writer {
                        section: self,
                        space,
                        author,
                    }
                    .apply(&op)?;
                }
            }
            SectionOp::Pages(edits) => {
                for (space, changes) in crate::pages::section_changes(self, edits, &[])? {
                    self.apply_changes(space, &[], changes)?;
                }
            }
            SectionOp::Delete(pages) => {
                for (space, changes) in crate::pages::section_changes(self, &[], pages)? {
                    self.apply_changes(space, &[], changes)?;
                }
            }
        }
        Ok(())
    }
}

/// Whether an op may store more than one revision of a page, so its failure part way needs
/// the page as it was.
fn stores_more_than_once(op: &Op) -> bool {
    match op {
        Op::Section(_) => true,
        Op::Page { op, .. } => matches!(
            op,
            PageOp::Link { .. }
                | PageOp::Insert { .. }
                | PageOp::Move { .. }
                | PageOp::Add { .. }
                | PageOp::Table {
                    edit: TableEdit::Rows { .. } | TableEdit::Column { .. },
                    ..
                }
        ),
    }
}

/// Why an op was not applied: a precondition it names, or what a writer refused.
enum Failure {
    Refused(OpError),
    Rejected(Error),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Rejected(error)
    }
}

impl From<OpError> for Failure {
    fn from(error: OpError) -> Self {
        Self::Refused(error)
    }
}

/// Writes ops onto one page of a section.
struct Writer<'s, 'a> {
    section: &'s mut Section<'a>,
    space: ExGuid,
    author: &'s str,
}

impl<'a> Writer<'_, 'a> {
    fn page(&mut self) -> Result<&ActivePage<'a>, Error> {
        self.section.active(self.space)
    }

    fn write(
        &mut self,
        changes: impl FnOnce(&ActivePage<'a>) -> Result<Changes, Error>,
    ) -> Result<(), Failure> {
        self.write_with(&[], changes)
    }

    fn write_with(
        &mut self,
        payloads: &[([u8; 16], &[u8])],
        changes: impl FnOnce(&ActivePage<'a>) -> Result<Changes, Error>,
    ) -> Result<(), Failure> {
        let changes = changes(self.page()?)?;
        self.section.apply_changes(self.space, payloads, changes)?;
        Ok(())
    }

    /// Requires `id` reachable on the page.
    fn target(&mut self, id: ExGuid) -> Result<(), Failure> {
        let page = self.page()?;
        if page.live.is_reachable(id) && page.view.nodes.contains_key(&id) {
            Ok(())
        } else {
            Err(OpError::TargetUnavailable(id).into())
        }
    }

    /// Requires `id` free for a new object: not reachable on the page.
    fn free(&mut self, id: ExGuid) -> Result<(), Failure> {
        if id.guid == [0; 16] || self.page()?.live.is_reachable(id) {
            Err(OpError::DuplicateIdentity(id).into())
        } else {
            Ok(())
        }
    }

    /// The only parent of `id` on the page.
    fn parent(&mut self, id: ExGuid) -> Result<ExGuid, Failure> {
        match self.page()?.parents.get(&id).map(Vec::as_slice) {
            Some([parent]) => Ok(*parent),
            _ => Err(OpError::StructureChanged("Page content must have one parent").into()),
        }
    }

    /// The container `id` lies in, outline groups passed over.
    fn container(&mut self, id: ExGuid) -> Result<ExGuid, Failure> {
        let mut at = self.parent(id)?;
        while matches!(self.page()?.view.nodes[&at].kind, Kind::OutlineGroup) {
            at = self.parent(at)?;
        }
        Ok(at)
    }

    /// The text object a paragraph holds.
    fn text_of(&mut self, paragraph: ExGuid) -> Result<ExGuid, Failure> {
        let page = self.page()?;
        let node = &page.view.nodes[&paragraph];
        match node.content.as_slice() {
            [text] if matches!(page.view.nodes[text].kind, Kind::RichText { .. }) => Ok(*text),
            _ => Err(OpError::Unsupported("Select a paragraph holding text").into()),
        }
    }

    /// The absolute outline level of paragraph `id`; zero for an outline or cell.
    fn level(&mut self, id: ExGuid) -> Result<u32, Failure> {
        if !matches!(self.page()?.view.nodes[&id].kind, Kind::Paragraph { .. }) {
            return Ok(0);
        }
        let container = self.container(id)?;
        let depth = self.depth(container, id)?;
        Ok(self.level(container)? + depth)
    }

    fn depth(&mut self, container: ExGuid, id: ExGuid) -> Result<u32, Failure> {
        levels::stored_depths(&self.page()?.view, container)
            .into_iter()
            .find(|(child, _)| *child == id)
            .map(|(_, depth)| depth)
            .ok_or_else(|| OpError::StructureChanged("A paragraph left its container").into())
    }

    /// Stores `container`'s children at the depths they have, `targets` overriding some.
    fn regroup(&mut self, container: ExGuid, targets: &[(ExGuid, u32)]) -> Result<(), Failure> {
        let page = self.page()?;
        let current = levels::stored_depths(&page.view, container);
        let is_cell = matches!(page.view.nodes[&container].kind, Kind::Cell { .. });
        let (children, depths): (Vec<ExGuid>, Vec<u32>) = current
            .iter()
            .map(|(child, depth)| {
                let target = targets
                    .iter()
                    .find(|(id, _)| id == child)
                    .map_or(*depth, |(_, depth)| *depth);
                (*child, target)
            })
            .unzip();
        if current.iter().map(|(_, depth)| *depth).eq(depths.iter().copied()) {
            return Ok(());
        }
        let (level, runs) = levels::runs(&children, &depths, is_cell)?;
        self.write(|page| levels::regroup_changes(page, container, level, &runs))
    }

    fn apply(&mut self, op: &PageOp) -> Result<(), Failure> {
        match op {
            PageOp::Text { text, range, with } => {
                self.target(*text)?;
                if with.contains(['\0', '\n', '\u{fffc}']) {
                    return Err(OpError::Unsupported("Use ordinary paragraph text").into());
                }
                self.write(|page| crate::edit::text_changes(page, *text, range.clone(), with))
            }
            PageOp::Format {
                text,
                range,
                set,
                clear,
            } => {
                self.target(*text)?;
                self.write(|page| {
                    crate::formatting::format_changes(page, *text, range.clone(), set, clear)
                })
            }
            PageOp::Link {
                text,
                range,
                target,
            } => {
                self.target(*text)?;
                let page = self.page()?;
                let current = crate::page::text_of(&page.view, &page.parents, *text)?;
                for op in link_ops(&current, *text, range.clone(), target.as_deref())? {
                    self.apply(&op)?;
                }
                Ok(())
            }
            PageOp::Equation { text, math } => {
                self.target(*text)?;
                self.write(|page| content::equation_changes(page, *text, math))
            }
            PageOp::Insert {
                container,
                before,
                paragraphs,
            } => {
                self.target(*container)?;
                self.insert(*container, *before, paragraphs)
            }
            PageOp::Split {
                text,
                at,
                paragraph,
                right,
                lists,
            } => {
                self.target(*text)?;
                for id in [*paragraph, *right].iter().chain(lists) {
                    self.free(*id)?;
                }
                let split = ParagraphSplit::new(*text, *at, self.author)?;
                self.write(|page| split.changes_as(page, *paragraph, *right, lists))
            }
            PageOp::Join { left, right } => {
                self.target(*left)?;
                self.target(*right)?;
                let join = ParagraphJoin::new(*left, *right, self.author)?;
                self.write(|page| join.changes(page))
            }
            PageOp::Move {
                object,
                parent,
                before,
            } => {
                self.target(*object)?;
                self.move_to(*object, *parent, *before)
            }
            PageOp::Delete { object } => {
                self.target(*object)?;
                let page = self.page()?;
                if matches!(page.view.nodes[object].kind, Kind::Paragraph { .. }) {
                    let container = self.container(*object)?;
                    if matches!(self.page()?.view.nodes[&container].kind, Kind::Cell { .. })
                        && levels::flat_children(&self.page()?.view, container) == [*object]
                    {
                        return Err(OpError::Unsupported(
                            "A table cell keeps a paragraph; insert its replacement first",
                        )
                        .into());
                    }
                }
                let edit = TreeEdit::delete(*object, self.author)?;
                self.write(|page| edit.changes(page))
            }
            PageOp::Level { paragraph, level } => {
                self.target(*paragraph)?;
                let container = self.container(*paragraph)?;
                let base = self.level(container)?;
                let depth = level
                    .checked_sub(base)
                    .filter(|depth| *depth > 0)
                    .ok_or(OpError::Unsupported(
                        "A paragraph lies deeper than its parent",
                    ))?;
                self.regroup(container, &[(*paragraph, depth)])
            }
            PageOp::Outline { object, edit } => {
                self.target(*object)?;
                self.write(|page| edit.changes(page, *object))
            }
            PageOp::Paragraph {
                paragraph,
                alignment,
                rtl,
                space_before,
                space_after,
                line_spacing,
                language,
            } => {
                self.target(*paragraph)?;
                let text = self.text_of(*paragraph)?;
                let values = ParagraphFields {
                    alignment: *alignment,
                    rtl: *rtl,
                    space_before: *space_before,
                    space_after: *space_after,
                    line_spacing: *line_spacing,
                    language: *language,
                }
                .values()?;
                if values.is_empty() {
                    return Ok(());
                }
                self.write(|page| properties::paragraph_format_changes(page, text, &values))
            }
            PageOp::Style {
                paragraph,
                style,
                definition,
            } => {
                self.target(*paragraph)?;
                let text = self.text_of(*paragraph)?;
                self.write(|page| properties::style_changes(page, text, *style, Some(definition)))
            }
            PageOp::List { paragraph, lists } => {
                self.target(*paragraph)?;
                let page = self.page()?;
                for (id, _) in lists {
                    if page
                        .parents
                        .get(id)
                        .is_some_and(|owners| owners.iter().any(|owner| owner != paragraph))
                    {
                        return Err(OpError::Unsupported("A list node belongs to one paragraph").into());
                    }
                }
                let nodes = lists
                    .iter()
                    .map(|(id, definition)| Ok((*id, properties::list_values(definition)?)))
                    .collect::<Result<Vec<_>, Error>>()?;
                self.write(|page| properties::list_changes(page, *paragraph, &nodes))
            }
            PageOp::Tags {
                target,
                tags,
                definitions,
            } => {
                self.target(*target)?;
                let mut entries = Vec::new();
                for tag in tags {
                    let definition = tag
                        .definition
                        .ok_or(OpError::Unsupported("A note tag names its definition"))?;
                    let model = definitions
                        .iter()
                        .find(|(id, _)| *id == definition)
                        .map(|(_, definition)| definition);
                    entries.push((definition, tag, model));
                }
                self.write(|page| properties::tag_changes(page, *target, &entries))
            }
            PageOp::Add { object, before } => self.add(object, *before),
            PageOp::Picture {
                picture,
                layout,
                alt,
            } => {
                self.target(*picture)?;
                let page = self.page()?;
                let node = &page.view.nodes[picture];
                if !matches!(node.kind, Kind::Image { .. }) {
                    return Err(OpError::Unsupported("Select a picture").into());
                }
                let stored = Image::read(&page.view, *picture, node)?;
                self.write(|page| {
                    content::picture_edit_changes(
                        page,
                        *picture,
                        (&stored.layout, &stored.alt),
                        layout,
                        alt,
                    )
                })
            }
            PageOp::Attachment {
                attachment,
                filename,
                source_path,
                size,
            } => {
                self.target(*attachment)?;
                let page = self.page()?;
                let node = &page.view.nodes[attachment];
                if !matches!(node.kind, Kind::Attachment { .. }) {
                    return Err(OpError::Unsupported("Select an attachment").into());
                }
                let stored = Attachment::read(&page.view, *attachment, node)?;
                let edited = Attachment {
                    filename: filename.clone(),
                    source_path: source_path.clone(),
                    size: *size,
                    ..stored.clone()
                };
                self.write(|page| content::attachment_edit_changes(page, &stored, &edited))
            }
            PageOp::Strokes { ink, add, remove } => {
                self.target(*ink)?;
                let page = self.page()?;
                let node = &page.view.nodes[ink];
                if !matches!(node.kind, Kind::Ink { .. }) {
                    return Err(OpError::Unsupported("Select ink").into());
                }
                let stored = Ink::read(&page.view, *ink, node)?;
                if !stored.groups.is_empty() {
                    return Err(OpError::Unsupported("Grouped ink keeps its strokes").into());
                }
                for id in remove {
                    if !stored.strokes.iter().any(|stroke| stroke.id == *id) {
                        return Err(OpError::TargetUnavailable(*id).into());
                    }
                }
                for stroke in add {
                    self.free(stroke.id)?;
                }
                let kept: Vec<ExGuid> = stored
                    .strokes
                    .iter()
                    .map(|stroke| stroke.id)
                    .filter(|id| !remove.contains(id))
                    .collect();
                let added: Vec<_> = add.iter().map(|stroke| (stroke.id, stroke)).collect();
                self.write(|page| content::strokes_changes(page, *ink, &kept, &added))
            }
            PageOp::Table { table, edit } => {
                self.target(*table)?;
                self.table(*table, edit)
            }
        }
    }

    /// Moves `object` before `before` in `parent`, or among the page's children.
    fn move_to(
        &mut self,
        object: ExGuid,
        parent: Option<ExGuid>,
        before: Option<ExGuid>,
    ) -> Result<(), Failure> {
        let Some(parent) = parent else {
            let [page] = self.page()?.pages[..] else {
                return Err(OpError::Unsupported("Choose a page space holding one page").into());
            };
            if let Some(before) = before
                && self.parent(before)? != page
            {
                return Err(OpError::StructureChanged("The anchor is not on the page").into());
            }
            let edit = TreeEdit::move_to(object, page, before, self.author)?;
            return self.write(|page| edit.changes(page));
        };
        self.target(parent)?;
        let mut at = parent;
        loop {
            if at == object {
                return Err(OpError::StructureChanged("A subtree cannot move inside itself").into());
            }
            match self.page()?.parents.get(&at).map(Vec::as_slice) {
                Some([up]) => at = *up,
                _ => break,
            }
        }
        let holder = match before {
            Some(before) => {
                self.target(before)?;
                if self.container(before)? != parent {
                    return Err(
                        OpError::StructureChanged("The anchor is no child of the container").into(),
                    );
                }
                self.parent(before)?
            }
            None => parent,
        };
        let level = self.level(object)?;
        let edit = TreeEdit::move_to(object, holder, before, self.author)?;
        self.write(|page| edit.changes(page))?;
        if matches!(self.page()?.view.nodes[&parent].kind, Kind::Cell { .. }) {
            return Ok(());
        }
        let base = self.level(parent)?;
        let depth = if level > base { level - base } else { 1 };
        self.regroup(parent, &[(object, depth)])
    }

    /// Inserts paragraphs into `container` before `before` as `PageOp::Insert` does.
    fn insert(
        &mut self,
        container: ExGuid,
        before: Option<ExGuid>,
        paragraphs: &[PageParagraph],
    ) -> Result<(), Failure> {
        if paragraphs.is_empty() {
            return Err(OpError::Unsupported("Insert at least one paragraph").into());
        }
        let holder = match before {
            Some(before) => {
                self.target(before)?;
                if self.container(before)? != container {
                    return Err(
                        OpError::StructureChanged("The anchor is no child of the container").into(),
                    );
                }
                self.parent(before)?
            }
            None => container,
        };
        let mut seen = BTreeSet::new();
        for paragraph in paragraphs {
            bare(paragraph)?;
            for id in super::model::identities(std::slice::from_ref(paragraph)) {
                if !seen.insert(id) {
                    return Err(OpError::DuplicateIdentity(id).into());
                }
                self.free(id)?;
            }
            if let Some(parent) = paragraph.parent
                && parent != container
                && !paragraphs.iter().any(|p| p.id == parent)
            {
                return Err(OpError::StructureChanged(
                    "An inserted paragraph's parent precedes it among them",
                )
                .into());
            }
        }
        let base = self.level(container)?;
        let mut targets: BTreeMap<ExGuid, Vec<(ExGuid, u32)>> = BTreeMap::new();
        for paragraph in paragraphs {
            let (parent, anchor, holder) = match paragraph.parent.filter(|p| *p != container) {
                Some(parent) => (parent, None, parent),
                None => (container, before, holder),
            };
            let parent_level = if parent == container {
                base
            } else {
                paragraphs.iter().find(|p| p.id == parent).unwrap().level
            };
            let depth = paragraph
                .level
                .checked_sub(parent_level)
                .filter(|depth| *depth > 0)
                .ok_or(OpError::Unsupported("A paragraph lies deeper than its parent"))?;
            targets.entry(parent).or_default().push((paragraph.id, depth));
            self.paragraph(holder, anchor, paragraph, None)?;
        }
        for (parent, targets) in targets {
            if !matches!(self.page()?.view.nodes[&parent].kind, Kind::Cell { .. }) {
                self.regroup(parent, &targets)?;
            }
        }
        Ok(())
    }

    /// Creates one paragraph with its content in `holder` before `anchor`; with `outline`,
    /// as the first paragraph of that new outline at its position.
    fn paragraph(
        &mut self,
        holder: ExGuid,
        anchor: Option<ExGuid>,
        paragraph: &PageParagraph,
        outline: Option<(ExGuid, f32, f32)>,
    ) -> Result<(), Failure> {
        let (text, text_id) = match &paragraph.content {
            ParagraphContent::Text(text) => (text.text.text(), text.id),
            _ => (
                "",
                ExGuid {
                    guid: crate::write::fresh_guid()?,
                    n: 1,
                },
            ),
        };
        let equation = paragraph
            .text()
            .is_some_and(|text| crate::page::Math::is_equation(&text.text));
        let insertion = match outline {
            Some((_, x, y)) => {
                let page = self.page()?.pages.first().copied().unwrap_or_default();
                Insertion::outline(page, x, y, text, self.author)?
            }
            None => Insertion::paragraph(holder, anchor, text, self.author)?,
        };
        let object = outline.map_or(paragraph.id, |(id, _, _)| id);
        self.write(|page| insertion.changes_as(page, object, paragraph.id, text_id))?;
        match &paragraph.content {
            ParagraphContent::Text(target) => {
                if equation {
                    return self.write(|page| content::equation_changes(page, text_id, &target.text));
                }
                let page = self.page()?;
                let stored = crate::page::text_of(&page.view, &page.parents, text_id)?;
                let values = paragraph_fields(&stored, &target.text)?.values()?;
                if !values.is_empty() {
                    self.write(|page| properties::paragraph_format_changes(page, text_id, &values))?;
                }
                let page = self.page()?;
                let stored = crate::page::text_of(&page.view, &page.parents, text_id)?;
                for (range, set, clear) in format_edits(&stored, &target.text, true)? {
                    self.write(|page| {
                        crate::formatting::format_changes(page, text_id, range, &set, &clear)
                    })?;
                }
                Ok(())
            }
            ParagraphContent::Table(table) => {
                super::table::validate_table(table)?;
                let structure = Structure {
                    rows: table
                        .rows
                        .iter()
                        .map(|row| (row.id, row.cells.iter().map(|cell| cell.id).collect()))
                        .collect(),
                    new_rows: table.rows.iter().map(|row| row.id).collect(),
                    new_cells: table
                        .rows
                        .iter()
                        .flat_map(|row| &row.cells)
                        .map(|cell| (cell.id, (table::cell_indents(&cell.indents, None), cell.shading)))
                        .collect(),
                    columns: &table.columns,
                    borders: table.borders,
                };
                self.write(|page| table::table_changes(page, table.id, Some(paragraph.id), &structure))?;
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
                    self.cell(cell)?;
                }
                Ok(())
            }
            ParagraphContent::Image(image) => {
                if image.layout.x.is_some() || image.layout.y.is_some() {
                    return Err(OpError::Unsupported("A paragraph picture has no position of its own").into());
                }
                self.picture(image, Some(paragraph.id))
            }
            ParagraphContent::Attachment(attachment) => {
                let Some(bytes) = &attachment.bytes else {
                    return Err(OpError::Unsupported("A new attachment needs its payload").into());
                };
                let ids = AttachmentIds {
                    object: attachment.id,
                    file: fresh()?,
                    payload: crate::write::fresh_guid()?,
                    preview: match &attachment.preview {
                        Some(_) => Some((crate::write::fresh_guid()?, fresh()?)),
                        None => None,
                    },
                };
                let mut payloads: Vec<([u8; 16], &[u8])> = vec![(ids.payload, bytes)];
                if let (Some((payload, _)), Some(icon)) = (ids.preview, &attachment.preview) {
                    payloads.push((payload, icon));
                }
                self.write_with(&payloads, |page| {
                    content::attachment_changes(page, attachment, &ids, paragraph.id)
                })
            }
            ParagraphContent::Ink(ink) => self.ink(ink, Some(paragraph.id)),
            ParagraphContent::Unsupported(_) => {
                Err(OpError::Unsupported("Unsupported content cannot be inserted").into())
            }
        }
    }

    /// Fills a new cell with its paragraphs.
    fn cell(&mut self, cell: &TableCell) -> Result<(), Failure> {
        if cell.paragraphs.is_empty() {
            return Err(OpError::Unsupported("A new table cell needs a paragraph").into());
        }
        self.insert(cell.id, None, &cell.paragraphs)
    }

    fn picture(&mut self, image: &Image, holder: Option<ExGuid>) -> Result<(), Failure> {
        let Some(bytes) = &image.bytes else {
            return Err(OpError::Unsupported("A new picture needs its payload").into());
        };
        let (file, payload) = (fresh()?, crate::write::fresh_guid()?);
        let bytes: &[u8] = bytes;
        self.write_with(&[(payload, bytes)], |page| {
            content::picture_changes(page, image, image.id, file, payload, holder)
        })
    }

    fn ink(&mut self, ink: &Ink, holder: Option<ExGuid>) -> Result<(), Failure> {
        for stroke in &ink.strokes {
            self.free(stroke.id)?;
        }
        let data = fresh()?;
        let strokes: Vec<_> = ink.strokes.iter().map(|stroke| (stroke.id, stroke)).collect();
        self.write(|page| content::ink_changes(page, ink, ink.id, data, &strokes, holder))
    }

    /// Adds a page object on top, then moves it before `before`.
    fn add(&mut self, object: &PageObject, before: Option<ExGuid>) -> Result<(), Failure> {
        self.free(object.id())?;
        match object {
            PageObject::Outline(outline) => {
                let (Some(x), Some(y)) = (outline.layout.x, outline.layout.y) else {
                    return Err(OpError::Unsupported("A new outline needs a position").into());
                };
                let [first, rest @ ..] = outline.paragraphs.as_slice() else {
                    return Err(OpError::Unsupported("A new outline needs a paragraph").into());
                };
                for paragraph in &outline.paragraphs {
                    bare(paragraph)?;
                    for id in super::model::identities(std::slice::from_ref(paragraph)) {
                        self.free(id)?;
                    }
                }
                if first.parent.is_some() || first.level != 1 {
                    return Err(OpError::Unsupported("An outline starts at its first level").into());
                }
                self.paragraph(outline.id, None, first, Some((outline.id, x, y)))?;
                // Later paragraphs follow the first, their levels as the outline gives them.
                if !rest.is_empty() {
                    let mut targets = vec![(first.id, 1)];
                    for paragraph in rest {
                        let parent = paragraph.parent.unwrap_or(outline.id);
                        let parent_level = rest
                            .iter()
                            .chain([first])
                            .find(|p| p.id == parent)
                            .map_or(0, |p| p.level);
                        let depth = paragraph
                            .level
                            .checked_sub(parent_level)
                            .filter(|depth| *depth > 0)
                            .ok_or(OpError::Unsupported("A paragraph lies deeper than its parent"))?;
                        self.paragraph(parent, None, paragraph, None)?;
                        if parent == outline.id {
                            targets.push((paragraph.id, depth));
                        } else {
                            self.regroup(parent, &[(paragraph.id, depth)])?;
                        }
                    }
                    self.regroup(outline.id, &targets)?;
                }
                // A new outline is 468 points wide unless it says otherwise.
                if let Some(points) = outline.layout.max_width
                    && (points, outline.layout.width_set_by_user) != (468.0, None)
                {
                    let edit = OutlineEdit::Width {
                        points,
                        user_set: outline.layout.width_set_by_user == Some(true),
                    };
                    self.write(|page| edit.changes(page, outline.id))?;
                }
                if !outline.indents.is_empty() && outline.indents != content::NATIVE_INDENTS {
                    let indents = content::measurement_bytes(&outline.indents, 4)?;
                    let id = outline.id;
                    self.write(|page| {
                        let mut node =
                            crate::write::PropertyObject::from_object(&page.live.revision.objects[&id])?;
                        node.set(&[(0x1c001c12, &indents)])?;
                        Ok(BTreeMap::from([(id, node)]))
                    })?;
                }
            }
            PageObject::Image(image) => {
                if image.layout.x.is_none() || image.layout.y.is_none() {
                    return Err(OpError::Unsupported("A new page-level picture needs a position").into());
                }
                self.picture(image, None)?;
            }
            PageObject::Ink(ink) => self.ink(ink, None)?,
            PageObject::Title(_) | PageObject::Unsupported(_) => {
                return Err(OpError::Unsupported("Titles and unsupported objects cannot be added").into());
            }
        }
        if before.is_some() {
            self.move_to(object.id(), None, before)?;
        }
        Ok(())
    }

    fn table(&mut self, id: ExGuid, edit: &TableEdit) -> Result<(), Failure> {
        let page = self.page()?;
        let node = &page.view.nodes[&id];
        let Kind::Table {
            widths,
            locked,
            borders,
            ..
        } = &node.kind
        else {
            return Err(OpError::Unsupported("Select a table").into());
        };
        let mut columns: Vec<TableColumn> = widths
            .iter()
            .enumerate()
            .map(|(i, width)| TableColumn {
                width: *width,
                locked: locked.get(i).copied().unwrap_or(false),
            })
            .collect();
        let mut rows: Vec<(ExGuid, Vec<ExGuid>)> = node
            .children
            .iter()
            .map(|row| (*row, page.view.nodes[row].children.clone()))
            .collect();
        let mut borders = *borders;
        let template = rows
            .first()
            .and_then(|(_, cells)| cells.first())
            .and_then(|cell| match &page.view.nodes[cell].kind {
                Kind::Cell { indents, .. } => Some(indents.clone()),
                _ => None,
            });
        let mut new_rows = BTreeSet::new();
        let mut new_cells: BTreeMap<ExGuid, (Vec<f32>, Option<u32>)> = BTreeMap::new();
        let mut filled: Vec<&TableCell> = Vec::new();
        let new_cell = |cell: &TableCell| {
            (
                table::cell_indents(&cell.indents, template.as_deref()).to_vec(),
                cell.shading,
            )
        };
        match edit {
            TableEdit::Rows { before, rows: added } => {
                let at = match before {
                    Some(before) => rows
                        .iter()
                        .position(|(row, _)| row == before)
                        .ok_or(OpError::StructureChanged("The anchor row is not in the table"))?,
                    None => rows.len(),
                };
                for row in added {
                    if row.cells.len() != columns.len() {
                        return Err(OpError::Unsupported("Every table row has one cell per column").into());
                    }
                    new_rows.insert(row.id);
                    for cell in &row.cells {
                        new_cells.insert(cell.id, new_cell(cell));
                        filled.push(cell);
                    }
                }
                rows.splice(
                    at..at,
                    added
                        .iter()
                        .map(|row| (row.id, row.cells.iter().map(|cell| cell.id).collect())),
                );
            }
            TableEdit::Column { at, width, cells } => {
                let at = *at as usize;
                if at > columns.len() || cells.len() != rows.len() {
                    return Err(OpError::Unsupported("A new column has one cell per row").into());
                }
                for ((_, row), cell) in rows.iter_mut().zip(cells) {
                    row.insert(at, cell.id);
                    new_cells.insert(cell.id, new_cell(cell));
                    filled.push(cell);
                }
                columns.insert(
                    at,
                    TableColumn {
                        width: *width,
                        locked: false,
                    },
                );
            }
            TableEdit::DeleteRow(row) => {
                let before = rows.len();
                rows.retain(|(id, _)| id != row);
                if rows.len() == before {
                    return Err(OpError::TargetUnavailable(*row).into());
                }
                if rows.is_empty() {
                    return Err(OpError::Unsupported("A table keeps a row; delete the table instead").into());
                }
            }
            TableEdit::DeleteColumn(at) => {
                let at = *at as usize;
                if at >= columns.len() || columns.len() == 1 {
                    return Err(OpError::Unsupported("Choose a column of a table with several").into());
                }
                columns.remove(at);
                for (_, cells) in &mut rows {
                    cells.remove(at);
                }
            }
            TableEdit::Columns(widths) => {
                if widths.len() != columns.len() {
                    return Err(OpError::Unsupported("Give every column its width").into());
                }
                columns = widths.clone();
            }
            TableEdit::Borders(value) => borders = Some(*value),
            TableEdit::Cell {
                cell,
                shading,
                indents,
            } => {
                if !rows.iter().any(|(_, cells)| cells.contains(cell)) {
                    return Err(OpError::TargetUnavailable(*cell).into());
                }
                let Kind::Cell {
                    shading: stored_shading,
                    indents: stored_indents,
                } = &page.view.nodes[cell].kind
                else {
                    return Err(OpError::Unsupported("Select a table cell").into());
                };
                let stored = (*stored_shading, stored_indents.clone());
                return self.write(|page| {
                    table::cell_changes(page, *cell, (stored.0, &stored.1), *shading, indents)
                });
            }
        }
        for cell in &filled {
            for id in std::iter::once(cell.id).chain(super::model::identities(&cell.paragraphs)) {
                self.free(id)?;
            }
        }
        for row in &new_rows {
            self.free(*row)?;
        }
        if columns.iter().any(|c| !c.width.is_finite() || c.width < 36.0) {
            return Err(OpError::Unsupported("Table columns are at least 36 points wide").into());
        }
        let structure = Structure {
            rows,
            new_rows,
            new_cells: new_cells
                .iter()
                .map(|(id, (indents, shading))| (*id, (indents.as_slice(), *shading)))
                .collect(),
            columns: &columns,
            borders,
        };
        self.write(|page| table::table_changes(page, id, None, &structure))?;
        for cell in filled {
            self.cell(cell)?;
        }
        Ok(())
    }
}

fn fresh() -> Result<ExGuid, Error> {
    Ok(ExGuid {
        guid: crate::write::fresh_guid()?,
        n: 1,
    })
}

/// Requires what `Insert` leaves to other ops unset.
fn bare(paragraph: &PageParagraph) -> Result<(), OpError> {
    let text_tags = paragraph.text().is_some_and(|text| !text.tags.is_empty());
    if paragraph.style.is_some()
        || !paragraph.lists.is_empty()
        || !paragraph.tags.is_empty()
        || text_tags
        || paragraph.collapsed
    {
        return Err(OpError::Unsupported(
            "Set styles, lists, tags and collapse state with their own ops",
        ));
    }
    if paragraph.media != Default::default() {
        return Err(OpError::Unsupported(
            "Recording annotations are made by OneNote while it records",
        ));
    }
    match &paragraph.content {
        ParagraphContent::Text(text) if text.date_field.is_some() => Err(OpError::Unsupported(
            "New paragraphs contain plain text without fields",
        )),
        ParagraphContent::Table(table) => {
            if !table.tags.is_empty() {
                return Err(OpError::Unsupported("Set a table's tags with their own op"));
            }
            for paragraph in table.rows.iter().flat_map(|row| &row.cells).flat_map(|cell| &cell.paragraphs)
            {
                bare(paragraph)?;
            }
            Ok(())
        }
        ParagraphContent::Attachment(attachment) if attachment.recording.is_some() => Err(
            OpError::Unsupported("Recordings are captured by OneNote, not inserted"),
        ),
        _ => Ok(()),
    }
}

/// The field code OneNote stores before a hyperlink's label.
fn field_code(target: &str) -> Result<String, Error> {
    if target.is_empty() || target.contains(['"', '\0', '\r', '\n', '\u{fddf}']) {
        return Err(invalid("Choose a link target without quotes or line breaks"));
    }
    Ok(format!("\u{fddf}HYPERLINK \"{target}\""))
}

/// The text edits making `range` of `text` a hyperlink to `target`: the field code
/// inserted hidden before the label, both flagged as link runs as OneNote stores them. With
/// `None` or an existing link, the field code before the label is removed first.
pub(crate) fn link_ops(
    text: &Paragraph,
    id: ExGuid,
    range: Range<u32>,
    target: Option<&str>,
) -> Result<Vec<PageOp>, Error> {
    if range.is_empty() {
        return Err(invalid("A link needs a label"));
    }
    let at = |offset: u32| super::lower::format_in(text, offset);
    let mut ops = Vec::new();
    let mut label = range.clone();
    // A field code is a hidden link run ending where the label starts.
    let start = text.byte_offset(range.start)?;
    let spans = text.spans();
    let code = spans.iter().enumerate().find_map(|(i, span)| {
        let from = if i == 0 { 0 } else { spans[i - 1].end };
        (span.end == start
            && span.format.hidden == Some(true)
            && span.format.hyperlink == Some(true)
            && text.text()[from..span.end].starts_with('\u{fddf}'))
        .then_some(from..span.end)
    });
    if let Some(code) = code {
        let code = text.utf16_offset(code.start)?..text.utf16_offset(code.end)?;
        let length = code.end - code.start;
        ops.push(PageOp::Text {
            text: id,
            range: code.clone(),
            with: String::new(),
        });
        label = label.start - length..label.end - length;
    } else if target.is_none() {
        return Err(invalid("The range is no link"));
    }
    let base = at(range.start)?.clone();
    match target {
        Some(target) => {
            let code = field_code(target)?;
            let length = code.encode_utf16().count() as u32;
            ops.push(PageOp::Text {
                text: id,
                range: label.start..label.start,
                with: code,
            });
            let mut set = vec![
                TextAttribute::Hyperlink(true),
                TextAttribute::HyperlinkLabel(true),
            ];
            if base.hidden != Some(true) {
                ops.push(PageOp::Format {
                    text: id,
                    range: label.start..label.start + length,
                    set: [set.clone(), vec![TextAttribute::Hidden(true)]].concat(),
                    clear: Vec::new(),
                });
            }
            set.retain(|attribute| match attribute {
                TextAttribute::Hyperlink(_) => base.hyperlink != Some(true),
                _ => base.hyperlink_label != Some(true),
            });
            if !set.is_empty() || base.hidden == Some(true) {
                ops.push(PageOp::Format {
                    text: id,
                    range: label.start + length..label.end + length,
                    set,
                    clear: Vec::new(),
                });
            }
        }
        None => ops.push(PageOp::Format {
            text: id,
            range: label,
            set: Vec::new(),
            clear: vec![TextProperty::Hyperlink, TextProperty::HyperlinkLabel],
        }),
    }
    ops.retain(|op| !matches!(op, PageOp::Format { set, clear, .. } if set.is_empty() && clear.is_empty()));
    Ok(ops)
}
