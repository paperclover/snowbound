//! OneNote 2010's Insert Space: what lies past a line across the page moves along one axis,
//! and a body outline the line crosses parts between its paragraphs.

use super::*;

/// Something Insert Space moves.
enum Piece {
    /// A body outline, picture or ink drawing, placed anew.
    Object(ExGuid),
    /// The root nodes of a body outline from `at` on.
    Tail { outline: ExGuid, at: usize },
}

impl CanvasEditor {
    /// What starts at or past `line` along `axis` (0 for x, 1 for y) with its extent as
    /// `[x0, y0, x1, y1]` page points, and the farthest end of what starts before the line.
    fn spaced(&self, axis: usize, line: f32) -> (Vec<(Piece, [f32; 4])>, f32) {
        let mut moved = Vec::new();
        let mut floor = f32::NEG_INFINITY;
        let mut stays = |rect: [f32; 4]| {
            if rect[axis] < line {
                floor = floor.max(rect[axis + 2]);
            }
        };
        for outline in &self.outlines {
            let bounds = outline.bounds();
            let rect = [bounds.x0, bounds.y0, bounds.x1, bounds.y1].map(|value| value as f32);
            if outline.title {
                stays(rect);
                continue;
            }
            if rect[axis] >= line {
                moved.push((Piece::Object(outline.id), rect));
                continue;
            }
            let top = outline.origin()[1];
            let spans: Vec<Option<[f32; 2]>> = outline.shaped.spans().collect();
            let at = (axis == 1)
                .then(|| {
                    spans
                        .iter()
                        .position(|span| span.is_some_and(|[start, _]| top + start >= line))
                })
                .flatten();
            let Some(at) = at else {
                stays(rect);
                continue;
            };
            let [start, _] = spans[at].unwrap();
            moved.push((
                Piece::Tail {
                    outline: outline.id,
                    at,
                },
                [rect[0], top + start, rect[2], rect[3]],
            ));
            let end = spans[..at]
                .iter()
                .flatten()
                .map(|[_, end]| *end)
                .fold(0.0, f32::max);
            stays([rect[0], rect[1], rect[2], top + end]);
        }
        for content in &self.objects {
            let (id, rect) = match content {
                page::Content::Outline {
                    source,
                    layout,
                    below_title: None,
                } => {
                    let [x, y] = [source.layout.x, source.layout.y].map(|v| v.unwrap_or(0.0));
                    (source.id, [x, y, x + layout.size[0], y + layout.size[1]])
                }
                page::Content::Image(image) => {
                    let (origin, size) = self.image_placement(image.id).unwrap_or_default();
                    let rect = [
                        origin[0],
                        origin[1],
                        origin[0] + size[0],
                        origin[1] + size[1],
                    ];
                    (image.id, rect)
                }
                page::Content::Ink(ink) => match page::ink_bounds(ink) {
                    Some(rect) => (ink.id, rect),
                    None => continue,
                },
                page::Content::ReadOnly(object)
                    if matches!(object.source, PageObject::Outline(_) | PageObject::Image(_)) =>
                {
                    (object.source.id(), object.rect())
                }
                page::Content::ReadOnly(object) => {
                    stays(object.rect());
                    continue;
                }
                page::Content::Outline { .. } | page::Content::Date { .. } => continue,
                page::Content::Editable(_) => continue,
            };
            if rect[axis] >= line {
                moved.push((Piece::Object(id), rect));
            } else {
                stays(rect);
            }
        }
        (moved, floor)
    }

    /// How far back from `line` Insert Space can take the page along `axis`: never over what
    /// stays before the line, so at most zero.
    pub fn space_limit(&self, axis: usize, line: f32) -> f32 {
        (self.spaced(axis, line).1 - line).min(0.0)
    }

    /// The extents, as `[x0, y0, x1, y1]` page points, of what Insert Space at `line` along
    /// `axis` moves.
    pub fn space_preview(&self, axis: usize, line: f32) -> Vec<[f32; 4]> {
        self.spaced(axis, line)
            .0
            .into_iter()
            .map(|(_, rect)| rect)
            .collect()
    }

    /// Moves what starts at or past `line` along `axis` by `delta`, taking no more space back
    /// than `space_limit` allows, as one undo step. False when nothing moves.
    pub fn insert_space(
        &mut self,
        engine: &mut TextEngine,
        axis: usize,
        line: f32,
        delta: f32,
    ) -> Result<bool, EditorError> {
        if !line.is_finite() || !delta.is_finite() || axis > 1 {
            return Err(EditError::InvalidRange.into());
        }
        let (moved, floor) = self.spaced(axis, line);
        let delta = delta.max((floor - line).min(0.0));
        if delta == 0.0 || moved.is_empty() {
            return Ok(false);
        }
        self.finish_composition();
        self.whole = None;
        if let Focus::Draft { index, .. } = self.active {
            self.active = Focus::Outline(index);
        }
        let shift = |mut position: [f32; 2]| {
            position[axis] += delta;
            position
        };
        let depth = self.undo.len();
        for (piece, rect) in moved {
            let result = match piece {
                Piece::Object(id) => {
                    let layout = self.object_layout_mut(id).unwrap();
                    // Undo restores an unset coordinate as the zero it stands for, as ops
                    // cannot unset one.
                    let origin = [layout.x, layout.y].map(|v| v.unwrap_or(0.0));
                    let previous = origin.map(Some);
                    [layout.x, layout.y] = shift(origin).map(Some);
                    self.undo.push(History::Position {
                        object: id,
                        position: previous,
                    });
                    self.record(self.placement_ops(&Placement {
                        id,
                        position: previous,
                    }));
                    Ok(())
                }
                Piece::Tail { outline, at } => {
                    let part = onestore::page::text::new_id()?;
                    self.split_outline(engine, outline, at, part, shift([rect[0], rect[1]]))
                        .map(|parents| {
                            self.undo.push(History::Join {
                                outline,
                                part,
                                parents,
                            })
                        })
                }
            };
            if let Err(error) = result {
                self.group(depth);
                return Err(error);
            }
        }
        self.group(depth);
        self.redo.clear();
        self.preferred_x = None;
        Ok(true)
    }

