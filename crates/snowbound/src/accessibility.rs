use accesskit::{
    Action, Affine, Node, NodeId, Rect, Role, TextDirection, TextSelection, TreeId, TreeInfo,
    TreeUpdate,
};
use canvas::gpu::Viewport;
use canvas::{
    date::DateField,
    document::TextPosition,
    editor::{CanvasEditor, Selection, TextOutline},
};
use onestore::page::text::EditError;
use parley::Affinity;
use std::{collections::HashMap, ops::Range};

pub const ROOT: NodeId = NodeId(0);

struct Run {
    outline: onestore::ExGuid,
    id: NodeId,
    node: Node,
    positions: Vec<TextPosition>,
}

#[derive(Hash, PartialEq, Eq)]
struct ParagraphKey {
    layout: u64,
    source: usize,
    next: Option<usize>,
    origin: [u32; 2],
}

pub struct Accessibility {
    outlines: Vec<(onestore::ExGuid, NodeId)>,
    runs: Vec<Run>,
    paragraphs: HashMap<ParagraphKey, Range<usize>>,
    read_only: Vec<NodeId>,
    dates: Vec<(DateField, NodeId)>,
    next_id: u64,
}

impl Default for Accessibility {
    fn default() -> Self {
        Self {
            outlines: Vec::new(),
            runs: Vec::new(),
            paragraphs: HashMap::new(),
            read_only: Vec::new(),
            dates: Vec::new(),
            next_id: 1,
        }
    }
}

impl Accessibility {
    /// Retire published runs without recycling their IDs.
    pub fn deactivate(&mut self) {
        self.runs.clear();
        self.paragraphs.clear();
        self.outlines.clear();
        self.read_only.clear();
        self.dates.clear();
    }

    pub fn append_page_fields(
        &mut self,
        update: &mut TreeUpdate,
        scene: Option<&(canvas::gpu::page::PageScene, [f32; 2])>,
        editor: &CanvasEditor,
        viewport: Viewport,
        focus: Option<usize>,
    ) {
        let mut count = 0;
        if let Some((scene, offset)) = scene {
            for (index, object) in scene.read_only(Some(editor)).enumerate() {
                if index == self.read_only.len() {
                    let id = self.allocate();
                    self.read_only.push(id);
                }
                let id = self.read_only[index];
                let mut node = Node::new(Role::Label);
                node.set_value(object.message);
                node.set_read_only();
                node.add_action(Action::Focus);
                if focus == Some(index) {
                    update.focus = id;
                }
                let [x0, y0, x1, y1] = object.rect();
                let x = |v| f64::from((v + offset[0]) * viewport.scale + viewport.origin[0]);
                let y = |v| f64::from((v + offset[1]) * viewport.scale + viewport.origin[1]);
                node.set_bounds(Rect::new(x(x0), y(y0), x(x1), y(y1)));
                update
                    .nodes
                    .iter_mut()
                    .find(|(id, _)| *id == ROOT)
                    .unwrap()
                    .1
                    .push_child(id);
                update.nodes.push((id, node));
                count += 1;
            }
        }
        self.read_only.truncate(count);
        let mut date_count = 0;
        if let Some((scene, offset)) = scene {
            for (index, (field, rect)) in scene.date_fields(editor).enumerate() {
                if index == self.dates.len() {
                    let id = self.allocate();
                    self.dates.push((field, id));
                }
                self.dates[index].0 = field;
                let id = self.dates[index].1;
                let mut node = Node::new(Role::Button);
                node.set_label(crate::DATE_LABELS[field as usize]);
                node.set_value(
                    editor.date().unwrap().source().paragraphs[index]
                        .text()
                        .unwrap()
                        .text
                        .text(),
                );
                node.add_action(Action::Click);
                let x = |v| f64::from((v + offset[0]) * viewport.scale + viewport.origin[0]);
                let y = |v| f64::from((v + offset[1]) * viewport.scale + viewport.origin[1]);
                node.set_bounds(Rect::new(x(rect[0]), y(rect[1]), x(rect[2]), y(rect[3])));
                update
                    .nodes
                    .iter_mut()
                    .find(|(id, _)| *id == ROOT)
                    .unwrap()
                    .1
                    .push_child(id);
                update.nodes.push((id, node));
                date_count += 1;
            }
        }
        self.dates.truncate(date_count);
    }

    fn allocate(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("Accessibility node IDs exhausted");
        id
    }

    pub fn read_only_for_node(&self, node: NodeId) -> Option<usize> {
        self.read_only.iter().position(|id| *id == node)
    }

    pub fn date_for_node(&self, node: NodeId) -> Option<DateField> {
        self.dates
            .iter()
            .find(|(_, id)| *id == node)
            .map(|(field, _)| *field)
    }

    pub fn outline_for_node(&self, node: NodeId) -> Option<onestore::ExGuid> {
        self.outlines
            .iter()
            .find(|(_, id)| *id == node)
            .map(|(outline, _)| *outline)
    }

