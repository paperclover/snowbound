use super::*;
use onestore::page::text::new_id;
use onestore::page::{ParagraphContent, TableCell, TableColumn, TableRow};

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

fn empty_cell(source: &TableCell) -> Result<TableCell, EditError> {
    let (_, _, paragraph) = leaves(&source.paragraphs, None)
        .next()
        .ok_or(EditError::UnsupportedContent)?;
    let format = paragraph.text().unwrap().text.format_at(0)?;
    let mut node = crate::document::node(
        Paragraph::new(String::new(), format.clone()),
        paragraph.format.clone(),
    )?;
    node.style = paragraph.style;
    Ok(TableCell {
        id: new_id()?,
        layout: Default::default(),
        indents: source.indents.clone(),
        shading: source.shading,
        paragraphs: vec![node],
        unsupported: Vec::new(),
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

impl CanvasEditor {
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
        let cell = || -> Result<TableCell, EditError> {
            let mut paragraph = crate::document::node(
                Paragraph::new(String::new(), format.clone()),
                node.format.clone(),
            )?;
            paragraph.style = node.style;
            Ok(TableCell {
                id: new_id()?,
                layout: Default::default(),
                indents: outline.indents.clone(),
                shading: None,
                paragraphs: vec![paragraph],
                unsupported: Vec::new(),
            })
        };
        let table = PageParagraph {
            id: new_id()?,
            parent: node.parent,
            level: node.level,
            style: None,
            format: node.format.clone(),
            lists: Vec::new(),
            tags: Vec::new(),
            media: Default::default(),
            collapsed: false,
            content: ParagraphContent::Table(onestore::page::Table {
                id: new_id()?,
                columns: vec![
                    TableColumn {
                        width: 37.11,
                        locked: false
                    };
                    columns
                ],
                rows: (0..rows)
                    .map(|_| {
                        Ok(TableRow {
                            id: new_id()?,
                            cells: (0..columns).map(|_| cell()).collect::<Result<_, _>>()?,
                        })
                    })
                    .collect::<Result<_, EditError>>()?,
                borders: Some(true),
                layout: Default::default(),
                tags: Vec::new(),
            }),
        };
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
                vec![
                    Paragraph::new(String::new(), format.clone()),
                    Paragraph::new(String::new(), format.clone()),
                ],
            )?;
            let id = new_id()?;
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
                    Ok(TableCell {
                        id: new_id()?,
                        layout: Default::default(),
                        indents: outline.indents.clone(),
                        shading: None,
                        paragraphs: vec![paragraph],
                        unsupported: Vec::new(),
                    })
                })
                .collect::<Result<Vec<_>, EditError>>()?;
            let wrapper = PageParagraph {
                id,
                parent: source.parent,
                level: source.level,
                style: None,
                format: source.format.clone(),
                lists,
                tags,
                media: Default::default(),
                collapsed,
                content: ParagraphContent::Table(onestore::page::Table {
                    id: new_id()?,
                    columns: vec![
                        TableColumn {
                            width: 37.11,
                            locked: false
                        };
                        2
                    ],
                    rows: vec![TableRow {
                        id: new_id()?,
                        cells,
                    }],
                    borders: Some(true),
                    layout: Default::default(),
                    tags: Vec::new(),
                }),
            };
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
                vec![
                    Paragraph::new(String::new(), format.clone()),
                    Paragraph::new(String::new(), format.clone()),
                ],
            )?;
            let cell = &mut table.rows[0].cells[location.column];
            cell.paragraphs.splice(split.range, split.replacement);
            let paragraphs = cell.paragraphs.split_off(local + 1);
            let target = TableCell {
                id: new_id()?,
                layout: Default::default(),
                indents: cell.indents.clone(),
                shading: cell.shading,
                paragraphs,
                unsupported: Vec::new(),
            };
            table.rows[0].cells.push(target);
            table.columns.push(TableColumn {
                width: 37.11,
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
        if soft {
            return self.insert(engine, "\u{000b}");
        }
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
        let (replacement, next) = if exit {
            table.rows.pop();
            let blank = || {
                Ok::<_, EditError>(PageParagraph {
                    parent: location.node.parent,
                    level: location.node.level,
                    format: location.node.format.clone(),
                    ..self.blank_paragraph(source)?
                })
            };
            let (first, second) = (blank()?, blank()?);
            (
                vec![wrapper, first, second],
                TextPosition {
                    paragraph: focus.paragraph + 1,
                    offset: 0,
                },
            )
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
            (
                vec![wrapper],
                TextPosition {
                    paragraph: focus.paragraph + 1,
                    offset: 0,
                },
            )
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
            assert_eq!(after.columns, before.columns);
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
}