    /// Makes the history entries past `depth` one undo step that leaves the selection alone.
    fn group(&mut self, depth: usize) {
        let entries = self.undo.split_off(depth);
        if !entries.is_empty() {
            self.undo.push(History::Group {
                entries,
                page: false,
            });
        }
    }

    /// Parts outline `id`'s root nodes from `at` into new outline `part` at `position`, which
    /// takes the rest of the outline's layout; returns each root `part` then holds with the
    /// parent it had.
    pub(super) fn split_outline(
        &mut self,
        engine: &mut TextEngine,
        id: ExGuid,
        at: usize,
        part: ExGuid,
        position: [f32; 2],
    ) -> Result<Vec<(ExGuid, Option<ExGuid>)>, EditorError> {
        let index = self
            .outlines
            .iter()
            .position(|outline| outline.id == id)
            .ok_or(EditError::InvalidRange)?;
        let mut kept = self.outlines[index].snapshot();
        let mut tail = kept.paragraphs.split_off(at);
        let head: BTreeSet<ExGuid> = kept.paragraphs.iter().map(|node| node.id).collect();
        let mut parents = Vec::new();
        for node in &mut tail {
            if node.parent.is_none_or(|parent| head.contains(&parent)) {
                parents.push((node.id, node.parent.take()));
            }
        }
        let [x, y] = position.map(Some);
        let moved = Outline {
            id: part,
            layout: onestore::document::Layout {
                x,
                y,
                max_height: None,
                ..kept.layout.clone()
            },
            paragraphs: tail,
            ..kept.clone()
        };
        let head = TextOutline::from_outline(engine, &kept, &self.definitions)?;
        let tail = TextOutline::from_outline(engine, &moved, &self.definitions)?;
        self.outlines[index] = head;
        self.outlines.insert(index + 1, tail);
        match &mut self.active {
            Focus::Outline(active) | Focus::Caret { index: active, .. } if *active > index => {
                *active += 1
            }
            _ => {}
        }
        if let Some(slot) = self
            .objects
            .iter()
            .position(|object| matches!(object, page::Content::Editable(slot) if *slot == id))
        {
            self.objects.insert(slot + 1, page::Content::Editable(part));
        }
        let anchor = ops::anchor()?;
        let lowered = self
            .added(Outline {
                paragraphs: vec![anchor.clone()],
                ..moved
            })
            .map(|mut added| {
                added.extend(parents.iter().map(|(root, _)| PageOp::Move {
                    object: *root,
                    parent: Some(part),
                    before: Some(anchor.id),
                }));
                added.push(PageOp::Delete { object: anchor.id });
                added
            });
        self.record(lowered);
        Ok(parents)
    }

    /// Returns outline `part`'s root nodes to the end of outline `id`, each under its parent
    /// in `parents`, and removes `part`; returns where `split_outline` would part them again.
    pub(super) fn join_outline(
        &mut self,
        engine: &mut TextEngine,
        id: ExGuid,
        part: ExGuid,
        parents: &[(ExGuid, Option<ExGuid>)],
    ) -> Result<(usize, [f32; 2]), EditorError> {
        let find = |id| self.outlines.iter().position(|outline| outline.id == id);
        let (Some(index), Some(from)) = (find(id), find(part)) else {
            return Err(EditError::InvalidRange.into());
        };
        let mut joined = self.outlines[index].snapshot();
        let at = joined.paragraphs.len();
        let position = self.outlines[from].origin();
        let parents: BTreeMap<ExGuid, Option<ExGuid>> = parents.iter().copied().collect();
        joined
            .paragraphs
            .extend(self.outlines[from].document.nodes().iter().map(|node| {
                let mut node = node.clone();
                if let Some(parent) = parents.get(&node.id) {
                    node.parent = *parent;
                }
                node
            }));
        let roots: Vec<ExGuid> = self.outlines[from]
            .document
            .nodes()
            .iter()
            .filter(|node| node.parent.is_none())
            .map(|node| node.id)
            .collect();
        self.outlines[index] = TextOutline::from_outline(engine, &joined, &self.definitions)?;
        self.outlines.remove(from);
        match &mut self.active {
            Focus::Outline(active) if *active == from => {
                *active = index - usize::from(index > from)
            }
            Focus::Outline(active) | Focus::Caret { index: active, .. } if *active > from => {
                *active -= 1
            }
            _ => {}
        }
        self.objects
            .retain(|object| !matches!(object, page::Content::Editable(slot) if *slot == part));
        // Moving its last paragraph out removes the emptied outline.
        let ops = roots
            .into_iter()
            .map(|root| PageOp::Move {
                object: root,
                parent: Some(parents.get(&root).copied().flatten().unwrap_or(id)),
                before: None,
            })
            .collect();
        self.record(Ok(ops));
        Ok((at, position))
    }
}
