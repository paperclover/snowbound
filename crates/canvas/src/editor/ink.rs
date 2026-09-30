//! Drawing on the page as OneNote 2010's Draw tab does (`corpus/ink-tools`): each stroke and
//! shape is a drawing of its own, the stroke eraser takes whole strokes, and the lasso picks
//! drawings to move or delete.

use super::{CanvasEditor, EditError, EditorError, History, Placement, page};
use onestore::{
    ExGuid,
    op::PageOp,
    page::{Ink, InkStroke, PageObject, ink::snap, text::new_id},
};

/// A pen or highlighter, as OneNote's pen gallery and Color & Thickness choose one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pen {
    /// Tip width in HIMETRIC (hundredths of a millimetre); a highlighter's tip is as tall as `HIGHLIGHTER_HEIGHT`.
    pub width: f32,
    /// COLORREF; `None` draws in the paper's ink, as OneNote's black pens store it.
    pub color: Option<u32>,
    pub highlighter: bool,
}

/// A highlighter tip's height in HIMETRIC.
const HIGHLIGHTER_HEIGHT: f32 = 400.0;

impl Pen {
    pub const fn new(width: f32, color: Option<u32>) -> Self {
        Self {
            width,
            color,
            highlighter: false,
        }
    }

    pub const fn highlighter(color: u32) -> Self {
        Self {
            width: 70.0,
            color: Some(color),
            highlighter: true,
        }
    }

    /// The pen shapes draw with: a little thicker, as OneNote's are.
    pub const fn shape(self) -> Self {
        Self {
            width: 50.0,
            ..self
        }
    }

    /// A stroke of this pen through `points`, snapped to the HIMETRIC grid stored.
    pub fn stroke(&self, points: &[[f32; 2]]) -> Result<InkStroke, EditError> {
        let points_of = |himetric: f32| snap(himetric * 72.0 / 2540.0);
        Ok(InkStroke {
            id: new_id()?,
            points: points.iter().map(|point| point.map(snap)).collect(),
            width: points_of(self.width),
            height: points_of(if self.highlighter {
                HIGHLIGHTER_HEIGHT
            } else {
                self.width
            }),
            color: self.color,
            transparency: self.highlighter.then_some(127),
            pen_tip: self.highlighter.then_some(1),
            raster_operation: self.highlighter.then_some(9),
        })
    }
}

/// OneNote 2010's favourite pens, in its gallery's order.
pub const FAVORITES: [Pen; 14] = {
    const RED: u32 = 0x241ced;
    const BLUE: u32 = 0xbb6531;
    const GREEN: u32 = 0x367d17;
    const GREY: u32 = 0x808080;
    [
        Pen::new(35.0, None),
        Pen::new(35.0, Some(RED)),
        Pen::new(35.0, Some(BLUE)),
        Pen::new(35.0, Some(GREEN)),
        Pen::new(35.0, Some(GREY)),
        Pen::highlighter(0x00ffff),
        Pen::highlighter(0xffff00),
        Pen::new(50.0, None),
        Pen::new(50.0, Some(RED)),
        Pen::new(50.0, Some(BLUE)),
        Pen::new(50.0, Some(GREEN)),
        Pen::new(50.0, Some(GREY)),
        Pen::highlighter(0x00ff00),
        Pen::highlighter(0xff00ff),
    ]
};

impl CanvasEditor {
    /// Puts `ink`, a stroke or shape just drawn, on top of the page as one undo step.
    pub fn draw(&mut self, ink: Ink) -> Result<(), EditorError> {
        if ink.strokes.is_empty() {
            return Err(EditError::InvalidRange.into());
        }
        self.finish_composition();
        let index = self.objects.len();
        self.add_ink(index, ink);
        self.undo.push(History::Ink { index, ink: None });
        self.redo.clear();
        Ok(())
    }

