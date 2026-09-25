//! How a page responds to pointer, keyboard and text input as OneNote does: hit layers, drags,
//! the placement grid, picture handles, outline and picture chrome, and key routing. Hosts
//! translate their platform's events into these calls and carry out the returned requests.

pub mod accessibility;
#[cfg(test)]
mod profile;
mod scroll;
#[cfg(test)]
mod tests;

use crate::gpu::{Viewport, page::PageScene};
use crate::{
    date::DateField,
    editor::{
        CanvasEditor, DEFAULT_OUTLINE_WIDTH, Movement, Selection, SelectionUnit, TextOutline,
    },
    layout::TextEngine,
};
use draw::{Primitive, Stroke};
use std::{
    error::Error,
    time::{Duration, Instant},
};

const HANDLE_HEIGHT: f32 = 6.75;
/// Accessible names of the page date's fields.
pub const DATE_LABELS: [&str; 2] = ["Page date", "Page time"];

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Named(NamedKey),
    Character(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NamedKey {
    Escape,
    Tab,
    Space,
    Enter,
    Backspace,
    Delete,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    /// Shift, Control, Option or Command pressed alone.
    Modifier,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    pub command: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cursor {
    Default,
    Text,
    Pointer,
    Move,
    EwResize,
    NsResize,
    NwseResize,
    NeswResize,
}

/// Work only the host can do, asked for by an event.
#[derive(Debug, PartialEq)]
pub enum Request {
    /// Show the date or time picker, then report the choice through `PageView::change_date`.
    EditDate(DateField),
    /// Put this text on the clipboard.
    Copy(String),
    /// Read the clipboard and hand its text to `PageView::commit_text`.
    Paste,
    /// Show the platform's character picker.
    CharacterPalette,
}

/// What an event did: `changed` means the page, selection or view changed (the host saves,
/// updates accessibility and redraws); `redraw` alone repaints hover feedback.
#[must_use]
#[derive(Debug, Default, PartialEq)]
pub struct Response {
    pub changed: bool,
    pub redraw: bool,
    pub request: Option<Request>,
}

impl Response {
    fn changed() -> Self {
        Self {
            changed: true,
            redraw: true,
            request: None,
        }
    }

    fn redraw() -> Self {
        Self {
            redraw: true,
            ..Self::default()
        }
    }

    fn request(request: Request) -> Self {
        Self {
            request: Some(request),
            ..Self::changed()
        }
    }
}

/// The platform's caret and text selection colours, linear RGBA.
#[derive(Clone, Copy, Debug)]
pub struct TextColors {
    pub caret: [f32; 4],
    pub selection: [f32; 4],
}

/// How to paint an outline: whether the caret is in its blink-on phase, the view scale,
/// document points per device pixel, and the text colours.
#[derive(Clone, Copy)]
struct Paint {
    show_caret: bool,
    scale: f32,
    pixel: f32,
    colors: TextColors,
}

enum Drag {
    Text {
        anchor: Selection,
        unit: SelectionUnit,
    },
    Scrollbar {
        axis: usize,
        grab: f32,
    },
    Resize {
        outline: Option<Box<TextOutline>>,
        grab: f32,
    },
    Outline {
        id: onestore::ExGuid,
        grab: [f32; 2],
        pending_press: Option<[f32; 2]>,
    },
    Image {
        id: onestore::ExGuid,
        handle: [i8; 2],
        /// Document point of the press.
        press: [f32; 2],
        pending_press: Option<[f32; 2]>,
    },
}

#[derive(Clone, Copy)]
enum PointerFeedback<'a> {
    Hover(onestore::ExGuid),
    Move(onestore::ExGuid, [f32; 2]),
    Resize(&'a TextOutline),
    /// A picture being moved or resized, drawn at this origin and size.
    Image(onestore::ExGuid, [f32; 2], [f32; 2]),
}

/// A non-text object holding focus, which hides the text caret and suspends typing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ObjectFocus {
    ReadOnly(usize),
    Image(onestore::ExGuid),
}

impl ObjectFocus {
    pub fn read_only(self) -> Option<usize> {
        match self {
            Self::ReadOnly(index) => Some(index),
            Self::Image(_) => None,
        }
    }
}

fn read_only_shortcut(key: &Key, modifiers: Modifiers) -> bool {
    key == &Key::Named(NamedKey::Escape)
        || (modifiers.control && key == &Key::Named(NamedKey::Tab))
        || (modifiers.command
            && matches!(key, Key::Character(value) if matches!(value.as_str(), "+" | "=" | "-" | "0") || (modifiers.shift && value.eq_ignore_ascii_case("n"))))
}

/// A page on screen: the editor, the drawn scene, the view onto them and the pointer and
/// keyboard state between events.
pub struct PageView {
    pub editor: CanvasEditor,
    pub engine: TextEngine,
    /// The page's noneditable content and where it sits in the view.
    pub scene: Option<(PageScene, [f32; 2])>,
    pub viewport: Viewport,
    /// Device pixels per logical pixel.
    display_scale: f32,
    double_click: Duration,
    pointer: [f32; 2],
    pointer_inside: bool,
    last_click: Option<(Instant, [f32; 2], u8)>,
    drag: Option<Drag>,
    object_focus: Option<ObjectFocus>,
    modifiers: Modifiers,
    focused: bool,
    caret: bool,
    blink_at: Instant,
}

impl PageView {
    /// `size` is in device pixels; `double_click` is the platform's double-click interval.
    pub fn new(
        editor: CanvasEditor,
        engine: TextEngine,
        scene: Option<(PageScene, [f32; 2])>,
        size: [u32; 2],
        display_scale: f32,
        double_click: Duration,
    ) -> Self {
        Self {
            editor,
            engine,
            scene,
            viewport: Viewport {
                size,
                scale: display_scale * 96.0 / 72.0,
                origin: [48.0 * display_scale; 2],
            },
            display_scale,
            double_click,
            pointer: [0.0; 2],
            pointer_inside: false,
            last_click: None,
            drag: None,
            object_focus: None,
            modifiers: Modifiers::default(),
            focused: true,
            caret: true,
            blink_at: Instant::now() + Duration::from_millis(500),
        }
    }

    /// Shows a page reloaded from storage in place of the edited one.
    pub fn replace(&mut self, editor: CanvasEditor, scene: Option<(PageScene, [f32; 2])>) {
        self.editor = editor;
        self.scene = scene;
        self.drag = None;
        self.object_focus = None;
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    pub fn object_focus(&self) -> Option<ObjectFocus> {
        self.object_focus
    }

    /// Text input goes to the editor unless an object holds focus.
    pub fn accepts_text(&self) -> bool {
        self.object_focus.is_none()
    }

    /// The outline being dragged and where it would land.
    pub fn outline_preview(&self) -> Option<(onestore::ExGuid, [f32; 2])> {
        let Drag::Outline {
            id,
            grab,
            pending_press,
        } = self.drag.as_ref()?
        else {
            return None;
        };
        let origin = self
            .editor
            .outlines()
            .iter()
            .find(|outline| outline.id == *id)?
            .origin();
        if pending_press.is_some() {
            return Some((*id, origin));
        }
        let point = self.viewport.document_point(self.pointer);
        let position = [point[0] - grab[0], point[1] - grab[1]];
        Some((
            *id,
            if self.modifiers.option {
                position
            } else {
                snap_to_grid(position, self.editor.margin_origin())
            },
        ))
    }

    fn image_preview(&self) -> Option<(onestore::ExGuid, [f32; 2], [f32; 2])> {
        let Drag::Image {
            id,
            handle,
            press,
            pending_press: None,
        } = self.drag.as_ref()?
        else {
            return None;
        };
        let (origin, size) = self.editor.image_placement(*id)?;
        let point = self.viewport.document_point(self.pointer);
        let delta = [point[0] - press[0], point[1] - press[1]];
        if *handle != [0, 0] {
            let (origin, size) = resize_image(origin, size, *handle, delta);
            return Some((*id, origin, size));
        }
        // A picture in an outline's flow keeps its place.
        if self.editor.image_in_outline(*id) {
            return Some((*id, origin, size));
        }
        let origin = [origin[0] + delta[0], origin[1] + delta[1]];
        let origin = if self.modifiers.option {
            origin
        } else {
            snap_to_grid(origin, self.editor.margin_origin())
        };
        Some((*id, origin, size))
    }

    fn pixel(&self) -> f32 {
        self.display_scale / self.viewport.scale
    }

    /// The selected picture's handles lie above the page; its body keeps its paint order.
    fn hit_test(&self, point: [f32; 2]) -> Option<Hit> {
        if let Some(ObjectFocus::Image(id)) = self.object_focus
            && let Some((origin, size)) = self.editor.image_placement(id)
            && let Some(handle) = image_handle_at(image_rect(origin, size), self.pixel(), point)
        {
            return Some(Hit::Image { id, handle });
        }
        page_hit_test(&self.editor, self.scene.as_ref(), point, self.pixel())
    }

    fn set_object_focus(&mut self, focus: Option<ObjectFocus>) {
        if self.object_focus != focus {
            self.editor.finish_composition();
            self.drag = None;
            self.object_focus = focus;
        }
    }

    /// Page coordinates of a focused object.
    fn object_rect(&self, focus: ObjectFocus) -> [f32; 4] {
        let (scene, offset) = self.scene.as_ref().unwrap();
        let [x0, y0, x1, y1] = match focus {
            ObjectFocus::ReadOnly(index) => scene
                .read_only(Some(&self.editor))
                .nth(index)
                .unwrap()
                .rect(),
            ObjectFocus::Image(id) => {
                let (origin, size) = self.editor.image_placement(id).unwrap();
                image_rect(origin, size)
            }
        };
        [
            x0 + offset[0],
            y0 + offset[1],
            x1 + offset[0],
            y1 + offset[1],
        ]
    }

    /// Keeps the view in bounds and restarts the caret blink after a change.
    fn changed(&mut self) -> Result<Response> {
        self.scroll().clamp(&mut self.viewport);
        self.caret = true;
        self.blink_at = Instant::now() + Duration::from_millis(500);
        Ok(Response::changed())
    }

    /// The caret's rectangle in device pixels, for placing the platform's input method.
    pub fn caret_area(&self) -> Result<[f32; 4]> {
        let mut rect = self.editor.caret(1.0)?;
        let origin = self
            .outline_preview()
            .map(|(_, origin)| origin)
            .unwrap_or_else(|| self.editor.active_outline().origin());
        rect.x0 += f64::from(origin[0]);
        rect.y0 += f64::from(origin[1]);
        rect.x1 += f64::from(origin[0]);
        rect.y1 += f64::from(origin[1]);
        let v = self.viewport;
        let x = rect.x0 as f32 * v.scale + v.origin[0];
        let y = rect.y0 as f32 * v.scale + v.origin[1];
        Ok([
            x,
            y,
            x + (rect.width() as f32 * v.scale).max(1.0),
            y + (rect.height() as f32 * v.scale).max(1.0),
        ])
    }

    fn reveal_focus(&mut self) -> Result<()> {
        if self.viewport.size.contains(&0) {
            return Ok(());
        }
        let rect = if let Some(focus) = self.object_focus {
            let [x0, y0, x1, y1] = self.object_rect(focus).map(f64::from);
            parley::BoundingBox { x0, y0, x1, y1 }
        } else {
            let mut rect = self.editor.caret(1.0)?;
            let origin = self.editor.active_outline().origin();
            rect.x0 += f64::from(origin[0]);
            rect.x1 += f64::from(origin[0]);
            rect.y0 += f64::from(origin[1]);
            rect.y1 += f64::from(origin[1]);
            rect
        };
        let outline = self.editor.active_outline().bounds();
        for (axis, (mut start, mut end)) in [(rect.x0, rect.x1), (rect.y0, rect.y1)]
            .into_iter()
            .enumerate()
        {
            let size = f64::from(self.viewport.size[axis]);
            let margin = f64::from(16.0 * self.display_scale).min(size * 0.25);
            let (outline_start, outline_end) =
                [(outline.x0, outline.x1), (outline.y0, outline.y1)][axis];
            let outline_start = outline_start.min(start);
            let outline_end = outline_end.max(end);
            if self.object_focus.is_none()
                && (outline_end - outline_start) * f64::from(self.viewport.scale)
                    <= size - margin * 2.0
            {
                start = outline_start;
                end = outline_end;
            }
            let start =
                start * f64::from(self.viewport.scale) + f64::from(self.viewport.origin[axis]);
            let end = end * f64::from(self.viewport.scale) + f64::from(self.viewport.origin[axis]);
            let shift = if start < margin {
                margin - start
            } else if end > size - margin {
                size - margin - end
            } else {
                0.0
            };
            self.viewport.origin[axis] += shift as f32;
        }
        Ok(())
    }

    /// Reveals the caret or focused object after an edit, then reports the change.
    fn edited(&mut self) -> Result<Response> {
        self.reveal_focus()?;
        self.changed()
    }

    fn zoom(&mut self, factor: f32) {
        let point = self.viewport.document_point(self.pointer);
        let dpr = self.display_scale;
        self.viewport.scale = (self.viewport.scale * factor).clamp(dpr / 3.0, dpr * 16.0 / 3.0);
        self.viewport.origin = [
            self.pointer[0] - point[0] * self.viewport.scale,
            self.pointer[1] - point[1] * self.viewport.scale,
        ];
    }

    fn scroll(&self) -> scroll::Scroll {
        let editable = self
            .editor
            .visible_outlines()
            .chain(self.editor.caret_outline())
            .map(|outline| {
                let rect = outline.bounds();
                let offset = self
                    .scene
                    .as_ref()
                    .filter(|_| self.editor.has_page_outline(outline.id))
                    .map(|(_, offset)| *offset)
                    .unwrap_or([0.0; 2]);
                [
                    rect.x0 as f32 + offset[0],
                    rect.y0 as f32 + offset[1],
                    rect.x1 as f32 + offset[0],
                    rect.y1 as f32 + offset[1],
                ]
            });
        let fixed = self.scene.iter().flat_map(|(scene, offset)| {
            scene.content_bounds(&self.editor).map(|rect| {
                [
                    rect[0] + offset[0],
                    rect[1] + offset[1],
                    rect[2] + offset[0],
                    rect[3] + offset[1],
                ]
            })
        });
        scroll::Scroll::new(self.viewport, editable.chain(fixed))
    }

    /// Everything to draw this frame, scrollbars included.
    pub fn primitives(&self, colors: TextColors) -> Result<Vec<Primitive<'_>>> {
        let preview = match &self.drag {
            Some(Drag::Resize {
                outline: Some(outline),
                ..
            }) => Some(PointerFeedback::Resize(outline)),
            _ => self
                .outline_preview()
                .map(|(id, origin)| PointerFeedback::Move(id, origin))
                .or_else(|| {
                    self.image_preview()
                        .map(|(id, origin, size)| PointerFeedback::Image(id, origin, size))
                })
                .or_else(|| {
                    if !self.pointer_inside {
                        return None;
                    }
                    match page_hit_test(
                        &self.editor,
                        self.scene.as_ref(),
                        self.viewport.document_point(self.pointer),
                        self.pixel(),
                    ) {
                        Some(
                            Hit::Text { id, .. } | Hit::Handle { id, .. } | Hit::Resize { id, .. },
                        ) => Some(PointerFeedback::Hover(id)),
                        _ => None,
                    }
                }),
        };
        let mut primitives = page_primitives(
            &self.editor,
            self.scene.as_ref(),
            preview,
            self.object_focus,
            Paint {
                show_caret: self.caret
                    && self.focused
                    && !matches!(
                        self.drag,
                        Some(Drag::Outline { .. } | Drag::Resize { .. } | Drag::Image { .. })
                    ),
                scale: self.viewport.scale,
                pixel: self.pixel(),
                colors,
            },
        )?;
        self.scroll()
            .append(self.viewport, self.display_scale, &mut primitives);
        Ok(primitives)
    }

    /// The pointer shape at the pointer's position.
    pub fn cursor(&self) -> Cursor {
        let hit = self.hit_test(self.viewport.document_point(self.pointer));
        let scrollbar = self
            .scroll()
            .hit_test(self.viewport, self.display_scale, self.pointer)
            .is_some();
        match (&self.drag, hit) {
            (Some(Drag::Scrollbar { .. }), _) => Cursor::Default,
            (None, _) if scrollbar => Cursor::Default,
            (Some(Drag::Image { handle, .. }), _) => handle_cursor(*handle),
            (None, Some(Hit::Image { handle, .. })) => handle_cursor(handle),
            (Some(Drag::Resize { .. }), _) | (None, Some(Hit::Resize { .. })) => Cursor::EwResize,
            (Some(Drag::Outline { .. }), _) | (None, Some(Hit::Handle { .. })) => Cursor::Move,
            (None, Some(Hit::Date(_))) => Cursor::Pointer,
            (None, Some(Hit::ReadOnly(_))) => Cursor::Default,
            _ => Cursor::Text,
        }
    }

    /// Advances the caret blink; returns whether to repaint and when to call again.
    pub fn blink(&mut self, now: Instant) -> (bool, Option<Instant>) {
        let [anchor, focus] = self.editor.selection().positions;
        if !(self.focused && self.object_focus.is_none() && anchor == focus) {
            return (false, None);
        }
        let repaint = now >= self.blink_at;
        if repaint {
            self.caret = !self.caret;
            self.blink_at = now + Duration::from_millis(500);
        }
        (repaint, Some(self.blink_at))
    }

    /// `size` in device pixels.
    pub fn resized(&mut self, size: [u32; 2]) -> Result<Response> {
        self.viewport.size = size;
        if size.contains(&0) {
            return Ok(Response::default());
        }
        self.changed()
    }

    pub fn scale_factor_changed(&mut self, scale: f32) -> Result<Response> {
        let ratio = scale / self.display_scale;
        self.viewport.scale *= ratio;
        self.viewport.origin[0] *= ratio;
        self.viewport.origin[1] *= ratio;
        self.display_scale = scale;
        self.changed()
    }

    pub fn focus_changed(&mut self, focused: bool) -> Result<Response> {
        self.focused = focused;
        if !focused {
            self.drag = None;
        }
        self.changed()
    }

    pub fn modifiers_changed(&mut self, modifiers: Modifiers) -> Result<Response> {
        self.modifiers = modifiers;
        if matches!(self.drag, Some(Drag::Outline { .. })) {
            return self.changed();
        }
        Ok(Response::default())
    }

    pub fn pointer_left(&mut self) -> Response {
        self.pointer_inside = false;
        Response::redraw()
    }

    /// `position` in device pixels from the view's top-left.
    pub fn pointer_moved(&mut self, position: [f32; 2]) -> Result<Response> {
        self.pointer_inside = true;
        self.pointer = position;
        match &mut self.drag {
            Some(Drag::Scrollbar { axis, grab }) => {
                let (axis, grab) = (*axis, *grab);
                self.scroll().drag(
                    &mut self.viewport,
                    self.display_scale,
                    axis,
                    self.pointer[axis],
                    grab,
                );
                self.changed()
            }
            Some(Drag::Text { anchor, unit }) => {
                let (anchor, unit) = (*anchor, *unit);
                let point = self.viewport.document_point(self.pointer);
                let origin = self.editor.active_outline().origin();
                let target =
                    self.editor
                        .selection_at(point[0] - origin[0], point[1] - origin[1], unit)?;
                self.editor.select(drag_selection(anchor, target, unit))?;
                self.changed()
            }
            Some(Drag::Outline { pending_press, .. } | Drag::Image { pending_press, .. }) => {
                if pending_press.is_some_and(|press| {
                    (0..2)
                        .any(|axis| (position[axis] - press[axis]).abs() > 2.0 * self.display_scale)
                }) {
                    *pending_press = None;
                }
                self.changed()
            }
            Some(Drag::Resize { outline, grab }) => {
                let point = self.viewport.document_point(self.pointer);
                let width = (point[0] - self.editor.active_outline().origin()[0] - *grab).max(36.0);
                if outline.is_some()
                    || (width - self.editor.active_outline().bounds().width() as f32).abs()
                        * self.viewport.scale
                        > 2.0 * self.display_scale
                {
                    *outline = Some(Box::new(
                        self.editor.preview_resize(&mut self.engine, width)?,
                    ));
                    return self.changed();
                }
                Ok(Response::default())
            }
            None => Ok(Response::redraw()),
        }
    }

    /// A primary-button press at the pointer's position, at `now` for click counting.
    pub fn pointer_pressed(&mut self, now: Instant) -> Result<Response> {
        let point = self.viewport.document_point(self.pointer);
        let count = self
            .last_click
            .filter(|(time, point, _)| {
                now.duration_since(*time) <= self.double_click
                    && (0..2).all(|axis| {
                        (point[axis] - self.pointer[axis]).abs() <= 4.0 * self.display_scale
                    })
            })
            .map_or(1, |(_, _, count)| (count % 3) + 1);
        self.last_click = Some((now, self.pointer, count));
        if let Some((axis, grab)) =
            self.scroll()
                .hit_test(self.viewport, self.display_scale, self.pointer)
        {
            self.drag = Some(Drag::Scrollbar { axis, grab });
            return self.changed();
        }
        match self.hit_test(point) {
            Some(Hit::Date(field)) => {
                self.editor.finish_composition();
                self.drag = None;
                return Ok(Response::request(Request::EditDate(field)));
            }
            Some(Hit::ReadOnly(index)) => self.set_object_focus(Some(ObjectFocus::ReadOnly(index))),
            Some(Hit::Image { id, handle }) => {
                self.set_object_focus(Some(ObjectFocus::Image(id)));
                self.drag = Some(Drag::Image {
                    id,
                    handle,
                    press: point,
                    pending_press: Some(self.pointer),
                });
            }
            Some(Hit::Handle { id, grab }) => {
                self.set_object_focus(None);
                self.editor.focus_outline(id)?;
                self.drag = Some(Drag::Outline {
                    id,
                    grab,
                    pending_press: Some(self.pointer),
                });
            }
            Some(Hit::Resize { id, grab }) => {
                self.set_object_focus(None);
                self.editor.focus_outline(id)?;
                self.drag = Some(Drag::Resize {
                    outline: None,
                    grab,
                });
            }
            Some(Hit::Text { id, point }) => {
                let extend = self.modifiers.shift
                    && self.object_focus.is_none()
                    && id == self.editor.active_outline().id;
                self.set_object_focus(None);
                self.editor.focus_outline(id)?;
                let previous = self.editor.selection();
                self.editor.select_below(&mut self.engine, id, point)?;
                let unit = match count {
                    2 => SelectionUnit::Word,
                    3 => SelectionUnit::Paragraph,
                    _ => SelectionUnit::Grapheme,
                };
                let selection = self.editor.selection_at(point[0], point[1], unit)?;
                let selection = if extend {
                    Selection {
                        positions: [previous.positions[0], selection.positions[1]],
                        affinities: [previous.affinities[0], selection.affinities[1]],
                    }
                } else {
                    selection
                };
                self.editor.select(selection)?;
                self.drag = Some(Drag::Text {
                    anchor: selection,
                    unit,
                });
            }
            None => {
                // A fresh click places text 7 px above the pointer, on the grid.
                let position = [point[0], point[1] - 7.0 * self.pixel()];
                let position = if self.modifiers.option {
                    position
                } else {
                    snap_to_grid(position, self.editor.margin_origin())
                };
                self.editor
                    .place_caret(&mut self.engine, position, DEFAULT_OUTLINE_WIDTH)?;
                self.set_object_focus(None);
                self.drag = Some(Drag::Text {
                    anchor: self.editor.selection(),
                    unit: SelectionUnit::Grapheme,
                });
            }
        }
        self.changed()
    }

    pub fn pointer_released(&mut self) -> Result<Response> {
        let preview = self.outline_preview();
        if let Some((id, origin, size)) = self.image_preview() {
            self.editor
                .place_image(&mut self.engine, id, origin, size)?;
        }
        if let Some(Drag::Resize {
            outline: Some(outline),
            ..
        }) = self.drag.take()
        {
            self.editor
                .resize(&mut self.engine, outline.bounds().width() as f32)?;
        }
        if let Some((id, origin)) = preview {
            self.editor.move_outline(id, origin)?;
        }
        self.changed()
    }

    /// `delta` in device pixels; Command zooms about the pointer instead of scrolling.
    pub fn wheel(&mut self, delta: [f32; 2]) -> Result<Response> {
        if self.modifiers.command {
            self.zoom((delta[1] * 0.005).exp());
        } else {
            self.viewport.origin[0] += delta[0];
            self.viewport.origin[1] += delta[1];
        }
        self.changed()
    }

    /// Text the platform inserts outside key events, such as the character picker's;
    /// control characters are not text here.
    pub fn insert_text(&mut self, text: String) -> Result<Response> {
        if text.is_empty() || text.chars().any(char::is_control) {
            return Ok(Response::default());
        }
        self.commit_text(text)
    }

    /// Text committed by an input method or pasted, line breaks included.
    pub fn commit_text(&mut self, text: String) -> Result<Response> {
        if !self.accepts_text() {
            return Ok(Response::default());
        }
        self.editor.commit_text(&mut self.engine, text)?;
        self.edited()
    }

    /// An input method's marked text; `cursor` is its UTF-8 selection within `text`.
    pub fn compose(&mut self, text: String, cursor: Option<(usize, usize)>) -> Result<Response> {
        if !self.accepts_text() {
            return Ok(Response::default());
        }
        if text.is_empty() {
            self.editor.cancel_composition(&mut self.engine)?;
        } else {
            let (start, end) = cursor.unwrap_or((text.len(), text.len()));
            let utf16 = |end: usize| -> Result<u32> {
                Ok(text
                    .get(..end)
                    .ok_or("IME range splits UTF-8")?
                    .encode_utf16()
                    .count()
                    .try_into()?)
            };
            let range = utf16(start)?..utf16(end)?;
            self.editor.compose(&mut self.engine, text, range)?;
        }
        self.edited()
    }

    /// The input method was switched off mid-composition.
    pub fn cancel_composition(&mut self) -> Result<Response> {
        if !self.accepts_text() {
            return Ok(Response::default());
        }
        self.editor.cancel_composition(&mut self.engine)?;
        self.edited()
    }

    /// The date or time chosen after `Request::EditDate`.
    pub fn change_date(&mut self, timestamp: u64, text: [String; 2]) -> Result<Response> {
        self.editor.change_date(&mut self.engine, timestamp, text)?;
        self.changed()
    }

    /// Gives keyboard focus to a read-only object, as an assistive technology asks.
    pub fn focus_read_only(&mut self, index: usize) -> Result<Response> {
        self.set_object_focus(Some(ObjectFocus::ReadOnly(index)));
        self.edited()
    }

    /// Returns focus to text after the host acted on the editor directly, as an assistive
    /// technology's selection or value change does.
    pub fn focus_text(&mut self) -> Result<Response> {
        self.set_object_focus(None);
        self.edited()
    }

    pub fn key(&mut self, key: &Key, text: Option<&str>) -> Result<Response> {
        let Modifiers {
            shift,
            control,
            option,
            command,
        } = self.modifiers;
        if let Some(ObjectFocus::Image(id)) = self.object_focus
            && matches!(key, Key::Named(NamedKey::Backspace | NamedKey::Delete))
        {
            self.editor.remove_image(&mut self.engine, id)?;
            self.set_object_focus(None);
            return self.changed();
        }
        if let Some(ObjectFocus::Image(id)) = self.object_focus
            && !(shift || command || option || control)
            && let Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight) = key
            && self.editor.step_from_image(
                &mut self.engine,
                id,
                key == &Key::Named(NamedKey::ArrowRight),
            )?
        {
            self.set_object_focus(None);
            return self.edited();
        }
        if let Some(focus) = self.object_focus {
            let undo = matches!(focus, ObjectFocus::Image(_))
                && command
                && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("z"));
            if !undo && !read_only_shortcut(key, self.modifiers) {
                return Ok(Response::default());
            }
            if key == &Key::Named(NamedKey::Escape) {
                self.set_object_focus(None);
                return self.edited();
            }
        }
        if command && control && key == &Key::Named(NamedKey::Space) {
            return Ok(Response {
                request: Some(Request::CharacterPalette),
                ..Response::default()
            });
        }
        if matches!(
            self.drag,
            Some(Drag::Outline { .. } | Drag::Resize { .. } | Drag::Image { .. })
        ) {
            if key == &Key::Named(NamedKey::Modifier) {
                return Ok(Response::default());
            }
            self.drag = None;
            if key == &Key::Named(NamedKey::Escape) {
                return self.changed();
            }
        }
        if command && option {
            let delta = match key {
                Key::Named(NamedKey::ArrowLeft) => Some([-1.0, 0.0]),
                Key::Named(NamedKey::ArrowRight) => Some([1.0, 0.0]),
                Key::Named(NamedKey::ArrowUp) => Some([0.0, -1.0]),
                Key::Named(NamedKey::ArrowDown) => Some([0.0, 1.0]),
                _ => None,
            };
            if let Some(delta) = delta {
                let outline = self.editor.active_outline();
                let origin = outline.origin();
                let step = if shift { 10.0 } else { 1.0 };
                self.editor.move_outline(
                    outline.id,
                    [origin[0] + delta[0] * step, origin[1] + delta[1] * step],
                )?;
                return self.edited();
            }
        }
        if control && key == &Key::Named(NamedKey::Tab) {
            let outlines = self.editor.outlines();
            let count = outlines.len()
                + self
                    .scene
                    .as_ref()
                    .map_or(0, |(scene, _)| scene.read_only(Some(&self.editor)).count());
            if count == 0 {
                return self.changed();
            }
            let index = self
                .object_focus
                .and_then(ObjectFocus::read_only)
                .map_or_else(
                    || {
                        outlines
                            .iter()
                            .position(|outline| outline.id == self.editor.active_outline().id)
                    },
                    |index| Some(outlines.len() + index),
                );
            let next = index.map_or(if shift { count - 1 } else { 0 }, |index| {
                if shift {
                    (index + count - 1) % count
                } else {
                    (index + 1) % count
                }
            });
            if next < outlines.len() {
                self.editor.focus_outline(outlines[next].id)?;
                self.set_object_focus(None);
            } else {
                self.set_object_focus(Some(ObjectFocus::ReadOnly(next - outlines.len())));
            }
            return self.edited();
        }
        let mut request = None;
        if command && let Key::Character(key) = key {
            match key.to_lowercase().as_str() {
                "n" if shift => {
                    let position = if let Some(focus) = self.object_focus {
                        let rect = self.object_rect(focus);
                        [rect[2] + 24.0, rect[1]]
                    } else {
                        let bounds = self.editor.active_outline().bounds();
                        [bounds.x1 as f32 + 24.0, bounds.y0 as f32]
                    };
                    self.editor.place_caret(
                        &mut self.engine,
                        snap_to_grid(position, self.editor.margin_origin()),
                        DEFAULT_OUTLINE_WIDTH,
                    )?;
                    self.set_object_focus(None);
                }
                "a" => self.editor.select_all()?,
                "z" => {
                    if shift {
                        self.editor.redo(&mut self.engine)?;
                    } else {
                        self.editor.undo(&mut self.engine)?;
                    }
                    if let Some(ObjectFocus::Image(id)) = self.object_focus
                        && self.editor.image_placement(id).is_none()
                    {
                        self.set_object_focus(None);
                    }
                }
                "c" | "x" => {
                    let [anchor, focus] = self.editor.selection().positions;
                    let selected = self
                        .editor
                        .active_outline()
                        .document()
                        .slice(anchor.min(focus)..anchor.max(focus))?;
                    let text = selected
                        .iter()
                        .map(|paragraph| {
                            paragraph
                                .project()
                                .map(|projection| projection.text().text().to_owned())
                        })
                        .collect::<std::result::Result<Vec<_>, _>>()?
                        .join("\n");
                    if !text.is_empty() {
                        if key.eq_ignore_ascii_case("x") {
                            self.editor.insert(&mut self.engine, "")?;
                        }
                        request = Some(Request::Copy(text));
                    }
                }
                "v" => request = Some(Request::Paste),
                "+" | "=" => {
                    self.zoom(1.1);
                    return self.changed();
                }
                "-" => {
                    self.zoom(1.0 / 1.1);
                    return self.changed();
                }
                "0" => {
                    self.zoom((self.display_scale * 96.0 / 72.0) / self.viewport.scale);
                    return self.changed();
                }
                _ => return Ok(Response::default()),
            }
        } else {
            let movement = match key {
                Key::Named(NamedKey::ArrowLeft) => Some(if command {
                    Movement::LineStart
                } else if option {
                    Movement::WordLeft
                } else {
                    Movement::Left
                }),
                Key::Named(NamedKey::ArrowRight) => Some(if command {
                    Movement::LineEnd
                } else if option {
                    Movement::WordRight
                } else {
                    Movement::Right
                }),
                Key::Named(NamedKey::ArrowUp) => Some(if command {
                    Movement::DocumentStart
                } else if option {
                    Movement::ParagraphStart
                } else {
                    Movement::Up
                }),
                Key::Named(NamedKey::ArrowDown) => Some(if command {
                    Movement::DocumentEnd
                } else if option {
                    Movement::ParagraphEnd
                } else {
                    Movement::Down
                }),
                Key::Named(NamedKey::Home) if shift => Some(Movement::DocumentStart),
                Key::Named(NamedKey::End) if shift => Some(Movement::DocumentEnd),
                Key::Character(key) if control && !option => match key.as_str() {
                    "a" => Some(Movement::LineStart),
                    "e" => Some(Movement::LineEnd),
                    "b" => Some(Movement::Left),
                    "f" => Some(Movement::Right),
                    "p" => Some(Movement::Up),
                    "n" => Some(Movement::Down),
                    _ => None,
                },
                _ => None,
            };
            if let Some(movement) = movement {
                self.editor
                    .move_selection(&mut self.engine, movement, shift)?;
            } else if self.editor.marked_range().is_none() {
                match key {
                    Key::Named(NamedKey::Backspace) if command || option => {
                        self.editor.delete_to(
                            &mut self.engine,
                            if command {
                                Movement::LineStart
                            } else {
                                Movement::WordLeft
                            },
                        )?;
                    }
                    Key::Named(NamedKey::Delete) if command || option => {
                        self.editor.delete_to(
                            &mut self.engine,
                            if command {
                                Movement::LineEnd
                            } else {
                                Movement::WordRight
                            },
                        )?;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        self.editor.delete(&mut self.engine, true)?;
                    }
                    Key::Named(NamedKey::Delete) => {
                        self.editor.delete(&mut self.engine, false)?;
                    }
                    Key::Named(end @ (NamedKey::Home | NamedKey::End)) => {
                        let limits = self.scroll();
                        self.viewport.origin[1] = -if *end == NamedKey::Home {
                            limits.min[1]
                        } else {
                            limits.max[1]
                        };
                        return self.changed();
                    }
                    Key::Character(key) if control && !option => match key.as_str() {
                        "h" => {
                            self.editor.delete(&mut self.engine, true)?;
                        }
                        "d" => {
                            self.editor.delete(&mut self.engine, false)?;
                        }
                        "k" if !self.editor.delete_to(&mut self.engine, Movement::LineEnd)? => {
                            self.editor.delete(&mut self.engine, false)?;
                        }
                        _ => {}
                    },
                    Key::Named(NamedKey::Enter) => {
                        self.editor.enter(&mut self.engine, shift)?;
                    }
                    Key::Named(NamedKey::Tab) => {
                        self.editor.tab(&mut self.engine, shift)?;
                    }
                    _ if !command && !control => {
                        if let Some(text) = text
                            .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
                        {
                            self.editor.insert(&mut self.engine, text)?;
                        }
                    }
                    _ => {}
                }
            } else if key == &Key::Named(NamedKey::Escape) {
                self.editor.cancel_composition(&mut self.engine)?;
            }
        }
        Ok(Response {
            request,
            ..self.edited()?
        })
    }
}

