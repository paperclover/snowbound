//! The Draw tab's tools on the page: pens and highlighters drawing a stroke per press, shapes
//! dragged out on the grid, the stroke eraser, and the lasso whose drawings move and delete,
//! as OneNote 2010 has them (`corpus/ink-tools`).

use super::{PageView, Response, Result, snap_to_grid};
use crate::editor::{FAVORITES, Pen};
use crate::gpu::colorref;
use draw::{
    Primitive, Stroke,
    edit::{Key, NamedKey},
};
use onestore::{
    ExGuid,
    page::{Ink, InkStroke, ink::ShapeKind, text::new_id},
};

/// The pen gallery under a section of tab colour `section` (linear RGBA): a pen in the
/// section's accent, then OneNote's favourites. The accent takes the light theme's shade in
/// either theme, so a stroke stores one colour and shows it in both.
pub fn pens(section: [f32; 4]) -> [Pen; 15] {
    let [red, green, blue] = draw::srgb_bytes({
        let [saturation, lightness] = draw::LIGHT_ACCENT;
        draw::hsl(draw::hue(section), saturation, lightness)
    });
    let accent = Pen::new(35.0, Some(u32::from_le_bytes([red, green, blue, 0])));
    std::array::from_fn(|place| place.checked_sub(1).map_or(accent, |at| FAVORITES[at]))
}

/// What a press on the page does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Tool {
    /// Select & Type: text, and drawings picked by a click.
    #[default]
    Select,
    /// A stroke per press.
    Pen(Pen),
    /// A shape dragged out, its corners on the grid unless Option is held.
    Shape(ShapeKind, Pen),
    /// The stroke eraser.
    Eraser,
    Lasso,
}

#[derive(Default)]
pub(super) struct State {
    tool: Tool,
    gesture: Option<Gesture>,
    /// Drawings picked by the lasso or a click.
    selection: Vec<ExGuid>,
}

enum Gesture {
    Stroke(Vec<[f32; 2]>),
    Shape([f32; 2]),
    /// `mark` counts the ops waiting when the sweep began.
    Erase {
        last: [f32; 2],
        erased: bool,
        mark: usize,
    },
    Lasso(Vec<[f32; 2]>),
    Move {
        press: [f32; 2],
    },
}

/// The eraser's reach either side of its path, in logical pixels.
const ERASER_REACH: f32 = 4.0;

impl PageView {
    pub fn tool(&self) -> Tool {
        self.ink.tool
    }

    /// Picks what presses on the page do, as the Draw tab's tools do; the lasso's selection
    /// goes.
    pub fn set_tool(&mut self, tool: Tool) -> Response {
        self.ink.tool = tool;
        self.leave_ink();
        Response::redraw()
    }

    /// Drawings the lasso or a click picked.
    pub fn ink_selection(&self) -> &[ExGuid] {
        &self.ink.selection
    }

    /// Ends a gesture as though it never began, as a second finger landing does on a touch
    /// screen: a sweep of the eraser gives back what it erased. The sweep's ops must not have
    /// been taken since it began.
    pub fn cancel_ink(&mut self) -> Result<Response> {
        let gesture = self.ink.gesture.take();
        self.editor.drag_ink(None);
        if let Some(Gesture::Erase {
            erased: true, mark, ..
        }) = gesture
        {
            self.editor.retract_erasing(&mut self.engine, mark)?;
        }
        self.changed()
    }

    /// Ends a gesture unfinished, as a lost focus does.
    pub(super) fn end_ink(&mut self) {
        self.ink.gesture = None;
        self.editor.drag_ink(None);
    }

    /// Another page shows: the gesture ends and nothing stays picked; the tool stays.
    pub(super) fn leave_ink(&mut self) {
        self.end_ink();
        self.ink.selection.clear();
    }

    /// Forgets picked drawings the page no longer holds, after a change made elsewhere. A
    /// stroke, shape or lasso under way goes on; a drag of drawings ends, and the eraser's
    /// next touch starts an undo step of its own.
    pub(super) fn refresh_ink(&mut self) {
        match &mut self.ink.gesture {
            Some(Gesture::Move { .. }) => self.ink.gesture = None,
            Some(Gesture::Erase { erased, .. }) => *erased = false,
            _ => {}
        }
        let editor = &self.editor;
        self.ink
            .selection
            .retain(|id| editor.ink_extent(std::slice::from_ref(id)).is_some());
    }

