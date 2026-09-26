use crate::gpu::{Viewport, page::PageScene};
use crate::{
    date::DateField,
    document::TextPosition,
    editor::{CanvasEditor, Selection},
    outline::ParagraphLayout,
};
use accesskit::{
    Action, Affine, Node, NodeId, Rect, Role, TextDirection, TextSelection, TreeId, TreeInfo,
    TreeUpdate,
};
use onestore::{ExGuid, page::text::EditError};
use parley::Affinity;
use std::collections::HashMap;

pub const ROOT: NodeId = NodeId(0);
/// The page in document coordinates; its transform is the viewport, so scrolling and zooming
/// change only this node.
const PAGE: NodeId = NodeId(1);

struct Run {
    id: NodeId,
    node: Node,
    /// Paragraph-relative source offset of each character boundary, except the end of a
    /// paragraph break, which is the next paragraph's start.
    offsets: Vec<u32>,
}

/// A paragraph's runs, in paragraph coordinates under a node at its origin, so reflow above
/// it moves one node.
struct Paragraph {
    id: NodeId,
    layout: u64,
    source: usize,
    origin: [f32; 2],
    /// Its text's byte length in the field's value.
    length: usize,
    runs: Vec<Run>,
}

impl Paragraph {
    fn node(&self) -> Node {
        let mut node = Node::new(Role::GenericContainer);
        node.set_transform(Affine::translate((
            f64::from(self.origin[0]),
            f64::from(self.origin[1]),
        )));
        node.set_children(self.runs.iter().map(|run| run.id).collect::<Vec<_>>());
        node
    }
}

struct Field {
    outline: ExGuid,
    id: NodeId,
    node: Node,
    paragraphs: Vec<Paragraph>,
}

/// The nodes last sent to assistive technology, so each update sends only what changed.
pub struct Accessibility {
    fields: Vec<Field>,
    read_only: Vec<(NodeId, Node)>,
    dates: Vec<(DateField, NodeId, Node)>,
    next_id: u64,
}

impl Default for Accessibility {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            read_only: Vec::new(),
            dates: Vec::new(),
            next_id: 2,
        }
    }
}

impl Accessibility {
    /// Retire published nodes without recycling their IDs; the next update sends the whole tree.
    pub fn deactivate(&mut self) {
        self.fields.clear();
        self.read_only.clear();
        self.dates.clear();
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
        self.read_only.iter().position(|(id, _)| *id == node)
    }

    pub fn date_for_node(&self, node: NodeId) -> Option<DateField> {
        self.dates
            .iter()
            .find(|(_, id, _)| *id == node)
            .map(|(field, ..)| *field)
    }

    pub fn outline_for_node(&self, node: NodeId) -> Option<ExGuid> {
        self.fields
            .iter()
            .find(|field| field.id == node)
            .map(|field| field.outline)
    }