/// What a document point lands on.
#[derive(Debug, PartialEq)]
pub enum Hit {
    Date(DateField),
    Resize {
        id: onestore::ExGuid,
        grab: f32,
    },
    Text {
        id: onestore::ExGuid,
        point: [f32; 2],
    },
    Handle {
        id: onestore::ExGuid,
        grab: [f32; 2],
    },
    ReadOnly(usize),
    /// `handle` is [0, 0] on the picture and a direction on the selected picture's handles.
    Image {
        id: onestore::ExGuid,
        handle: [i8; 2],
    },
}

/// What a document point lands on; `pixel` is document points per device pixel.
pub fn page_hit_test(
    editor: &CanvasEditor,
    scene: Option<&(PageScene, [f32; 2])>,
    point: [f32; 2],
    pixel: f32,
) -> Option<Hit> {
    /// Grips and text resolve front to back before any outline's padding, as a native
    /// width handle stays reachable under the next outline's left padding; the typing room
    /// below an outline comes last.
    #[derive(Clone, Copy, PartialEq)]
    enum Layer {
        Grips,
        Body,
        Below,
    }
    let hit = |outline: &TextOutline, offset: [f32; 2], layer: Layer| {
        if layer == Layer::Below {
            let outline = editor
                .outlines()
                .iter()
                .find(|source| source.id == outline.id)?;
            let local = [
                point[0] - offset[0] - outline.origin()[0],
                point[1] - offset[1] - outline.origin()[1],
            ];
            return outline.contains_extension(local).then_some(Hit::Text {
                id: outline.id,
                point: local,
            });
        }
        let (bounds, body_top) = outline_chrome(outline, pixel);
        let local = [point[0] - offset[0], point[1] - offset[1]];
        let [x, y] = local;
        if layer == Layer::Grips {
            // Width handles: 15 px inside to 6 px outside the header's right end, and 6 px
            // either side of the right border below it.
            let inside = if y < body_top { 15.0 } else { 6.0 };
            if !outline.title
                && (bounds[2] - inside * pixel..=bounds[2] + 6.0 * pixel).contains(&x)
                && (bounds[1]..=bounds[3]).contains(&y)
            {
                return Some(Hit::Resize {
                    id: outline.id,
                    grab: point[0] - outline.bounds().x1 as f32,
                });
            }
            if x >= bounds[0] && x <= bounds[2] && y >= bounds[1] && y < body_top {
                return Some(Hit::Handle {
                    id: outline.id,
                    grab: [
                        point[0] - outline.origin()[0],
                        point[1] - outline.origin()[1],
                    ],
                });
            }
            let text = outline.bounds();
            if !(text.x0..=text.x1).contains(&f64::from(x))
                || !(text.y0..=text.y1).contains(&f64::from(y))
            {
                return None;
            }
        }
        let inner = [x - outline.origin()[0], y - outline.origin()[1]];
        if let Some(picture) = outline.shaped().objects.iter().find(|object| {
            matches!(object.kind, crate::outline::ObjectKind::Picture)
                && (object.rect[0]..=object.rect[2]).contains(&inner[0])
                && (object.rect[1]..=object.rect[3]).contains(&inner[1])
        }) {
            return Some(Hit::Image {
                id: picture.id,
                handle: [0, 0],
            });
        }
        if (x >= bounds[0] && x <= bounds[2] && y >= body_top && y <= bounds[3])
            || outline.layouts().any(|(_, paragraph)| {
                paragraph.tags.iter().any(|tag| {
                    let origin = outline.origin();
                    let x = origin[0] + outline.shaped().tag_column_offset() + tag.origin[0];
                    let y = origin[1] + paragraph.origin[1] + tag.origin[1];
                    let size = crate::outline::ParagraphTag::SIZE;
                    (x..=x + size).contains(&local[0]) && (y..=y + size).contains(&local[1])
                })
            })
        {
            return Some(Hit::Text {
                id: outline.id,
                point: [
                    local[0] - outline.origin()[0],
                    local[1] - outline.origin()[1],
                ],
            });
        }
        None
    };
    let pass = |layer| {
        if let Some(hit) = editor
            .visible_outlines()
            .rev()
            .filter(|outline| scene.is_none() || !editor.has_page_outline(outline.id))
            .find_map(|outline| hit(outline, [0.0; 2], layer))
        {
            return Some(hit);
        }
        let (scene, offset) = scene?;
        match scene.hit_test(
            [point[0] - offset[0], point[1] - offset[1]],
            Some(editor),
            |id| {
                editor
                    .visible_outlines()
                    .find(|outline| outline.id == id)
                    .and_then(|outline| hit(outline, *offset, layer))
            },
        )? {
            crate::gpu::page::SceneHit::Outline(hit) => Some(hit),
            crate::gpu::page::SceneHit::Date(field) => Some(Hit::Date(field)),
            crate::gpu::page::SceneHit::ReadOnly(index) => Some(Hit::ReadOnly(index)),
            crate::gpu::page::SceneHit::Image(id) => Some(Hit::Image { id, handle: [0, 0] }),
        }
    };
    [Layer::Grips, Layer::Body, Layer::Below]
        .into_iter()
        .find_map(pass)
}