    fn reach(&self) -> f32 {
        ERASER_REACH * self.pixel()
    }

    /// A press at page point `point`, when a drawing tool takes it or it picks ink.
    pub(super) fn ink_pressed(&mut self, point: [f32; 2]) -> Result<Option<Response>> {
        let selected = !self.ink.selection.is_empty()
            && self
                .editor
                .ink_extent(&self.ink.selection)
                .is_some_and(|[x0, y0, x1, y1]| {
                    (x0..=x1).contains(&point[0]) && (y0..=y1).contains(&point[1])
                });
        let gesture = match self.ink.tool {
            Tool::Select | Tool::Lasso if selected => Gesture::Move { press: point },
            Tool::Select => {
                self.ink.selection.clear();
                if self.hit_test(point).is_some() {
                    return Ok(None);
                }
                let Some(id) = self.editor.ink_at(point, self.reach()) else {
                    return Ok(None);
                };
                self.set_object_focus(None);
                self.ink.selection = vec![id];
                Gesture::Move { press: point }
            }
            Tool::Pen(_) => Gesture::Stroke(vec![point]),
            Tool::Shape(..) => Gesture::Shape(self.grid(point)),
            Tool::Eraser => {
                let mark = self.editor.pending_ops();
                let erased = self.editor.erase(point, point, self.reach(), false)?;
                Gesture::Erase {
                    last: point,
                    erased,
                    mark,
                }
            }
            Tool::Lasso => {
                self.ink.selection.clear();
                Gesture::Lasso(vec![point])
            }
        };
        self.drag = None;
        self.ink.gesture = Some(gesture);
        Ok(Some(self.changed()?))
    }

    fn grid(&self, point: [f32; 2]) -> [f32; 2] {
        if self.modifiers.option {
            point
        } else {
            snap_to_grid(point, self.editor.margin_origin())
        }
    }

    /// The pointer moved to page point `point` during a gesture.
    pub(super) fn ink_moved(&mut self, point: [f32; 2]) -> Result<Response> {
        match &mut self.ink.gesture {
            Some(Gesture::Stroke(points) | Gesture::Lasso(points)) => {
                let last = points[points.len() - 1];
                // Samples closer than a HIMETRIC unit store as one point.
                if (point[0] - last[0]).hypot(point[1] - last[1]) >= 72.0 / 2540.0 {
                    points.push(point);
                }
                Ok(Response::redraw())
            }
            Some(Gesture::Erase { last, erased, .. }) => {
                let from = std::mem::replace(last, point);
                let join = *erased;
                if self.editor.erase(from, point, self.reach(), join)? {
                    if let Some(Gesture::Erase { erased, .. }) = &mut self.ink.gesture {
                        *erased = true;
                    }
                    return self.changed();
                }
                Ok(Response::default())
            }
            Some(Gesture::Move { press }) => {
                let delta = [point[0] - press[0], point[1] - press[1]];
                self.editor
                    .drag_ink(Some((self.ink.selection.clone(), delta)));
                Ok(Response::redraw())
            }
            Some(Gesture::Shape(_)) => Ok(Response::redraw()),
            None => Ok(Response::default()),
        }
    }

    /// The press ended at page point `point`, finishing the gesture.
    pub(super) fn ink_released(&mut self, point: [f32; 2]) -> Result<Response> {
        let Some(gesture) = self.ink.gesture.take() else {
            return Ok(Response::default());
        };
        match gesture {
            Gesture::Stroke(points) => {
                let Tool::Pen(pen) = self.ink.tool else {
                    return Ok(Response::default());
                };
                self.editor.draw(Ink {
                    id: new_id()?,
                    layout: Default::default(),
                    strokes: vec![pen.stroke(&points)?],
                    groups: Vec::new(),
                    shape: None,
                })?;
            }
            Gesture::Shape(from) => {
                let Tool::Shape(kind, pen) = self.ink.tool else {
                    return Ok(Response::default());
                };
                let to = self.grid(point);
                if from == to {
                    return Ok(Response::redraw());
                }
                self.editor
                    .draw(Ink::drawn(kind, from, to, &pen.stroke(&[])?)?)?;
            }
            Gesture::Erase { .. } => return Ok(Response::redraw()),
            Gesture::Lasso(mut points) => {
                points.push(point);
                self.ink.selection = self.editor.ink_within(&points);
                return Ok(Response::redraw());
            }
            Gesture::Move { press } => {
                let delta = [point[0] - press[0], point[1] - press[1]];
                // As OneNote does, a drawing dropped outside the view stays where it was.
                let inside = (0..2).all(|axis| {
                    (0.0..self.viewport.size[axis] as f32).contains(&self.pointer[axis])
                });
                let selection = self.ink.selection.clone();
                self.editor
                    .move_ink(&selection, if inside { delta } else { [0.0; 2] })?;
            }
        }
        self.changed()
    }

