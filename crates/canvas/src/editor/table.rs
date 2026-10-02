use super::*;
use onestore::page::text::new_id;
use onestore::page::{ParagraphContent, Table, TableCell, TableColumn, TableRow};

struct CellLocation<'a> {
    container: Option<ExGuid>,
    paragraph: usize,
    node: &'a PageParagraph,
    row: usize,
    column: usize,
}

fn locate(document: &TextDocument, id: ExGuid) -> Option<CellLocation<'_>> {
    let mut pending = vec![(None, document.nodes())];
    while let Some((container, nodes)) = pending.pop() {
        for (paragraph, node) in nodes.iter().enumerate() {
            if let ParagraphContent::Table(table) = &node.content {
                for (row, source) in table.rows.iter().enumerate() {
                    for (column, cell) in source.cells.iter().enumerate() {
                        if cell.id == id {
                            return Some(CellLocation {
                                container,
                                paragraph,
                                node,
                                row,
                                column,
                            });
                        }
                        pending.push((Some(cell.id), cell.paragraphs.as_slice()));
                    }
                }
            }
        }
    }
    None
}

/// Table `id`'s paragraph, its container and its index there.
fn table_node(
    document: &TextDocument,
    id: ExGuid,
) -> Result<(Option<ExGuid>, usize, &PageParagraph), EditError> {
    descendants(document.nodes(), None)
        .find(|(_, _, node)| {
            matches!(&node.content, ParagraphContent::Table(table) if table.id == id)
        })
        .ok_or(EditError::InvalidRange)
}

/// A new cell like `source`, one empty paragraph in its first text's format; a cell holding
/// no text, only pictures or files, gives a plain one.
fn empty_cell(source: &TableCell) -> Result<TableCell, EditError> {
    let node = match leaves(&source.paragraphs, None).next() {
        Some((_, _, paragraph)) => {
            let format = paragraph.text().unwrap().text.format_at(0)?;
            let mut node = crate::document::node(
                Paragraph::new(String::new(), format.clone()),
                paragraph.format.clone(),
            )?;
            node.style = paragraph.style;
            node
        }
        None => crate::document::node(
            Paragraph::new(String::new(), Default::default()),
            Default::default(),
        )?,
    };
    new_cell(source.indents.clone(), source.shading, vec![node])
}

/// A new cell of `paragraphs`.
fn new_cell(
    indents: Vec<f32>,
    shading: Option<u32>,
    paragraphs: Vec<PageParagraph>,
) -> Result<TableCell, EditError> {
    Ok(TableCell {
        id: new_id()?,
        layout: Default::default(),
        indents,
        shading,
        paragraphs,
        unsupported: Vec::new(),
    })
}

/// A new bordered table of `rows` and `columns` new columns, in `source`'s place in the
/// outline's tree.
fn table_paragraph(
    source: &PageParagraph,
    columns: usize,
    rows: Vec<TableRow>,
) -> Result<PageParagraph, EditError> {
    Ok(PageParagraph {
        id: new_id()?,
        parent: source.parent,
        level: source.level,
        style: None,
        format: source.format.clone(),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
        content: ParagraphContent::Table(Table {
            id: new_id()?,
            columns: vec![
                TableColumn {
                    width: COLUMN_WIDTH,
                    locked: false
                };
                columns
            ],
            rows,
            borders: Some(true),
            layout: Default::default(),
            tags: Vec::new(),
        }),
    })
}

fn cell_range(document: &TextDocument, cell: &TableCell) -> Result<Range<TextPosition>, EditError> {
    let mut content = leaves(&cell.paragraphs, None);
    let (_, _, first) = content.next().ok_or(EditError::InvalidStructure)?;
    let (_, _, last) = content.last().unwrap_or((None, 0, first));
    let start = document
        .text_nodes()
        .position(|node| node.id == first.id)
        .ok_or(EditError::InvalidStructure)?;
    let end = document
        .text_nodes()
        .position(|node| node.id == last.id)
        .ok_or(EditError::InvalidStructure)?;
    let text = &last.text().unwrap().text;
    Ok(TextPosition {
        paragraph: start,
        offset: 0,
    }..TextPosition {
        paragraph: end,
        offset: text.utf16_offset(text.text().len())?,
    })
}

/// OneNote 2010's width for a new column, and the narrowest it fits or drags one to.
pub(super) const COLUMN_WIDTH: f32 = 37.11;
/// What OneNote 2010 fits an unlocked column to beyond its widest line (lab, 2026-09-30).
const COLUMN_ROOM: f32 = 4.347;

/// `width` as it reads back from the file, which stores it in half inches.
fn stored(width: f32) -> f32 {
    width / 36.0 * 36.0
}