    pub fn update(
        &mut self,
        editor: &CanvasEditor,
        viewport: Viewport,
        title: &str,
        preview: Option<(onestore::ExGuid, [f32; 2])>,
    ) -> Result<TreeUpdate, EditError> {
        let mut runs = Vec::new();
        let mut outlines = Vec::new();
        let mut paragraph_ranges = HashMap::new();
        for outline in editor.visible_outlines().chain(editor.caret_outline()) {
            let field = self
                .outlines
                .iter()
                .find(|(id, _)| *id == outline.id)
                .map(|(_, node)| *node)
                .unwrap_or_else(|| self.allocate());
            outlines.push((outline.id, field));
            let mut paragraphs = outline.layouts().peekable();
            while let Some((paragraph, shaped)) = paragraphs.next() {
                let layout = &shaped.text;
                let origin = shaped.origin;
                let key = ParagraphKey {
                    layout: layout.id(),
                    source: paragraph,
                    next: paragraphs.peek().map(|(next, _)| *next),
                    origin: origin.map(f32::to_bits),
                };
                let start = runs.len();
                if let Some(range) = self.paragraphs.get(&key) {
                    runs.extend(
                        self.runs[range.clone()]
                            .iter()
                            .map(|run| (run.outline, run.node.clone(), run.positions.clone())),
                    );
                    paragraph_ranges.insert(key, start..runs.len());
                    continue;
                }
                let projection = &shaped.projection;
                let text = projection.text().text();
                let boundaries: Vec<_> = projection.source_boundaries().collect();
                let source_position = |byte| -> Result<TextPosition, EditError> {
                    let index = boundaries
                        .binary_search_by_key(&byte, |(byte, _)| *byte)
                        .map_err(|_| EditError::InvalidRange)?;
                    Ok(TextPosition {
                        paragraph,
                        offset: boundaries[index].1,
                    })
                };
                let line_count = layout.lines().count();
                for (line_index, (line, bounds)) in layout.lines().enumerate() {
                    let line_start = runs.len();
                    let mut x = line.metrics().offset;
                    let mut logical_runs = Vec::new();
                    for run in line.runs() {
                        let advance = run.advance();
                        logical_runs.push((run, x));
                        x += advance;
                    }
                    logical_runs.sort_by_key(|(run, _)| run.text_range().start);
                    for (run, x) in logical_runs {
                        // AccessKit stores each character's byte length in a u8.
                        let mut characters = Vec::new();
                        for cluster in run.clusters() {
                            let range = cluster.text_range();
                            if range.is_empty() || range.start == text.len() {
                                continue;
                            }
                            let cluster_text =
                                text.get(range.clone()).ok_or(EditError::InvalidRange)?;
                            if range.len() <= u8::MAX as usize {
                                characters.push((
                                    range,
                                    cluster.advance(),
                                    cluster.is_word_boundary(),
                                ));
                            } else {
                                for (index, (offset, ch)) in cluster_text.char_indices().enumerate()
                                {
                                    characters.push((
                                        range.start + offset..range.start + offset + ch.len_utf8(),
                                        if index == 0 { cluster.advance() } else { 0.0 },
                                        index == 0 && cluster.is_word_boundary(),
                                    ));
                                }
                            }
                        }
                        let mut advance = 0.0;
                        for chunk in characters.chunks(256) {
                            let width: f32 = chunk.iter().map(|(_, width, _)| width).sum();
                            let left = x + if run.is_rtl() {
                                run.advance() - advance - width
                            } else {
                                advance
                            };
                            let mut node = Node::new(Role::TextRun);
                            node.set_bounds(Rect::new(
                                f64::from(left + origin[0]),
                                f64::from(bounds.top + origin[1]),
                                f64::from(left + width + origin[0]),
                                f64::from(bounds.top + bounds.height + origin[1]),
                            ));
                            node.set_text_direction(if run.is_rtl() {
                                TextDirection::RightToLeft
                            } else {
                                TextDirection::LeftToRight
                            });
                            node.set_font_size(run.font_size());
                            let start = chunk[0].0.start;
                            let end = chunk.last().unwrap().0.end;
                            node.set_value(text.get(start..end).ok_or(EditError::InvalidRange)?);
                            node.set_character_lengths(
                                chunk
                                    .iter()
                                    .map(|(range, _, _)| u8::try_from(range.len()).unwrap())
                                    .collect::<Vec<_>>(),
                            );
                            let mut offset = 0.0;
                            node.set_character_positions(
                                chunk
                                    .iter()
                                    .map(|(_, width, _)| {
                                        let position = offset;
                                        offset += width;
                                        position
                                    })
                                    .collect::<Vec<_>>(),
                            );
                            node.set_character_widths(
                                chunk.iter().map(|(_, width, _)| *width).collect::<Vec<_>>(),
                            );
                            node.set_word_starts(
                                chunk
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(index, (_, _, word))| word.then_some(index as u8))
                                    .collect::<Vec<_>>(),
                            );
                            let positions = chunk
                                .iter()
                                .map(|(range, _, _)| source_position(range.start))
                                .chain(std::iter::once(source_position(end)))
                                .collect::<Result<Vec<_>, _>>()?;
                            runs.push((outline.id, node, positions));
                            advance += width;
                        }
                    }
                    // Empty lines still need a text position for selection and insertion.
                    if runs.len() == line_start {
                        let mut node = Node::new(Role::TextRun);
                        node.set_value("");
                        node.set_character_lengths(Vec::<u8>::new());
                        node.set_character_positions(Vec::<f32>::new());
                        node.set_character_widths(Vec::<f32>::new());
                        node.set_text_direction(TextDirection::LeftToRight);
                        node.set_bounds(Rect::new(
                            f64::from(origin[0]),
                            f64::from(bounds.top + origin[1]),
                            f64::from(origin[0]),
                            f64::from(bounds.top + bounds.height + origin[1]),
                        ));
                        runs.push((
                            outline.id,
                            node,
                            vec![source_position(bounds.source.start)?],
                        ));
                    }
                    if line_index + 1 == line_count
                        && let Some((next, _)) = paragraphs.peek()
                    {
                        let (_, node, source_positions) = runs.last_mut().unwrap();
                        let mut value = node.value().unwrap().to_owned();
                        value.push('\n');
                        node.set_value(value);
                        let mut lengths = node.character_lengths().to_vec();
                        lengths.push(1);
                        node.set_character_lengths(lengths);
                        if let Some(rect) = node.bounds() {
                            let mut positions =
                                node.character_positions().unwrap_or_default().to_vec();
                            positions.push(rect.width() as f32);
                            node.set_character_positions(positions);
                            let mut widths = node.character_widths().unwrap_or_default().to_vec();
                            widths.push(4.0);
                            node.set_character_widths(widths);
                        }
                        source_positions.push(TextPosition {
                            paragraph: *next,
                            offset: 0,
                        });
                    }
                }
                if !shaped.tags.is_empty() {
                    let descriptions = shaped
                        .tags
                        .iter()
                        .map(|tag| {
                            use canvas::outline::TagIcon;
                            let fallback = match tag.icon {
                                TagIcon::CheckBox { .. } => "To do",
                                TagIcon::Question => "Question",
                                TagIcon::Music => "Music",
                            };
                            let label = if tag.label.is_empty() {
                                fallback
                            } else {
                                &tag.label
                            };
                            let state = match tag.icon {
                                TagIcon::CheckBox { checked: true } => ", completed",
                                TagIcon::CheckBox { checked: false } => ", incomplete",
                                _ => "",
                            };
                            format!(
                                "{label}{state}{}",
                                if tag.disabled { ", disabled" } else { "" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    runs[start].1.set_description(descriptions);
                }
                paragraph_ranges.insert(key, start..runs.len());
            }
        }
        let mut runs: Vec<_> = runs
            .into_iter()
            .enumerate()
            .map(|(index, (outline, mut node, positions))| {
                node.clear_next_on_line();
                node.clear_previous_on_line();
                let id = match self.runs.get(index) {
                    Some(old)
                        if old.outline == outline
                            && old.positions == positions
                            && old.node.value() == node.value() =>
                    {
                        old.id
                    }
                    _ => self.allocate(),
                };
                Run {
                    outline,
                    id,
                    node,
                    positions,
                }
            })
            .collect();
        for index in 1..runs.len() {
            if runs[index - 1].outline == runs[index].outline
                && runs[index - 1].node.bounds().map(|r| r.y0)
                    == runs[index].node.bounds().map(|r| r.y0)
            {
                let (before, after) = runs.split_at_mut(index);
                before[index - 1].node.set_next_on_line(after[0].id);
                after[0].node.set_previous_on_line(before[index - 1].id);
            }
        }
        self.runs = runs;
        self.paragraphs = paragraph_ranges;
        self.outlines = outlines;
        let mut root = Node::new(Role::Window);
        root.set_label(title);
        root.set_children(self.outlines.iter().map(|(_, id)| *id).collect::<Vec<_>>());
        root.set_bounds(Rect::new(
            0.0,
            0.0,
            viewport.size[0] as f64,
            viewport.size[1] as f64,
        ));
        let mut nodes = vec![(ROOT, root)];
        let mut focus = ROOT;
        for (index, (outline, (_, id))) in editor
            .visible_outlines()
            .chain(editor.caret_outline())
            .zip(&self.outlines)
            .enumerate()
        {
            let mut field = Node::new(Role::MultilineTextInput);
            field.set_label(if outline.title {
                "Page title".into()
            } else if editor
                .caret_outline()
                .is_some_and(|caret| caret.id == outline.id)
            {
                "Text input".into()
            } else {
                format!("Text outline {}", index + 1)
            });
            field.add_action(Action::Focus);
            field.add_action(Action::SetTextSelection);
            field.add_action(Action::ReplaceSelectedText);
            field.add_action(Action::SetValue);
            let runs = self.runs.iter().filter(|run| run.outline == outline.id);
            field.set_value(
                runs.clone()
                    .filter_map(|run| run.node.value())
                    .collect::<String>(),
            );
            field.set_children(runs.map(|run| run.id).collect::<Vec<_>>());
            let origin = preview
                .filter(|(id, _)| *id == outline.id)
                .map(|(_, origin)| origin)
                .unwrap_or_else(|| outline.origin());
            field.set_transform(Affine::new([
                f64::from(viewport.scale),
                0.0,
                0.0,
                f64::from(viewport.scale),
                f64::from(viewport.origin[0]) + f64::from(origin[0]) * f64::from(viewport.scale),
                f64::from(viewport.origin[1]) + f64::from(origin[1]) * f64::from(viewport.scale),
            ]));
            let bounds = outline.bounds();
            field.set_bounds(Rect::new(0.0, 0.0, bounds.width(), bounds.height()));
            if outline.id == editor.active_outline().id {
                let Selection {
                    positions: [anchor, caret],
                    affinities,
                } = editor.selection();
                field.set_text_selection(TextSelection {
                    anchor: self.position(outline, anchor, affinities[0])?,
                    focus: self.position(outline, caret, affinities[1])?,
                });
                focus = *id;
            }
            nodes.push((*id, field));
        }
        nodes.extend(self.runs.iter().map(|run| (run.id, run.node.clone())));
        Ok(TreeUpdate {
            nodes,
            tree: Some(TreeInfo::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus,
        })
    }

    fn position(
        &self,
        outline: &TextOutline,
        mut position: TextPosition,
        affinity: Affinity,
    ) -> Result<accesskit::TextPosition, EditError> {
        let shaped = outline.paragraph_layout(position.paragraph)?;
        let projection = &shaped.projection;
        let layout = &shaped.text;
        let origin = shaped.origin;
        let cursor = layout.cursor(
            projection
                .text()
                .byte_offset(projection.visible_offset(position.offset)?)?,
            affinity,
        );
        position.offset = projection.source_offset(
            projection.text().utf16_offset(cursor.index())?,
            onestore::page::text::Affinity::Downstream,
        )?;
        let caret = layout.caret(cursor, 0.0);
        let mut positions = self
            .runs
            .iter()
            .filter(|run| {
                run.outline == outline.id
                    && run
                        .node
                        .bounds()
                        .is_some_and(|rect| rect.y0 == f64::from(caret.y0 as f32 + origin[1]))
            })
            .filter_map(|run| {
                run.positions
                    .iter()
                    .position(|p| *p == position)
                    .map(|character_index| accesskit::TextPosition {
                        node: run.id,
                        character_index,
                    })
            });
        match affinity {
            Affinity::Upstream => positions.next(),
            Affinity::Downstream => positions.next_back(),
        }
        .ok_or(EditError::InvalidRange)
    }

    pub fn selection(
        &self,
        outline: onestore::ExGuid,
        selection: TextSelection,
    ) -> Result<Selection, EditError> {
        let position = |position: accesskit::TextPosition| {
            let run = self
                .runs
                .iter()
                .find(|run| run.outline == outline && run.id == position.node)
                .ok_or(EditError::InvalidRange)?;
            let source = *run
                .positions
                .get(position.character_index)
                .ok_or(EditError::InvalidRange)?;
            let affinity = if position.character_index > 0
                && position.character_index + 1 == run.positions.len()
            {
                Affinity::Upstream
            } else {
                Affinity::Downstream
            };
            Ok::<_, EditError>((source, affinity))
        };
        let (anchor, anchor_affinity) = position(selection.anchor)?;
        let (focus, focus_affinity) = position(selection.focus)?;
        Ok(Selection {
            positions: [anchor, focus],
            affinities: [anchor_affinity, focus_affinity],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::{document::TextDocument, layout::TextEngine};
    use onestore::document::Format;
    use onestore::page::text::Paragraph;

    #[test]
    fn title_flow_updates_read_only_hit_and_accessibility_bounds() {
        use onestore::page::{Page, PageObject, Title, Unsupported};
        let mut engine = TextEngine::default();
        let mut title = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(
                "Header".into(),
                Format {
                    font: Some("Arial".into()),
                    font_size: Some(8.0),
                    line_spacing: Some(20.0),
                    ..Default::default()
                },
            )])
            .unwrap(),
            468.0,
            [0.0; 2],
        )
        .unwrap()
        .snapshot();
        title.title = true;
        let page = Page {
            title: "Header".into(),
            created: None,
            margin_origin: [36.0, 14.4],
            definitions: Default::default(),
            objects: vec![
                PageObject::Title(Title {
                    id: Default::default(),
                    date: None,
                    layout: Default::default(),
                    outlines: vec![title],
                }),
                PageObject::Unsupported(Unsupported {
                    id: Default::default(),
                    jcid: 0xdead,
                    layout: onestore::document::Layout {
                        x: Some(120.0),
                        y: Some(80.0),
                        max_width: Some(200.0),
                        max_height: Some(60.0),
                        ..Default::default()
                    },
                }),
            ],
        };
        let (scene, mut editor) =
            canvas::gpu::page::PageScene::from_page(page, &mut engine).unwrap();
        let scene = (scene, [30.0, 40.0]);
        let viewport = Viewport {
            size: [800, 600],
            origin: [10.0, -20.0],
            scale: 2.0,
        };
        let mut access = Accessibility::default();
        let mut identity = None;
        for phase in 0..4 {
            match phase {
                1 => {
                    editor.select_all().unwrap();
                    editor.insert(&mut engine, "Header\u{000b}Second\u{000b}Third\u{000b}Fourth\u{000b}Fifth\u{000b}Sixth").unwrap();
                }
                2 => {
                    editor.undo(&mut engine).unwrap();
                }
                3 => {
                    editor.redo(&mut engine).unwrap();
                }
                _ => {}
            }
            let mut update = access.update(&editor, viewport, "Test", None).unwrap();
            access.append_page_fields(&mut update, Some(&scene), &editor, viewport, Some(0));
            let (id, node) = update
                .nodes
                .iter()
                .find(|(_, node)| node.role() == Role::Label)
                .unwrap();
            if let Some(identity) = identity {
                assert_eq!(*id, identity);
            }
            identity = Some(*id);
            let y = if phase % 2 == 0 { 80.0 } else { 145.92 };
            let expected = Rect::new(
                310.0,
                f64::from((y + 40.0) * 2.0 - 20.0),
                710.0,
                f64::from((y + 100.0) * 2.0 - 20.0),
            );
            let bounds = node.bounds().unwrap();
            assert_eq!(bounds.x0, expected.x0);
            assert_eq!(bounds.x1, expected.x1);
            assert!((bounds.y0 - expected.y0).abs() < 0.001);
            assert!((bounds.y1 - expected.y1).abs() < 0.001);
            assert_eq!(
                crate::page_hit_test(&editor, Some(&scene), [155.0, y + 45.0], 1.0),
                Some(crate::Hit::ReadOnly(0))
            );
            if phase % 2 == 1 {
                assert_ne!(
                    crate::page_hit_test(&editor, Some(&scene), [155.0, 125.0], 1.0),
                    Some(crate::Hit::ReadOnly(0))
                );
            }
        }
    }

    #[test]
    fn read_only_objects_keep_accessibility_identity_through_edits_and_view_changes() {
        use canvas::gpu::page::PageScene;
        use onestore::page::{Page, PageObject, Unsupported};
        let page = Page {
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            definitions: Default::default(),
            objects: vec![PageObject::Unsupported(Unsupported {
                id: Default::default(),
                jcid: 0xdead,
                layout: onestore::document::Layout {
                    x: Some(10.0),
                    y: Some(20.0),
                    max_width: Some(200.0),
                    max_height: Some(60.0),
                    ..Default::default()
                },
            })],
        };
        let mut engine = TextEngine::default();
        let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
        let scene = (scene, [30.0, 40.0]);
        let mut access = Accessibility::default();
        let mut identity = None;
        for scale in [1.0, 2.0, 0.5] {
            editor.insert(&mut engine, "annotation ").unwrap();
            let viewport = Viewport {
                size: [800, 600],
                origin: [-200.0, 10.0],
                scale,
            };
            let mut update = access.update(&editor, viewport, "Test", None).unwrap();
            access.append_page_fields(&mut update, Some(&scene), &editor, viewport, None);
            let (id, node) = update
                .nodes
                .iter()
                .find(|(_, node)| node.role() == Role::Label)
                .unwrap();
            if let Some(identity) = identity {
                assert_eq!(*id, identity);
            }
            identity = Some(*id);
            assert_eq!(node.value(), Some("Unsupported content\nRead-only"));
            assert!(node.is_read_only());
            assert!(node.supports_action(Action::Focus));
            for action in [
                Action::SetValue,
                Action::ReplaceSelectedText,
                Action::SetTextSelection,
            ] {
                assert!(!node.supports_action(action));
            }
            assert_eq!(
                node.bounds(),
                Some(Rect::new(
                    f64::from(40.0 * scale - 200.0),
                    f64::from(60.0 * scale + 10.0),
                    f64::from(240.0 * scale - 200.0),
                    f64::from(120.0 * scale + 10.0)
                ))
            );
            assert_eq!(access.outline_for_node(*id), None);
            let tree = accesskit_consumer::Tree::new(update, true);
            assert_eq!(
                tree.state().focus().unwrap().document_range().text(),
                "annotation "
            );
            let mut focused = access.update(&editor, viewport, "Test", None).unwrap();
            access.append_page_fields(&mut focused, Some(&scene), &editor, viewport, Some(0));
            assert_eq!(focused.focus, identity.unwrap());
            assert_eq!(access.read_only_for_node(focused.focus), Some(0));
            let tree = accesskit_consumer::Tree::new(focused, true);
            assert_eq!(
                tree.state().focus().unwrap().value().as_deref(),
                Some("Unsupported content\nRead-only")
            );
            editor.undo(&mut engine).unwrap();
        }
        access.deactivate();
        assert!(access.read_only.is_empty());
    }

    #[test]
    fn tag_descriptions_preserve_plain_text_and_survive_edit_undo_and_cache_reuse() {
        use onestore::page::{Definition, Outline};
        use onestore::{
            ExGuid,
            document::{Kind, Layout, Tag},
        };
        use std::collections::BTreeMap;
        let text = TextDocument::new(
            ["task one", "task two", "question", "music"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap();
        let mut nodes = text.nodes().to_vec();
        let mut definitions = BTreeMap::new();
        for (index, (shape, label, status)) in [
            (3, Some("Rehearsal"), 0),
            (3, Some("Rehearsal"), 1),
            (15, None, 1),
            (121, None, 3),
        ]
        .into_iter()
        .enumerate()
        {
            let id = ExGuid {
                n: index as u32 + 500,
                ..ExGuid::default()
            };
            definitions.insert(
                id,
                Definition {
                    kind: Kind::TagDefinition {
                        label: label.map(str::to_owned),
                        action_type: None,
                        shape: Some(shape),
                        color: None,
                        highlight: None,
                    },
                    format: Format::default(),
                },
            );
            nodes[index].text_mut().unwrap().tags.push(Tag {
                definition: Some(id),
                action_type: None,
                status,
                created: None,
                completed: None,
                start: None,
                due: None,
                task_id: None,
                extra_set: 0,
            });
        }
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::from_outlines(
            &mut engine,
            vec![Outline {
                title: false,
                min_width: None,
                id: ExGuid {
                    n: 900,
                    ..ExGuid::default()
                },
                layout: Layout {
                    max_width: Some(120.0),
                    ..Layout::default()
                },
                indents: vec![18.0, 0.0],
                paragraphs: nodes,
                unsupported: Vec::new(),
            }],
            definitions,
        )
        .unwrap();
        let mut access = Accessibility::default();
        let viewport = Viewport {
            size: [800, 600],
            origin: [48.0; 2],
            scale: 2.0,
        };
        for step in 0..4 {
            if step == 2 {
                editor.insert(&mut engine, "new ").unwrap();
            }
            if step == 3 {
                editor.undo(&mut engine).unwrap();
            }
            let update = access.update(&editor, viewport, "Test", None).unwrap();
            assert_eq!(
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, n)| n.description())
                    .collect::<Vec<_>>(),
                [
                    "Rehearsal, incomplete",
                    "Rehearsal, completed",
                    "Question",
                    "Music, disabled"
                ]
            );
            let tree = accesskit_consumer::Tree::new(update, true);
            let text = tree.state().focus().unwrap().document_range().text();
            assert_eq!(
                text,
                if step == 2 {
                    "new task one\ntask two\nquestion\nmusic"
                } else {
                    "task one\ntask two\nquestion\nmusic"
                }
            );
        }
    }

    #[test]
    fn collapsed_paragraphs_keep_visible_accessibility_ranges_in_source_coordinates() {
        let document = TextDocument::new(
            [
                "root",
                "folded",
                "hidden",
                "last",
                "folded tail",
                "hidden tail",
            ]
            .into_iter()
            .map(|text| Paragraph::new(text.into(), Format::default()))
            .collect(),
        )
        .unwrap();
        let mut nodes = document.nodes().to_vec();
        for (child, parent, level) in [(1, 0, 2), (2, 1, 3), (4, 3, 2), (5, 4, 3)] {
            nodes[child].parent = Some(nodes[parent].id);
            nodes[child].level = level;
        }
        nodes[1].collapsed = true;
        nodes[4].collapsed = true;
        let mut engine = TextEngine::default();
        let mut editor =
            CanvasEditor::new(&mut engine, TextDocument::from_nodes(nodes).unwrap(), 180.0)
                .unwrap();
        let mut access = Accessibility::default();
        let viewport = Viewport {
            size: [900, 700],
            scale: 2.0,
            origin: [13.0, 19.0],
        };
        let update = access.update(&editor, viewport, "Test", None).unwrap();
        let tree = accesskit_consumer::Tree::new(update, true);
        let field = tree.state().focus().unwrap();
        assert_eq!(
            field.document_range().text(),
            "root\nfolded\nlast\nfolded tail"
        );
        assert!(
            access
                .runs
                .iter()
                .flat_map(|run| &run.positions)
                .all(|position| ![2, 5].contains(&position.paragraph))
        );
        let source = access
            .selection(
                editor.active_outline().id,
                field.document_range().to_text_selection(),
            )
            .unwrap();
        assert_eq!(
            source.positions[0],
            TextPosition {
                paragraph: 0,
                offset: 0
            }
        );
        assert_eq!(
            source.positions[1],
            TextPosition {
                paragraph: 4,
                offset: 11
            }
        );
        editor.select(source).unwrap();
        let update = access.update(&editor, viewport, "Test", None).unwrap();
        let tree = accesskit_consumer::Tree::new(update, true);
        assert_eq!(
            tree.state()
                .focus()
                .unwrap()
                .text_selection()
                .unwrap()
                .text(),
            "root\nfolded\nlast\nfolded tail"
        );
        editor
            .select(
                [TextPosition {
                    paragraph: 4,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        access.update(&editor, viewport, "Test", None).unwrap();
        let caret = editor.caret(1.0).unwrap();
        assert_eq!(caret.x0, 27.0);
        let position = access
            .position(
                editor.active_outline(),
                editor.selection().positions[1],
                Affinity::Downstream,
            )
            .unwrap();
        let run = access
            .runs
            .iter()
            .find(|run| run.id == position.node)
            .unwrap();
        assert_eq!(run.node.bounds().unwrap().x0, 27.0);
        assert_eq!(run.node.bounds().unwrap().y0, caret.y0);
    }

    #[test]
    fn consumer_reads_unicode_blank_paragraphs_and_hidden_fields() {
        let cases = [
            "",
            "a\n\nשלום\nend",
            "café e\u{301} 🌳 👩‍👩‍👧‍👦 words wrap here",
            "\n\n",
            "a\nb\n",
        ];
        let mut engine = TextEngine::default();
        for text in cases
            .into_iter()
            .map(str::to_owned)
            .chain([format!("a{}", "\u{301}".repeat(300)), "x".repeat(600)])
        {
            for width in [24.0, 480.0, 10000.0] {
                let document = TextDocument::new(
                    text.split('\n')
                        .map(|line| {
                            Paragraph::from_runs([
                                (
                                    String::from("secret"),
                                    Format {
                                        hidden: Some(true),
                                        ..Format::default()
                                    },
                                ),
                                (line.to_owned(), Format::default()),
                            ])
                        })
                        .collect(),
                )
                .unwrap();
                let mut editor = CanvasEditor::new(&mut engine, document, width).unwrap();
                let mut access = Accessibility::default();
                let viewport = Viewport {
                    size: [800, 600],
                    origin: [48.0; 2],
                    scale: 2.0,
                };
                let update = access.update(&editor, viewport, "Test", None).unwrap();
                for (_, node) in &update.nodes {
                    if node.role() == Role::TextRun {
                        assert_eq!(
                            node.character_lengths()
                                .iter()
                                .map(|n| *n as usize)
                                .sum::<usize>(),
                            node.value().unwrap().len()
                        );
                        if let Some(positions) = node.character_positions() {
                            assert_eq!(positions.len(), node.character_lengths().len());
                        }
                        if let Some(widths) = node.character_widths() {
                            assert_eq!(widths.len(), node.character_lengths().len());
                        }
                    }
                }
                let tree = accesskit_consumer::Tree::new(update, true);
                let field = tree.state().focus().unwrap();
                assert_eq!(field.document_range().text(), text, "width {width}");
                let selection = field.document_range().to_text_selection();
                let source_selection = access
                    .selection(editor.active_outline().id, selection)
                    .unwrap();
                editor.select(source_selection).unwrap();
                let selected = access.update(&editor, viewport, "Test", None).unwrap();
                let tree = accesskit_consumer::Tree::new(selected, true);
                assert_eq!(
                    tree.state()
                        .focus()
                        .unwrap()
                        .text_selection()
                        .unwrap()
                        .text(),
                    text
                );
                let stable_ids: Vec<_> = access.runs.iter().map(|run| run.id).collect();
                access
                    .update(
                        &editor,
                        Viewport {
                            scale: 3.0,
                            ..viewport
                        },
                        "Test",
                        None,
                    )
                    .unwrap();
                assert_eq!(
                    stable_ids,
                    access.runs.iter().map(|run| run.id).collect::<Vec<_>>()
                );
                access.deactivate();
                access.update(&editor, viewport, "Test", None).unwrap();
                assert!(
                    access
                        .selection(editor.active_outline().id, selection)
                        .is_err()
                );
                let selection = access
                    .update(&editor, viewport, "Test", None)
                    .unwrap()
                    .nodes[1]
                    .1
                    .text_selection()
                    .copied()
                    .unwrap();
                editor.insert(&mut engine, "replacement").unwrap();
                access.update(&editor, viewport, "Test", None).unwrap();
                assert!(
                    access
                        .selection(editor.active_outline().id, selection)
                        .is_err()
                );
                editor.undo(&mut engine).unwrap();
                let update = access.update(&editor, viewport, "Test", None).unwrap();
                let tree = accesskit_consumer::Tree::new(update, true);
                assert_eq!(tree.state().focus().unwrap().document_range().text(), text);
            }
        }
    }

    #[test]
    fn caret_geometry_uses_visible_offsets_and_canvas_line_metrics() {
        let mut engine = TextEngine::default();
        let document = TextDocument::new(vec![
            Paragraph::from_runs([
                ("abc ".into(), Format::default()),
                (
                    "hidden field".into(),
                    Format {
                        hidden: Some(true),
                        ..Format::default()
                    },
                ),
                ("e\u{301} 🌳 words wrap here".into(), Format::default()),
            ]),
            Paragraph::new("".into(), Format::default()),
            Paragraph::new("abc שלום def عالم end".into(), Format::default()),
            Paragraph::new("last\nline\n".into(), Format::default()),
        ])
        .unwrap();
        let positions: Vec<_> = document
            .paragraphs()
            .enumerate()
            .flat_map(|(paragraph, text)| {
                text.text()
                    .char_indices()
                    .map(|(byte, _)| text.utf16_offset(byte).unwrap())
                    .chain(std::iter::once(
                        text.utf16_offset(text.text().len()).unwrap(),
                    ))
                    .map(move |offset| TextPosition { paragraph, offset })
            })
            .collect();
        let mut editor = CanvasEditor::new(&mut engine, document, 48.0).unwrap();
        let mut access = Accessibility::default();
        let viewport = Viewport {
            size: [800, 600],
            origin: [31.0, -57.0],
            scale: 2.5,
        };
        for (position, affinity) in positions.into_iter().flat_map(|position| {
            [Affinity::Upstream, Affinity::Downstream].map(|affinity| (position, affinity))
        }) {
            editor
                .select(Selection {
                    positions: [position; 2],
                    affinities: [affinity; 2],
                })
                .unwrap();
            let caret = editor.caret(0.0).unwrap();
            let update = access.update(&editor, viewport, "Test", None).unwrap();
            let tree = accesskit_consumer::Tree::new(update, true);
            let field = tree.state().focus().unwrap();
            assert_eq!(
                field.document_range().text(),
                "abc e\u{301} 🌳 words wrap here\n\nabc שלום def عالم end\nlast\nline\n"
            );
            let rects = field.text_selection().unwrap().bounding_boxes();
            assert_eq!(rects.len(), 1, "{position:?}");
            let rect = rects[0];
            for (actual, expected) in [
                (rect.x0, caret.x0 * 2.5 + 31.0),
                (rect.y0, caret.y0 * 2.5 - 57.0),
                (rect.y1, caret.y1 * 2.5 - 57.0),
            ] {
                assert!(
                    (actual - expected).abs() < 0.0001,
                    "{position:?} {affinity:?}: {rect:?} vs {caret:?}"
                );
            }
            let selection = access
                .selection(
                    editor.active_outline().id,
                    field.text_selection().unwrap().to_text_selection(),
                )
                .unwrap();
            editor.select(selection).unwrap();
            assert_eq!(
                editor.caret(0.0).unwrap(),
                caret,
                "{position:?} {affinity:?}: AX selection moved the caret"
            );
        }
    }
    #[test]
    fn keyboard_and_accessibility_preserve_visual_caret_at_wraps() {
        use canvas::editor::Movement;
        let mut engine = TextEngine::default();
        let document = TextDocument::new(vec![
            Paragraph::new(
                "first words wrap here and continue".into(),
                Format::default(),
            ),
            Paragraph::new("abc שלום def عالم end".into(), Format::default()),
            Paragraph::new("last\nline".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 72.0).unwrap();
        let mut access = Accessibility::default();
        let viewport = Viewport {
            size: [800, 600],
            origin: [0.0; 2],
            scale: 1.0,
        };
        for movement in [
            Movement::LineEnd,
            Movement::Right,
            Movement::Left,
            Movement::Down,
            Movement::LineStart,
            Movement::LineEnd,
            Movement::Down,
            Movement::Down,
            Movement::Down,
            Movement::LineEnd,
            Movement::Left,
            Movement::Left,
            Movement::Right,
            Movement::LineStart,
            Movement::Up,
            Movement::Right,
            Movement::LineEnd,
        ] {
            editor.move_selection(&mut engine, movement, false).unwrap();
            let caret = editor.caret(0.0).unwrap();
            let tree = accesskit_consumer::Tree::new(
                access.update(&editor, viewport, "Test", None).unwrap(),
                true,
            );
            let field = tree.state().focus().unwrap();
            let selection = field.text_selection().unwrap();
            let rects = selection.bounding_boxes();
            assert_eq!(rects.len(), 1);
            let rect = rects[0];
            for (actual, expected) in [
                (rect.x0, caret.x0),
                (rect.y0, caret.y0),
                (rect.y1, caret.y1),
            ] {
                assert!(
                    (actual - expected).abs() < 0.0001,
                    "{movement:?}: {rect:?} vs {caret:?}"
                );
            }
            let source_selection = access
                .selection(editor.active_outline().id, selection.to_text_selection())
                .unwrap();
            editor.select(source_selection).unwrap();
            let restored = editor.caret(0.0).unwrap();
            assert_eq!(
                restored, caret,
                "{movement:?}: AX selection moved the caret"
            );
        }
    }
    #[test]
    fn cached_runs_match_reconstruction_through_editing_and_view_changes() {
        let mut engine = TextEngine::default();
        let document = TextDocument::new(
            [
                "one two three",
                "",
                "אבג 日本 👩🏽‍💻 e\u{301}",
                "last paragraph",
            ]
            .into_iter()
            .map(|text| Paragraph::new(text.into(), Format::default()))
            .collect(),
        )
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 72.0).unwrap();
        let mut cached = Accessibility::default();
        let mut rebuilt = Accessibility::default();
        let mut compare = |editor: &CanvasEditor, step: usize| {
            let viewport = Viewport {
                size: [800, 600],
                origin: [step as f32 * 0.125, -17.25],
                scale: 0.75 + (step % 5) as f32 * 0.5,
            };
            let preview = step
                .is_multiple_of(2)
                .then_some((editor.active_outline().id, [13.5, -7.25]));
            for repeat in 0..2 {
                rebuilt.paragraphs.clear();
                let actual = cached.update(editor, viewport, "Test", preview).unwrap();
                let expected = rebuilt.update(editor, viewport, "Test", preview).unwrap();
                assert_eq!(actual.nodes, expected.nodes, "step {step}, repeat {repeat}");
                assert_eq!(actual.focus, expected.focus);
                let tree = accesskit_consumer::Tree::new(actual, true);
                let field = tree.state().focus().unwrap();
                let selection = field.document_range().to_text_selection();
                assert_eq!(
                    cached
                        .selection(editor.active_outline().id, selection)
                        .unwrap(),
                    rebuilt
                        .selection(editor.active_outline().id, selection)
                        .unwrap()
                );
                let count = editor.outlines().iter().map(|o| o.layouts().count()).sum();
                assert_eq!(cached.paragraphs.len(), count);
            }
        };
        compare(&editor, 0);
        let mut seed = 0x923e_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as usize
        };
        for step in 1..=160 {
            let id = editor.outlines()[next() % editor.outlines().len()].id;
            editor.focus_outline(id).unwrap();
            let positions: Vec<_> = editor
                .active_outline()
                .document()
                .paragraphs()
                .enumerate()
                .flat_map(|(paragraph, text)| {
                    (0..=text.text().len()).filter_map(move |byte| {
                        text.utf16_offset(byte)
                            .ok()
                            .map(|offset| TextPosition { paragraph, offset })
                    })
                })
                .collect();
            let anchor = positions[next() % positions.len()];
            let focus = if step % 3 == 0 {
                positions[next() % positions.len()]
            } else {
                anchor
            };
            editor.select([anchor, focus].into()).unwrap();
            match step % 10 {
                0 => editor.insert(&mut engine, "\n").unwrap(),
                1 => editor.insert(&mut engine, "abc 👩🏽‍💻 שלום e\u{301}").unwrap(),
                2 => {
                    editor.delete(&mut engine, true).unwrap();
                }
                3 => editor
                    .resize(&mut engine, 48.0 + (step % 7) as f32 * 31.25)
                    .unwrap(),
                4 => {
                    editor
                        .create_outline(&mut engine, [step as f32, -5.0], 97.0)
                        .unwrap();
                    editor.insert(&mut engine, "annotation").unwrap();
                }
                5 => {
                    editor.undo(&mut engine).unwrap();
                }
                6 => {
                    editor.redo(&mut engine).unwrap();
                }
                7 => editor.move_outline(id, [17.5, step as f32]).unwrap(),
                8 => {
                    editor.compose(&mut engine, "仮\n名".into(), 1..1).unwrap();
                    compare(&editor, step);
                    editor.cancel_composition(&mut engine).unwrap();
                }
                _ => {}
            }
            compare(&editor, step);
        }
        while editor.undo(&mut engine).unwrap() {
            compare(&editor, 161);
        }
        while editor.redo(&mut engine).unwrap() {
            compare(&editor, 162);
        }
    }
    #[test]
    fn outlines_have_independent_accessibility_identity_and_selection() {
        let mut engine = TextEngine::default();
        let document =
            TextDocument::new(vec![Paragraph::new("lyrics".into(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 240.0).unwrap();
        let first = editor.active_outline().id;
        let second = editor
            .create_outline(&mut engine, [300.0, 40.0], 120.0)
            .unwrap();
        editor.insert(&mut engine, "annotation 🌳").unwrap();
        let mut access = Accessibility::default();
        let viewport = Viewport {
            size: [1000, 720],
            scale: 2.0,
            origin: [48.0; 2],
        };
        let update = access.update(&editor, viewport, "Test", None).unwrap();
        let fields: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::MultilineTextInput)
            .map(|(id, node)| (*id, node.value().unwrap().to_owned()))
            .collect();
        assert_eq!(
            fields
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>(),
            ["lyrics", "annotation 🌳"]
        );
        assert_ne!(fields[0].0, fields[1].0);
        assert_eq!(access.outline_for_node(fields[0].0), Some(first));
        assert_eq!(access.outline_for_node(fields[1].0), Some(second));
        let tree = accesskit_consumer::Tree::new(update, true);
        let field = tree.state().focus().unwrap();
        assert_eq!(field.document_range().text(), "annotation 🌳");
        let selection = field.document_range().to_text_selection();
        assert!(access.selection(first, selection).is_err());
        let source_selection = access.selection(second, selection).unwrap();
        editor.select(source_selection).unwrap();
        let preview = access
            .update(&editor, viewport, "Test", Some((second, [400.0, 50.0])))
            .unwrap();
        let preview_tree = accesskit_consumer::Tree::new(preview, true);
        let preview_field = preview_tree.state().focus().unwrap();
        let rect = preview_field.document_range().bounding_boxes()[0];
        assert_eq!(rect.x0, 848.0);
        assert_eq!(rect.y0, 148.0);
        assert_eq!(editor.active_outline().origin(), [300.0, 40.0]);
        editor.move_outline(second, [400.0, 50.0]).unwrap();
        access.update(&editor, viewport, "Test", None).unwrap();
        assert_eq!(access.outline_for_node(fields[1].0), Some(second));
        assert!(access.selection(second, selection).is_ok());
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        let update = access.update(&editor, viewport, "Test", None).unwrap();
        assert_eq!(
            update
                .nodes
                .iter()
                .filter(|(_, node)| node.role() == Role::MultilineTextInput)
                .count(),
            1
        );
        assert_eq!(access.outline_for_node(fields[1].0), None);
        assert!(access.selection(second, selection).is_err());
        assert_eq!(access.outline_for_node(fields[0].0), Some(first));
    }
}