fn page_primitives<'a>(
    editor: &'a CanvasEditor,
    scene: Option<&'a (PageScene, [f32; 2])>,
    preview: Option<PointerFeedback<'a>>,
    object_focus: Option<ObjectFocus>,
    paint: Paint,
) -> Result<Vec<Primitive<'a>>> {
    let mut primitives = Vec::new();
    let draw_outline = |id, offset: [f32; 2], primitives: &mut Vec<_>| {
        let outline = editor
            .visible_outlines()
            .find(|outline| outline.id == id)
            .ok_or(crate::gpu::page::SceneError::MissingOutline)?;
        let outline = match preview {
            Some(PointerFeedback::Resize(resized)) if resized.id == outline.id => resized,
            _ => outline,
        };
        let origin = match preview {
            Some(PointerFeedback::Move(id, origin)) if id == outline.id => origin,
            _ => outline.origin(),
        };
        if (object_focus.is_none() && outline.id == editor.active_outline().id)
            || matches!(preview, Some(PointerFeedback::Hover(id) | PointerFeedback::Move(id, _)) if id == outline.id)
            || matches!(preview, Some(PointerFeedback::Resize(resized)) if resized.id == outline.id)
        {
            append_outline_chrome(
                outline,
                [origin[0] + offset[0], origin[1] + offset[1]],
                paint.pixel,
                primitives,
            );
        }
        append_outline(
            (object_focus.is_none()
                && outline.id == editor.active_outline().id
                && !matches!(preview, Some(PointerFeedback::Resize(_))))
            .then_some(editor),
            outline,
            [origin[0] + offset[0], origin[1] + offset[1]],
            paint,
            primitives,
        )?;
        if let Some((scene, _)) = scene {
            let moving = match preview {
                Some(PointerFeedback::Image(id, origin, size)) => {
                    let [x0, y0, x1, y1] = image_rect(origin, size);
                    Some((
                        id,
                        [
                            x0 + offset[0],
                            y0 + offset[1],
                            x1 + offset[0],
                            y1 + offset[1],
                        ],
                    ))
                }
                _ => None,
            };
            scene.append_outline_objects(
                outline.shaped(),
                [origin[0] + offset[0], origin[1] + offset[1]],
                moving,
                primitives,
            );
        }
        Ok::<_, Box<dyn Error>>(())
    };
    if let Some((scene, origin)) = scene {
        let moving = match preview {
            Some(PointerFeedback::Image(id, origin, size)) => Some((id, image_rect(origin, size))),
            _ => None,
        };
        scene.append_primitives_with(
            &mut primitives,
            *origin,
            Some(editor),
            moving,
            &draw_outline,
        )?;
    }
    for outline in editor
        .visible_outlines()
        .filter(|outline| scene.is_none() || !editor.has_page_outline(outline.id))
    {
        draw_outline(outline.id, [0.0; 2], &mut primitives)?;
    }
    if object_focus.is_none()
        && let Some(outline) = editor.caret_outline()
    {
        append_outline(
            Some(editor),
            outline,
            outline.origin(),
            paint,
            &mut primitives,
        )?;
    }
    if let Some(ObjectFocus::Image(id)) = object_focus {
        let (origin, size) = match preview {
            Some(PointerFeedback::Image(moving, origin, size)) if moving == id => (origin, size),
            _ => editor
                .image_placement(id)
                .ok_or("The selected picture is missing.")?,
        };
        append_image_chrome(image_rect(origin, size), paint.pixel, &mut primitives);
    }
    if let Some(ObjectFocus::ReadOnly(index)) = object_focus {
        let (scene, offset) = scene.unwrap();
        let [x0, y0, x1, y1] = scene.read_only(Some(editor)).nth(index).unwrap().rect();
        let [x0, y0, x1, y1] = [
            x0 + offset[0],
            y0 + offset[1],
            x1 + offset[0],
            y1 + offset[1],
        ];
        let border = 2.0 / paint.scale;
        for rect in [
            [x0, y0, x1, y0 + border],
            [x0, y1 - border, x1, y1],
            [x0, y0, x0 + border, y1],
            [x1 - border, y0, x1, y1],
        ] {
            primitives.push(Primitive::Rect {
                rect,
                color: [0.25, 0.45, 0.7, 1.0],
            });
        }
    }
    Ok(primitives)
}