/// `table`'s widths with its unlocked columns in `only`, or all, fit to their widest cell
/// as `natural` measures it, the table no wider than `room` unless its narrowest columns are.
fn fitted(
    table: &Table,
    only: Option<usize>,
    room: f32,
    mut natural: impl FnMut(&TableCell) -> Result<f32, LayoutError>,
) -> Result<Vec<f32>, LayoutError> {
    let mut widths = table
        .columns
        .iter()
        .map(|column| column.width)
        .collect::<Vec<_>>();
    for (index, column) in table.columns.iter().enumerate() {
        if column.locked || only.is_some_and(|only| only != index) {
            continue;
        }
        let mut widest = 0.0_f32;
        for row in &table.rows {
            widest = widest.max(natural(
                row.cells.get(index).ok_or(LayoutError::InvalidWidth)?,
            )?);
        }
        // Columns sit 4.98 pt apart, and the table ends 3.15 pt past the last one.
        let others = widths.iter().map(|width| width + 4.98).sum::<f32>() - widths[index] - 1.83;
        widths[index] = stored((widest + COLUMN_ROOM).min(room - others).max(COLUMN_WIDTH));
    }
    Ok(widths)
}

/// Fits the tables in `nodes`, innermost first, placed by `indents` in an outline `wrap` wide.
fn fit_tables(
    nodes: &mut [PageParagraph],
    indents: &[f32],
    wrap: f32,
    natural: &mut impl FnMut(&TableCell) -> Result<f32, LayoutError>,
) -> Result<(), LayoutError> {
    for node in nodes {
        let level = node.level;
        if let ParagraphContent::Table(table) = &mut node.content {
            for TableCell {
                paragraphs,
                indents,
                ..
            } in table.rows.iter_mut().flat_map(|row| &mut row.cells)
            {
                fit_tables(paragraphs, indents, wrap, natural)?;
            }
            let room = wrap - crate::outline::indentation(level, indents, wrap)?;
            let widths = fitted(table, None, room, &mut *natural)?;
            for (column, width) in table.columns.iter_mut().zip(widths) {
                column.width = width;
            }
        }
    }
    Ok(())
}

impl CanvasEditor {
    /// Fits the unlocked columns `edit` writes in to their content, as OneNote 2010 widens
    /// and narrows them while typing: the tables it adds or rewrites, and the column of each
    /// table around the cell it edits. A table stops at the outline's width and wraps there.
    pub(super) fn fit_columns(
        &self,
        engine: &mut TextEngine,
        edit: &mut DocumentEdit,
    ) -> Result<(), EditorError> {
        let outline = self.active_outline();
        if outline.title {
            return Ok(());
        }
        let wrap = outline.wrap_width();
        let mut natural = |cell: &TableCell, edit: Option<&DocumentEdit>| {
            let flow = OutlineLayout::flow(
                crate::document::edited_nodes(&cell.paragraphs, Some(cell.id), edit),
                &cell.indents,
                f32::from(u16::MAX),
                false,
                1,
                edit,
                &mut |node, previous, width, indents| {
                    ParagraphLayout::shape(
                        engine,
                        node,
                        previous,
                        width,
                        indents,
                        &self.definitions,
                    )
                },
            )?;
            Ok::<_, LayoutError>(flow.content_width())
        };
        let indents = |container: Option<ExGuid>| match container {
            Some(cell) => {
                let location =
                    locate(&outline.document, cell).ok_or(EditError::InvalidStructure)?;
                let ParagraphContent::Table(table) = &location.node.content else {
                    unreachable!()
                };
                Ok::<_, EditError>(&table.rows[location.row].cells[location.column].indents)
            }
            None => Ok(&outline.indents),
        };
        fit_tables(
            &mut edit.replacement,
            indents(edit.container)?,
            wrap,
            &mut |cell| natural(cell, None),
        )?;
        let mut container = edit.container;
        while let Some(cell) = container {
            let location = locate(&outline.document, cell).ok_or(EditError::InvalidStructure)?;
            let ParagraphContent::Table(table) = &location.node.content else {
                unreachable!()
            };
            if !edit.columns.contains_key(&table.id) {
                let room = wrap
                    - crate::outline::indentation(
                        location.node.level,
                        indents(location.container)?,
                        wrap,
                    )?;
                let widths = fitted(table, Some(location.column), room, |cell| {
                    natural(cell, Some(edit))
                })?;
                if widths
                    .iter()
                    .zip(&table.columns)
                    .any(|(width, column)| *width != column.width)
                {
                    edit.columns.insert(table.id, widths);
                }
            }
            container = location.container;
        }
        Ok(())
    }
}