    /// The nodes that changed since the last update. After an error, `deactivate` before the
    /// next update.
    pub fn update(
        &mut self,
        editor: &CanvasEditor,
        scene: Option<&(PageScene, [f32; 2])>,
        viewport: Viewport,
        title: &str,
        preview: Option<(ExGuid, [f32; 2])>,
        read_only_focus: Option<usize>,
    ) -> Result<TreeUpdate, EditError> {
        let mut nodes = Vec::new();
        let mut focus = ROOT;
        let mut previous: HashMap<_, _> = self
            .fields
            .drain(..)
            .map(|field| (field.outline, field))
            .collect();
        for (ordinal, outline) in editor
            .visible_outlines()
            .chain(editor.caret_outline())
            .enumerate()
        {
            let (id, sent, old) = match previous.remove(&outline.id) {
                Some(field) => (field.id, Some(field.node), field.paragraphs),
                None => (self.allocate(), None, Vec::new()),
            };
            let layouts: Vec<_> = outline.layouts().collect();
            // Only a paragraph with a successor ends its last run with the paragraph break.
            let same = |old_index: usize, index: usize| {
                old[old_index].layout == layouts[index].1.text.id()
                    && (old_index + 1 == old.len()) == (index + 1 == layouts.len())
            };
            let shorter = old.len().min(layouts.len());
            let prefix = (0..shorter).take_while(|&i| same(i, i)).count();
            let suffix = (1..=shorter - prefix)
                .take_while(|&i| same(old.len() - i, layouts.len() - i))
                .count();
            let text = sent.as_ref().and_then(Node::value).unwrap_or_default();
            let length = |paragraphs: &[Paragraph]| paragraphs.iter().map(|p| p.length).sum();
            let mut value = text[..length(&old[..prefix])].to_owned();
            let tail = &text[text.len() - length(&old[old.len() - suffix..])..];
            let mut paragraphs = old;
            let kept = paragraphs.split_off(paragraphs.len() - suffix);
            let mut replaced = paragraphs.split_off(prefix).into_iter();
            for (at, (source, shaped)) in layouts
                .iter()
                .enumerate()
                .take(layouts.len() - suffix)
                .skip(prefix)
            {
                let (id, old) = match replaced.next() {
                    Some(paragraph) => (paragraph.id, paragraph.runs),
                    None => (self.allocate(), Vec::new()),
                };
                let mut runs = Vec::new();
                for (index, (node, offsets)) in runs_of(shaped, at + 1 < layouts.len())?
                    .into_iter()
                    .enumerate()
                {
                    let id = match old.get(index) {
                        Some(old) if old.offsets == offsets && old.node.value() == node.value() => {
                            old.id
                        }
                        _ => self.allocate(),
                    };
                    runs.push(Run { id, node, offsets });
                }
                for index in 1..runs.len() {
                    if runs[index - 1].node.bounds().map(|r| r.y0)
                        == runs[index].node.bounds().map(|r| r.y0)
                    {
                        let (before, after) = runs.split_at_mut(index);
                        before[index - 1].node.set_next_on_line(after[0].id);
                        after[0].node.set_previous_on_line(before[index - 1].id);
                    }
                }
                for (index, run) in runs.iter().enumerate() {
                    if old
                        .get(index)
                        .is_none_or(|old| old.id != run.id || old.node != run.node)
                    {
                        nodes.push((run.id, run.node.clone()));
                    }
                }
                let start = value.len();
                value.extend(runs.iter().filter_map(|run| run.node.value()));
                let paragraph = Paragraph {
                    id,
                    layout: shaped.text.id(),
                    source: *source,
                    origin: shaped.origin,
                    length: value.len() - start,
                    runs,
                };
                nodes.push((id, paragraph.node()));
                paragraphs.push(paragraph);
            }
            paragraphs.extend(kept);
            value.push_str(tail);
            for (paragraph, (source, shaped)) in paragraphs.iter_mut().zip(&layouts) {
                paragraph.source = *source;
                if paragraph.origin != shaped.origin {
                    paragraph.origin = shaped.origin;
                    nodes.push((paragraph.id, paragraph.node()));
                }
            }
            let mut node = Node::new(Role::MultilineTextInput);
            node.set_label(if outline.title {
                "Page title".into()
            } else if editor
                .caret_outline()
                .is_some_and(|caret| caret.id == outline.id)
            {
                "Text input".into()
            } else {
                format!("Text outline {}", ordinal + 1)
            });
            node.add_action(Action::Focus);
            node.add_action(Action::SetTextSelection);
            node.add_action(Action::ReplaceSelectedText);
            node.add_action(Action::SetValue);
            node.set_value(value);
            node.set_children(
                paragraphs
                    .iter()
                    .map(|paragraph| paragraph.id)
                    .collect::<Vec<_>>(),
            );
            let origin = preview
                .filter(|(id, _)| *id == outline.id)
                .map(|(_, origin)| origin)
                .unwrap_or_else(|| outline.origin());
            node.set_transform(Affine::translate((
                f64::from(origin[0]),
                f64::from(origin[1]),
            )));
            let bounds = outline.bounds();
            node.set_bounds(Rect::new(0.0, 0.0, bounds.width(), bounds.height()));
            if outline.id == editor.active_outline().id {
                let Selection {
                    positions: [anchor, caret],
                    affinities,
                } = editor.selection();
                node.set_text_selection(TextSelection {
                    anchor: position(&layouts, &paragraphs, anchor, affinities[0])?,
                    focus: position(&layouts, &paragraphs, caret, affinities[1])?,
                });
                focus = id;
            }
            if sent.as_ref() != Some(&node) {
                nodes.push((id, node.clone()));
            }
            self.fields.push(Field {
                outline: outline.id,
                id,
                node,
                paragraphs,
            });
        }
        let mut children: Vec<_> = self.fields.iter().map(|field| field.id).collect();
        let sent_read_only = std::mem::take(&mut self.read_only);
        let sent_dates = std::mem::take(&mut self.dates);
        if let Some((scene, offset)) = scene {
            let rect = |[x0, y0, x1, y1]: [f32; 4]| {
                Rect::new(
                    f64::from(x0 + offset[0]),
                    f64::from(y0 + offset[1]),
                    f64::from(x1 + offset[0]),
                    f64::from(y1 + offset[1]),
                )
            };
            for (index, object) in scene.read_only(Some(editor)).enumerate() {
                let mut node = Node::new(Role::Label);
                node.set_value(object.message);
                node.set_read_only();
                node.add_action(Action::Focus);
                node.set_bounds(rect(object.rect()));
                let sent = sent_read_only.get(index);
                let id = sent.map_or_else(|| self.allocate(), |(id, _)| *id);
                if sent.is_none_or(|(_, sent)| *sent != node) {
                    nodes.push((id, node.clone()));
                }
                if read_only_focus == Some(index) {
                    focus = id;
                }
                children.push(id);
                self.read_only.push((id, node));
            }
            for (index, (field, bounds)) in scene.date_fields(editor).enumerate() {
                let mut node = Node::new(Role::Button);
                node.set_label(super::DATE_LABELS[field as usize]);
                node.set_value(
                    editor.date().unwrap().source().paragraphs[index]
                        .text()
                        .unwrap()
                        .text
                        .text(),
                );
                node.add_action(Action::Click);
                node.set_bounds(rect(bounds));
                let sent = sent_dates.get(index);
                let id = sent.map_or_else(|| self.allocate(), |(_, id, _)| *id);
                if sent.is_none_or(|(_, _, sent)| *sent != node) {
                    nodes.push((id, node.clone()));
                }
                children.push(id);
                self.dates.push((field, id, node));
            }
        }
        let mut page = Node::new(Role::GenericContainer);
        page.set_transform(Affine::new([
            f64::from(viewport.scale),
            0.0,
            0.0,
            f64::from(viewport.scale),
            f64::from(viewport.origin[0]),
            f64::from(viewport.origin[1]),
        ]));
        page.set_children(children);
        let mut root = Node::new(Role::Window);
        root.set_label(title);
        root.set_children(vec![PAGE]);
        root.set_bounds(Rect::new(
            0.0,
            0.0,
            viewport.size[0] as f64,
            viewport.size[1] as f64,
        ));
        nodes.push((ROOT, root));
        nodes.push((PAGE, page));
        Ok(TreeUpdate {
            nodes,
            tree: Some(TreeInfo::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus,
        })
    }

    pub fn selection(
        &self,
        outline: ExGuid,
        selection: TextSelection,
    ) -> Result<Selection, EditError> {
        let field = self
            .fields
            .iter()
            .find(|field| field.outline == outline)
            .ok_or(EditError::InvalidRange)?;
        let position = |position: accesskit::TextPosition| {
            let (index, paragraph, run) = field
                .paragraphs
                .iter()
                .enumerate()
                .find_map(|(index, paragraph)| {
                    let run = paragraph
                        .runs
                        .iter()
                        .position(|run| run.id == position.node)?;
                    Some((index, paragraph, run))
                })
                .ok_or(EditError::InvalidRange)?;
            let offsets = &paragraph.runs[run].offsets;
            let breaks = run + 1 == paragraph.runs.len() && index + 1 < field.paragraphs.len();
            let count = offsets.len() + usize::from(breaks);
            let character = position.character_index;
            if character >= count {
                return Err(EditError::InvalidRange);
            }
            let source = match offsets.get(character) {
                Some(offset) => TextPosition {
                    paragraph: paragraph.source,
                    offset: *offset,
                },
                None => TextPosition {
                    paragraph: field.paragraphs[index + 1].source,
                    offset: 0,
                },
            };
            let affinity = if character > 0 && character + 1 == count {
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

/// The accessible position of a source position in `layouts`, which `paragraphs` mirrors.
fn position(
    layouts: &[(usize, &ParagraphLayout)],
    paragraphs: &[Paragraph],
    mut position: TextPosition,
    affinity: Affinity,
) -> Result<accesskit::TextPosition, EditError> {
    let index = layouts
        .binary_search_by_key(&position.paragraph, |(source, _)| *source)
        .map_err(|_| EditError::InvalidRange)?;
    let shaped = layouts[index].1;
    let projection = &shaped.projection;
    let layout = &shaped.text;
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
    let mut positions = paragraphs[index]
        .runs
        .iter()
        .filter(|run| run.node.bounds().is_some_and(|rect| rect.y0 == caret.y0))
        .filter_map(|run| {
            run.offsets
                .iter()
                .position(|offset| *offset == position.offset)
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

/// A paragraph's text runs in paragraph coordinates, each with its character boundaries'
/// source offsets; `breaks` ends the last run with the paragraph break.
fn runs_of(shaped: &ParagraphLayout, breaks: bool) -> Result<Vec<(Node, Vec<u32>)>, EditError> {
    let mut runs = Vec::new();
    let layout = &shaped.text;
    let projection = &shaped.projection;
    let text = projection.text().text();
    let boundaries: Vec<_> = projection.source_boundaries().collect();
    let source_offset = |byte| -> Result<u32, EditError> {
        let index = boundaries
            .binary_search_by_key(&byte, |(byte, _)| *byte)
            .map_err(|_| EditError::InvalidRange)?;
        Ok(boundaries[index].1)
    };
    for (line, bounds) in layout.lines() {
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
                let cluster_text = text.get(range.clone()).ok_or(EditError::InvalidRange)?;
                if range.len() <= u8::MAX as usize {
                    characters.push((range, cluster.advance(), cluster.is_word_boundary()));
                } else {
                    for (index, (offset, ch)) in cluster_text.char_indices().enumerate() {
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
                    f64::from(left),
                    f64::from(bounds.top),
                    f64::from(left + width),
                    f64::from(bounds.top + bounds.height),
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
                let offsets = chunk
                    .iter()
                    .map(|(range, _, _)| source_offset(range.start))
                    .chain(std::iter::once(source_offset(end)))
                    .collect::<Result<Vec<_>, _>>()?;
                runs.push((node, offsets));
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
                0.0,
                f64::from(bounds.top),
                0.0,
                f64::from(bounds.top + bounds.height),
            ));
            runs.push((node, vec![source_offset(bounds.source.start)?]));
        }
    }
    if breaks {
        let (node, _) = runs.last_mut().unwrap();
        let mut value = node.value().unwrap().to_owned();
        value.push('\n');
        node.set_value(value);
        let mut lengths = node.character_lengths().to_vec();
        lengths.push(1);
        node.set_character_lengths(lengths);
        if let Some(rect) = node.bounds() {
            let mut positions = node.character_positions().unwrap_or_default().to_vec();
            positions.push(rect.width() as f32);
            node.set_character_positions(positions);
            let mut widths = node.character_widths().unwrap_or_default().to_vec();
            widths.push(4.0);
            node.set_character_widths(widths);
        }
    }
    if !shaped.tags.is_empty() {
        let descriptions = shaped
            .tags
            .iter()
            .map(|tag| {
                use crate::outline::TagIcon;
                let fallback = match tag.icon {
                    TagIcon::CheckBox { .. } => "To do",
                    TagIcon::Question => "Question",
                    TagIcon::Music => "Music",
                    TagIcon::Exclamation => "Critical",
                    TagIcon::RedSquare => "Project A",
                    TagIcon::YellowSquare => "Project B",
                    TagIcon::BlueSquare => "Project C",
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
        runs[0].0.set_description(descriptions);
    }
    Ok(runs)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::{document::TextDocument, editor::TextOutline, layout::TextEngine};
    use accesskit_consumer::{NodeRef, Tree, TreeState};
    use onestore::document::Format;
    use onestore::page::text::Paragraph;

    /// Applies `update` to the platform's copy of the tree, as an adapter does.
    pub(in crate::interaction) fn apply(tree: &mut Option<Tree>, update: TreeUpdate) -> &TreeState {
        struct Ignore;
        impl accesskit_consumer::TreeChangeHandler for Ignore {
            fn node_added(&mut self, _: &NodeRef) {}
            fn node_updated(&mut self, _: &NodeRef, _: &NodeRef) {}
            fn focus_moved(&mut self, _: Option<&NodeRef>, _: Option<&NodeRef>) {}
            fn node_removed(&mut self, _: &NodeRef) {}
        }
        match tree {
            Some(tree) => tree.update_and_process_changes(update, &mut Ignore),
            None => *tree = Some(Tree::new(update, true)),
        }
        tree.as_ref().unwrap().state()
    }

    /// Every node of `state`, depth first.
    pub(in crate::interaction) fn nodes(state: &TreeState) -> Vec<NodeRef<'_>> {
        let mut nodes = Vec::new();
        let mut pending = vec![state.root()];
        while let Some(node) = pending.pop() {
            pending.extend(node.children().rev());
            nodes.push(node);
        }
        nodes
    }

    /// `state`'s nodes and focus with IDs renumbered depth first, to compare trees whose IDs differ.
    fn canonical(state: &TreeState) -> (Vec<Node>, NodeId) {
        let nodes = nodes(state);
        let order: HashMap<_, _> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.locate().0, NodeId(index as u64)))
            .collect();
        let id = |id: NodeId| order[&id];
        let canonical = nodes
            .iter()
            .map(|node| {
                let mut data = node.data().clone();
                data.set_children(
                    data.children()
                        .iter()
                        .map(|child| id(*child))
                        .collect::<Vec<_>>(),
                );
                if let Some(next) = data.next_on_line() {
                    data.set_next_on_line(id(next));
                }
                if let Some(previous) = data.previous_on_line() {
                    data.set_previous_on_line(id(previous));
                }
                if let Some(&TextSelection { anchor, focus }) = data.text_selection() {
                    data.set_text_selection(TextSelection {
                        anchor: accesskit::TextPosition {
                            node: id(anchor.node),
                            ..anchor
                        },
                        focus: accesskit::TextPosition {
                            node: id(focus.node),
                            ..focus
                        },
                    });
                }
                data
            })
            .collect();
        (canonical, id(state.focus_in_tree().locate().0))
    }

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
            identity: None,
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
            crate::gpu::page::PageScene::from_page(page, &mut engine).unwrap();
        let scene = (scene, [30.0, 40.0]);
        let viewport = Viewport {
            size: [800, 600],
            origin: [10.0, -20.0],
            scale: 2.0,
        };
        let mut access = Accessibility::default();
        let mut tree = None;
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
            let update = access
                .update(&editor, Some(&scene), viewport, "Test", None, Some(0))
                .unwrap();
            let node = nodes(apply(&mut tree, update))
                .into_iter()
                .find(|node| node.role() == Role::Label)
                .unwrap();
            let id = node.locate().0;
            if let Some(identity) = identity {
                assert_eq!(id, identity);
            }
            identity = Some(id);
            let y = if phase % 2 == 0 { 80.0 } else { 145.92 };
            let expected = Rect::new(
                310.0,
                f64::from((y + 40.0) * 2.0 - 20.0),
                710.0,
                f64::from((y + 100.0) * 2.0 - 20.0),
            );
            let bounds = node.bounding_box().unwrap();
            assert_eq!(bounds.x0, expected.x0);
            assert_eq!(bounds.x1, expected.x1);
            assert!((bounds.y0 - expected.y0).abs() < 0.001);
            assert!((bounds.y1 - expected.y1).abs() < 0.001);
            assert_eq!(
                super::super::page_hit_test(&editor, Some(&scene), [155.0, y + 45.0], 1.0),
                Some(super::super::Hit::ReadOnly(0))
            );
            if phase % 2 == 1 {
                assert_ne!(
                    super::super::page_hit_test(&editor, Some(&scene), [155.0, 125.0], 1.0),
                    Some(super::super::Hit::ReadOnly(0))
                );
            }
        }
    }

    #[test]
    fn read_only_objects_keep_accessibility_identity_through_edits_and_view_changes() {
        use crate::gpu::page::PageScene;
        use onestore::page::{Page, PageObject, Unsupported};
        let page = Page {
            identity: None,
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
        let mut tree = None;
        let mut identity = None;
        for scale in [1.0, 2.0, 0.5] {
            editor.insert(&mut engine, "annotation ").unwrap();
            let viewport = Viewport {
                size: [800, 600],
                origin: [-200.0, 10.0],
                scale,
            };
            let update = access
                .update(&editor, Some(&scene), viewport, "Test", None, None)
                .unwrap();
            let state = apply(&mut tree, update);
            let node = nodes(state)
                .into_iter()
                .find(|node| node.role() == Role::Label)
                .unwrap();
            let id = node.locate().0;
            if let Some(identity) = identity {
                assert_eq!(id, identity);
            }
            identity = Some(id);
            assert_eq!(node.data().value(), Some("Unsupported content\nRead-only"));
            assert!(node.is_read_only());
            assert!(node.data().supports_action(Action::Focus));
            for action in [
                Action::SetValue,
                Action::ReplaceSelectedText,
                Action::SetTextSelection,
            ] {
                assert!(!node.data().supports_action(action));
            }
            assert_eq!(
                node.bounding_box(),
                Some(Rect::new(
                    f64::from(40.0 * scale - 200.0),
                    f64::from(60.0 * scale + 10.0),
                    f64::from(240.0 * scale - 200.0),
                    f64::from(120.0 * scale + 10.0)
                ))
            );
            assert_eq!(access.outline_for_node(id), None);
            assert_eq!(
                state.focus().unwrap().document_range().text(),
                "annotation "
            );
            let focused = access
                .update(&editor, Some(&scene), viewport, "Test", None, Some(0))
                .unwrap();
            assert_eq!(focused.focus, identity.unwrap());
            assert_eq!(access.read_only_for_node(focused.focus), Some(0));
            assert_eq!(
                apply(&mut tree, focused)
                    .focus()
                    .unwrap()
                    .value()
                    .as_deref(),
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
        let mut paragraphs = text.nodes().to_vec();
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
            paragraphs[index].text_mut().unwrap().tags.push(Tag {
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
                paragraphs,
                unsupported: Vec::new(),
            }],
            definitions,
        )
        .unwrap();
        let mut access = Accessibility::default();
        let mut tree = None;
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
            let update = access
                .update(&editor, None, viewport, "Test", None, None)
                .unwrap();
            let state = apply(&mut tree, update);
            assert_eq!(
                nodes(state)
                    .iter()
                    .filter_map(|node| node.data().description())
                    .collect::<Vec<_>>(),
                [
                    "Rehearsal, incomplete",
                    "Rehearsal, completed",
                    "Question",
                    "Music, disabled"
                ]
            );
            let text = state.focus().unwrap().document_range().text();
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
        let mut tree = None;
        let viewport = Viewport {
            size: [900, 700],
            scale: 2.0,
            origin: [13.0, 19.0],
        };
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        let field = apply(&mut tree, update).focus().unwrap();
        assert_eq!(
            field.document_range().text(),
            "root\nfolded\nlast\nfolded tail"
        );
        assert_eq!(
            access.fields[0]
                .paragraphs
                .iter()
                .map(|paragraph| paragraph.source)
                .collect::<Vec<_>>(),
            [0, 1, 3, 4]
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
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        assert_eq!(
            apply(&mut tree, update)
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
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        let caret = editor.caret(1.0).unwrap();
        assert_eq!(caret.x0, 27.0);
        let rects = apply(&mut tree, update)
            .focus()
            .unwrap()
            .text_selection()
            .unwrap()
            .bounding_boxes();
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].x0, 27.0 * 2.0 + 13.0);
        assert_eq!(rects[0].y0, caret.y0 * 2.0 + 19.0);
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
                let mut tree = None;
                let viewport = Viewport {
                    size: [800, 600],
                    origin: [48.0; 2],
                    scale: 2.0,
                };
                let update = access
                    .update(&editor, None, viewport, "Test", None, None)
                    .unwrap();
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
                let field = apply(&mut tree, update).focus().unwrap();
                assert_eq!(field.document_range().text(), text, "width {width}");
                let selection = field.document_range().to_text_selection();
                let source_selection = access
                    .selection(editor.active_outline().id, selection)
                    .unwrap();
                editor.select(source_selection).unwrap();
                let selected = access
                    .update(&editor, None, viewport, "Test", None, None)
                    .unwrap();
                assert_eq!(
                    apply(&mut tree, selected)
                        .focus()
                        .unwrap()
                        .text_selection()
                        .unwrap()
                        .text(),
                    text
                );
                let zoomed = access
                    .update(
                        &editor,
                        None,
                        Viewport {
                            scale: 3.0,
                            ..viewport
                        },
                        "Test",
                        None,
                        None,
                    )
                    .unwrap();
                assert_eq!(
                    zoomed.nodes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                    [ROOT, PAGE]
                );
                apply(&mut tree, zoomed);
                access.deactivate();
                let update = access
                    .update(&editor, None, viewport, "Test", None, None)
                    .unwrap();
                let state = apply(&mut tree, update);
                assert!(
                    access
                        .selection(editor.active_outline().id, selection)
                        .is_err()
                );
                let selection = *state.focus().unwrap().data().text_selection().unwrap();
                editor.insert(&mut engine, "replacement").unwrap();
                let update = access
                    .update(&editor, None, viewport, "Test", None, None)
                    .unwrap();
                apply(&mut tree, update);
                assert!(
                    access
                        .selection(editor.active_outline().id, selection)
                        .is_err()
                );
                editor.undo(&mut engine).unwrap();
                let update = access
                    .update(&editor, None, viewport, "Test", None, None)
                    .unwrap();
                assert_eq!(
                    apply(&mut tree, update)
                        .focus()
                        .unwrap()
                        .document_range()
                        .text(),
                    text
                );
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
        let mut tree = None;
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
            let update = access
                .update(&editor, None, viewport, "Test", None, None)
                .unwrap();
            let field = apply(&mut tree, update).focus().unwrap();
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
        use draw::edit::Movement;
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
        let mut tree = None;
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
            let update = access
                .update(&editor, None, viewport, "Test", None, None)
                .unwrap();
            let field = apply(&mut tree, update).focus().unwrap();
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
        let mut tree = None;
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
                let update = cached
                    .update(editor, None, viewport, "Test", preview, None)
                    .unwrap();
                let actual = apply(&mut tree, update);
                let mut rebuilt = Accessibility::default();
                let expected = Tree::new(
                    rebuilt
                        .update(editor, None, viewport, "Test", preview, None)
                        .unwrap(),
                    true,
                );
                assert_eq!(
                    canonical(actual),
                    canonical(expected.state()),
                    "step {step}, repeat {repeat}"
                );
                let [actual, expected] =
                    [(&cached, actual), (&rebuilt, expected.state())].map(|(access, state)| {
                        let field = state.focus().unwrap();
                        let selection = field.document_range().to_text_selection();
                        access
                            .selection(editor.active_outline().id, selection)
                            .unwrap()
                    });
                assert_eq!(actual, expected);
                let count = editor
                    .outlines()
                    .iter()
                    .map(|o| o.layouts().count())
                    .sum::<usize>();
                assert_eq!(
                    cached
                        .fields
                        .iter()
                        .map(|field| field.paragraphs.len())
                        .sum::<usize>(),
                    count
                );
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
    fn scrolling_sends_the_page_and_typing_sends_the_edited_paragraph() {
        let mut engine = TextEngine::default();
        let document = TextDocument::new(
            (0..200)
                .map(|index| Paragraph::new(format!("paragraph {index}"), Format::default()))
                .collect(),
        )
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 480.0).unwrap();
        let mut access = Accessibility::default();
        let mut tree = None;
        let mut viewport = Viewport {
            size: [800, 600],
            origin: [48.0; 2],
            scale: 2.0,
        };
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        apply(&mut tree, update);
        let ids = |update: &TreeUpdate| update.nodes.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        for (scroll, zoom) in [(-300.0, 1.0), (150.0, 1.25)] {
            viewport.origin[1] += scroll;
            viewport.scale *= zoom;
            let update = access
                .update(&editor, None, viewport, "Test", None, None)
                .unwrap();
            assert_eq!(ids(&update), [ROOT, PAGE]);
            apply(&mut tree, update);
        }
        let field = access.fields[0].id;
        editor
            .select(
                [TextPosition {
                    paragraph: 150,
                    offset: 3,
                }; 2]
                    .into(),
            )
            .unwrap();
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        assert_eq!(ids(&update), [field, ROOT, PAGE]);
        apply(&mut tree, update);
        editor.insert(&mut engine, "x").unwrap();
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        let paragraph = &access.fields[0].paragraphs[150];
        assert_eq!(
            ids(&update),
            [paragraph.runs[0].id, paragraph.id, field, ROOT, PAGE]
        );
        assert_eq!(update.nodes[0].1.value(), Some("parxagraph 150\n"));
        let state = apply(&mut tree, update);
        let expected = Tree::new(
            Accessibility::default()
                .update(&editor, None, viewport, "Test", None, None)
                .unwrap(),
            true,
        );
        assert_eq!(canonical(state), canonical(expected.state()));
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
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
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
        let mut tree = None;
        let field = apply(&mut tree, update).focus().unwrap();
        assert_eq!(field.document_range().text(), "annotation 🌳");
        let selection = field.document_range().to_text_selection();
        assert!(access.selection(first, selection).is_err());
        let source_selection = access.selection(second, selection).unwrap();
        editor.select(source_selection).unwrap();
        let preview = access
            .update(
                &editor,
                None,
                viewport,
                "Test",
                Some((second, [400.0, 50.0])),
                None,
            )
            .unwrap();
        let preview_field = apply(&mut tree, preview).focus().unwrap();
        let rect = preview_field.document_range().bounding_boxes()[0];
        assert_eq!(rect.x0, 848.0);
        assert_eq!(rect.y0, 148.0);
        assert_eq!(editor.active_outline().origin(), [300.0, 40.0]);
        editor.move_outline(second, [400.0, 50.0]).unwrap();
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        apply(&mut tree, update);
        assert_eq!(access.outline_for_node(fields[1].0), Some(second));
        assert!(access.selection(second, selection).is_ok());
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        assert_eq!(
            nodes(apply(&mut tree, update))
                .iter()
                .filter(|node| node.role() == Role::MultilineTextInput)
                .count(),
            1
        );
        assert_eq!(access.outline_for_node(fields[1].0), None);
        assert!(access.selection(second, selection).is_err());
        assert_eq!(access.outline_for_node(fields[0].0), Some(first));
    }
}
