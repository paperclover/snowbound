use onestore::page::text::{EditError, Paragraph, new_id};
use onestore::page::{PageParagraph, ParagraphContent, TextObject};
use onestore::{ExGuid, document::Format};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub paragraph: usize,
    pub offset: u32,
}

/// Paragraph positions enumerate text leaves in document order, including table cells.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDocument {
    nodes: Vec<PageParagraph>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DocumentEdit {
    /// Cell identity, or None for the document root.
    pub container: Option<ExGuid>,
    pub range: Range<usize>,
    pub replacement: Vec<PageParagraph>,
    /// Width patches address surviving tables outside the replacement subtree.
    pub columns: BTreeMap<ExGuid, Vec<f32>>,
}

pub(crate) fn node(text: Paragraph, format: Format) -> Result<PageParagraph, EditError> {
    Ok(PageParagraph {
        id: new_id()?,
        parent: None,
        level: 1,
        format,
        content: onestore::page::ParagraphContent::Text(TextObject {
            date_field: None,
            id: new_id()?,
            text,
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        collapsed: false,
        style: None,
    })
}

pub(crate) fn edited_nodes<'a>(
    nodes: &'a [PageParagraph],
    container: Option<ExGuid>,
    edit: Option<&'a DocumentEdit>,
) -> impl Clone + Iterator<Item = &'a PageParagraph> {
    let (range, replacement) = match edit.filter(|edit| edit.container == container) {
        Some(edit) => (edit.range.clone(), edit.replacement.as_slice()),
        None => (0..0, &[][..]),
    };
    nodes[..range.start]
        .iter()
        .chain(replacement)
        .chain(&nodes[range.end..])
}

pub(crate) fn leaves<'a>(
    nodes: &'a [PageParagraph],
    edit: Option<&'a DocumentEdit>,
) -> impl Iterator<Item = (Option<ExGuid>, usize, &'a PageParagraph)> {
    descendants(nodes, edit).filter(|(_, _, node)| node.text().is_some())
}

pub(crate) fn descendants<'a>(
    nodes: &'a [PageParagraph],
    edit: Option<&'a DocumentEdit>,
) -> impl Iterator<Item = (Option<ExGuid>, usize, &'a PageParagraph)> {
    let mut current = (None, edited_nodes(nodes, None, edit).enumerate());
    let mut pending = Vec::new();
    std::iter::from_fn(move || {
        loop {
            let Some((index, node)) = current.1.next() else {
                current = pending.pop()?;
                continue;
            };
            let container = current.0;
            if let ParagraphContent::Table(table) = &node.content {
                pending.push(current.clone());
                pending.extend(
                    table
                        .rows
                        .iter()
                        .rev()
                        .flat_map(|row| row.cells.iter().rev())
                        .map(|cell| {
                            (
                                Some(cell.id),
                                edited_nodes(&cell.paragraphs, Some(cell.id), edit).enumerate(),
                            )
                        }),
                );
                current = pending.pop().unwrap();
            }
            return Some((container, index, node));
        }
    })
}

pub(crate) fn swap_columns(nodes: &mut [PageParagraph], widths: &mut BTreeMap<ExGuid, Vec<f32>>) {
    if widths.is_empty() {
        return;
    }
    for node in nodes {
        if let ParagraphContent::Table(table) = &mut node.content {
            if let Some(widths) = widths.get_mut(&table.id) {
                for (column, width) in table.columns.iter_mut().zip(widths) {
                    std::mem::swap(&mut column.width, width);
                }
            }
            for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                swap_columns(&mut cell.paragraphs, widths);
            }
        }
    }
}