/// Native picture handles sit on a selection border drawn 5 px outside the picture, named
/// by their direction from its center.
fn image_handles(rect: [f32; 4], pixel: f32) -> impl Iterator<Item = ([i8; 2], [f32; 2])> {
    let border = [
        rect[0] - 5.0 * pixel,
        rect[1] - 5.0 * pixel,
        rect[2] + 5.0 * pixel,
        rect[3] + 5.0 * pixel,
    ];
    [-1, 0, 1]
        .into_iter()
        .flat_map(|y| [-1, 0, 1].map(|x| [x, y]))
        .filter(|handle| *handle != [0, 0])
        .map(move |handle| {
            (
                handle,
                std::array::from_fn(|axis| {
                    let [start, end] = [border[axis], border[axis + 2]];
                    start + (end - start) * f32::from(handle[axis] + 1) * 0.5
                }),
            )
        })
}

fn image_handle_at(rect: [f32; 4], pixel: f32, point: [f32; 2]) -> Option<[i8; 2]> {
    image_handles(rect, pixel)
        .find(|(_, center)| (0..2).all(|axis| (point[axis] - center[axis]).abs() <= 5.0 * pixel))
        .map(|(handle, _)| handle)
}

fn handle_cursor(handle: [i8; 2]) -> Cursor {
    match handle {
        [0, 0] => Cursor::Move,
        [_, 0] => Cursor::EwResize,
        [0, _] => Cursor::NsResize,
        [x, y] if x == y => Cursor::NwseResize,
        _ => Cursor::NeswResize,
    }
}