impl TextOutline {
    /// The table column whose right border lies within `reach` of outline-local `point`, as
    /// `(table, column, width)`.
    pub(crate) fn column_border(
        &self,
        point: [f32; 2],
        reach: f32,
    ) -> Option<(ExGuid, usize, f32)> {
        let columns = descendants(self.document.nodes(), None)
            .filter_map(|(_, _, node)| match &node.content {
                ParagraphContent::Table(table) => Some((table.id, table.columns.len())),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        // Nested tables follow the tables holding them.
        self.shaped.tables.iter().rev().find_map(|table| {
            let (first, last) = (table.cells.first()?, table.cells.last()?);
            if !(first.rect[1]..=last.rect[3]).contains(&point[1]) {
                return None;
            }
            let row = table.cells.get(..*columns.get(&table.id)?)?;
            row.iter().enumerate().find_map(|(column, cell)| {
                // A cell's box reaches 3.6 pt before its column and 1.38 pt past it.
                ((point[0] - cell.rect[2]).abs() <= reach)
                    .then(|| (table.id, column, cell.rect[2] - cell.rect[0] - 4.98))
            })
        })
    }
}

impl CanvasEditor {
    /// The active outline with column `column` of `table` `width` wide, as a border drag
    /// shows it before release.
    pub(crate) fn preview_column(
        &self,
        engine: &mut TextEngine,
        table: ExGuid,
        column: usize,
        width: f32,
    ) -> Result<TextOutline, EditorError> {
        let outline = self.active_outline();
        let (_, _, node) = table_node(&outline.document, table)?;
        let ParagraphContent::Table(source) = &node.content else {
            unreachable!()
        };
        let mut widths = source
            .columns
            .iter()
            .map(|column| column.width)
            .collect::<Vec<_>>();
        *widths.get_mut(column).ok_or(EditError::InvalidRange)? = width.max(COLUMN_WIDTH);
        let edit = DocumentEdit {
            container: None,
            range: 0..0,
            replacement: Vec::new(),
            columns: [(table, widths)].into(),
        };
        let shaped = OutlineLayout::flow(
            outline.document.nodes().iter(),
            &outline.indents,
            outline.wrap_width(),
            outline.layout.width_set_by_user == Some(true),
            0,
            Some(&edit),
            &mut |node, previous, width, indents| {
                ParagraphLayout::shape(engine, node, previous, width, indents, &self.definitions)
            },
        )?;
        let mut preview = outline.clone();
        preview.document.apply(edit)?;
        preview.shaped = shaped;
        Ok(preview)
    }

    /// Drags a border to make column `column` of `table` `width` wide, as OneNote 2010 does:
    /// no narrower than a new column, locked against fitting, the columns after it moving
    /// with it. One edit and one undo step.
    pub fn resize_column(
        &mut self,
        engine: &mut TextEngine,
        table: ExGuid,
        column: usize,
        width: f32,
    ) -> Result<(), EditorError> {
        if !width.is_finite() {
            return Err(LayoutError::InvalidWidth.into());
        }
        let outline = self.active_outline();
        let (container, index, node) = table_node(&outline.document, table)?;
        let mut node = node.clone();
        let ParagraphContent::Table(source) = &mut node.content else {
            unreachable!()
        };
        let resized = TableColumn {
            width: stored(width.max(COLUMN_WIDTH)),
            locked: true,
        };
        let slot = source
            .columns
            .get_mut(column)
            .ok_or(EditError::InvalidRange)?;
        if *slot == resized {
            return Ok(());
        }
        *slot = resized;
        let selection = outline.selection;
        self.finish_composition();
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index..index + 1,
                replacement: vec![node],
            },
            selection,
        )
    }
    /// Insert, Table: an empty table of `rows` by `columns` at the caret with the caret in
    /// its first cell, as OneNote 2010 inserts one: in place of an empty paragraph, before
    /// or after the caret's paragraph at its start or end, and between its halves otherwise.
    pub fn insert_table(
        &mut self,
        engine: &mut TextEngine,
        rows: usize,
        columns: usize,
    ) -> Result<(), EditorError> {
        let outline = self.active_outline();
        if outline.title || rows == 0 || columns == 0 {
            return Ok(());
        }
        let [anchor, focus] = outline.selection.positions;
        let caret = anchor.max(focus);
        let (container, index, node) = outline
            .document
            .leaf(caret.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let text = &node.text().unwrap().text;
        let end = text.utf16_offset(text.text().len())?;
        let format = text.format_at(caret.offset)?;
        let empty = || -> Result<TableCell, EditError> {
            let mut paragraph = crate::document::node(
                Paragraph::new(String::new(), format.clone()),
                node.format.clone(),
            )?;
            paragraph.style = node.style;
            new_cell(outline.indents.clone(), None, vec![paragraph])
        };
        let rows = (0..rows)
            .map(|_| {
                Ok(TableRow {
                    id: new_id()?,
                    cells: (0..columns).map(|_| empty()).collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, EditError>>()?;
        let table = table_paragraph(node, columns, rows)?;
        let parent = |candidate: &PageParagraph| candidate.parent == Some(node.id);
        let childless = !outline.document.container(container)?.iter().any(parent);
        let (edit, first) = if end == 0 && childless {
            let edit = DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index..index + 1,
                replacement: vec![table],
            };
            (edit, caret.paragraph)
        } else if caret.offset == 0 && end > 0 {
            let edit = DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index..index,
                replacement: vec![table],
            };
            (edit, caret.paragraph)
        } else if caret.offset == end {
            let edit = DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index + 1..index + 1,
                replacement: vec![table],
            };
            (edit, caret.paragraph + 1)
        } else {
            let empty = Paragraph::new(String::new(), format.clone());
            let mut edit = outline
                .document
                .replace(caret..caret, vec![empty.clone(), empty])?;
            edit.replacement.insert(1, table);
            (edit, caret.paragraph + 1)
        };
        let first = TextPosition {
            paragraph: first,
            offset: 0,
        };
        self.commit(engine, edit, [first; 2].into())
    }

    pub fn tab(&mut self, engine: &mut TextEngine, backward: bool) -> Result<(), EditorError> {
        if self.active_outline().title && !backward {
            return self.leave_title(engine);
        }
        let outline = self.active_outline();
        let [anchor, focus] = outline.selection.positions;
        let (cell, local, source) = outline
            .document
            .leaf(focus.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let Some(cell) = cell else {
            if backward || anchor != focus || focus.offset == 0 {
                self.indent(engine, backward)?;
                return Ok(());
            }
            let format = source.text().unwrap().text.format_at(focus.offset)?;
            let mut split = outline.document.replace(
                focus..focus,
                vec![Paragraph::new(String::new(), format.clone()); 2],
            )?;
            // The table's paragraph holds the list, tags and children (`evidence/structural-edits/
            // xml/c6-*-midtab.xml`, `c10-tab-parent-1.xml`).
            let children = split.replacement.split_off(2);
            let head = &mut split.replacement[0];
            let lists = std::mem::take(&mut head.lists);
            let mut tags = std::mem::take(&mut head.tags);
            tags.append(&mut head.text_mut().unwrap().tags);
            let collapsed = std::mem::take(&mut split.replacement[1].collapsed);
            let tail = split.replacement[1].id;
            let cells = split
                .replacement
                .drain(..)
                .map(|mut paragraph| {
                    paragraph.level = 1;
                    paragraph.parent = None;
                    paragraph.lists.clear();
                    new_cell(outline.indents.clone(), None, vec![paragraph])
                })
                .collect::<Result<Vec<_>, EditError>>()?;
            let row = TableRow {
                id: new_id()?,
                cells,
            };
            let wrapper = PageParagraph {
                lists,
                tags,
                collapsed,
                ..table_paragraph(source, 2, vec![row])?
            };
            let id = wrapper.id;
            split.replacement.push(wrapper);
            split
                .replacement
                .extend(children.into_iter().map(|mut child| {
                    if child.parent == Some(tail) {
                        child.parent = Some(id);
                    }
                    child
                }));
            return self.commit(
                engine,
                split,
                [TextPosition {
                    paragraph: focus.paragraph + 1,
                    offset: 0,
                }; 2]
                    .into(),
            );
        };
        let location = locate(&outline.document, cell).ok_or(EditError::InvalidStructure)?;
        let ParagraphContent::Table(table) = &location.node.content else {
            unreachable!()
        };
        let slot = location.row * table.columns.len() + location.column;
        let next = if backward {
            slot.checked_sub(1)
        } else if slot + 1 < table.rows.len() * table.columns.len() {
            Some(slot + 1)
        } else {
            None
        };
        if let Some(next) = next {
            let target = &table.rows[next / table.columns.len()].cells[next % table.columns.len()];
            let range = cell_range(&outline.document, target)?;
            return self
                .select([range.start, range.end].into())
                .map_err(Into::into);
        }
        if backward {
            return Ok(());
        }
        let mut wrapper = location.node.clone();
        let ParagraphContent::Table(table) = &mut wrapper.content else {
            unreachable!()
        };
        let next = if table.rows.len() == 1 {
            let format = source.text().unwrap().text.format_at(focus.offset)?;
            let split = outline.document.replace(
                focus..focus,
                vec![Paragraph::new(String::new(), format.clone()); 2],
            )?;
            let source = &mut table.rows[0].cells[location.column];
            source.paragraphs.splice(split.range, split.replacement);
            let paragraphs = source.paragraphs.split_off(local + 1);
            let target = new_cell(source.indents.clone(), source.shading, paragraphs)?;
            table.rows[0].cells.push(target);
            table.columns.push(TableColumn {
                width: COLUMN_WIDTH,
                locked: false,
            });
            TextPosition {
                paragraph: focus.paragraph + 1,
                offset: 0,
            }
        } else {
            let cells = table
                .rows
                .last()
                .unwrap()
                .cells
                .iter()
                .map(empty_cell)
                .collect::<Result<Vec<_>, _>>()?;
            table.rows.push(TableRow {
                id: new_id()?,
                cells,
            });
            let paragraph = cell_range(
                &outline.document,
                &table.rows[location.row].cells[location.column],
            )?
            .end
            .paragraph
                + 1;
            TextPosition {
                paragraph,
                offset: 0,
            }
        };
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container: location.container,
                range: location.paragraph..location.paragraph + 1,
                replacement: vec![wrapper],
            },
            [next; 2].into(),
        )
    }
    pub fn enter(&mut self, engine: &mut TextEngine, soft: bool) -> Result<(), EditorError> {
        let [anchor, focus] = self.active_outline().selection.positions;
        // Inside a link's label OneNote 2010 ignores Shift+Enter, and Enter follows the link
        // (`PageView` asks the host), so neither breaks it.
        if anchor == focus
            && self.link_at(focus).is_some_and(|link| {
                link.label.start < focus.offset && focus.offset < link.label.end
            })
        {
            return Ok(());
        }
        if soft {
            return self.insert(engine, "\u{000b}");
        }
        self.take_objects()?;
        if self.active_outline().title {
            return self.leave_title(engine);
        }
        let [anchor, focus] = self.active_outline().selection.positions;
        if anchor == focus {
            self.link_typed_url(engine, focus)?;
            // Enter at a label's start splits before its field code.
            self.leave_link_code()?;
            if self.break_equation(engine)? {
                return Ok(());
            }
        }
        let [anchor, focus] = self.active_outline().selection.positions;
        let outline = self.active_outline();
        let (cell, local, source) = outline
            .document
            .leaf(focus.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let Some(cell) = cell.filter(|_| anchor == focus) else {
            return self.split(engine);
        };
        let location = locate(&outline.document, cell).ok_or(EditError::InvalidStructure)?;
        let ParagraphContent::Table(table) = &location.node.content else {
            unreachable!()
        };
        let row = &table.rows[location.row];
        let exit = location.row + 1 == table.rows.len()
            && location.row > 0
            && location.column == 0
            && local == 0
            && focus.offset == 0
            && row.cells.iter().all(|cell| {
                cell.paragraphs
                    .iter()
                    .all(|node| node.text().is_some_and(|text| text.text.text().is_empty()))
            });
        let text = &source.text().unwrap().text;
        let append = location.row + 1 == table.rows.len()
            && location.column + 1 == table.columns.len()
            && local + 1 == row.cells[location.column].paragraphs.len()
            && focus.offset == text.utf16_offset(text.text().len())?;
        if !exit && !append {
            return self.split(engine);
        }
        let mut wrapper = location.node.clone();
        let ParagraphContent::Table(table) = &mut wrapper.content else {
            unreachable!()
        };
        let replacement = if exit {
            table.rows.pop();
            let blank = || {
                Ok::<_, EditError>(PageParagraph {
                    parent: location.node.parent,
                    level: location.node.level,
                    format: location.node.format.clone(),
                    ..self.blank_paragraph(source)?
                })
            };
            vec![wrapper, blank()?, blank()?]
        } else {
            let cells = row
                .cells
                .iter()
                .map(empty_cell)
                .collect::<Result<Vec<_>, _>>()?;
            table.rows.push(TableRow {
                id: new_id()?,
                cells,
            });
            vec![wrapper]
        };
        let next = TextPosition {
            paragraph: focus.paragraph + 1,
            offset: 0,
        };
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container: location.container,
                range: location.paragraph..location.paragraph + 1,
                replacement,
            },
            [next; 2].into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::document::Format;

    fn editor(engine: &mut TextEngine, text: &str) -> CanvasEditor {
        CanvasEditor::new(
            engine,
            TextDocument::new(vec![Paragraph::new(text.into(), Format::default())]).unwrap(),
            180.0,
        )
        .unwrap()
    }

    fn cells(editor: &CanvasEditor) -> Vec<Vec<String>> {
        let ParagraphContent::Table(table) = &editor.active_outline().document.nodes()[0].content
        else {
            panic!()
        };
        table
            .rows
            .iter()
            .flat_map(|row| &row.cells)
            .map(|cell| {
                cell.paragraphs
                    .iter()
                    .map(|node| node.text().unwrap().text.text().to_owned())
                    .collect()
            })
            .collect()
    }

    /// As OneNote 2010 inserts from the Table gallery (lab, 2026-09-28).
    #[test]
    fn an_inserted_table_takes_an_empty_paragraph_or_splits_at_the_caret() {
        let mut engine = TextEngine::default();
        let texts = |editor: &CanvasEditor| {
            editor
                .active_outline()
                .document
                .nodes()
                .iter()
                .map(|node| match &node.content {
                    ParagraphContent::Table(table) => {
                        format!("{}x{}", table.columns.len(), table.rows.len())
                    }
                    _ => node.text().unwrap().text.text().to_owned(),
                })
                .collect::<Vec<_>>()
        };
        for (text, offset, expected) in [
            ("", 0, vec!["3x2"]),
            ("abc", 0, vec!["3x2", "abc"]),
            ("abc", 3, vec!["abc", "3x2"]),
            ("abc", 1, vec!["a", "3x2", "bc"]),
        ] {
            let mut editor = editor(&mut engine, text);
            let caret = TextPosition {
                paragraph: 0,
                offset,
            };
            editor.select([caret; 2].into()).unwrap();
            editor.insert_table(&mut engine, 2, 3).unwrap();
            assert_eq!(texts(&editor), expected, "{text:?} at {offset}");
            let first = usize::from(expected[0] != "3x2");
            let (cell, _, _) = editor
                .active_outline()
                .document
                .leaf(editor.selection().positions[0].paragraph)
                .unwrap();
            assert!(cell.is_some() && editor.selection().positions[0].paragraph == first);
        }
    }

    #[test]
    fn tab_creates_splits_and_selects_native_cells() {
        let mut engine = TextEngine::default();
        let mut editor = editor(&mut engine, "abcdef");
        let original = editor.active_outline().document.clone();
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 3,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.tab(&mut engine, false).unwrap();
        assert_eq!(cells(&editor), [vec!["abc"], vec!["def"]]);
        assert_eq!(
            editor
                .active_outline()
                .document
                .text_nodes()
                .next()
                .unwrap()
                .id,
            original.nodes()[0].id
        );
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 1,
                offset: 0
            }; 2]
        );
        editor.insert(&mut engine, "X").unwrap();
        editor.tab(&mut engine, false).unwrap();
        assert_eq!(cells(&editor), [vec!["abc"], vec!["X"], vec!["def"]]);
        editor.tab(&mut engine, true).unwrap();
        assert_eq!(
            editor.selection().positions,
            [
                TextPosition {
                    paragraph: 1,
                    offset: 0
                },
                TextPosition {
                    paragraph: 1,
                    offset: 1
                }
            ]
        );
        editor.insert(&mut engine, "REPLACED").unwrap();
        assert_eq!(cells(&editor)[1], ["REPLACED"]);
        editor.tab(&mut engine, true).unwrap();
        assert_eq!(
            editor.selection().positions,
            [
                TextPosition {
                    paragraph: 0,
                    offset: 0
                },
                TextPosition {
                    paragraph: 0,
                    offset: 3
                }
            ]
        );
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 2,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.tab(&mut engine, true).unwrap();
        editor.insert(&mut engine, "!").unwrap();
        assert_eq!(cells(&editor)[0], ["ab!c"]);
        editor.tab(&mut engine, false).unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 1,
                offset: 8
            }
        );
        assert_eq!(editor.active_outline().layout.max_width, Some(180.0));
        for _ in 0..5 {
            assert!(editor.undo(&mut engine).unwrap());
        }
        assert_eq!(editor.active_outline().document, original);
        for _ in 0..5 {
            assert!(editor.redo(&mut engine).unwrap());
        }
        assert_eq!(
            cells(&editor),
            [vec!["ab!c"], vec!["REPLACED"], vec!["def"]]
        );
    }

    #[test]
    fn adding_rows_preserves_nested_cells_and_undo() {
        let mut engine = TextEngine::default();
        let mut outer = editor(&mut engine, "");
        outer.insert(&mut engine, "Alpha").unwrap();
        outer.tab(&mut engine, false).unwrap();
        outer.insert(&mut engine, "Beta").unwrap();
        outer.enter(&mut engine, false).unwrap();
        outer.insert(&mut engine, "Gamma").unwrap();
        outer.tab(&mut engine, false).unwrap();
        outer.insert(&mut engine, "Delta").unwrap();
        let mut inner = editor(&mut engine, "");
        inner.insert(&mut engine, "Nested").unwrap();
        inner.tab(&mut engine, false).unwrap();
        let mut source = outer.active_outline().snapshot();
        let ParagraphContent::Table(table) = &mut source.paragraphs[0].content else {
            panic!()
        };
        table.rows[1].cells[0].paragraphs = inner.active_outline().document.nodes().to_vec();
        for tab in [false, true] {
            let mut editor =
                CanvasEditor::from_outlines(&mut engine, vec![source.clone()], BTreeMap::new())
                    .unwrap();
            let original = editor.active_outline().document.clone();
            let end = TextPosition {
                paragraph: original.text_nodes().count() - 1,
                offset: 5,
            };
            editor.select([end; 2].into()).unwrap();
            if tab {
                editor.tab(&mut engine, false).unwrap();
            } else {
                editor.enter(&mut engine, false).unwrap();
            }
            let added = editor.active_outline().document.clone();
            let ParagraphContent::Table(before) = &original.nodes()[0].content else {
                panic!()
            };
            let ParagraphContent::Table(after) = &added.nodes()[0].content else {
                panic!()
            };
            assert_eq!(after.rows.len(), 3);
            assert_eq!(&after.rows[..2], &before.rows);
            // The first column widens to the table placed in it unfitted.
            assert!(after.columns[0].width > before.columns[0].width + 40.0);
            assert_eq!(after.columns[1], before.columns[1]);
            for cell in &after.rows[2].cells {
                assert_eq!(cell.paragraphs.len(), 1);
                assert!(cell.paragraphs[0].text().unwrap().text.text().is_empty());
            }
            assert_eq!(editor.selection().positions[1].paragraph, end.paragraph + 1);
            editor.insert(&mut engine, "New row").unwrap();
            assert_eq!(
                editor
                    .active_outline()
                    .document
                    .paragraphs()
                    .nth(end.paragraph + 1)
                    .unwrap()
                    .text(),
                "New row"
            );
            assert!(editor.undo(&mut engine).unwrap());
            assert_eq!(editor.active_outline().document, added);
            assert!(editor.undo(&mut engine).unwrap());
            assert_eq!(editor.active_outline().document, original);
            assert_eq!(editor.selection().positions, [end; 2]);
            assert!(editor.redo(&mut engine).unwrap());
            assert_eq!(editor.active_outline().document, added);
        }
    }

    #[test]
    fn native_rows_exit_and_text_have_separate_undo_steps() {
        let mut engine = TextEngine::default();
        let mut editor = editor(&mut engine, "Alpha");
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 5,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.tab(&mut engine, false).unwrap();
        editor.insert(&mut engine, "Beta").unwrap();
        editor.enter(&mut engine, false).unwrap();
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 2,
                offset: 0
            }; 2]
        );
        editor.insert(&mut engine, "Gamma").unwrap();
        editor.tab(&mut engine, false).unwrap();
        editor.insert(&mut engine, "Delta").unwrap();
        let two_rows = editor.active_outline().document.clone();
        editor.tab(&mut engine, false).unwrap();
        editor.insert(&mut engine, "Epsilon").unwrap();
        assert_eq!(
            cells(&editor),
            [
                vec!["Alpha"],
                vec!["Beta"],
                vec!["Gamma"],
                vec!["Delta"],
                vec!["Epsilon"],
                vec![""]
            ]
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(cells(&editor)[4], [""]);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, two_rows);
        editor.redo(&mut engine).unwrap();
        editor.enter(&mut engine, false).unwrap();
        assert_eq!(cells(&editor).len(), 4);
        assert_eq!(editor.active_outline().document.nodes().len(), 3);
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 5,
                offset: 0
            }; 2]
        );
        editor.insert(&mut engine, "Outside").unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .last()
                .unwrap()
                .text(),
            "Outside"
        );
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        assert_eq!(cells(&editor).len(), 6);
    }

    #[test]
    fn enter_and_soft_break_stay_in_cells_until_the_final_end() {
        let mut engine = TextEngine::default();
        let mut editor = editor(&mut engine, "Alpha");
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 5,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.tab(&mut engine, false).unwrap();
        editor.insert(&mut engine, "Beta").unwrap();
        editor
            .select(
                [TextPosition {
                    paragraph: 1,
                    offset: 2,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.enter(&mut engine, false).unwrap();
        assert_eq!(cells(&editor), [vec!["Alpha"], vec!["Be", "ta"]]);
        editor.enter(&mut engine, true).unwrap();
        assert_eq!(cells(&editor)[1], ["Be", "\u{000b}ta"]);
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 5,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.enter(&mut engine, false).unwrap();
        assert_eq!(cells(&editor)[0], ["Alpha", ""]);
    }

    #[test]
    fn paragraph_start_and_selection_tab_indent_without_creating_a_table() {
        let mut engine = TextEngine::default();
        let mut editor = editor(&mut engine, "Alpha");
        editor.tab(&mut engine, false).unwrap();
        assert_eq!(editor.active_outline().document.nodes()[0].level, 2);
        editor
            .select(
                [
                    TextPosition {
                        paragraph: 0,
                        offset: 1,
                    },
                    TextPosition {
                        paragraph: 0,
                        offset: 3,
                    },
                ]
                .into(),
            )
            .unwrap();
        let selection = editor.selection();
        editor.tab(&mut engine, false).unwrap();
        assert_eq!(editor.active_outline().document.nodes()[0].level, 3);
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "Alpha"
        );
        assert_eq!(editor.selection(), selection);
        editor.tab(&mut engine, true).unwrap();
        assert_eq!(editor.active_outline().document.nodes()[0].level, 2);
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 5,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.tab(&mut engine, false).unwrap();
        assert_eq!(editor.active_outline().document.nodes()[0].level, 2);
        assert_eq!(
            editor
                .active_outline()
                .document
                .text_nodes()
                .next()
                .unwrap()
                .level,
            1
        );
        assert_eq!(editor.active_outline().shaped.paragraphs[0].origin[0], 27.0);
    }

    fn calibri(engine: &mut TextEngine) -> CanvasEditor {
        let format = Format {
            font: Some("Calibri".into()),
            font_size: Some(11.0),
            ..Format::default()
        };
        CanvasEditor::new(
            engine,
            TextDocument::new(vec![Paragraph::new(String::new(), format)]).unwrap(),
            468.0,
        )
        .unwrap()
    }

    fn table(editor: &CanvasEditor) -> &onestore::page::Table {
        let ParagraphContent::Table(table) = &editor.active_outline().document.nodes()[0].content
        else {
            panic!()
        };
        table
    }

    fn widths(editor: &CanvasEditor) -> Vec<f32> {
        table(editor)
            .columns
            .iter()
            .map(|column| column.width)
            .collect()
    }

    fn type_rows(editor: &mut CanvasEditor, engine: &mut TextEngine, rows: &[&[&str]]) {
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                editor.enter(engine, false).unwrap();
            }
            for (column, text) in row.iter().enumerate() {
                if column > 0 {
                    editor.tab(engine, false).unwrap();
                }
                editor.insert(engine, text).unwrap();
            }
        }
    }

    /// OneNote 2010's widths for the same table typed the same way (lab, 2026-09-30).
    #[test]
    fn typing_fits_unlocked_columns_to_their_widest_line() {
        let mut engine = TextEngine::default();
        let mut editor = calibri(&mut engine);
        type_rows(
            &mut editor,
            &mut engine,
            &[
                &["Fruit", "Colour", "Notes"],
                &["Apple", "Red", "Crisp and sweet, good for pies"],
                &["Watermelon", "Green", "Summer"],
            ],
        );
        for (width, onenote) in widths(&editor)
            .into_iter()
            .zip([60.75945, 37.11, 139.08636])
        {
            assert!((width - onenote).abs() < 0.01, "{width} against {onenote}");
        }
        // Deleting the widest line narrows the column to the next widest, here the minimum.
        let end = TextPosition {
            paragraph: 6,
            offset: 10,
        };
        editor
            .select([TextPosition { offset: 0, ..end }, end].into())
            .unwrap();
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(widths(&editor)[0], COLUMN_WIDTH);
        assert!(editor.undo(&mut engine).unwrap());
        assert!((widths(&editor)[0] - 60.75945).abs() < 0.01);
    }

    #[test]
    fn a_widening_keystroke_stores_its_width_with_its_text() {
        let mut engine = TextEngine::default();
        let mut editor = calibri(&mut engine);
        type_rows(&mut editor, &mut engine, &[&["A", ""]]);
        editor.take_ops().unwrap();
        let before = widths(&editor);
        editor.insert(&mut engine, "Watermelon").unwrap();
        let ops = editor.take_ops().unwrap();
        let columns = ops
            .iter()
            .filter_map(|op| match op {
                PageOp::Table {
                    edit: onestore::op::TableEdit::Columns(columns),
                    ..
                } => Some(
                    columns
                        .iter()
                        .map(|column| column.width)
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(columns, [widths(&editor)]);
        assert!(ops.len() > 1);
        assert!(widths(&editor)[1] > before[1]);
        // A keystroke the column already holds stores no width.
        editor.delete(&mut engine, true).unwrap();
        editor.insert(&mut engine, "n").unwrap();
        editor.take_ops().unwrap();
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 1,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.insert(&mut engine, "!").unwrap();
        assert!(!editor.take_ops().unwrap().iter().any(|op| matches!(
            op,
            PageOp::Table {
                edit: onestore::op::TableEdit::Columns(_),
                ..
            }
        )));
    }

    /// OneNote 2010 stops a table at the outline's width and wraps the cell from there.
    #[test]
    fn a_table_stops_widening_at_the_outline_width() {
        let mut engine = TextEngine::default();
        let mut editor = calibri(&mut engine);
        type_rows(&mut editor, &mut engine, &[&["A", ""]]);
        for _ in 0..40 {
            editor.insert(&mut engine, "word ").unwrap();
        }
        let widths = widths(&editor);
        let table = widths.iter().map(|width| width + 4.98).sum::<f32>() - 1.83;
        assert!((table - 468.0).abs() < 0.01, "{table}");
        assert!(
            editor.active_outline().shaped.paragraphs[1]
                .text
                .lines()
                .count()
                > 1
        );
    }

    #[test]
    fn a_dragged_column_locks_moves_the_columns_after_it_and_undoes_in_one_step() {
        let mut engine = TextEngine::default();
        let mut editor = calibri(&mut engine);
        type_rows(&mut editor, &mut engine, &[&["B", "Word"]]);
        let id = table(&editor).id;
        let fitted = widths(&editor);
        let cells = |editor: &CanvasEditor| {
            editor.active_outline().shaped.tables[0]
                .cells
                .iter()
                .map(|cell| cell.rect)
                .collect::<Vec<_>>()
        };
        let before = cells(&editor);
        // The border of the first column, from its cell's right edge.
        let border = [before[0][2], (before[0][1] + before[0][3]) / 2.0];
        assert_eq!(
            editor.active_outline().column_border(border, 2.0),
            Some((id, 0, fitted[0]))
        );
        assert_eq!(
            editor
                .active_outline()
                .column_border([border[0] - 10.0, border[1]], 2.0),
            None
        );
        let preview = editor.preview_column(&mut engine, id, 0, 88.86).unwrap();
        assert_eq!(widths(&editor), fitted);
        editor.take_ops().unwrap();
        let history = editor.undo.len();
        editor.resize_column(&mut engine, id, 0, 88.86).unwrap();
        assert_eq!(editor.undo.len(), history + 1);
        assert_eq!(
            table(&editor).columns[0],
            TableColumn {
                width: 88.86,
                locked: true
            }
        );
        assert_eq!(
            table(&editor).columns[1],
            TableColumn {
                width: fitted[1],
                locked: false
            }
        );
        assert_eq!(cells(&editor), {
            let ParagraphContent::Table(_) = &preview.document.nodes()[0].content else {
                panic!()
            };
            preview.shaped.tables[0]
                .cells
                .iter()
                .map(|cell| cell.rect)
                .collect::<Vec<_>>()
        });
        let moved = cells(&editor);
        assert!((moved[1][0] - before[1][0] - (88.86 - fitted[0])).abs() < 0.001);
        let ops = editor.take_ops().unwrap();
        assert!(
            matches!(
                ops.as_slice(),
                [PageOp::Table {
                    edit: onestore::op::TableEdit::Columns(_),
                    ..
                }]
            ),
            "{ops:?}"
        );
        // A locked column keeps its width while typing; dragging stops at a new column's width.
        editor
            .insert(&mut engine, " and a long line of text")
            .unwrap();
        assert_eq!(table(&editor).columns[0].width, 88.86);
        editor.resize_column(&mut engine, id, 1, 5.0).unwrap();
        assert_eq!(
            table(&editor).columns[1],
            TableColumn {
                width: COLUMN_WIDTH,
                locked: true
            }
        );
        for _ in 0..3 {
            editor.undo(&mut engine).unwrap();
        }
        assert_eq!(widths(&editor), fitted);
        assert!(table(&editor).columns.iter().all(|column| !column.locked));
        assert_eq!(cells(&editor), before);
    }
}