    /// Erases what the eraser touches moving from `from` to `to`, `reach` page points either
    /// side of its path: a whole drawing where it is a shape, a single stroke or a group, as
    /// OneNote erases a shape at a touch, and otherwise the strokes touched. With `join` the
    /// erasing joins the last undo step, which the same sweep of the eraser made. Returns
    /// whether anything was erased.
    pub fn erase(
        &mut self,
        from: [f32; 2],
        to: [f32; 2],
        reach: f32,
        join: bool,
    ) -> Result<bool, EditorError> {
        let mut entries = Vec::new();
        let mut index = self.objects.len();
        while index > 0 {
            index -= 1;
            let page::Content::Ink(ink) = &self.objects[index] else {
                continue;
            };
            let offset = page::ink_offset(ink);
            let touched = |stroke: &InkStroke| {
                let reach = reach + stroke.width.max(stroke.height) / 2.0;
                let mut points = stroke
                    .points
                    .iter()
                    .map(|p| [p[0] + offset[0], p[1] + offset[1]]);
                let Some(mut previous) = points.next() else {
                    return false;
                };
                if segments_distance([previous, previous], [from, to]) <= reach {
                    return true;
                }
                points.any(|point| {
                    let near = segments_distance([previous, point], [from, to]) <= reach;
                    previous = point;
                    near
                })
            };
            let (kept, erased): (Vec<InkStroke>, Vec<InkStroke>) = ink
                .strokes
                .iter()
                .cloned()
                .partition(|stroke| !touched(stroke));
            let grouped = ink
                .groups
                .iter()
                .any(|group| group_touched(group, &touched));
            if erased.is_empty() && !grouped {
                continue;
            }
            if ink.shape.is_some() || !ink.groups.is_empty() || kept.is_empty() {
                let removed = self.remove_ink(index);
                entries.push(History::Ink {
                    index,
                    ink: Some(Box::new(removed)),
                });
            } else {
                let id = ink.id;
                entries.push(self.set_strokes(id, kept));
            }
        }
        if entries.is_empty() {
            return Ok(false);
        }
        self.finish_composition();
        if join
            && let Some(History::Group {
                entries: earlier, ..
            }) = self.undo.last_mut()
        {
            earlier.extend(entries);
        } else {
            self.undo.push(History::Group {
                entries,
                page: false,
            });
        }
        self.redo.clear();
        Ok(true)
    }

    /// How many ops wait for `take_ops`.
    pub(crate) fn pending_ops(&self) -> usize {
        self.ops.as_ref().map_or(0, Vec::len)
    }

    /// Takes back an eraser's sweep as though it never happened: its undo step, and the ops
    /// recorded since there were `mark`, none of which were taken since.
    pub(crate) fn retract_erasing(
        &mut self,
        engine: &mut crate::layout::TextEngine,
        mark: usize,
    ) -> Result<(), EditorError> {
        if let Some(step) = self.undo.pop() {
            self.apply_history(engine, step)
                .map_err(|(_, error)| error)?;
        }
        if let Ok(ops) = &mut self.ops {
            ops.truncate(mark);
        }
        Ok(())
    }

    /// The page's drawings, on top first, with most of their points inside `lasso`, a
    /// closed path in page points.
    pub fn ink_within(&self, lasso: &[[f32; 2]]) -> Vec<ExGuid> {
        self.objects
            .iter()
            .rev()
            .filter_map(|object| match object {
                page::Content::Ink(ink) => {
                    let offset = page::ink_offset(ink);
                    let points = points(ink);
                    let inside = points
                        .iter()
                        .filter(|p| contains(lasso, [p[0] + offset[0], p[1] + offset[1]]))
                        .count();
                    (!points.is_empty() && inside * 2 > points.len()).then_some(ink.id)
                }
                _ => None,
            })
            .collect()
    }

    /// The drawing on top whose strokes pass within `reach` of `point`.
    pub fn ink_at(&self, point: [f32; 2], reach: f32) -> Option<ExGuid> {
        self.objects.iter().rev().find_map(|object| match object {
            page::Content::Ink(ink) => {
                let offset = page::ink_offset(ink);
                let near = |stroke: &InkStroke| {
                    let reach = reach + stroke.width.max(stroke.height) / 2.0;
                    let mut points = stroke
                        .points
                        .iter()
                        .map(|p| [p[0] + offset[0], p[1] + offset[1]]);
                    let first = points.next();
                    first.is_some_and(|first| {
                        let mut previous = first;
                        segments_distance([first, first], [point, point]) <= reach
                            || points.any(|p| {
                                let near =
                                    segments_distance([previous, p], [point, point]) <= reach;
                                previous = p;
                                near
                            })
                    })
                };
                (ink.strokes.iter().any(near) || ink.groups.iter().any(|g| group_touched(g, &near)))
                    .then_some(ink.id)
            }
            _ => None,
        })
    }