fn image_rect(origin: [f32; 2], size: [f32; 2]) -> [f32; 4] {
    [
        origin[0],
        origin[1],
        origin[0] + size[0],
        origin[1] + size[1],
    ]
}

/// Drags a picture's `handle` by `delta`: edges stretch one axis, corners keep the aspect
/// ratio at the larger of the two scales, and the opposite side stays fixed.
fn resize_image(
    origin: [f32; 2],
    size: [f32; 2],
    handle: [i8; 2],
    delta: [f32; 2],
) -> ([f32; 2], [f32; 2]) {
    let mut scale: [f32; 2] = std::array::from_fn(|axis| {
        (size[axis] + f32::from(handle[axis]) * delta[axis]) / size[axis]
    });
    if !handle.contains(&0) {
        scale = [scale[0].max(scale[1]); 2];
    }
    let resized: [f32; 2] = std::array::from_fn(|axis| (size[axis] * scale[axis]).max(1.0));
    let origin = std::array::from_fn(|axis| {
        if handle[axis] < 0 {
            origin[axis] + size[axis] - resized[axis]
        } else {
            origin[axis]
        }
    });
    (origin, resized)
}

fn append_image_chrome(rect: [f32; 4], pixel: f32, primitives: &mut Vec<Primitive<'_>>) {
    let mut tint = crate::gpu::colorref(0x00e0d2e6);
    // Native pictures take this tint at 25% in sRGB; 10% in linear light matches it.
    tint[3] = 0.1;
    primitives.push(Primitive::Rect { rect, color: tint });
    primitives.push(Primitive::RoundedRect {
        rect: [
            rect[0] - 5.0 * pixel,
            rect[1] - 5.0 * pixel,
            rect[2] + 5.0 * pixel,
            rect[3] + 5.0 * pixel,
        ],
        radius: [0.0; 2],
        stroke: Some(Stroke::Dashed(pixel)),
        color: crate::gpu::colorref(0x00ff9a31),
    });
    for (handle, [x, y]) in image_handles(rect, pixel) {
        let (half, radius) = if handle.contains(&0) {
            (3.5, 0.0)
        } else {
            (4.0, 4.0)
        };
        let rect = [
            x - half * pixel,
            y - half * pixel,
            x + half * pixel,
            y + half * pixel,
        ];
        primitives.push(Primitive::RoundedRect {
            rect,
            radius: [radius * pixel; 2],
            stroke: None,
            color: crate::gpu::colorref(0x00ffefe7),
        });
        primitives.push(Primitive::RoundedRect {
            rect,
            radius: [radius * pixel; 2],
            stroke: Some(Stroke::Solid(pixel)),
            color: crate::gpu::colorref(0x00dea67b),
        });
    }
}