pub(crate) fn validate_nodes(
    nodes: &[PageParagraph],
    edit: Option<&DocumentEdit>,
    ids: &mut BTreeSet<ExGuid>,
) -> Result<(), EditError> {
    let mut pending = vec![(None, nodes, 0)];
    while let Some((container, nodes, depth)) = pending.pop() {
        if depth > 64 {
            return Err(EditError::InvalidStructure);
        }
        let mut nodes = edited_nodes(nodes, container, edit).peekable();
        if nodes.peek().is_none() {
            return Err(EditError::InvalidRange);
        }
        let mut parents = BTreeMap::new();
        for node in nodes {
            if !ids.insert(node.id)
                || node.level == 0
                || node
                    .parent
                    .is_some_and(|id| parents.get(&id).is_none_or(|level| *level >= node.level))
            {
                return Err(EditError::InvalidStructure);
            }
            parents.insert(node.id, node.level);
            match &node.content {
                ParagraphContent::Text(text) => {
                    if !ids.insert(text.id) {
                        return Err(EditError::InvalidStructure);
                    }
                }
                ParagraphContent::Image(_)
                | ParagraphContent::Attachment(_)
                | ParagraphContent::Unsupported(_) => {
                    return Err(EditError::UnsupportedContent);
                }
                ParagraphContent::Table(table) => {
                    if !ids.insert(table.id)
                        || table.rows.is_empty()
                        || table.columns.is_empty()
                        || table.columns.len() > 255
                        || table
                            .columns
                            .iter()
                            .any(|column| !column.width.is_finite() || column.width < 36.0)
                    {
                        return Err(EditError::InvalidStructure);
                    }
                    for row in &table.rows {
                        if !ids.insert(row.id) || row.cells.len() != table.columns.len() {
                            return Err(EditError::InvalidStructure);
                        }
                        for cell in &row.cells {
                            if !ids.insert(cell.id) {
                                return Err(EditError::InvalidStructure);
                            }
                            if !cell.unsupported.is_empty() {
                                return Err(EditError::UnsupportedContent);
                            }
                            pending.push((Some(cell.id), cell.paragraphs.as_slice(), depth + 1));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_text(nodes: &[PageParagraph]) -> Result<(), EditError> {
    for (_, _, node) in leaves(nodes, None) {
        let text = &node.text().unwrap().text;
        text.utf16_offset(text.text().len())?;
    }
    Ok(())
}

pub(crate) fn validate_flat<'a>(
    mut nodes: impl Iterator<Item = &'a PageParagraph>,
) -> Result<(), EditError> {
    if nodes.any(|node| {
        !node.lists.is_empty()
            || !node.tags.is_empty()
            || node.collapsed
            || node.parent.is_some()
            || node.text().is_none_or(|text| !text.tags.is_empty())
    }) {
        return Err(EditError::UnsupportedContent);
    }
    Ok(())
}

pub(crate) fn container_mut(
    nodes: &mut Vec<PageParagraph>,
    id: Option<ExGuid>,
) -> Option<&mut Vec<PageParagraph>> {
    let Some(id) = id else {
        return Some(nodes);
    };
    for node in nodes {
        if let ParagraphContent::Table(table) = &mut node.content {
            for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                if cell.id == id {
                    return Some(&mut cell.paragraphs);
                }
                if let Some(nodes) = container_mut(&mut cell.paragraphs, Some(id)) {
                    return Some(nodes);
                }
            }
        }
    }
    None
}

impl TextDocument {
    pub fn new(paragraphs: Vec<Paragraph>) -> Result<Self, EditError> {
        Self::from_nodes(
            paragraphs
                .into_iter()
                .map(|text| node(text, Format::default()))
                .collect::<Result<_, _>>()?,
        )
    }

    pub fn from_nodes(nodes: Vec<PageParagraph>) -> Result<Self, EditError> {
        validate_nodes(&nodes, None, &mut BTreeSet::new())?;
        validate_text(&nodes)?;
        Ok(Self { nodes })
    }

    pub fn nodes(&self) -> &[PageParagraph] {
        &self.nodes
    }

    pub(crate) fn validate_flat(&self) -> Result<(), EditError> {
        validate_flat(self.nodes.iter())
    }

    pub fn text_nodes(&self) -> impl Iterator<Item = &PageParagraph> {
        leaves(&self.nodes, None).map(|(_, _, node)| node)
    }

    pub fn paragraphs(&self) -> impl Iterator<Item = &Paragraph> {
        self.text_nodes().map(|node| &node.text().unwrap().text)
    }

    pub(crate) fn container(&self, id: Option<ExGuid>) -> Result<&[PageParagraph], EditError> {
        let Some(id) = id else {
            return Ok(&self.nodes);
        };
        let mut pending = vec![self.nodes.as_slice()];
        while let Some(nodes) = pending.pop() {
            for node in nodes {
                if let ParagraphContent::Table(table) = &node.content {
                    for cell in table.rows.iter().flat_map(|row| &row.cells) {
                        if cell.id == id {
                            return Ok(&cell.paragraphs);
                        }
                        pending.push(&cell.paragraphs);
                    }
                }
            }
        }
        Err(EditError::InvalidRange)
    }

    pub fn slice(&self, range: Range<TextPosition>) -> Result<Vec<Paragraph>, EditError> {
        if range.start > range.end {
            return Err(EditError::InvalidRange);
        }
        let count = range
            .end
            .paragraph
            .checked_sub(range.start.paragraph)
            .and_then(|n| n.checked_add(1))
            .ok_or(EditError::InvalidRange)?;
        let result = self
            .paragraphs()
            .skip(range.start.paragraph)
            .take(count)
            .enumerate()
            .map(|(index, paragraph)| {
                let start = if index == 0 { range.start.offset } else { 0 };
                let end = if index == count - 1 {
                    range.end.offset
                } else {
                    paragraph.utf16_offset(paragraph.text().len())?
                };
                paragraph.slice(start..end)
            })
            .collect::<Result<Vec<_>, EditError>>()?;
        if result.len() != count {
            return Err(EditError::InvalidRange);
        }
        Ok(result)
    }

    pub(crate) fn replace(
        &self,
        range: Range<TextPosition>,
        replacement: Vec<Paragraph>,
    ) -> Result<DocumentEdit, EditError> {
        if range.start > range.end {
            return Err(EditError::InvalidRange);
        }
        let (container, start, first) = leaves(&self.nodes, None)
            .nth(range.start.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let (end_container, end, last) = leaves(&self.nodes, None)
            .nth(range.end.paragraph)
            .ok_or(EditError::InvalidRange)?;
        if container != end_container {
            return Err(EditError::UnsupportedContent);
        }
        if range.start.paragraph != range.end.paragraph || replacement.len() != 1 {
            validate_flat(self.container(container)?.iter().enumerate().filter_map(
                |(index, node)| {
                    ((start..=end).contains(&index) || node.text().is_some()).then_some(node)
                },
            ))?;
        }
        let mut prefix = first.text().unwrap().text.slice(0..range.start.offset)?;
        let last_text = &last.text().unwrap().text;
        let suffix =
            last_text.slice(range.end.offset..last_text.utf16_offset(last_text.text().len())?)?;
        let mut replacement = replacement.into_iter();
        prefix.append(replacement.next().ok_or(EditError::InvalidRange)?)?;
        let mut head = first.clone();
        head.text_mut().unwrap().text = prefix;
        let mut nodes = vec![head];
        let following = replacement.len();
        for (index, text) in replacement.enumerate() {
            if index + 1 == following && range.start.paragraph != range.end.paragraph {
                let mut end = last.clone();
                end.text_mut().unwrap().text = text;
                nodes.push(end);
            } else {
                let mut next = node(text, first.format.clone())?;
                next.level = first.level;
                next.style = first.style;
                nodes.push(next);
            }
        }
        if !suffix.text().is_empty() {
            let end = nodes.last_mut().unwrap();
            end.text_mut().unwrap().text.append(suffix)?;
        }
        Ok(DocumentEdit {
            columns: BTreeMap::new(),
            container,
            range: start..end + 1,
            replacement: nodes,
        })
    }

    pub(crate) fn validate_edit(&self, edit: &DocumentEdit) -> Result<(), EditError> {
        let nodes = self.container(edit.container)?;
        if edit.range.start > edit.range.end || edit.range.end > nodes.len() {
            return Err(EditError::InvalidRange);
        }
        validate_nodes(&self.nodes, Some(edit), &mut BTreeSet::new())?;
        if !edit.columns.is_empty() {
            let replaced = descendants(&edit.replacement, None)
                .filter_map(|(_, _, node)| match &node.content {
                    ParagraphContent::Table(table) => Some(table.id),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            let tables = descendants(&self.nodes, Some(edit))
                .filter_map(|(_, _, node)| match &node.content {
                    ParagraphContent::Table(table) => Some((table.id, table)),
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>();
            for (id, widths) in &edit.columns {
                if replaced.contains(id)
                    || tables
                        .get(id)
                        .is_none_or(|table| table.columns.len() != widths.len())
                    || widths
                        .iter()
                        .any(|width| !width.is_finite() || *width < 36.0)
                {
                    return Err(EditError::InvalidStructure);
                }
            }
        }
        validate_text(&edit.replacement)
    }

    pub(crate) fn apply(&mut self, edit: DocumentEdit) -> Result<DocumentEdit, EditError> {
        self.validate_edit(&edit)?;
        let end = edit
            .range
            .start
            .checked_add(edit.replacement.len())
            .ok_or(EditError::TextTooLong)?;
        let range = edit.range.start..end;
        let nodes =
            container_mut(&mut self.nodes, edit.container).ok_or(EditError::InvalidRange)?;
        let replacement = nodes.splice(edit.range, edit.replacement).collect();
        let mut columns = edit.columns;
        swap_columns(&mut self.nodes, &mut columns);
        Ok(DocumentEdit {
            container: edit.container,
            range,
            replacement,
            columns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::document::Format;
    use onestore::page::{Table, TableCell, TableColumn, TableRow};

    fn position(paragraph: usize, offset: u32) -> TextPosition {
        TextPosition { paragraph, offset }
    }

    fn table_document() -> TextDocument {
        let format = Format::default();
        let mut cells = Vec::new();
        for texts in [vec!["a🌲b", "second"], vec!["right"]] {
            cells.push(TableCell {
                id: new_id().unwrap(),
                layout: Default::default(),
                indents: vec![18.0, 0.0, 27.0, 27.0],
                shading: None,
                paragraphs: texts
                    .into_iter()
                    .map(|text| {
                        node(Paragraph::new(text.into(), format.clone()), format.clone()).unwrap()
                    })
                    .collect(),
                unsupported: Vec::new(),
            });
        }
        let table = PageParagraph {
            id: new_id().unwrap(),
            parent: None,
            level: 1,
            style: None,
            format: format.clone(),
            lists: Vec::new(),
            tags: Vec::new(),
            collapsed: false,
            content: ParagraphContent::Table(Table {
                id: new_id().unwrap(),
                columns: vec![
                    TableColumn {
                        width: 72.0,
                        locked: true
                    };
                    2
                ],
                rows: vec![TableRow {
                    id: new_id().unwrap(),
                    cells,
                }],
                borders: Some(true),
                layout: Default::default(),
                tags: Vec::new(),
            }),
        };
        TextDocument::from_nodes(vec![
            node(
                Paragraph::new("before".into(), format.clone()),
                format.clone(),
            )
            .unwrap(),
            table,
            node(Paragraph::new("after".into(), format.clone()), format).unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn text_and_column_widths_share_an_exact_inverse() {
        let mut document = table_document();
        let source = document.clone();
        let ParagraphContent::Table(table) = &source.nodes()[1].content else {
            panic!()
        };
        let id = table.id;
        let neighbor = document.paragraphs().nth(3).unwrap().text().as_ptr();
        let mut edit = document
            .replace(
                position(1, 1)..position(1, 1),
                vec![Paragraph::new("X".into(), Format::default())],
            )
            .unwrap();
        edit.columns.insert(id, vec![81.24, 36.0]);
        let mut undo = document.apply(edit).unwrap();
        assert_eq!(undo.columns[&id], [72.0, 72.0]);
        let after = document.clone();
        for _ in 0..3 {
            let redo = document.apply(undo).unwrap();
            assert_eq!(document, source);
            assert_eq!(
                document.paragraphs().nth(3).unwrap().text().as_ptr(),
                neighbor
            );
            undo = document.apply(redo).unwrap();
            assert_eq!(document, after);
            assert_eq!(
                document.paragraphs().nth(3).unwrap().text().as_ptr(),
                neighbor
            );
        }
        let resize = DocumentEdit {
            container: None,
            range: 0..0,
            replacement: Vec::new(),
            columns: [(id, vec![37.11, 99.875])].into(),
        };
        let undo = document.apply(resize).unwrap();
        document.apply(undo).unwrap();
        assert_eq!(document, after);
    }

    #[test]
    fn invalid_column_patches_do_not_partially_edit_text() {
        let mut document = table_document();
        let source = document.clone();
        let ParagraphContent::Table(table) = &source.nodes()[1].content else {
            panic!()
        };
        let id = table.id;
        for widths in [
            vec![],
            vec![72.0],
            vec![72.0, 72.0, 72.0],
            vec![72.0, 35.99],
            vec![f32::NAN, 72.0],
            vec![f32::INFINITY, 72.0],
        ] {
            let mut edit = document
                .replace(
                    position(1, 1)..position(1, 1),
                    vec![Paragraph::new("X".into(), Format::default())],
                )
                .unwrap();
            edit.columns.insert(id, widths);
            assert!(document.apply(edit).is_err());
            assert_eq!(document, source);
        }
        for (range, replacement, id) in [
            (0..0, Vec::new(), new_id().unwrap()),
            (1..2, Vec::new(), id),
            (1..2, vec![source.nodes()[1].clone()], id),
        ] {
            let edit = DocumentEdit {
                container: None,
                range,
                replacement,
                columns: [(id, vec![81.0, 90.0])].into(),
            };
            assert!(document.apply(edit).is_err());
            assert_eq!(document, source);
        }
    }

    #[test]
    fn cell_text_edits_preserve_the_table_and_restore_paragraph_identity() {
        let mut document = table_document();
        let original = document.clone();
        let ParagraphContent::Table(table) = &document.nodes()[1].content else {
            panic!()
        };
        let cell = table.rows[0].cells[0].id;
        let other_text = table.rows[0].cells[1].paragraphs[0]
            .text()
            .unwrap()
            .text
            .text()
            .as_ptr();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "a🌲b", "second", "right", "after"]
        );
        assert_eq!(
            document
                .slice(position(1, 1)..position(3, 2))
                .unwrap()
                .iter()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["🌲b", "second", "ri"]
        );
        let edit = document
            .replace(
                position(1, 1)..position(2, 3),
                vec![Paragraph::new("NEW".into(), Format::default())],
            )
            .unwrap();
        assert_eq!(edit.container, Some(cell));
        assert_eq!(edit.range, 0..2);
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "aNEWond", "right", "after"]
        );
        let ParagraphContent::Table(table) = &document.nodes()[1].content else {
            panic!()
        };
        assert_eq!(
            table.rows[0].cells[1].paragraphs[0]
                .text()
                .unwrap()
                .text
                .text()
                .as_ptr(),
            other_text
        );
        assert_eq!(
            document.text_nodes().nth(1).unwrap().id,
            original.text_nodes().nth(1).unwrap().id
        );
        assert_eq!(undo.container, Some(cell));
        assert_eq!(undo.range, 0..1);
        let redo = document.apply(undo).unwrap();
        assert_eq!(document, original);
        let undo = document.apply(redo).unwrap();
        document.apply(undo).unwrap();
        assert_eq!(document, original);
    }

    #[test]
    fn surrounding_text_splits_without_flattening_the_table() {
        let mut document = table_document();
        let original = document.clone();
        let edit = document
            .replace(
                position(4, 2)..position(4, 2),
                vec![Paragraph::new(String::new(), Format::default()); 2],
            )
            .unwrap();
        assert_eq!(edit.container, None);
        assert_eq!(edit.range, 2..3);
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "a🌲b", "second", "right", "af", "ter"]
        );
        assert_eq!(document.nodes()[1], original.nodes()[1]);
        document.apply(undo).unwrap();
        assert_eq!(document, original);
        assert_eq!(
            document.replace(
                position(0, 0)..position(4, 0),
                vec![Paragraph::new(String::new(), Format::default())]
            ),
            Err(EditError::UnsupportedContent)
        );
    }

    #[test]
    fn cell_edits_validate_utf16_and_container_boundaries_before_mutation() {
        let mut document = table_document();
        let original = document.clone();
        let text = || vec![Paragraph::new("X".into(), Format::default())];
        assert_eq!(
            document.replace(position(1, 2)..position(1, 3), text()),
            Err(EditError::InvalidRange)
        );
        assert_eq!(
            document.replace(position(1, 0)..position(3, 1), text()),
            Err(EditError::UnsupportedContent)
        );
        let ParagraphContent::Table(table) = &document.nodes()[1].content else {
            panic!()
        };
        let cell = table.rows[0].cells[1].id;
        for edit in [
            DocumentEdit {
                columns: BTreeMap::new(),
                container: Some(cell),
                range: 0..1,
                replacement: Vec::new(),
            },
            DocumentEdit {
                columns: BTreeMap::new(),
                container: Some(new_id().unwrap()),
                range: 0..1,
                replacement: vec![document.nodes()[0].clone()],
            },
            DocumentEdit {
                columns: BTreeMap::new(),
                container: Some(cell),
                range: 0..usize::MAX,
                replacement: Vec::new(),
            },
        ] {
            assert_eq!(document.apply(edit), Err(EditError::InvalidRange));
            assert_eq!(document, original);
        }
        let alias = DocumentEdit {
            columns: BTreeMap::new(),
            container: Some(cell),
            range: 0..1,
            replacement: vec![document.nodes()[0].clone()],
        };
        assert_eq!(document.apply(alias), Err(EditError::InvalidStructure));
        assert_eq!(document, original);
    }

    #[test]
    fn nested_cell_edits_keep_leaf_order_and_split_paragraphs_locally() {
        let mut document = table_document();
        let nested = table_document().nodes.remove(1);
        let ParagraphContent::Table(table) = &document.nodes()[1].content else {
            panic!()
        };
        let parent = table.rows[0].cells[0].id;
        document
            .apply(DocumentEdit {
                columns: BTreeMap::new(),
                container: Some(parent),
                range: 0..2,
                replacement: vec![nested],
            })
            .unwrap();
        let original = document.clone();
        let edit = document
            .replace(
                position(2, 3)..position(2, 3),
                vec![
                    Paragraph::new("X".into(), Format::default()),
                    Paragraph::new("Y".into(), Format::default()),
                ],
            )
            .unwrap();
        assert_ne!(edit.container, Some(parent));
        assert_eq!(edit.range, 1..2);
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "a🌲b", "secX", "Yond", "right", "right", "after"]
        );
        document.apply(undo).unwrap();
        assert_eq!(document, original);
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION and CANVAS_TEST_PAGE private fixture inputs"]
    fn imported_nodes_preserve_identity_through_edit_and_undo() {
        use onestore::page::{Page, PageObject};
        use onestore::{RevisionIndex, Store, document::Document};
        let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let page = {
            let store = Store::parse(&bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            Page::from_document(
                &Document::parse(&index).unwrap(),
                &std::env::var("CANVAS_TEST_PAGE").unwrap(),
            )
            .unwrap()
        };
        drop(bytes);
        let mut outlines = 0;
        let mut paragraphs = 0;
        for outline in page.objects.iter().flat_map(|object| match object {
            PageObject::Outline(outline) => std::slice::from_ref(outline),
            PageObject::Title(title) => &title.outlines,
            _ => &[],
        }) {
            let original = TextDocument::from_nodes(outline.paragraphs.clone()).unwrap();
            for paragraph in 0..original.nodes().len() {
                let mut document = original.clone();
                let position = TextPosition {
                    paragraph,
                    offset: 0,
                };
                let format = document
                    .paragraphs()
                    .nth(paragraph)
                    .unwrap()
                    .format_at(0)
                    .unwrap()
                    .clone();
                let edit = document
                    .replace(
                        position..position,
                        vec![Paragraph::new("probe".into(), format)],
                    )
                    .unwrap();
                let undo = document.apply(edit).unwrap();
                let edited = document.clone();
                assert_eq!(edited.nodes()[paragraph].id, original.nodes()[paragraph].id);
                assert_eq!(
                    edited.nodes()[paragraph].text().unwrap().id,
                    original.nodes()[paragraph].text().unwrap().id
                );
                let mut restored_text = edited.nodes().to_vec();
                restored_text[paragraph].text_mut().unwrap().text =
                    original.nodes()[paragraph].text().unwrap().text.clone();
                assert_eq!(restored_text, original.nodes());
                let redo = document.apply(undo).unwrap();
                assert_eq!(document, original);
                document.apply(redo).unwrap();
                assert_eq!(document, edited);
                paragraphs += 1;
            }
            assert_eq!(outline.paragraphs, original.nodes());
            outlines += 1;
        }
        assert!(outlines > 0);
        eprintln!(
            "Edited and restored {outlines} imported outlines containing {paragraphs} paragraphs"
        );
    }

    #[test]
    fn structural_edits_preserve_surviving_node_identity_and_undo_all_metadata() {
        let mut source = ["A🌲B", "", "tail"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                node(
                    Paragraph::new(text.into(), Format::default()),
                    Format {
                        language: Some(1033 + index as u32),
                        ..Format::default()
                    },
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        source[2].text_mut().unwrap().text = Paragraph::new(
            "tail".into(),
            Format {
                bold: Some(true),
                ..Format::default()
            },
        );
        let original = TextDocument::from_nodes(source).unwrap();
        let mut document = original.clone();
        let edit = document
            .replace(
                position(0, 1)..position(2, 2),
                ["x", "y", "z"]
                    .into_iter()
                    .map(|s| Paragraph::new(s.into(), Format::default()))
                    .collect(),
            )
            .unwrap();
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["Ax", "y", "zil"]
        );
        for index in [0, 2] {
            assert_eq!(document.nodes()[index].id, original.nodes()[index].id);
            assert_eq!(
                document.nodes()[index].text().unwrap().id,
                original.nodes()[index].text().unwrap().id
            );
            assert_eq!(
                document.nodes()[index].format,
                original.nodes()[index].format
            );
        }
        let old_ids: BTreeSet<_> = original
            .nodes()
            .iter()
            .flat_map(|n| [n.id, n.text().unwrap().id])
            .collect();
        assert!(!old_ids.contains(&document.nodes()[1].id));
        assert!(!old_ids.contains(&document.nodes()[1].text().unwrap().id));
        let edited = document.clone();
        let redo = document.apply(undo).unwrap();
        assert_eq!(document, original);
        document.apply(redo).unwrap();
        assert_eq!(document, edited);
        let mut split = original.clone();
        let edit = split
            .replace(
                position(0, 1)..position(0, 1),
                vec![Paragraph::new(String::new(), Format::default()); 2],
            )
            .unwrap();
        split.apply(edit).unwrap();
        assert_eq!(split.nodes()[0].id, original.nodes()[0].id);
        assert!(!old_ids.contains(&split.nodes()[1].id));
        assert_eq!(&split.nodes()[2..], &original.nodes()[1..]);
    }

    #[test]
    fn replacement_cannot_alias_existing_objects_or_publish_unsupported_nodes() {
        let mut document =
            TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap();
        let original = document.clone();
        let duplicate = DocumentEdit {
            columns: BTreeMap::new(),
            container: None,
            range: 1..1,
            replacement: original.nodes().to_vec(),
        };
        assert_eq!(document.apply(duplicate), Err(EditError::InvalidStructure));
        let mut unsupported = original.nodes().to_vec();
        unsupported[0].content =
            onestore::page::ParagraphContent::Unsupported(onestore::page::Unsupported {
                id: new_id().unwrap(),
                jcid: 0x60012,
                layout: Default::default(),
            });
        assert_eq!(
            TextDocument::from_nodes(unsupported.clone()),
            Err(EditError::UnsupportedContent)
        );
        assert_eq!(
            document.apply(DocumentEdit {
                columns: BTreeMap::new(),
                container: None,
                range: 0..1,
                replacement: unsupported
            }),
            Err(EditError::UnsupportedContent)
        );
        assert_eq!(document, original);
    }

    #[test]
    fn paragraph_links_require_an_earlier_parent_at_a_shallower_level() {
        let mut nodes = TextDocument::new(
            ["parent", "child", "grandchild"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap()
        .nodes;
        nodes[1].parent = Some(nodes[0].id);
        nodes[1].level = 3;
        nodes[2].parent = Some(nodes[1].id);
        nodes[2].level = 5;
        let original = TextDocument::from_nodes(nodes.clone()).unwrap();
        for parent in [nodes[2].id, nodes[0].text().unwrap().id, new_id().unwrap()] {
            let mut invalid = nodes.clone();
            invalid[1].parent = Some(parent);
            assert_eq!(
                TextDocument::from_nodes(invalid),
                Err(EditError::InvalidStructure)
            );
        }
        for level in [0, 1] {
            let mut invalid = nodes.clone();
            invalid[1].level = level;
            assert_eq!(
                TextDocument::from_nodes(invalid),
                Err(EditError::InvalidStructure)
            );
        }
        let mut document = original.clone();
        assert_eq!(
            document.apply(DocumentEdit {
                columns: BTreeMap::new(),
                container: None,
                range: 0..1,
                replacement: Vec::new()
            }),
            Err(EditError::InvalidStructure)
        );
        assert_eq!(document, original);
    }

    #[test]
    fn editing_nested_tagged_text_preserves_metadata_and_refuses_structural_changes() {
        let mut nodes = TextDocument::new(
            ["parent", "a🌳e\u{301}z", "child"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap()
        .nodes;
        nodes[1].parent = Some(nodes[0].id);
        nodes[1].level = 2;
        nodes[1].lists.push(new_id().unwrap());
        nodes[1].tags.push(onestore::document::Tag {
            definition: Some(new_id().unwrap()),
            action_type: Some(0),
            status: 1,
            created: Some(123),
            completed: None,
            start: Some(124),
            due: Some(456),
            task_id: Some([7; 16]),
            extra_set: 0,
        });
        nodes[1].text_mut().unwrap().tags = nodes[1].tags.clone();
        nodes[1].collapsed = true;
        nodes[2].parent = Some(nodes[1].id);
        nodes[2].level = 3;
        let original = TextDocument::from_nodes(nodes).unwrap();
        let text = original.paragraphs().nth(1).unwrap();
        let offsets: Vec<_> = text
            .text()
            .char_indices()
            .map(|(byte, _)| text.utf16_offset(byte).unwrap())
            .chain(std::iter::once(
                text.utf16_offset(text.text().len()).unwrap(),
            ))
            .collect();
        for &start in &offsets {
            for &end in offsets.iter().filter(|&&end| end >= start) {
                let mut document = original.clone();
                let edit = document
                    .replace(
                        position(1, start)..position(1, end),
                        vec![Paragraph::new(
                            "👩🏽‍💻".into(),
                            Format {
                                bold: Some(true),
                                ..Format::default()
                            },
                        )],
                    )
                    .unwrap();
                let undo = document.apply(edit).unwrap();
                let edited = document.clone();
                let mut restored = edited.nodes().to_vec();
                restored[1].text_mut().unwrap().text = text.clone();
                assert_eq!(restored, original.nodes());
                let redo = document.apply(undo).unwrap();
                assert_eq!(document, original);
                document.apply(redo).unwrap();
                assert_eq!(document, edited);
            }
        }
        for (range, count) in [
            (position(1, 0)..position(1, 0), 2),
            (position(0, 6)..position(1, 0), 1),
            (position(1, 0)..position(2, 0), 1),
        ] {
            assert_eq!(
                original.replace(
                    range,
                    vec![Paragraph::new(String::new(), Format::default()); count]
                ),
                Err(EditError::UnsupportedContent)
            );
        }
    }

    #[test]
    fn split_join_and_undo_preserve_empty_paragraph_formats() {
        let regular = Format::default();
        let bold = Format {
            bold: Some(true),
            ..Format::default()
        };
        let original = TextDocument::new(vec![
            Paragraph::new("left".into(), regular.clone()),
            Paragraph::new(String::new(), bold.clone()),
            Paragraph::new("right".into(), bold.clone()),
        ])
        .unwrap();
        let mut document = original.clone();
        let split = document
            .replace(
                position(0, 2)..position(0, 2),
                vec![Paragraph::new(String::new(), regular.clone()); 2],
            )
            .unwrap();
        let undo = document.apply(split).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["le", "ft", "", "right"]
        );
        let redo = document.apply(undo).unwrap();
        assert_eq!(document, original);
        document.apply(redo).unwrap();
        let join = document
            .replace(
                position(1, 2)..position(3, 0),
                vec![Paragraph::new(String::new(), regular)],
            )
            .unwrap();
        let before_join = document.clone();
        let undo = document.apply(join).unwrap();
        assert_eq!(document.paragraphs().nth(1).unwrap().text(), "ftright");
        assert_eq!(
            document.paragraphs().nth(1).unwrap().spans()[1].format,
            bold
        );
        document.apply(undo).unwrap();
        assert_eq!(document, before_join);
    }

    #[test]
    fn every_scalar_range_replaces_across_paragraphs_and_round_trips() {
        let regular = Format::default();
        let bold = Format {
            bold: Some(true),
            ..Format::default()
        };
        let original = TextDocument::new(vec![
            Paragraph::from_runs([
                ("a🌳".into(), regular.clone()),
                ("e\u{301}".into(), bold.clone()),
            ]),
            Paragraph::new(String::new(), bold.clone()),
            Paragraph::new("日本z".into(), regular.clone()),
        ])
        .unwrap();
        for (original, regions) in [
            (original, vec![0, 0, 0]),
            (table_document(), vec![0, 1, 1, 2, 3]),
        ] {
            let positions: Vec<_> = original
                .paragraphs()
                .enumerate()
                .flat_map(|(index, paragraph)| {
                    (0..=paragraph.text().len()).filter_map(move |byte| {
                        paragraph
                            .utf16_offset(byte)
                            .ok()
                            .map(|offset| position(index, offset))
                    })
                })
                .collect();
            let replacements = [
                vec![Paragraph::new(String::new(), regular.clone())],
                vec![Paragraph::new("x👩🏽‍💻".into(), bold.clone())],
                vec![
                    Paragraph::new(String::new(), bold.clone()),
                    Paragraph::new(String::new(), regular.clone()),
                ],
                vec![
                    Paragraph::new("one".into(), bold.clone()),
                    Paragraph::new(String::new(), regular.clone()),
                    Paragraph::new("two".into(), bold.clone()),
                ],
            ];
            let plain = original
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>()
                .join("\n");
            for &start in &positions {
                for &end in positions.iter().filter(|&&end| end >= start) {
                    for replacement in &replacements {
                        let mut document = original.clone();
                        let planned = document.replace(start..end, replacement.clone());
                        if regions[start.paragraph] != regions[end.paragraph] {
                            assert_eq!(planned, Err(EditError::UnsupportedContent));
                            assert_eq!(document, original);
                            continue;
                        }
                        let planned = planned.unwrap();
                        assert_eq!(document, original);
                        let undo = document.apply(planned).unwrap();
                        assert!(document.paragraphs().next().is_some());
                        let byte = |position: TextPosition| {
                            original
                                .paragraphs()
                                .take(position.paragraph)
                                .map(|p| p.text().len() + 1)
                                .sum::<usize>()
                                + original
                                    .paragraphs()
                                    .nth(position.paragraph)
                                    .unwrap()
                                    .byte_offset(position.offset)
                                    .unwrap()
                        };
                        let mut expected = plain.clone();
                        expected.replace_range(
                            byte(start)..byte(end),
                            &replacement
                                .iter()
                                .map(Paragraph::text)
                                .collect::<Vec<_>>()
                                .join("\n"),
                        );
                        assert_eq!(
                            document
                                .paragraphs()
                                .map(Paragraph::text)
                                .collect::<Vec<_>>()
                                .join("\n"),
                            expected
                        );
                        let after = document.clone();
                        let redo = document.apply(undo).unwrap();
                        assert_eq!(document, original, "{start:?}..{end:?}");
                        document.apply(redo).unwrap();
                        assert_eq!(document, after);
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_ranges_and_last_paragraph_removal_are_atomic() {
        let original =
            TextDocument::new(vec![Paragraph::new("🌳".into(), Format::default())]).unwrap();
        for range in [
            position(0, 1)..position(0, 2),
            position(0, 0)..position(1, 0),
            position(0, 2)..position(0, 0),
        ] {
            assert!(
                original
                    .replace(
                        range,
                        vec![Paragraph::new(String::new(), Format::default())]
                    )
                    .is_err()
            );
        }
        for range in [0..1, 0..2, Range { start: 2, end: 1 }] {
            let mut document = original.clone();
            assert!(
                document
                    .apply(DocumentEdit {
                        columns: BTreeMap::new(),
                        container: None,
                        range,
                        replacement: Vec::new()
                    })
                    .is_err()
            );
            assert_eq!(document, original);
        }
        assert!(TextDocument::new(Vec::new()).is_err());
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION pointing to the native Tab table capture"]
    fn native_table_import() {
        use onestore::{
            RevisionIndex, Store,
            document::Document,
            page::{Page, PageObject, ParagraphContent},
        };
        let page = {
            let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
            let store = Store::parse(&bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            for (space, page) in document.pages().unwrap() {
                let space = &document.spaces[&space];
                let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
                Page::from_revision(revision, page).unwrap();
            }
            Page::from_document(&document, "rows").unwrap()
        };
        let table = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) => {
                    outline.paragraphs.iter().find_map(|p| match &p.content {
                        ParagraphContent::Table(table) => Some(table),
                        _ => None,
                    })
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.columns.len(), 2);
        assert!(table.columns.iter().all(|c| !c.locked));
        let cells: Vec<_> = table.rows.iter().flat_map(|row| &row.cells).collect();
        let text: Vec<_> = cells
            .iter()
            .map(|cell| cell.paragraphs[0].text().unwrap().text.text())
            .collect();
        assert_eq!(text, ["Alpha", "Beta", "Gamma", "Delta", "Epsilon", ""]);
        assert!(cells.iter().all(|c| c.indents == [18.0, 0.0, 27.0, 27.0]));
        let outline = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline)
                    if outline
                        .paragraphs
                        .iter()
                        .any(|p| matches!(p.content, ParagraphContent::Table(_))) =>
                {
                    Some(outline)
                }
                _ => None,
            })
            .unwrap();
        let mut document = TextDocument::from_nodes(outline.paragraphs.clone()).unwrap();
        let original = document.clone();
        for (index, cell) in cells.iter().enumerate() {
            let position = TextPosition {
                paragraph: index,
                offset: 0,
            };
            let edit = document
                .replace(
                    position..position,
                    vec![
                        Paragraph::new("🧊".into(), Format::default()),
                        Paragraph::new("text".into(), Format::default()),
                    ],
                )
                .unwrap();
            assert_eq!(edit.container, Some(cell.id));
            let undo = document.apply(edit).unwrap();
            assert_eq!(document.paragraphs().nth(index).unwrap().text(), "🧊");
            document.apply(undo).unwrap();
            assert_eq!(document, original);
        }
    }
}