    /// Delete and Backspace delete picked drawings; Escape lets them go, then leaves a
    /// drawing tool for Select & Type, as OneNote's Escape does.
    pub(super) fn ink_key(&mut self, key: &Key) -> Result<Option<Response>> {
        match key {
            Key::Named(NamedKey::Backspace | NamedKey::Delete)
                if !self.ink.selection.is_empty() =>
            {
                let selection = std::mem::take(&mut self.ink.selection);
                self.editor.delete_ink(&selection)?;
                Ok(Some(self.changed()?))
            }
            Key::Named(NamedKey::Escape) if !self.ink.selection.is_empty() => {
                self.ink.selection.clear();
                Ok(Some(Response::redraw()))
            }
            Key::Named(NamedKey::Escape) if self.ink.tool != Tool::Select => {
                Ok(Some(self.set_tool(Tool::Select)))
            }
            _ => {
                self.ink.selection.clear();
                Ok(None)
            }
        }
    }

    /// Whether a press on the page began a gesture of the drawing tools.
    pub fn inking(&self) -> bool {
        self.ink.gesture.is_some()
    }

    /// Whether a drawing tool takes presses, which then show a crosshair.
    pub(super) fn drawing(&self) -> bool {
        self.ink.tool != Tool::Select
    }

    /// The gesture under way and the picked drawings' frame, over the page.
    pub(super) fn append_ink_feedback(
        &self,
        automatic: [f32; 4],
        primitives: &mut Vec<Primitive<'_>>,
    ) {
        let pixel = self.pixel();
        let point = self.viewport.document_point(self.pointer);
        let preview = |ink: Ink, primitives: &mut Vec<Primitive<'_>>| {
            crate::gpu::page::append_ink(&ink, [0.0; 2], automatic, primitives)
        };
        match (&self.ink.gesture, self.ink.tool) {
            (Some(Gesture::Stroke(points)), Tool::Pen(pen)) => {
                if let Ok(stroke) = pen.stroke(points) {
                    preview(ink(vec![stroke]), primitives);
                }
            }
            (Some(Gesture::Shape(from)), Tool::Shape(kind, pen)) => {
                let to = self.grid(point);
                if let Ok(pen) = pen.stroke(&[])
                    && let Ok(shape) = Ink::drawn(kind, *from, to, &pen)
                {
                    preview(shape, primitives);
                }
            }
            (Some(Gesture::Lasso(points)), _) => {
                let lasso = colorref(0x00808080);
                for pair in points.windows(2) {
                    primitives.push(Primitive::Segment {
                        from: pair[0],
                        to: pair[1],
                        width: pixel,
                        round: true,
                        color: lasso,
                    });
                }
            }
            _ => {}
        }
        if let Some([x0, y0, x1, y1]) = self.editor.ink_extent(&self.ink.selection) {
            let pad = 3.0 * pixel;
            primitives.push(Primitive::RoundedRect {
                rect: [x0 - pad, y0 - pad, x1 + pad, y1 + pad],
                radius: [0.0; 2],
                stroke: Some(Stroke::Dashed(pixel)),
                color: colorref(0x00e0a060),
            });
        }
    }
}

fn ink(strokes: Vec<InkStroke>) -> Ink {
    Ink {
        id: ExGuid::default(),
        layout: Default::default(),
        strokes,
        groups: Vec::new(),
        shape: None,
    }
}