fn drag_selection(anchor: Selection, target: Selection, unit: SelectionUnit) -> Selection {
    if unit == SelectionUnit::Grapheme {
        return Selection {
            positions: [anchor.positions[0], target.positions[1]],
            affinities: [anchor.affinities[0], target.affinities[1]],
        };
    }
    let backwards = target.positions[0] < anchor.positions[0].min(anchor.positions[1]);
    let start = usize::from((anchor.positions[0] > anchor.positions[1]) != backwards);
    let end = usize::from((target.positions[0] > target.positions[1]) == backwards);
    Selection {
        positions: [anchor.positions[start], target.positions[end]],
        affinities: [anchor.affinities[start], target.affinities[end]],
    }
}

fn snap_to_grid(point: [f32; 2], margin: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|axis| {
        let offset = margin[axis];
        let cell = (point[axis] - offset) / 18.0;
        let nearest = cell.round();
        // Native midpoints remain free; account for the source coordinate's float precision.
        if ((cell - nearest).abs() - 0.5).abs() <= f32::EPSILON * cell.abs().max(1.0) {
            point[axis]
        } else {
            nearest * 18.0 + offset
        }
    })
}

fn outline_chrome(outline: &TextOutline, pixel: f32) -> ([f32; 4], f32) {
    let bounds = outline.bounds();
    // Native chrome combines page-scaled gutters with a fixed screen inset.
    let inset = 5.0 * pixel;
    let body_top = bounds.y0 as f32 - inset;
    if outline.title {
        let (_, paragraph) = outline.layouts().last().unwrap();
        let bottom = outline.origin()[1] + paragraph.origin[1] + paragraph.text.height();
        return (
            [
                bounds.x0 as f32 - inset - 6.0 * pixel,
                body_top,
                bounds.x1 as f32 + inset - 2.0 * pixel,
                bottom + inset,
            ],
            body_top,
        );
    }
    (
        [
            bounds.x0 as f32 - 7.5 - inset,
            body_top - HANDLE_HEIGHT,
            bounds.x1 as f32 + inset,
            bounds.y1 as f32 + HANDLE_HEIGHT + inset,
        ],
        body_top,
    )
}