    /// The painted extent of drawings `ids`, as `[x0, y0, x1, y1]` page points, while dragged
    /// where they show.
    pub fn ink_extent(&self, ids: &[ExGuid]) -> Option<[f32; 4]> {
        self.objects
            .iter()
            .filter_map(|object| match object {
                page::Content::Ink(ink) if ids.contains(&ink.id) => {
                    let [dx, dy] = self.ink_drag_offset(ink.id);
                    page::ink_bounds(ink)
                        .map(|[x0, y0, x1, y1]| [x0 + dx, y0 + dy, x1 + dx, y1 + dy])
                }
                _ => None,
            })
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            })
    }

    /// Shows drawings dragged `delta` from where they lie, without storing it, until the
    /// drag ends with `move_ink` or `None`.
    pub fn drag_ink(&mut self, drag: Option<(Vec<ExGuid>, [f32; 2])>) {
        self.ink_drag = drag;
    }

    /// How far a drawing shows from where it lies, while dragged.
    pub(crate) fn ink_drag_offset(&self, id: ExGuid) -> [f32; 2] {
        match &self.ink_drag {
            Some((ids, delta)) if ids.contains(&id) => *delta,
            _ => [0.0; 2],
        }
    }

    /// Moves drawings `ids` by `delta` as one undo step, as OneNote moves ink: by the
    /// drawing's offset, its strokes as they were.
    pub fn move_ink(&mut self, ids: &[ExGuid], delta: [f32; 2]) -> Result<(), EditorError> {
        self.ink_drag = None;
        if delta == [0.0; 2] {
            return Ok(());
        }
        let mut entries = Vec::new();
        for &id in ids {
            let Some(layout) = self.ink_layout_mut(id) else {
                continue;
            };
            let stored = [layout.x, layout.y];
            // Undoing restores a position: a drawing never moved lies at no offset.
            let position = stored.map(|v| Some(v.unwrap_or(0.0)));
            [layout.x, layout.y] = [0, 1].map(|axis| Some(position[axis].unwrap() + delta[axis]));
            self.record(self.placement_ops(&Placement {
                id,
                position: stored,
            }));
            entries.push(History::Position {
                object: id,
                position,
            });
        }
        self.push_ink_step(entries);
        Ok(())
    }

    /// Deletes drawings `ids` as one undo step.
    pub fn delete_ink(&mut self, ids: &[ExGuid]) -> Result<(), EditorError> {
        let mut entries = Vec::new();
        let mut index = self.objects.len();
        while index > 0 {
            index -= 1;
            if matches!(&self.objects[index], page::Content::Ink(ink) if ids.contains(&ink.id)) {
                let ink = self.remove_ink(index);
                entries.push(History::Ink {
                    index,
                    ink: Some(Box::new(ink)),
                });
            }
        }
        self.push_ink_step(entries);
        Ok(())
    }

    fn push_ink_step(&mut self, entries: Vec<History>) {
        if entries.is_empty() {
            return;
        }
        self.finish_composition();
        self.undo.push(History::Group {
            entries,
            page: false,
        });
        self.redo.clear();
    }

    fn ink_layout_mut(&mut self, id: ExGuid) -> Option<&mut onestore::document::Layout> {
        self.objects.iter_mut().find_map(|object| match object {
            page::Content::Ink(ink) if ink.id == id => Some(&mut ink.layout),
            _ => None,
        })
    }

    /// Puts a drawing on the page at `index` in paint order, recording its op.
    pub(super) fn add_ink(&mut self, index: usize, ink: Ink) {
        let object = PageObject::Ink(ink.clone());
        self.objects.insert(index, page::Content::Ink(ink));
        let before = self.successor(object.id());
        self.record(Ok(vec![PageOp::Add { object, before }]));
    }

    /// Takes the drawing at `index` in paint order off the page, recording its op.
    pub(super) fn remove_ink(&mut self, index: usize) -> Ink {
        let page::Content::Ink(ink) = self.objects.remove(index) else {
            unreachable!("the index holds a drawing")
        };
        self.record(Ok(vec![PageOp::Delete { object: ink.id }]));
        ink
    }

    /// Gives drawing `id` `strokes`, recording the op that erases the strokes it loses and
    /// adds those it gains; the entry restoring what it had.
    pub(super) fn set_strokes(&mut self, id: ExGuid, strokes: Vec<InkStroke>) -> History {
        let ink = self
            .objects
            .iter_mut()
            .find_map(|object| match object {
                page::Content::Ink(ink) if ink.id == id => Some(ink),
                _ => None,
            })
            .expect("the drawing is on the page");
        let known = |stroke: &InkStroke, list: &[InkStroke]| list.iter().any(|s| s.id == stroke.id);
        let remove = ink
            .strokes
            .iter()
            .filter(|stroke| !known(stroke, &strokes))
            .map(|stroke| stroke.id)
            .collect();
        let (mut kept, add): (Vec<InkStroke>, Vec<InkStroke>) = strokes
            .into_iter()
            .partition(|stroke| known(stroke, &ink.strokes));
        // Strokes a drawing gains follow those it keeps, as the stored list orders them.
        kept.extend(add.iter().cloned());
        let previous = std::mem::replace(&mut ink.strokes, kept);
        self.record(Ok(vec![PageOp::Strokes {
            ink: id,
            add,
            remove,
        }]));
        History::Strokes {
            ink: id,
            strokes: previous,
        }
    }

    pub(super) fn has_ink(&self, id: ExGuid) -> bool {
        self.objects
            .iter()
            .any(|object| matches!(object, page::Content::Ink(ink) if ink.id == id))
    }
}