fn append_outline_chrome(
    outline: &TextOutline,
    origin: [f32; 2],
    pixel: f32,
    primitives: &mut Vec<Primitive<'_>>,
) {
    let [x, y] = origin;
    let (bounds, body_top) = outline_chrome(outline, pixel);
    let [dx, dy] = [x - outline.origin()[0], y - outline.origin()[1]];
    let [left, top, right, bottom] = [
        bounds[0] + dx,
        bounds[1] + dy,
        bounds[2] + dx,
        bounds[3] + dy,
    ];
    let body_top = body_top + dy;
    if outline.title {
        primitives.push(Primitive::RoundedRect {
            rect: [left, top, right, bottom],
            radius: [6.0 * pixel, (bottom - top) * 0.5],
            stroke: Some(Stroke::Dashed(pixel)),
            color: crate::gpu::colorref(0x007f7f7f),
        });
        return;
    }
    primitives.push(Primitive::RoundedRect {
        rect: [left, top, right, body_top],
        radius: [3.0 * pixel; 2],
        stroke: None,
        color: crate::gpu::colorref(0x00e8ebed),
    });
    primitives.push(Primitive::RoundedRect {
        rect: [right - 9.0 * pixel, top, right, body_top],
        radius: [3.0 * pixel; 2],
        stroke: None,
        color: crate::gpu::colorref(0x00e5dee7),
    });
    primitives.push(Primitive::RoundedRect {
        rect: [left, top, right, bottom],
        radius: [3.0 * pixel; 2],
        stroke: Some(Stroke::Solid(pixel)),
        color: crate::gpu::colorref(0x00d9cfd8),
    });
    let middle = (top + body_top) * 0.5;
    for offset in [-3.0, 0.0, 3.0] {
        let center = (left + right) * 0.5 + offset * pixel;
        primitives.push(Primitive::RoundedRect {
            rect: [
                center - pixel * 0.5,
                middle - pixel * 0.5,
                center + pixel * 0.5,
                middle + pixel * 0.5,
            ],
            radius: [pixel * 0.5; 2],
            stroke: None,
            color: crate::gpu::colorref(0x00b4a5b4),
        });
    }
    for column in [0.0, 1.0, 2.0] {
        let half = (column + 0.5) * pixel;
        for x in [
            right - (8.0 - column) * pixel,
            right - (2.0 + column) * pixel,
        ] {
            primitives.push(Primitive::Rect {
                rect: [x, middle - half, x + pixel, middle + half],
                color: crate::gpu::colorref(0x00b4a5b4),
            });
        }
    }
}

fn append_outline<'a>(
    editor: Option<&'a CanvasEditor>,
    outline: &'a TextOutline,
    origin: [f32; 2],
    paint: Paint,
    primitives: &mut Vec<Primitive<'a>>,
) -> Result<()> {
    let [x, y] = origin;
    let Paint {
        show_caret,
        scale,
        pixel,
        colors,
    } = paint;
    outline.shaped().append_table_primitives(primitives, origin);
    outline
        .shaped()
        .append_background_primitives(primitives, origin);
    if let Some(editor) = editor {
        for rect in editor.selection_rects()? {
            primitives.push(Primitive::Rect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y0 as f32 + y,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                color: colors.selection,
            });
        }
    }
    for (index, (_, paragraph)) in outline.layouts().enumerate() {
        outline
            .shaped()
            .append_paragraph_primitives(index, paragraph, origin, primitives);
    }
    if let Some(editor) = editor {
        for rect in editor.marked_rects()? {
            primitives.push(Primitive::Rect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y1 as f32 + y - 1.0 / scale,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                color: [0.0, 0.0, 0.0, 1.0],
            });
        }
        let [anchor, focus] = editor.selection().positions;
        if show_caret && anchor == focus {
            let rect = editor.caret(2.0 * pixel)?;
            primitives.push(Primitive::RoundedRect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y0 as f32 + y,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                radius: [pixel; 2],
                stroke: None,
                color: colors.caret,
            });
        }
    }
    Ok(())
}