fn group_touched(ink: &Ink, touched: &impl Fn(&InkStroke) -> bool) -> bool {
    ink.strokes.iter().any(touched) || ink.groups.iter().any(|group| group_touched(group, touched))
}

fn points(ink: &Ink) -> Vec<[f32; 2]> {
    ink.strokes
        .iter()
        .flat_map(|stroke| stroke.points.iter().copied())
        .chain(ink.groups.iter().flat_map(points))
        .collect()
}

/// Whether `point` lies inside the closed path `polygon` (even-odd).
fn contains(polygon: &[[f32; 2]], point: [f32; 2]) -> bool {
    let mut inside = false;
    let mut previous = match polygon.last() {
        Some(last) => *last,
        None => return false,
    };
    for &current in polygon {
        if (current[1] > point[1]) != (previous[1] > point[1])
            && point[0]
                < (previous[0] - current[0]) * (point[1] - current[1]) / (previous[1] - current[1])
                    + current[0]
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

/// The least distance between segments `a` and `b`.
fn segments_distance(a: [[f32; 2]; 2], b: [[f32; 2]; 2]) -> f32 {
    let cross = |o: [f32; 2], p: [f32; 2], q: [f32; 2]| {
        (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0])
    };
    let [d1, d2] = [cross(b[0], b[1], a[0]), cross(b[0], b[1], a[1])];
    let [d3, d4] = [cross(a[0], a[1], b[0]), cross(a[0], a[1], b[1])];
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    let to_segment = |p: [f32; 2], [s, e]: [[f32; 2]; 2]| {
        let [dx, dy] = [e[0] - s[0], e[1] - s[1]];
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 {
            (((p[0] - s[0]) * dx + (p[1] - s[1]) * dy) / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (p[0] - s[0] - t * dx).hypot(p[1] - s[1] - t * dy)
    };
    to_segment(a[0], b)
        .min(to_segment(a[1], b))
        .min(to_segment(b[0], a))
        .min(to_segment(b[1], a))
}
