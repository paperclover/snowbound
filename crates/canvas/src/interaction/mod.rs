//! How a page responds to pointer, keyboard and text input as OneNote does: hit layers, drags,
//! the placement grid, picture handles, outline and picture chrome, and key routing. Hosts
//! translate their platform's events into these calls and carry out the returned requests.

pub mod accessibility;
pub mod ink;
#[cfg(test)]
mod profile;
mod scroll;
mod space;

pub use scroll::Scroll;
#[cfg(test)]
mod tests;

use crate::gpu::{Paper, Viewport, page::PageScene};
use crate::{
    date::DateField,
    editor::{
        Awaited, CanvasEditor, Clip, DEFAULT_OUTLINE_WIDTH, Formatting, Piece, Selection,
        TextOutline, Whole,
    },
    layout::TextEngine,
};
use draw::{
    Primitive, Stroke,
    edit::{self, Clicks, Command, Key, Modifiers, Movement, NamedKey, SelectionUnit},
};
use std::{error::Error, time::Duration};
use web_time::Instant;

const HANDLE_HEIGHT: f32 = 6.75;
/// Accessible names of the page date's fields.
pub const DATE_LABELS: [&str; 2] = ["Page date", "Page time"];

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error>>;

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
    /// Insert Space's line across the page.
    RowResize,
    /// Insert Space's line down the page.
    ColResize,
    /// A drawing tool's.
    Crosshair,
}

/// Work only the host can do, asked for by an event.
#[derive(Debug, PartialEq)]
pub enum Request {
    /// Show the date or time picker, then report the choice through `PageView::change_date`.
    EditDate(DateField),
    /// Put this on the clipboard.
    Copy(Clip),
    /// Read the clipboard and hand its text to `PageView::commit_text`.
    Paste,
    /// Open a link's address, as a click on a link does.
    OpenLink(String),
    /// Open a copy of a file with the system's application for it, as a double click does.
    OpenAttachment(onestore::page::Attachment),
    /// Play recording `file` from `at_ms`, as a note's play button does.
    Play {
        file: onestore::page::Attachment,
        at_ms: u32,
    },
}

/// What a secondary press lands on, which its context menu offers commands for.
#[derive(Debug, Default, PartialEq)]
pub struct Context {
    /// The address of the link at the caret.
    pub link: Option<String>,
    pub equation: bool,
    /// Whether text is selected, which Cut and Copy take.
    pub selected: bool,
    /// The paragraph at the caret, which Copy Link to Paragraph names.
    pub paragraph: Option<onestore::ExGuid>,
    /// The file the press selected, which Open and Save As take.
    pub attachment: Option<onestore::page::Attachment>,
    /// The marked word the press landed on, which the menu offers corrections for.
    pub spelling: Option<Correction>,
}

/// A marked word under a context menu, and what it could become.
#[derive(Debug, PartialEq)]
pub struct Correction {
    pub word: String,
    /// Replacements, best first; none for a repeated word, which Delete Repeated Word
    /// removes with the space before it.
    pub suggestions: Vec<String>,
    pub repeated: bool,
    outline: onestore::ExGuid,
    /// What a correction replaces: the word, or a repeated word and the space before it.
    range: Selection,
}

/// What an event did: `changed` means the page or selection changed (the host saves,
/// updates accessibility and redraws); `moved` that only the view scrolled or zoomed (the
/// host updates accessibility and redraws); `redraw` alone repaints hover feedback.
#[must_use]
#[derive(Debug, Default, PartialEq)]
pub struct Response {
    pub changed: bool,
    pub moved: bool,
    pub(crate) redraw: bool,
    pub request: Option<Request>,
}

impl Response {
    fn changed() -> Self {
        Self {
            changed: true,
            redraw: true,
            ..Self::default()
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

/// The platform's caret and text selection colours, linear RGBA, and the page's paper.
#[derive(Clone, Copy, Debug)]
pub struct TextColors {
    pub caret: [f32; 4],
    pub selection: [f32; 4],
    pub paper: Paper,
}

/// How to paint an outline: the caret's opacity, the view scale, document points per
/// logical pixel, and the text colours.
#[derive(Clone, Copy)]
struct Paint<'a> {
    caret: f32,
    scale: f32,
    /// The device pixel the document origin lands on, for marks drawn on the pixel grid.
    device_origin: [f32; 2],
    pixel: f32,
    colors: TextColors,
    /// The document's top and bottom the view shows; paragraphs wholly outside it are
    /// not painted.
    visible: [f32; 2],
    /// Whether the active outline shows its frame and grips.
    chrome: bool,
    /// Search matches, marked as OneNote marks them.
    found: &'a [crate::search::PageMatch],
    /// The note See Playback highlights.
    played: Option<&'a crate::search::PageMatch>,
    spelling: Option<&'a crate::spelling::Spelling>,
    tag_art: &'a crate::gpu::TagArt,
}

enum Drag {
    Text {
        anchor: Selection,
        unit: SelectionUnit,
    },
    Resize {
        outline: Option<Box<TextOutline>>,
        grab: f32,
    },
    /// A table column's right border from document x `press`, where the column was `width`
    /// wide; the preview holds the width it has reached.
    Column {
        table: onestore::ExGuid,
        column: usize,
        width: f32,
        press: f32,
        preview: Option<(f32, Box<TextOutline>)>,
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
    /// Insert Space from a press on `line` along `axis`, 0 for x.
    Space { axis: usize, line: f32 },
}

#[derive(Clone, Copy)]
enum PointerFeedback<'a> {
    Hover(onestore::ExGuid),
    /// An outline dragged this far, with the rest of the page selection it belongs to.
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
    /// A file in an outline's flow or on the page.
    File(onestore::ExGuid),
}

impl ObjectFocus {
    pub fn read_only(self) -> Option<usize> {
        match self {
            Self::ReadOnly(index) => Some(index),
            Self::Image(_) | Self::File(_) => None,
        }
    }
}

fn read_only_shortcut(key: &Key, modifiers: Modifiers) -> bool {
    key == &Key::Named(NamedKey::Escape) || (modifiers.control && key == &Key::Named(NamedKey::Tab))
}

/// Where a page was left: its scroll and the focused outline's selection.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    origin: [f32; 2],
    outline: onestore::ExGuid,
    selection: Selection,
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
    clicks: Clicks,
    pointer: [f32; 2],
    pointer_inside: bool,
    drag: Option<Drag>,
    /// Insert Space waits for its press: its line follows the pointer.
    space: bool,
    object_focus: Option<ObjectFocus>,
    modifiers: Modifiers,
    focused: bool,
    /// Input is by finger: the focused outline's grips reach further, and outline chrome
    /// shows only while the view is focused, as there is no hover to reveal it.
    pub touch: bool,
    /// A platform scroll view owns the viewport: changes neither clamp it nor reveal the
    /// caret, which the host does knowing its bars and keyboard.
    pub host_viewport: bool,
    /// OneNote's Snap to Grid: clicks, drags and shapes land on the placement grid unless
    /// Option (Alt) is held. Off, they land where the pointer is.
    pub snap_to_grid: bool,
    /// Matches of the search shown on the page, marked under their text.
    pub found: Vec<crate::search::PageMatch>,
    /// The note playing, highlighted as See Playback highlights it.
    played: Option<crate::search::PageMatch>,
    /// Marks misspelled and repeated words; none leaves words unmarked.
    pub spelling: Option<crate::spelling::Spelling>,
    /// The art the page's notebook draws its tags with.
    pub tag_art: std::sync::Arc<crate::gpu::TagArt>,
    /// The caret's opacity in its blink.
    caret: f32,
    /// When the caret last moved, which restarts its blink.
    blink_from: Instant,
    ink: ink::State,
    /// What the view showed before each zoom since the page opened, in document points:
    /// scrolling still reaches it, so a zoom keeps the point under the pointer in place.
    reach: Option<[f32; 4]>,
}

/// Room, in OneNote pixels, the view leaves beyond content it scrolls to.
const PAD: f32 = 11.0;

/// Where a page's origin sits in a view that has not scrolled, in device pixels.
fn home(display_scale: f32) -> [f32; 2] {
    [48.0 * display_scale; 2]
}

/// Brings `scene`, drawn at `offset` in the view, to what `viewport` shows.
fn update_pictures(
    scene: &mut PageScene,
    [x, y]: [f32; 2],
    editor: &CanvasEditor,
    viewport: Viewport,
    paper: Paper,
    waker: &std::task::Waker,
) -> bool {
    let [x0, y0] = viewport.document_point([0.0; 2]);
    let [x1, y1] = viewport.document_point(viewport.size.map(|side| side as f32));
    let view = [x0 - x, y0 - y, x1 - x, y1 - y];
    scene.update_pictures(Some(editor), view, viewport.scale, paper, waker)
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
        let mut view = Self {
            editor,
            engine,
            scene,
            viewport: Viewport {
                size,
                scale: display_scale * 96.0 / 72.0,
                origin: home(display_scale),
            },
            display_scale,
            clicks: Clicks::new(double_click),
            pointer: [0.0; 2],
            pointer_inside: false,
            drag: None,
            space: false,
            object_focus: None,
            modifiers: Modifiers::default(),
            focused: true,
            touch: false,
            host_viewport: false,
            snap_to_grid: true,
            found: Vec::new(),
            played: None,
            spelling: None,
            tag_art: Default::default(),
            caret: 1.0,
            blink_from: Instant::now(),
            ink: Default::default(),
            reach: None,
        };
        view.place_opened();
        view
    }

    /// Shows a page reloaded from storage in place of the edited one, whose editor it returns.
    pub(crate) fn replace(
        &mut self,
        mut editor: CanvasEditor,
        scene: Option<(PageScene, [f32; 2])>,
    ) -> CanvasEditor {
        editor.default_font = std::mem::take(&mut self.editor.default_font);
        let left = std::mem::replace(&mut self.editor, editor);
        self.scene = scene;
        self.drag = None;
        self.space = false;
        self.object_focus = None;
        self.found.clear();
        self.played = None;
        self.leave_ink();
        left
    }

    /// Shows the stored page after a change made elsewhere in place of the one shown,
    /// keeping the scroll, the caret and selection, and the pictures already drawn. Marked
    /// text must be committed or cancelled first.
    pub fn refresh(&mut self, page: onestore::page::Page) -> Result<Response> {
        if !self.editor.refresh(page, &mut self.engine)? {
            return Ok(Response::default());
        }
        if let Some((scene, _)) = &mut self.scene {
            scene.refresh(&mut self.editor, &mut self.engine)?;
        }
        self.drag = None;
        self.object_focus = None;
        self.refresh_ink();
        self.moved()
    }

    /// Shows another page as OneNote opens one, keeping the zoom: at `place` if the page was
    /// left there earlier. OneNote keeps the scroll in device pixels across zoom changes and
    /// does not reveal the restored selection. Returns the editor of the page left.
    pub fn open(
        &mut self,
        editor: CanvasEditor,
        scene: Option<(PageScene, [f32; 2])>,
        place: Option<Place>,
    ) -> CanvasEditor {
        let left = self.replace(editor, scene);
        self.reach = None;
        match place {
            Some(place) => {
                self.viewport.origin = place.origin;
                self.scroll().clamp(&mut self.viewport);
                if self.editor.focus_outline(place.outline).is_ok() {
                    let _ = self.editor.select(place.selection);
                }
            }
            None => self.place_opened(),
        }
        left
    }

    /// Takes up `parked`, the editor the page `open` just showed was left with, history and
    /// all, in place of the one it opened with.
    pub fn resume(&mut self, parked: CanvasEditor) -> Result<()> {
        self.editor.resume(parked, &mut self.engine)?;
        if let Some((scene, _)) = &mut self.scene {
            scene.refresh(&mut self.editor, &mut self.engine)?;
        }
        Ok(())
    }

    /// Where the page is left, for `open` to return to.
    pub fn place(&self) -> Place {
        Place {
            origin: self.viewport.origin,
            outline: self.editor.active_outline().id,
            selection: self.editor.selection(),
        }
    }

    /// OneNote 2010 opens a page scrolled fully up, however far down its content lies, and
    /// fully left, or right on a right-to-left page.
    fn place_opened(&mut self) {
        let scroll = self.scroll();
        let x = if self.editor.rtl() {
            scroll.max[0]
        } else {
            scroll.min[0]
        };
        self.viewport.origin = [-x, -scroll.min[1]];
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    pub fn object_focus(&self) -> Option<ObjectFocus> {
        self.object_focus
    }

    /// Starts OneNote's Insert Space: a line follows the pointer, across the page or, within
    /// half an inch of the view's left or right edge, down it; dragging from it moves what
    /// lies past it, and Escape cancels.
    pub fn insert_space(&mut self) -> Response {
        self.space = true;
        self.drag = None;
        Response::redraw()
    }

    /// Whether Insert Space waits for or follows a drag.
    pub fn inserting_space(&self) -> bool {
        self.space || matches!(self.drag, Some(Drag::Space { .. }))
    }

    /// The axis Insert Space's line moves the page along, 0 for x, while it shows.
    fn space_axis(&self) -> Option<usize> {
        if let Some(Drag::Space { axis, .. }) = self.drag {
            return Some(axis);
        }
        let reach = 48.0 * self.display_scale;
        let x = self.pointer[0];
        self.space.then_some(usize::from(
            x >= reach && x <= self.viewport.size[0] as f32 - reach,
        ))
    }

    /// Text input goes to the editor unless an object holds focus.
    pub fn accepts_text(&self) -> bool {
        self.object_focus.is_none()
    }

    /// Edits from the toolbar and menus wait while an input method composes or an object holds
    /// focus.
    fn edits_wait(&self) -> bool {
        !self.accepts_text() || self.editor.marked_range().is_some()
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
        Some((*id, self.grid(position)))
    }

    /// The dragged outline and how far it would move.
    fn drag_delta(&self) -> Option<(onestore::ExGuid, [f32; 2])> {
        let (id, [x, y]) = self.outline_preview()?;
        let outline = self
            .editor
            .outlines()
            .iter()
            .find(|outline| outline.id == id)?;
        let [from_x, from_y] = outline.origin();
        Some((id, [x - from_x, y - from_y]))
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
        Some((*id, self.grid(origin), size))
    }

    /// `point` on the placement grid, where Snap to Grid is on and Option isn't held.
    fn grid(&self, point: [f32; 2]) -> [f32; 2] {
        if self.snap_to_grid && !self.modifiers.option {
            snap_to_grid(point, self.editor.margin_origin())
        } else {
            point
        }
    }

    fn pixel(&self) -> f32 {
        self.display_scale / self.viewport.scale
    }

    /// The selected picture's handles lie above the page; its body keeps its paint order.
    fn hit_test(&self, point: [f32; 2]) -> Option<Hit> {
        if let Some(ObjectFocus::Image(id)) = self.object_focus
            && let Some((origin, size)) = self.editor.image_placement(id)
            && let Some(handle) = image_handle_at(
                image_rect(origin, size),
                self.pixel(),
                point,
                if self.touch { TOUCH_REACH } else { 0.0 },
            )
        {
            return Some(Hit::Image { id, handle });
        }
        let touched = (self.touch && self.focused && self.object_focus.is_none())
            .then(|| self.editor.active_outline().id);
        page_hit(
            &self.editor,
            self.scene.as_ref(),
            point,
            self.pixel(),
            touched,
        )
    }

    /// What the view point `position`, in device pixels, lands on, for the host to route a
    /// gesture before it starts.
    pub fn hit(&self, position: [f32; 2]) -> Option<Hit> {
        self.hit_test(self.viewport.document_point(position))
    }

    /// The Outlook task icon under the pointer, in view device pixels, for the host to name.
    pub fn task_under_pointer(&self) -> Option<[f32; 4]> {
        if !self.pointer_inside || self.drag.is_some() {
            return None;
        }
        let [x, y] = self.viewport.document_point(self.pointer);
        let view = |v: f32, axis: usize| v * self.viewport.scale + self.viewport.origin[axis];
        self.editor.visible_outlines().find_map(|outline| {
            let offset = match &self.scene {
                Some((_, offset)) if self.editor.has_page_outline(outline.id) => *offset,
                _ => [0.0; 2],
            };
            let shaped = outline.shaped();
            let left = offset[0] + outline.origin()[0] + shaped.tag_column_offset();
            let top = offset[1] + outline.origin()[1];
            shaped.tags().find_map(|(_, origin, tag)| {
                let x0 = left + origin[0];
                let y0 = top + origin[1];
                (matches!(tag.icon, crate::outline::TagIcon::Task { .. })
                    && (x0..=x0 + tag.size).contains(&x)
                    && (y0..=y0 + tag.size).contains(&y))
                .then(|| {
                    let [x1, y1] = [x0 + tag.size, y0 + tag.size];
                    [view(x0, 0), view(y0, 1), view(x1, 0), view(y1, 1)]
                })
            })
        })
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
        let rect = match focus {
            ObjectFocus::ReadOnly(index) => scene
                .read_only(Some(&self.editor))
                .nth(index)
                .unwrap()
                .rect(),
            ObjectFocus::Image(id) => {
                let (origin, size) = self.editor.image_placement(id).unwrap();
                image_rect(origin, size)
            }
            ObjectFocus::File(id) => self.editor.attachment_rect(id).unwrap(),
        };
        crate::translated(rect, *offset)
    }

    /// Keeps the view in bounds and restarts the caret blink after a change.
    fn changed(&mut self) -> Result<Response> {
        if !self.host_viewport {
            self.scroll().clamp(&mut self.viewport);
        }
        self.caret = 1.0;
        self.blink_from = Instant::now();
        Ok(Response::changed())
    }

    fn moved(&mut self) -> Result<Response> {
        if !self.host_viewport {
            self.scroll().clamp(&mut self.viewport);
        }
        Ok(Response {
            moved: true,
            redraw: true,
            ..Response::default()
        })
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
        let outline = self
            .object_focus
            .is_none()
            .then(|| self.editor.active_outline().bounds());
        self.reveal(rect, outline);
        Ok(())
    }

    /// Scrolls document `rect` into view, and with it all of `outline` where that fits.
    fn reveal(&mut self, rect: parley::BoundingBox, outline: Option<parley::BoundingBox>) {
        for (axis, (mut start, mut end)) in [(rect.x0, rect.x1), (rect.y0, rect.y1)]
            .into_iter()
            .enumerate()
        {
            let size = f64::from(self.viewport.size[axis]);
            let margin = f64::from(16.0 * self.display_scale).min(size * 0.25);
            if let Some(outline) = outline {
                let (outline_start, outline_end) =
                    [(outline.x0, outline.x1), (outline.y0, outline.y1)][axis];
                let outline_start = outline_start.min(start);
                let outline_end = outline_end.max(end);
                if (outline_end - outline_start) * f64::from(self.viewport.scale)
                    <= size - margin * 2.0
                {
                    start = outline_start;
                    end = outline_end;
                }
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
    }

    /// Highlights `played`, the note playing, as See Playback does, scrolling it into view
    /// as OneNote does when the note changes.
    pub fn set_played(&mut self, played: Option<crate::search::PageMatch>) -> Result<Response> {
        if played == self.played {
            return Ok(Response::default());
        }
        self.played = played;
        let outline = played.and_then(|(id, _)| {
            self.editor
                .outlines()
                .iter()
                .find(|outline| outline.id == id)
        });
        if let (Some(outline), Some((_, selection))) = (outline, played)
            && !self.host_viewport
            && !self.viewport.size.contains(&0)
        {
            let [x, y] = outline.origin().map(f64::from);
            let rect = outline
                .range_rects(selection)?
                .into_iter()
                .reduce(|a, b| a.union(b))
                .map(|rect| parley::BoundingBox {
                    x0: rect.x0 + x,
                    y0: rect.y0 + y,
                    x1: rect.x1 + x,
                    y1: rect.y1 + y,
                });
            if let Some(rect) = rect {
                self.reveal(rect, None);
                return self.moved();
            }
        }
        Ok(Response::redraw())
    }

    /// Moves the caret by lines until it has gone a view's height, then scrolls as far so
    /// it keeps its place on screen.
    fn move_page(&mut self, up: bool, extend: bool) -> Result<()> {
        let movement = if up { Movement::Up } else { Movement::Down };
        let start = self.caret_area()?[1];
        let mut caret = self.caret_area()?;
        while (caret[1] - start).abs() < self.viewport.size[1] as f32 {
            self.editor
                .move_selection(&mut self.engine, movement, extend)?;
            let moved = self.caret_area()?;
            if moved == caret {
                break;
            }
            caret = moved;
        }
        self.viewport.origin[1] -= caret[1] - start;
        Ok(())
    }

    /// Reveals the caret or focused object after an edit, then reports the change.
    fn edited(&mut self) -> Result<Response> {
        if !self.host_viewport {
            self.reveal_focus()?;
        }
        self.changed()
    }

    /// The page's zoom, where 1 is OneNote's 100%: 96 pixels per inch.
    pub fn zoom(&self) -> f32 {
        self.viewport.scale / (self.display_scale * 96.0 / 72.0)
    }

    /// Zooms to `zoom` about the middle of the view, within 25% to 400%.
    pub fn set_zoom(&mut self, zoom: f32) -> Result<Response> {
        let middle = self.viewport.size.map(|size| size as f32 / 2.0);
        self.zoom_about(zoom / self.zoom(), middle);
        self.moved()
    }

    /// Scales the view by `factor` about `anchor`, in device pixels from the view's
    /// top-left, as a trackpad pinch does about the pointer.
    pub fn pinch(&mut self, factor: f32, anchor: [f32; 2]) -> Result<Response> {
        self.zoom_about(factor, anchor);
        self.moved()
    }

    /// Scales the view by `factor`, keeping the document point under `anchor` in place.
    fn zoom_about(&mut self, factor: f32, anchor: [f32; 2]) {
        let [x0, y0] = self.viewport.document_point([0.0; 2]);
        let [x1, y1] = self
            .viewport
            .document_point(self.viewport.size.map(|size| size as f32));
        self.reach = Some(self.reach.map_or([x0, y0, x1, y1], |reach| {
            [
                reach[0].min(x0),
                reach[1].min(y0),
                reach[2].max(x1),
                reach[3].max(y1),
            ]
        }));
        let point = self.viewport.document_point(anchor);
        let dpr = self.display_scale;
        self.viewport.scale = (self.viewport.scale * factor).clamp(dpr / 3.0, dpr * 16.0 / 3.0);
        self.viewport.origin = [
            anchor[0] - point[0] * self.viewport.scale,
            anchor[1] - point[1] * self.viewport.scale,
        ];
    }

    /// How far the view may scroll; the current offset is `-viewport.origin`.
    pub fn scroll(&self) -> Scroll {
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
                let [left, top] = reach(outline);
                [
                    left + offset[0],
                    top + offset[1],
                    rect.x1 as f32 + offset[0],
                    rect.y1 as f32 + offset[1],
                ]
            });
        let pad = PAD * self.display_scale;
        let fixed = self.scene.iter().flat_map(|(scene, offset)| {
            scene
                .content_bounds(&self.editor)
                .map(move |(rect, padded)| {
                    (
                        crate::translated(rect, *offset),
                        if padded { pad } else { 0.0 },
                    )
                })
        });
        let mut scroll = scroll::Scroll::new(
            self.viewport,
            editable.map(|rect| (rect, pad)).chain(fixed),
            self.editor.rtl(),
        );
        if let Some(reach) = self.reach {
            for axis in 0..2 {
                scroll.min[axis] = scroll.min[axis].min(reach[axis] * self.viewport.scale);
                scroll.max[axis] = scroll.max[axis]
                    .max(reach[axis + 2] * self.viewport.scale - self.viewport.size[axis] as f32);
            }
        }
        scroll
    }

    /// Scrolls the view's corner `offset` device pixels from the page origin along `axis`,
    /// within the page's bounds.
    pub fn scroll_to(&mut self, axis: usize, offset: f32) -> Result<Response> {
        self.viewport.origin[axis] = -offset;
        self.moved()
    }

    /// Brings the page's template backgrounds to `paper` and its pictures to the view and
    /// zoom before a frame's primitives; `waker` is woken off the main thread when the view
    /// should be drawn again because a raster landed.
    pub fn update_pictures(&mut self, paper: Paper, waker: &std::task::Waker) {
        if let Some((scene, offset)) = &mut self.scene {
            update_pictures(scene, *offset, &self.editor, self.viewport, paper, waker);
        }
    }

    /// `update_pictures` for a page `open` is about to show at `offset`; true once the
    /// pictures it would show at first are drawn.
    pub fn prepare(
        &self,
        (scene, offset): &mut (PageScene, [f32; 2]),
        editor: &CanvasEditor,
        paper: Paper,
        waker: &std::task::Waker,
    ) -> bool {
        let viewport = Viewport {
            origin: home(self.display_scale),
            ..self.viewport
        };
        update_pictures(scene, *offset, editor, viewport, paper, waker)
    }

    /// Everything to draw this frame.
    pub fn primitives(&self, colors: TextColors) -> Result<Vec<Primitive<'_>>> {
        let preview = match &self.drag {
            Some(
                Drag::Resize {
                    outline: Some(outline),
                    ..
                }
                | Drag::Column {
                    preview: Some((_, outline)),
                    ..
                },
            ) => Some(PointerFeedback::Resize(outline)),
            _ => self
                .drag_delta()
                .map(|(id, delta)| PointerFeedback::Move(id, delta))
                .or_else(|| {
                    self.image_preview()
                        .map(|(id, origin, size)| PointerFeedback::Image(id, origin, size))
                })
                .or_else(|| {
                    if !self.pointer_inside || self.space {
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
        let [left, top] = self.viewport.document_point([0.0; 2]);
        let [right, bottom] = self
            .viewport
            .document_point(self.viewport.size.map(|side| side as f32));
        let mut primitives = rule_primitives(
            self.editor.rule_lines(),
            self.editor.margin_origin(),
            [left, top, right, bottom],
            self.pixel(),
            colors.paper,
        );
        primitives.extend(page_primitives(
            &self.editor,
            self.scene.as_ref(),
            preview,
            self.object_focus,
            Paint {
                caret: if self.focused && matches!(self.drag, None | Some(Drag::Text { .. })) {
                    self.caret
                } else {
                    0.0
                },
                scale: self.viewport.scale,
                device_origin: self.viewport.origin,
                pixel: self.pixel(),
                colors,
                visible: [0.0, self.viewport.size[1] as f32]
                    .map(|y| (y - self.viewport.origin[1]) / self.viewport.scale),
                chrome: !self.touch || self.focused,
                found: &self.found,
                played: self.played.as_ref(),
                spelling: self.spelling.as_ref(),
                tag_art: &self.tag_art,
            },
        )?);
        if self.drag.is_none()
            && self.pointer_inside
            && !self.space
            && let Some((button, ..)) = self
                .editor
                .play_button(self.viewport.document_point(self.pointer))
        {
            append_play_button(button, colors.paper, &mut primitives);
        }
        if let Some(axis) = self
            .space_axis()
            .filter(|_| self.pointer_inside || self.drag.is_some())
        {
            let point = self.viewport.document_point(self.pointer)[axis];
            let (line, to, moved) = match self.drag {
                Some(Drag::Space { line, .. }) => {
                    let delta = (point - line).max(self.editor.space_limit(axis, line));
                    (line, line + delta, self.editor.space_preview(axis, line))
                }
                _ => (point, point, Vec::new()),
            };
            space::append(
                axis,
                [line, to],
                &moved,
                [left, top, right, bottom],
                self.pixel(),
                &mut primitives,
            );
        }
        self.append_ink_feedback(colors.paper.ink, &mut primitives);
        Ok(primitives)
    }

    /// The pointer shape at the pointer's position.
    pub fn cursor(&self) -> Cursor {
        if let Some(axis) = self.space_axis() {
            return [Cursor::ColResize, Cursor::RowResize][axis];
        }
        let point = self.viewport.document_point(self.pointer);
        if self
            .editor
            .ink_extent(self.ink_selection())
            .is_some_and(|[x0, y0, x1, y1]| {
                (x0..=x1).contains(&point[0]) && (y0..=y1).contains(&point[1])
            })
        {
            return Cursor::Move;
        }
        if self.drawing() {
            return Cursor::Crosshair;
        }
        if self.drag.is_none() && self.play_button(point).is_some() {
            return Cursor::Pointer;
        }
        let hit = self.hit_test(point);
        match (&self.drag, hit) {
            (Some(Drag::Image { handle, .. }), _) => handle_cursor(*handle),
            (None, Some(Hit::Image { id, handle: [0, 0] }))
                if self.modifiers.command && self.editor.picture_link(id).is_some() =>
            {
                Cursor::Pointer
            }
            (None, Some(Hit::Image { handle, .. })) => handle_cursor(handle),
            (Some(Drag::Resize { .. }), _) | (None, Some(Hit::Resize { .. })) => Cursor::EwResize,
            (Some(Drag::Column { .. }), _) | (None, Some(Hit::Column { .. })) => Cursor::ColResize,
            (Some(Drag::Outline { .. }), _) | (None, Some(Hit::Handle { .. })) => Cursor::Move,
            (None, Some(Hit::Date(_))) => Cursor::Pointer,
            (None, Some(Hit::Text { id, point }))
                if self.editor.link_under(id, point).is_some() =>
            {
                Cursor::Pointer
            }
            (
                None,
                Some(Hit::ReadOnly(_) | Hit::Check { .. } | Hit::ObjectCheck(_) | Hit::File(_)),
            ) => Cursor::Default,
            _ => Cursor::Text,
        }
    }

    /// Advances the caret blink; returns whether to repaint and when to call again.
    pub fn blink(&mut self, now: Instant) -> (bool, Option<Instant>) {
        let [anchor, focus] = self.editor.selection().positions;
        if !(self.focused && self.object_focus.is_none() && anchor == focus) {
            return (false, None);
        }
        let (caret, hold) = edit::caret_blink(now.saturating_duration_since(self.blink_from));
        let repaint = caret != self.caret;
        self.caret = caret;
        (repaint, hold.map(|hold| now + hold))
    }

    /// `size` in device pixels. A right-to-left page keeps its right edge in place, as
    /// OneNote 2010's does.
    pub fn resized(&mut self, size: [u32; 2]) -> Result<Response> {
        if self.editor.rtl() {
            self.viewport.origin[0] += size[0] as f32 - self.viewport.size[0] as f32;
        }
        self.viewport.size = size;
        if size.contains(&0) {
            return Ok(Response::default());
        }
        self.moved()
    }

    pub fn scale_factor_changed(&mut self, scale: f32) -> Result<Response> {
        let ratio = scale / self.display_scale;
        self.viewport.scale *= ratio;
        self.viewport.origin[0] *= ratio;
        self.viewport.origin[1] *= ratio;
        self.display_scale = scale;
        self.moved()
    }

    pub fn focus_changed(&mut self, focused: bool) -> Result<Response> {
        self.focused = focused;
        if !focused {
            self.drag = None;
            self.space = false;
            self.end_ink();
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
        if self.inking() {
            return self.ink_moved(self.viewport.document_point(position));
        }
        match &mut self.drag {
            Some(Drag::Text { anchor, unit }) => {
                let (anchor, unit) = (*anchor, *unit);
                let point = self.viewport.document_point(self.pointer);
                let origin = self.editor.active_outline().origin();
                let target =
                    self.editor
                        .selection_at(point[0] - origin[0], point[1] - origin[1], unit)?;
                let pair = |selection: Selection| {
                    [0, 1].map(|end| (selection.positions[end], selection.affinities[end]))
                };
                let ends = edit::drag(pair(anchor), pair(target), unit, |(position, _)| position);
                self.editor.select(Selection {
                    positions: ends.map(|(position, _)| position),
                    affinities: ends.map(|(_, affinity)| affinity),
                })?;
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
            Some(Drag::Space { .. }) => Ok(Response::redraw()),
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
            Some(Drag::Column {
                table,
                column,
                width,
                press,
                preview,
            }) => {
                let width = *width + self.viewport.document_point(self.pointer)[0] - *press;
                if preview.as_ref().is_none_or(|(shown, _)| *shown != width) {
                    let outline =
                        self.editor
                            .preview_column(&mut self.engine, *table, *column, width)?;
                    *preview = Some((width, Box::new(outline)));
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
        if let Some(axis) = self.space_axis().filter(|_| self.space) {
            self.space = false;
            self.set_object_focus(None);
            self.drag = Some(Drag::Space {
                axis,
                line: point[axis],
            });
            return Ok(Response::redraw());
        }
        if let Some((file, at_ms)) = self.play_button(point) {
            self.drag = None;
            return Ok(Response::request(Request::Play { file, at_ms }));
        }
        if let Some(response) = self.ink_pressed(point)? {
            return Ok(response);
        }
        let unit = self
            .clicks
            .press(now, self.pointer, 4.0 * self.display_scale);
        match self.hit_test(point) {
            Some(Hit::Date(field)) => {
                self.editor.finish_composition();
                self.drag = None;
                return Ok(Response::request(Request::EditDate(field)));
            }
            Some(Hit::ReadOnly(index)) => self.set_object_focus(Some(ObjectFocus::ReadOnly(index))),
            Some(Hit::Check { outline, paragraph }) => {
                self.set_object_focus(None);
                self.drag = None;
                self.editor
                    .click_check(&mut self.engine, outline, paragraph)?;
                return self.changed();
            }
            Some(Hit::ObjectCheck(id)) => {
                self.set_object_focus(None);
                self.drag = None;
                self.editor.click_object_check(id)?;
                return self.changed();
            }
            Some(Hit::File(id)) => {
                self.set_object_focus(Some(ObjectFocus::File(id)));
                self.drag = None;
                // A finger plays a recording with a tap, having no play button to hover.
                if self.touch
                    && let Some(file) = self.editor.attachment(id)
                    && file.recording.is_some()
                {
                    let file = file.clone();
                    return Ok(Response::request(Request::Play { file, at_ms: 0 }));
                }
                if unit != SelectionUnit::Grapheme
                    && let Some(file) = self.editor.attachment(id)
                {
                    return Ok(Response::request(Request::OpenAttachment(file.clone())));
                }
                // A file on the page drags as a picture does.
                if !self.editor.image_in_outline(id) {
                    self.drag = Some(Drag::Image {
                        id,
                        handle: [0, 0],
                        press: point,
                        pending_press: Some(self.pointer),
                    });
                }
            }
            // A click selects a linked picture; Ctrl+click follows its link, as in OneNote.
            Some(Hit::Image { id, handle: [0, 0] })
                if self.modifiers.command
                    && let Some(address) = self.editor.picture_link(id) =>
            {
                self.drag = None;
                return Ok(Response {
                    request: Some(Request::OpenLink(address.to_owned())),
                    ..Response::default()
                });
            }
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
                // Any outline's handle drags the whole page selection.
                if self.editor.whole() != Some(Whole::Page) {
                    self.editor.focus_outline(id)?;
                }
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
            Some(Hit::Column {
                id,
                table,
                column,
                width,
            }) => {
                self.set_object_focus(None);
                self.editor.focus_outline(id)?;
                self.drag = Some(Drag::Column {
                    table,
                    column,
                    width,
                    press: point[0],
                    preview: None,
                });
            }
            Some(Hit::Text { id, point })
                if unit == SelectionUnit::Grapheme
                    && !self.modifiers.shift
                    && let Some(address) = self.editor.link_under(id, point) =>
            {
                self.drag = None;
                return Ok(Response {
                    request: Some(Request::OpenLink(address)),
                    ..Response::default()
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
                self.place_caret(point)?;
                self.set_object_focus(None);
                self.drag = Some(Drag::Text {
                    anchor: self.editor.selection(),
                    unit: SelectionUnit::Grapheme,
                });
            }
        }
        self.changed()
    }

    /// A fresh click places text 7 px above the pointer, on the grid.
    fn place_caret(&mut self, point: [f32; 2]) -> Result<()> {
        let position = [point[0], point[1] - 7.0 * self.pixel()];
        let position = self.grid(position);
        self.editor
            .place_caret(&mut self.engine, position, DEFAULT_OUTLINE_WIDTH)?;
        Ok(())
    }

    pub fn pointer_released(&mut self) -> Result<Response> {
        if self.inking() {
            return self.ink_released(self.viewport.document_point(self.pointer));
        }
        if let Some(Drag::Space { axis, line }) = self.drag {
            self.drag = None;
            let delta = self.viewport.document_point(self.pointer)[axis] - line;
            self.editor
                .insert_space(&mut self.engine, axis, line, delta)?;
            return self.changed();
        }
        // As in OneNote, an outline or picture dropped outside the view stays where it was.
        let inside =
            (0..2).all(|axis| (0.0..self.viewport.size[axis] as f32).contains(&self.pointer[axis]));
        let moving_image = matches!(self.drag, Some(Drag::Image { handle: [0, 0], .. }));
        let clicked = match self.drag {
            Some(Drag::Outline {
                id,
                pending_press: Some(_),
                ..
            }) => Some(id),
            _ => None,
        };
        let preview = self.outline_preview().filter(|_| inside);
        let delta = self.drag_delta().filter(|_| inside);
        if let Some((id, origin, size)) = self.image_preview().filter(|_| inside || !moving_image) {
            self.editor
                .place_image(&mut self.engine, id, origin, size)?;
        }
        match self.drag.take() {
            Some(Drag::Resize {
                outline: Some(outline),
                ..
            }) => {
                self.editor
                    .resize(&mut self.engine, outline.bounds().width() as f32)?;
            }
            Some(Drag::Column {
                table,
                column,
                preview: Some((width, _)),
                ..
            }) => {
                self.editor
                    .resize_column(&mut self.engine, table, column, width)?;
            }
            _ => {}
        }
        if let Some(id) = clicked {
            // A click on an outline's handle selects it as OneNote does.
            self.editor.select_outline(id)?;
        } else if let Some((_, delta)) = delta
            && self.editor.whole() == Some(Whole::Page)
        {
            self.editor.move_page(delta)?;
        } else if let Some((id, origin)) = preview {
            self.editor.move_outline(id, origin)?;
        }
        self.changed()
    }

    /// `delta` in device pixels; Command zooms about the pointer instead of scrolling.
    pub fn wheel(&mut self, delta: [f32; 2]) -> Result<Response> {
        if self.modifiers.command {
            self.zoom_about((delta[1] * 0.005).exp(), self.pointer);
        } else {
            self.viewport.origin[0] += delta[0];
            self.viewport.origin[1] += delta[1];
        }
        self.moved()
    }

    /// Text the platform inserts outside key events, such as the character picker's;
    /// control characters are not text here.
    pub fn insert_text(&mut self, text: String) -> Result<Response> {
        if text.is_empty() || text.chars().any(char::is_control) {
            return Ok(Response::default());
        }
        self.commit_text(text)
    }

    /// Insert Symbol: see [`CanvasEditor::insert_symbol`].
    pub fn insert_symbol(&mut self, symbol: char) -> Result<Response> {
        if self.edits_wait() {
            return Ok(Response::default());
        }
        self.editor.insert_symbol(&mut self.engine, symbol)?;
        self.edited()
    }

    /// Text committed by an input method, line breaks included.
    pub fn commit_text(&mut self, text: String) -> Result<Response> {
        if !self.accepts_text() {
            return Ok(Response::default());
        }
        self.editor.commit_text(&mut self.engine, text)?;
        self.edited()
    }

    /// Content Snowbound copied; see [`CanvasEditor::paste_clip`].
    pub fn paste_clip(&mut self, clip: Clip) -> Result<Response> {
        if !self.accepts_text() {
            return Ok(Response::default());
        }
        self.editor.paste_clip(&mut self.engine, clip)?;
        self.follow_pictures()?;
        self.edited()
    }

    /// Clipboard text in `language`, an LCID; see [`CanvasEditor::paste`].
    pub fn paste(&mut self, text: &str, language: u32) -> Result<Response> {
        if !self.accepts_text() {
            return Ok(Response::default());
        }
        self.editor.paste(&mut self.engine, text, language)?;
        self.edited()
    }

    /// An input method's marked text; `cursor` is its UTF-8 selection within `text`.
    pub fn compose(&mut self, text: String, cursor: Option<(usize, usize)>) -> Result<Response> {
        // Wayland input methods clear an absent composition after each cursor update.
        if !self.accepts_text() || text.is_empty() && self.editor.marked_range().is_none() {
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

    /// A toolbar command, ignored like other edits while an input method composes or an object
    /// holds focus.
    pub fn format(&mut self, command: Formatting) -> Result<Response> {
        if let Some(ObjectFocus::Image(id) | ObjectFocus::File(id)) = self.object_focus
            && self.editor.format_object(&mut self.engine, id, &command)?
        {
            return self.edited();
        }
        if self.edits_wait() {
            return Ok(Response::default());
        }
        self.editor.format(&mut self.engine, command)?;
        self.edited()
    }

    /// Copy, or Cut with `cut`: [`CanvasEditor::clip`] goes to the clipboard.
    pub fn copy(&mut self, cut: bool) -> Result<Response> {
        let page = self.editor.whole() == Some(Whole::Page);
        let Some(clip) = self.editor.clip()? else {
            return Ok(Response::default());
        };
        if cut && page {
            self.editor.delete(&mut self.engine, false)?;
        } else if cut {
            self.editor.insert(&mut self.engine, "")?;
        }
        Ok(Response {
            request: Some(Request::Copy(clip)),
            ..self.edited()?
        })
    }

    /// A secondary press at the pointer: text there takes the caret unless it lies in the
    /// selection, and a file is selected, as OneNote's context menu acts where it opens.
    /// `None` off text and files.
    pub fn context(&mut self) -> Result<Option<(Response, Context)>> {
        let point = self.viewport.document_point(self.pointer);
        let (id, point) = match self.hit_test(point) {
            Some(Hit::Text { id, point }) => (id, point),
            Some(Hit::File(id)) => {
                let attachment = self.editor.attachment(id).cloned();
                self.set_object_focus(Some(ObjectFocus::File(id)));
                let context = Context {
                    attachment,
                    ..Context::default()
                };
                return Ok(Some((self.changed()?, context)));
            }
            _ => return Ok(None),
        };
        if self.object_focus.is_some() || !self.accepts_text() {
            return Ok(None);
        }
        let outline = self.editor.active_outline().id;
        let [anchor, focus] = self.editor.selection().positions;
        self.editor.focus_outline(id)?;
        let at = self
            .editor
            .selection_at(point[0], point[1], SelectionUnit::Grapheme)?;
        let inside = outline == id
            && anchor.min(focus) <= at.positions[0]
            && at.positions[0] <= anchor.max(focus);
        if !inside {
            self.editor.select(at)?;
        }
        let [anchor, focus] = self.editor.selection().positions;
        let context = Context {
            link: self
                .editor
                .link_at(anchor.min(focus))
                .map(|link| link.target),
            equation: self.editor.in_equation(),
            selected: anchor != focus,
            paragraph: self
                .editor
                .active_outline()
                .document()
                .leaf(anchor.min(focus).paragraph)
                .map(|(_, _, node)| node.id),
            attachment: None,
            spelling: self.selected_correction(),
        };
        Ok(Some((self.changed()?, context)))
    }

    /// The marked word at the caret, or wholly selected, with its corrections.
    pub fn selected_correction(&self) -> Option<Correction> {
        let [anchor, focus] = self.editor.selection().positions;
        let correction = self.correction(anchor.min(focus))?;
        (anchor == focus || self.correction(anchor.max(focus)).as_ref() == Some(&correction))
            .then_some(correction)
    }

    /// The marked word at `at` in the focused outline, with its corrections.
    fn correction(&self, at: crate::document::TextPosition) -> Option<Correction> {
        let spelling = self.spelling.as_ref()?;
        let outline = self.editor.active_outline();
        let paragraph = outline.document().paragraph(at.paragraph)?;
        let byte = paragraph.byte_offset(at.offset).ok()?;
        let mark = spelling
            .marks(paragraph)
            .into_iter()
            .find(|mark| mark.range.start <= byte && byte <= mark.range.end)?;
        correction(spelling, outline, at.paragraph, &mark)
    }

    /// The Spelling pane's next word: the first marked word after the selection in page
    /// order, wrapping round to the top, selected. None once the page marks no word.
    pub fn next_correction(&mut self) -> Result<Option<(Response, Correction)>> {
        let Some(spelling) = &self.spelling else {
            return Ok(None);
        };
        let (id, word, correction) = {
            let mut outlines: Vec<&TextOutline> = self.editor.outlines().iter().collect();
            outlines.sort_by(|a, b| {
                let [ax, ay] = a.origin();
                let [bx, by] = b.origin();
                ay.total_cmp(&by).then(ax.total_cmp(&bx))
            });
            let active = self.editor.active_outline().id;
            let active = outlines.iter().position(|outline| outline.id == active);
            let [anchor, focus] = self.editor.selection().positions;
            let after = anchor.max(focus);
            let mut marked = outlines.iter().enumerate().flat_map(|(order, outline)| {
                outline.layouts().flat_map(move |(index, _)| {
                    let paragraph = outline.document().paragraph(index);
                    paragraph
                        .map(|paragraph| spelling.marks_now(paragraph))
                        .unwrap_or_default()
                        .into_iter()
                        .map(move |mark| (order, *outline, index, mark))
                })
            });
            let mut first = None;
            // The first word not wholly before the selection; the page's first after them all.
            let found = marked.find(|(order, outline, index, mark)| {
                first.get_or_insert_with(|| (*order, *outline, *index, mark.clone()));
                let Some(active) = active else {
                    return true;
                };
                let paragraph = outline.document().paragraph(*index);
                let offset = |byte| {
                    paragraph.map_or(0, |paragraph| paragraph.utf16_offset(byte).unwrap_or(0))
                };
                let caret = (after.paragraph, after.offset);
                *order > active
                    || *order == active
                        && ((*index, offset(mark.range.end)) > caret
                            || (*index, offset(mark.range.start)) >= caret)
            });
            let Some((_, outline, index, mark)) = found.or(first) else {
                return Ok(None);
            };
            let correction = correction(spelling, outline, index, &mark).ok_or("Unmarked word")?;
            let paragraph = outline
                .document()
                .paragraph(index)
                .ok_or("Missing paragraph")?;
            let at = |byte| -> Result<crate::document::TextPosition> {
                Ok(crate::document::TextPosition {
                    paragraph: index,
                    offset: paragraph.utf16_offset(byte)?,
                })
            };
            let word: Selection = [at(mark.range.start)?, at(mark.range.end)?].into();
            (outline.id, word, correction)
        };
        self.editor.focus_outline(id)?;
        self.editor.select(word)?;
        Ok(Some((self.edited()?, correction)))
    }

    /// Replaces the word `correction` names with `text`, or with nothing, as one edit; not
    /// once an edit since has changed the word.
    pub fn correct(&mut self, correction: &Correction, text: &str) -> Result<Response> {
        let [start, end] = correction.range.positions;
        let unchanged = self
            .editor
            .outlines()
            .iter()
            .find(|outline| outline.id == correction.outline)
            .and_then(|outline| outline.document().paragraph(start.paragraph))
            .and_then(|paragraph| {
                let range = paragraph.byte_offset(start.offset).ok()?
                    ..paragraph.byte_offset(end.offset).ok()?;
                Some(paragraph.text().get(range)?.trim_start() == correction.word)
            });
        if unchanged != Some(true) {
            return Ok(Response::default());
        }
        self.editor.focus_outline(correction.outline)?;
        self.editor.select(correction.range)?;
        self.editor.correct(&mut self.engine, text)?;
        self.edited()
    }

    /// The text and address the Link dialog opens with for the selection; none where it does
    /// not open.
    pub fn link_prefill(&self) -> Option<(String, String)> {
        self.editor.link_prefill()
    }

    /// OK in the Link dialog: links what [`Self::link_prefill`] picked, shown as `text`.
    pub fn set_link(&mut self, text: &str, address: &str) -> Result<Response> {
        if !self.accepts_text() {
            return Ok(Response::default());
        }
        self.editor.set_link(&mut self.engine, text, address)?;
        self.edited()
    }

    /// Remove Link, or Select Link with `select`, on the link at the caret.
    pub fn unlink(&mut self, select: bool) -> Result<Response> {
        if select {
            self.editor.select_link()?;
        } else {
            self.editor.remove_link(&mut self.engine)?;
        }
        self.edited()
    }

    /// Alt+= and the toolbar's Equation: see [`CanvasEditor::insert_equation`].
    pub fn insert_equation(&mut self) -> Result<Response> {
        if self.edits_wait() {
            return Ok(Response::default());
        }
        self.editor.insert_equation(&mut self.engine)?;
        self.edited()
    }

    /// Insert, Table: see [`CanvasEditor::insert_table`].
    pub fn insert_table(&mut self, rows: usize, columns: usize) -> Result<Response> {
        if self.edits_wait() {
            return Ok(Response::default());
        }
        self.editor.insert_table(&mut self.engine, rows, columns)?;
        self.edited()
    }

    /// Insert, Picture, or a pasted one: a PNG, JPEG or GIF `size` points large; see
    /// [`CanvasEditor::insert_picture`].
    pub fn insert_picture(&mut self, bytes: Vec<u8>, size: [f32; 2]) -> Result<Response> {
        if self.edits_wait() {
            return Ok(Response::default());
        }
        let image = crate::editor::picture(bytes, size)?;
        self.editor.insert_picture(&mut self.engine, image)?;
        self.follow_pictures()?;
        self.edited()
    }

    /// Pasted clips and pictures, in order and as one undo step; see
    /// [`CanvasEditor::paste_pieces`].
    pub fn paste_pieces(&mut self, pieces: Vec<Piece>) -> Result<(Vec<Awaited>, Response)> {
        if self.edits_wait() {
            return Ok((Vec::new(), Response::default()));
        }
        let awaited = self.editor.paste_pieces(&mut self.engine, pieces)?;
        self.follow_pictures()?;
        Ok((awaited, self.edited()?))
    }

    /// An awaited picture arrived: see [`CanvasEditor::insert_awaited`]. The view stays.
    pub fn insert_awaited(
        &mut self,
        at: Awaited,
        bytes: Vec<u8>,
        size: [f32; 2],
    ) -> Result<Response> {
        let image = crate::editor::picture(bytes, size)?;
        if !self.editor.insert_awaited(&mut self.engine, at, image)? {
            return Ok(Response::default());
        }
        self.follow_pictures()?;
        self.changed()
    }

    /// Record Audio: see [`CanvasEditor::start_recording`]; none where text cannot go.
    pub fn start_recording(&mut self, label: &str) -> Result<(Option<[u8; 16]>, Response)> {
        if self.edits_wait() {
            return Ok((None, Response::default()));
        }
        let id = self.editor.start_recording(&mut self.engine, label)?;
        Ok((Some(id), self.edited()?))
    }

    /// Stop: see [`CanvasEditor::finish_recording`]. A file without an icon takes the
    /// page's blank one.
    pub fn finish_recording(&mut self, mut file: onestore::page::Attachment) -> Result<Response> {
        file.preview
            .get_or_insert_with(|| crate::gpu::page::file_icon().into());
        self.editor.finish_composition();
        self.editor.finish_recording(&mut self.engine, file)?;
        self.follow_pictures()?;
        self.edited()
    }

    /// The recording a press at document point `point` plays and the moment it plays from:
    /// that of a play button OneNote shows beside a linked note or a recording.
    fn play_button(&self, point: [f32; 2]) -> Option<(onestore::page::Attachment, u32)> {
        if !self.pointer_inside || self.space {
            return None;
        }
        let pointer = self.viewport.document_point(self.pointer);
        let ([x0, y0, x1, y1], recording, at) = self.editor.play_button(pointer)?;
        ((x0..=x1).contains(&point[0]) && (y0..=y1).contains(&point[1]))
            .then(|| Some((self.editor.recording_file(recording)?.clone(), at)))
            .flatten()
    }

    /// Insert, Attach File: see [`CanvasEditor::insert_attachment`]. A file without an icon
    /// takes the page's blank one.
    pub fn insert_attachment(&mut self, mut file: onestore::page::Attachment) -> Result<Response> {
        if self.edits_wait() {
            return Ok(Response::default());
        }
        file.preview
            .get_or_insert_with(|| crate::gpu::page::file_icon().into());
        self.editor.insert_attachment(&mut self.engine, file)?;
        self.follow_pictures()?;
        self.edited()
    }

    /// A file dropped at view point `position`, in device pixels: text or blank page there
    /// takes the caret as a click would, then the file is attached at the caret, on the page
    /// where the caret is on blank page.
    pub fn drop_attachment(
        &mut self,
        position: [f32; 2],
        file: onestore::page::Attachment,
    ) -> Result<Response> {
        self.drop_caret(position)?;
        self.insert_attachment(file)
    }

    /// A picture file dropped at view point `position`, in device pixels, placed as
    /// [`Self::drop_attachment`] places a file and sized as [`Self::insert_picture`].
    pub fn drop_picture(
        &mut self,
        position: [f32; 2],
        bytes: Vec<u8>,
        size: [f32; 2],
    ) -> Result<Response> {
        self.drop_caret(position)?;
        self.insert_picture(bytes, size)
    }

    /// Puts the caret where a drop at `position` lands, as a click there would.
    fn drop_caret(&mut self, position: [f32; 2]) -> Result<()> {
        let point = self.viewport.document_point(position);
        self.set_object_focus(None);
        match self.hit_test(point) {
            Some(Hit::Text { id, point }) => {
                self.editor.focus_outline(id)?;
                self.editor.select_below(&mut self.engine, id, point)?;
                let at = self
                    .editor
                    .selection_at(point[0], point[1], SelectionUnit::Grapheme)?;
                self.editor.select(at)?;
            }
            None => self.place_caret(point)?,
            _ => {}
        }
        Ok(())
    }

    /// Professional, or Linear with `linear`, on the equation at the caret.
    pub fn switch_equation(&mut self, linear: bool) -> Result<Response> {
        if linear {
            self.editor.linear_equation(&mut self.engine)?;
        } else {
            self.editor.build_equation(&mut self.engine)?;
        }
        self.edited()
    }

    /// Undoes the last edit, or redoes the last undone one.
    pub fn undo(&mut self, redo: bool) -> Result<Response> {
        if redo {
            self.editor.redo(&mut self.engine)?;
        } else {
            self.editor.undo(&mut self.engine)?;
        }
        self.follow_pictures()?;
        if let Some(ObjectFocus::Image(id) | ObjectFocus::File(id)) = self.object_focus
            && self.editor.image_placement(id).is_none()
        {
            self.set_object_focus(None);
        }
        self.edited()
    }

    /// Select All, widening the selection a unit at a time as OneNote 2010's Ctrl+A does.
    pub fn widen_selection(&mut self) -> Result<Response> {
        self.editor.widen_selection()?;
        // Revealing the last outline would scroll the page selection's start away.
        if self.editor.whole() == Some(Whole::Page) {
            return self.changed();
        }
        self.edited()
    }

    /// The date or time chosen after `Request::EditDate`.
    pub fn change_date(&mut self, timestamp: u64, text: [String; 2]) -> Result<Response> {
        self.editor.change_date(&mut self.engine, timestamp, text)?;
        self.changed()
    }

    /// Gives the page a colour (COLORREF), rule lines and, when given, `art` in place of its
    /// background pictures, as one undo step: see [`CanvasEditor::set_paper`].
    pub fn set_paper(
        &mut self,
        color: Option<u32>,
        rule_lines: Option<onestore::page::RuleLines>,
        art: Option<Vec<onestore::page::Image>>,
    ) -> Result<Response> {
        if !self.editor.set_paper(color, rule_lines, art) {
            return Ok(Response::default());
        }
        self.follow_pictures()?;
        self.changed()
    }

    /// Draws page-level pictures an edit or undo brought to the page.
    fn follow_pictures(&mut self) -> Result<()> {
        if let Some((scene, _)) = &mut self.scene {
            scene.follow(&mut self.editor, &mut self.engine)?;
        }
        Ok(())
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
        if key == &Key::Named(NamedKey::Modifier) {
            return Ok(Response::default());
        }
        if let Some(response) = self.ink_key(key)? {
            return Ok(response);
        }
        if let Some(ObjectFocus::Image(id) | ObjectFocus::File(id)) = self.object_focus
            && matches!(key, Key::Named(NamedKey::Backspace | NamedKey::Delete))
        {
            self.editor.remove_image(&mut self.engine, id)?;
            self.set_object_focus(None);
            return self.changed();
        }
        if let Some(ObjectFocus::Image(id) | ObjectFocus::File(id)) = self.object_focus
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
        if self.object_focus.is_some() {
            if !read_only_shortcut(key, self.modifiers) {
                return Ok(Response::default());
            }
            if key == &Key::Named(NamedKey::Escape) {
                self.set_object_focus(None);
                return self.edited();
            }
        }
        if !matches!(self.drag, None | Some(Drag::Text { .. })) || self.space {
            self.drag = None;
            self.space = false;
            if key == &Key::Named(NamedKey::Escape) {
                return self.changed();
            }
        }
        if self.editor.whole() == Some(Whole::Page) {
            match (Command::from_key(key, self.modifiers), key) {
                // OneNote 2010 keeps its page selection on these.
                (
                    Some(
                        Command::Move(Movement::Up | Movement::Down)
                        | Command::MovePage { .. }
                        | Command::MoveParagraphs { .. },
                    ),
                    _,
                )
                | (_, Key::Named(NamedKey::Tab)) => return Ok(Response::default()),
                // Enter leaves the caret at the selection's end, as Right does.
                (_, Key::Named(NamedKey::Enter)) => {
                    self.editor
                        .move_selection(&mut self.engine, Movement::Right, false)?;
                    return self.edited();
                }
                _ => {}
            }
        }
        if let Some(Command::MoveParagraphs { up }) = Command::from_key(key, self.modifiers)
            && self.editor.marked_range().is_none()
        {
            self.editor.move_paragraphs(&mut self.engine, up)?;
            return self.edited();
        }
        if command && option {
            let delta = match key {
                Key::Named(NamedKey::ArrowLeft) => Some([-1.0, 0.0]),
                Key::Named(NamedKey::ArrowRight) => Some([1.0, 0.0]),
                Key::Named(NamedKey::ArrowUp) => Some([0.0, -1.0]),
                Key::Named(NamedKey::ArrowDown) => Some([0.0, 1.0]),
                _ => None,
            };
            if let Some(delta) = delta
                && self.editor.whole() == Some(Whole::Page)
            {
                let step = if shift { 10.0 } else { 1.0 };
                self.editor.move_page(delta.map(|axis| axis * step))?;
                return self.edited();
            }
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
        // The host runs the shortcut modifier's chords from its command table.
        if command && let Key::Character(_) = key {
            return Ok(Response::default());
        }
        let chord = Command::from_key(key, self.modifiers);
        if let Some(Command::Move(movement)) = chord {
            self.editor
                .move_selection(&mut self.engine, movement, shift)?;
        } else if let Some(Command::ScrollPage { up }) = chord {
            // AppKit keeps ten points of the last page in view.
            let page = (self.viewport.size[1] as f32 - 10.0 * self.display_scale).max(0.0);
            self.viewport.origin[1] += if up { page } else { -page };
            return self.moved();
        } else if let Some(Command::MovePage { up }) = chord {
            self.move_page(up, shift)?;
        } else if self.editor.marked_range().is_none() {
            match (chord, key) {
                (Some(Command::DeleteTo(movement)), _) => {
                    self.editor.delete_to(&mut self.engine, movement)?;
                }
                (Some(Command::Delete { backward }), _) => {
                    self.editor.delete(&mut self.engine, backward)?;
                }
                (Some(Command::Kill), _) => {
                    if !self.editor.delete_to(&mut self.engine, Movement::LineEnd)? {
                        self.editor.delete(&mut self.engine, false)?;
                    }
                }
                (_, Key::Named(end @ (NamedKey::Home | NamedKey::End))) => {
                    let limits = self.scroll();
                    self.viewport.origin[1] = -if *end == NamedKey::Home {
                        limits.min[1]
                    } else {
                        limits.max[1]
                    };
                    return self.moved();
                }
                (_, Key::Named(NamedKey::Enter)) => {
                    // Enter inside a link follows it, as OneNote's does.
                    let [anchor, focus] = self.editor.selection().positions;
                    if let Some(link) = self.editor.link_at(focus).filter(|link| {
                        anchor == focus
                            && !shift
                            && link.label.start < focus.offset
                            && focus.offset < link.label.end
                    }) {
                        return Ok(Response {
                            request: Some(Request::OpenLink(link.target)),
                            ..Response::default()
                        });
                    }
                    self.editor.enter(&mut self.engine, shift)?;
                }
                (_, Key::Named(NamedKey::Tab)) => {
                    self.editor.tab(&mut self.engine, shift)?;
                }
                _ if !command && !control => {
                    if let Some(text) =
                        text.filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
                    {
                        self.editor.insert(&mut self.engine, text)?;
                    }
                }
                _ => {}
            }
        } else if key == &Key::Named(NamedKey::Escape) {
            self.editor.cancel_composition(&mut self.engine)?;
        }
        self.edited()
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
    /// The right border of `column` of `table`, which is `width` wide.
    Column {
        id: onestore::ExGuid,
        table: onestore::ExGuid,
        column: usize,
        width: f32,
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
    /// A check box tag on paragraph `paragraph`.
    Check {
        outline: onestore::ExGuid,
        paragraph: onestore::ExGuid,
    },
    /// `handle` is [0, 0] on the picture and a direction on the selected picture's handles.
    Image {
        id: onestore::ExGuid,
        handle: [i8; 2],
    },
    /// A file in an outline's flow or on the page, its icon or name.
    File(onestore::ExGuid),
    /// A check box tag on a picture or file on the page.
    ObjectCheck(onestore::ExGuid),
}

/// What a document point lands on; `pixel` is document points per device pixel.
pub(crate) fn page_hit_test(
    editor: &CanvasEditor,
    scene: Option<&(PageScene, [f32; 2])>,
    point: [f32; 2],
    pixel: f32,
) -> Option<Hit> {
    page_hit(editor, scene, point, pixel, None)
}

/// OneNote 2010's play button: a blue disc with a white triangle, in `rect`.
fn append_play_button(rect: [f32; 4], paper: Paper, primitives: &mut Vec<Primitive<'_>>) {
    let side = rect[2] - rect[0];
    primitives.push(Primitive::Gradient {
        rect,
        radius: [side / 2.0; 2],
        colors: [0x00e6a56e, 0x00c8783a].map(|color| paper.tint(crate::gpu::colorref(color))),
    });
    primitives.push(Primitive::Path {
        data: "M4 3L8 5.25L4 7.5Z",
        origin: [rect[0], rect[1]],
        style: draw::PathStyle::Fill,
        colors: [[1.0; 4]; 2],
    });
}

/// Logical pixels a finger's grip targets extend beyond the drawn grips.
const TOUCH_REACH: f32 = 16.0;

/// `page_hit_test` where the `touched` outline's grips reach `TOUCH_REACH` further: outward,
/// and inward along its header.
fn page_hit(
    editor: &CanvasEditor,
    scene: Option<&(PageScene, [f32; 2])>,
    point: [f32; 2],
    pixel: f32,
    touched: Option<onestore::ExGuid>,
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
            let extra = if touched == Some(outline.id) && !outline.title {
                TOUCH_REACH * pixel
            } else {
                0.0
            };
            // Width handles: 15 px inside to 6 px outside the header's right end, and 6 px
            // either side of the right border below it.
            let inside = if y < body_top {
                15.0 * pixel + extra
            } else {
                6.0 * pixel
            };
            if !outline.title
                && (bounds[2] - inside..=bounds[2] + 6.0 * pixel + extra).contains(&x)
                && (bounds[1] - extra..=bounds[3] + extra).contains(&y)
            {
                return Some(Hit::Resize {
                    id: outline.id,
                    grab: point[0] - outline.bounds().x1 as f32,
                });
            }
            if (bounds[0] - extra..=bounds[2] + extra).contains(&x)
                && y >= bounds[1] - extra
                && y < body_top
            {
                return Some(Hit::Handle {
                    id: outline.id,
                    grab: [
                        point[0] - outline.origin()[0],
                        point[1] - outline.origin()[1],
                    ],
                });
            }
            let shaped = outline.shaped();
            let [left, top] = outline.origin();
            let check = shaped.tags().find(|(_, origin, tag)| {
                let tag_x = left + shaped.tag_column_offset() + origin[0];
                let tag_y = top + origin[1];
                tag.icon.checkable()
                    && (tag_x..=tag_x + tag.size).contains(&x)
                    && (tag_y..=tag_y + tag.size).contains(&y)
            });
            if let Some((paragraph, ..)) = check {
                return Some(Hit::Check {
                    outline: outline.id,
                    paragraph,
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
        if let Some((table, column, width)) = outline.column_border(inner, 3.0 * pixel) {
            return Some(Hit::Column {
                id: outline.id,
                table,
                column,
                width,
            });
        }
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
        if let Some(file) = outline.shaped().objects.iter().find(|object| {
            let [x0, y0, x1, y1] = object.bounds();
            matches!(object.kind, crate::outline::ObjectKind::File(_))
                && (x0..=x1).contains(&inner[0])
                && (y0..=y1).contains(&inner[1])
        }) {
            return Some(Hit::File(file.id));
        }
        if x >= bounds[0] && x <= bounds[2] && y >= body_top && y <= bounds[3] {
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
            crate::gpu::page::SceneHit::File(id) => Some(Hit::File(id)),
            crate::gpu::page::SceneHit::Check(id) => Some(Hit::ObjectCheck(id)),
        }
    };
    [Layer::Grips, Layer::Body, Layer::Below]
        .into_iter()
        .find_map(pass)
}

/// The page's rule lines over `visible`, a document rectangle, as OneNote 2010 draws them:
/// 1/96 inch wide, or a device pixel where that is wider, horizontal lines from the margin
/// origin down, and a grid's vertical lines through it or a margin line 1/96 inch left of it.
pub(crate) fn rule_primitives(
    lines: Option<onestore::page::RuleLines>,
    [x, y]: [f32; 2],
    [left, top, right, bottom]: [f32; 4],
    pixel: f32,
    paper: Paper,
) -> Vec<Primitive<'static>> {
    use onestore::page::VerticalRule;
    let Some(lines) = lines else {
        return Vec::new();
    };
    let width = pixel.max(0.75);
    // Stored spacings are in half inches; nothing draws closer than a device pixel apart.
    let step = |spacing: f32| (spacing * 36.0).max(pixel);
    let color = |color| paper.tint(crate::gpu::colorref(color));
    let horizontal = step(lines.spacing);
    let first = ((top - y) / horizontal).ceil().max(0.0);
    let mut primitives: Vec<_> = (first as u32..)
        .map(|index| y + index as f32 * horizontal)
        .take_while(|at| *at <= bottom + width)
        .map(|at| Primitive::Rect {
            rect: [left, at - width / 2.0, right, at + width / 2.0],
            color: color(lines.color),
        })
        .collect();
    let vertical = |at: f32, rule_color| Primitive::Rect {
        rect: [at - width / 2.0, top, at + width / 2.0, bottom],
        color: color(rule_color),
    };
    match lines.vertical {
        VerticalRule::Margin(rule_color) => primitives.push(vertical(x - 0.75, rule_color)),
        VerticalRule::Grid { spacing, color } => {
            let spacing = step(spacing);
            let first = ((left - x) / spacing).floor();
            primitives.extend(
                (0..)
                    .map(|index| x + (first + index as f32) * spacing)
                    .take_while(|at| *at <= right + width)
                    .map(|at| vertical(at, color)),
            );
        }
    }
    primitives
}

fn page_primitives<'a>(
    editor: &'a CanvasEditor,
    scene: Option<&'a (PageScene, [f32; 2])>,
    preview: Option<PointerFeedback<'a>>,
    object_focus: Option<ObjectFocus>,
    paint: Paint,
) -> Result<Vec<Primitive<'a>>> {
    let mut primitives = Vec::new();
    let active = editor.active_outline().id;
    let whole = editor
        .whole()
        .filter(|_| paint.chrome && object_focus.is_none());
    let selected = |outline: &TextOutline| match whole {
        Some(Whole::Page) => !outline.title,
        Some(Whole::Outline) => outline.id == active,
        None => false,
    };
    let draw_outline = |id, offset: [f32; 2], primitives: &mut Vec<_>| {
        // An emptied page outline leaves the editor but keeps its paint slot for undo.
        let Some(outline) = editor.visible_outlines().find(|outline| outline.id == id) else {
            return Ok(());
        };
        let outline = match preview {
            Some(PointerFeedback::Resize(resized)) if resized.id == outline.id => resized,
            _ => outline,
        };
        let [x, y] = outline.origin();
        let origin = match preview {
            Some(PointerFeedback::Move(id, [dx, dy]))
                if id == outline.id || whole == Some(Whole::Page) && !outline.title =>
            {
                [x + dx, y + dy]
            }
            _ => [x, y],
        };
        // OneNote frames the title whether or not it is focused or hovered.
        if outline.title
            || selected(outline)
            || (paint.chrome && object_focus.is_none() && outline.id == active)
            || matches!(preview, Some(PointerFeedback::Hover(id) | PointerFeedback::Move(id, _)) if id == outline.id)
            || matches!(preview, Some(PointerFeedback::Resize(resized)) if resized.id == outline.id)
        {
            append_outline_chrome(
                outline,
                [origin[0] + offset[0], origin[1] + offset[1]],
                paint.pixel,
                paint.colors.paper,
                selected(outline),
                primitives,
            );
        }
        append_outline(
            (object_focus.is_none()
                && outline.id == active
                && !matches!(preview, Some(PointerFeedback::Resize(_))))
            .then_some(editor),
            selected(outline),
            outline,
            [origin[0] + offset[0], origin[1] + offset[1]],
            paint,
            primitives,
        )?;
        if let Some((scene, _)) = scene {
            let moving = match preview {
                Some(PointerFeedback::Image(id, origin, size)) => {
                    Some((id, crate::translated(image_rect(origin, size), offset)))
                }
                _ => None,
            };
            scene.append_outline_objects(
                outline.shaped(),
                [origin[0] + offset[0], origin[1] + offset[1]],
                moving,
                paint.colors.paper,
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
            paint.colors.paper,
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
            false,
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
    // OneNote shades a selected file's column and frames it with dashes.
    if let Some(ObjectFocus::File(id)) = object_focus {
        let rect = match preview {
            Some(PointerFeedback::Image(moving, origin, size)) if moving == id => {
                image_rect(origin, size)
            }
            _ => editor
                .attachment_rect(id)
                .ok_or("The selected file is missing.")?,
        };
        primitives.extend([
            Primitive::Rect {
                rect,
                color: [0.0, 0.0, 0.0, 0.08],
            },
            Primitive::RoundedRect {
                rect,
                radius: [0.0; 2],
                stroke: Some(Stroke::Dashed(paint.pixel)),
                color: [0.25, 0.45, 0.7, 1.0],
            },
        ]);
    }
    if let Some(ObjectFocus::ReadOnly(index)) = object_focus {
        let (scene, offset) = scene.unwrap();
        let [x0, y0, x1, y1] = crate::translated(
            scene.read_only(Some(editor)).nth(index).unwrap().rect(),
            *offset,
        );
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

/// The handle of a picture at `rect` under `point`, each reaching `reach` pixels beyond its
/// drawn square.
fn image_handle_at(rect: [f32; 4], pixel: f32, point: [f32; 2], reach: f32) -> Option<[i8; 2]> {
    image_handles(rect, pixel)
        .find(|(_, center)| {
            (0..2).all(|axis| (point[axis] - center[axis]).abs() <= (5.0 + reach) * pixel)
        })
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

/// How far left and up OneNote 2010 scrolls to show an outline: 7.5 pt past its text, to
/// its list markers, 0.75 pt into its tag column's first slot, and 6 pt above it.
fn reach(outline: &TextOutline) -> [f32; 2] {
    let shaped = outline.shaped();
    let column = shaped.tag_column_offset();
    let left = shaped
        .paragraphs
        .iter()
        .flat_map(|paragraph| {
            let markers = paragraph.markers.iter().map(|(_, [x, _])| *x);
            std::iter::once(paragraph.origin[0] - 7.5).chain(markers)
        })
        .chain(
            shaped
                .tags()
                .map(|(_, origin, _)| origin[0] + column + 0.75),
        )
        .fold(-7.5, f32::min);
    let [x, y] = outline.origin();
    [x + left, y - 6.0]
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
    if outline.title {
        // The title's box scales with the page, 5 px from its text at 100% (6 px more to the
        // left, 2 px less to the right).
        let inset = 3.75;
        let (_, paragraph) = outline.layouts().last().unwrap();
        let bottom = outline.origin()[1] + paragraph.origin[1] + paragraph.text.height();
        let top = bounds.y0 as f32 - inset;
        return (
            [
                bounds.x0 as f32 - inset - 4.5,
                top,
                bounds.x1 as f32 + inset - 1.5,
                bottom + inset,
            ],
            top,
        );
    }
    // Native chrome combines page-scaled gutters with a fixed screen inset.
    let inset = 5.0 * pixel;
    let body_top = bounds.y0 as f32 - inset;
    // OneNote widens the box leftward to hold the tag column and list markers; text and right
    // edge stay.
    let shaped = outline.shaped();
    let column = shaped.tag_column_offset() + crate::outline::ParagraphTag::INSET
        - crate::outline::ParagraphTag::SIZE;
    let left = shaped
        .paragraphs
        .iter()
        .flat_map(|paragraph| paragraph.markers.iter().map(|(_, [x, _])| x + 7.5))
        .chain(shaped.tags().map(|(_, origin, _)| origin[0] + column))
        .fold(0.0, f32::min);
    (
        [
            bounds.x0 as f32 + left - 7.5 - inset,
            body_top - HANDLE_HEIGHT,
            bounds.x1 as f32 + inset,
            bounds.y1 as f32 + HANDLE_HEIGHT + inset,
        ],
        body_top,
    )
}

/// `selected` greys the body, as OneNote marks an outline Select All has selected whole.
fn append_outline_chrome(
    outline: &TextOutline,
    origin: [f32; 2],
    pixel: f32,
    paper: crate::gpu::Paper,
    selected: bool,
    primitives: &mut Vec<Primitive<'_>>,
) {
    let [x, y] = origin;
    let (bounds, body_top) = outline_chrome(outline, pixel);
    let [dx, dy] = [x - outline.origin()[0], y - outline.origin()[1]];
    let [left, top, right, bottom] = crate::translated(bounds, [dx, dy]);
    let body_top = body_top + dy;
    if outline.title {
        primitives.push(Primitive::RoundedRect {
            rect: [left, top, right, bottom],
            radius: [4.5, (bottom - top) * 0.5],
            // A page pixel at 100% zoom, so it scales when zoomed in but stays a screen pixel
            // when zoomed out.
            stroke: Some(Stroke::Dashed(pixel.max(0.75))),
            color: paper.shade(crate::gpu::colorref(0x007f7f7f)),
        });
        return;
    }
    primitives.push(Primitive::RoundedRect {
        rect: [left, top, right, body_top],
        radius: [3.0 * pixel; 2],
        stroke: None,
        color: paper.shade(crate::gpu::colorref(0x00e8ebed)),
    });
    primitives.push(Primitive::RoundedRect {
        rect: [right - 9.0 * pixel, top, right, body_top],
        radius: [3.0 * pixel; 2],
        stroke: None,
        color: paper.shade(crate::gpu::colorref(0x00e5dee7)),
    });
    if selected {
        primitives.push(Primitive::RoundedRect {
            rect: [left, body_top, right, bottom],
            radius: [3.0 * pixel; 2],
            stroke: None,
            color: paper.shade(crate::gpu::colorref(0x00f0f0f0)),
        });
    }
    primitives.push(Primitive::RoundedRect {
        rect: [left, top, right, bottom],
        radius: [3.0 * pixel; 2],
        stroke: Some(Stroke::Solid(pixel)),
        color: paper.shade(crate::gpu::colorref(0x00d9cfd8)),
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
            color: paper.shade(crate::gpu::colorref(0x00b4a5b4)),
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
                color: paper.shade(crate::gpu::colorref(0x00b4a5b4)),
            });
        }
    }
}

/// `editor` paints its selection and caret in the focused outline; `selected` highlights all
/// the text of another.
fn append_outline<'a>(
    editor: Option<&'a CanvasEditor>,
    selected: bool,
    outline: &'a TextOutline,
    origin: [f32; 2],
    paint: Paint,
    primitives: &mut Vec<Primitive<'a>>,
) -> Result<()> {
    let [x, y] = origin;
    let Paint {
        caret,
        scale,
        device_origin,
        pixel,
        colors,
        visible,
        found,
        played,
        spelling,
        ..
    } = paint;
    let rows = [visible[0] - y, visible[1] - y];
    outline
        .shaped()
        .append_table_primitives(primitives, origin, colors.paper);
    outline
        .shaped()
        .append_block_tag_primitives(primitives, origin, paint.tag_art);
    outline
        .shaped()
        .append_background_primitives(primitives, origin, rows, colors.paper);
    // OneNote's search highlight: yellow, deepened to gold under light text on dark paper.
    let [red, green, blue, _] = colors.paper.color;
    let marker = if red + green + blue < 1.5 {
        draw::srgb(0x7a, 0x62, 0x00)
    } else {
        draw::srgb(0xff, 0xe6, 0x00)
    };
    for (_, selection) in found.iter().filter(|(id, _)| *id == outline.id) {
        for rect in outline.range_rects(*selection)? {
            primitives.push(Primitive::Rect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y0 as f32 + y,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                color: marker,
            });
        }
    }
    let mut highlight = match editor {
        Some(editor) => editor.selection_rects()?,
        None if selected => outline.range_rects(outline.whole())?,
        None => Vec::new(),
    };
    // OneNote highlights the note playing as it highlights a selection.
    if let Some((_, selection)) = played.filter(|(id, _)| *id == outline.id) {
        highlight.extend(outline.range_rects(*selection)?);
    }
    for rect in highlight {
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
    for (index, paragraph) in outline.shaped().visible(rows) {
        outline.shaped().append_paragraph_primitives(
            index,
            paragraph,
            origin,
            colors.paper.ink,
            paint.tag_art,
            primitives,
        );
    }
    if let Some(spelling) = spelling {
        let typing = editor
            .and_then(CanvasEditor::typing)
            .filter(|(id, _)| *id == outline.id)
            .map(|(_, at)| at);
        append_marks(
            spelling,
            outline,
            typing,
            origin,
            rows,
            [scale, device_origin[0], device_origin[1]],
            pixel,
            primitives,
        )?;
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
                color: colors.paper.ink,
            });
        }
        let [anchor, focus] = editor.selection().positions;
        if caret > 0.0 && anchor == focus {
            // AppKit centres the caret on the insertion point.
            let rect = editor.caret(0.0)?;
            let half = edit::CARET_WIDTH / 2.0 * pixel;
            let middle = rect.x0 as f32 + x;
            primitives.push(Primitive::RoundedRect {
                rect: [
                    middle - half,
                    rect.y0 as f32 + y,
                    middle + half,
                    rect.y1 as f32 + y,
                ],
                radius: [half; 2],
                stroke: None,
                color: edit::caret_color(colors.caret, colors.paper.color, caret),
            });
        }
    }
    Ok(())
}

/// OneNote's red zigzag under each marked word of the paragraphs between outline-local
/// `rows`: a pixel thick, two high and four long, 1.5 to 3.5 pixels below the baseline, laid
/// on the device grid `[scale, x, y]` so it stays hard-edged. The word ending at the caret of
/// a run of typing, `typing`, is still being written and goes unmarked.
#[allow(clippy::too_many_arguments)]
fn append_marks(
    spelling: &crate::spelling::Spelling,
    outline: &TextOutline,
    typing: Option<crate::document::TextPosition>,
    origin: [f32; 2],
    rows: [f32; 2],
    [scale, device_x, device_y]: [f32; 3],
    pixel: f32,
    primitives: &mut Vec<Primitive<'_>>,
) -> Result<()> {
    let color = draw::srgb(0xff, 0x00, 0x00);
    let dot = (pixel * scale).round().max(1.0);
    let shown = outline.layouts().filter(|(_, paragraph)| {
        paragraph.origin[1] + paragraph.text.height() >= rows[0] && paragraph.origin[1] <= rows[1]
    });
    for (index, _) in shown {
        let Some(paragraph) = outline.document().paragraph(index) else {
            continue;
        };
        for mark in spelling.marks(paragraph) {
            let start = paragraph.utf16_offset(mark.range.start)?;
            let end = paragraph.utf16_offset(mark.range.end)?;
            if typing.is_some_and(|at| at.paragraph == index && at.offset == end) {
                continue;
            }
            for [left, right, baseline] in outline.underlines(index, start..end)? {
                let right = ((origin[0] + right) * scale + device_x).round();
                let mut x = ((origin[0] + left) * scale + device_x).round();
                let top = ((origin[1] + baseline) * scale + device_y).round() + dot;
                for step in [1.0, 2.0, 1.0, 0.0].into_iter().cycle() {
                    if x >= right {
                        break;
                    }
                    let y = top + step * dot;
                    primitives.push(Primitive::Rect {
                        rect: [
                            (x - device_x) / scale,
                            (y - device_y) / scale,
                            (x + dot - device_x) / scale,
                            (y + dot - device_y) / scale,
                        ],
                        color,
                    });
                    x += dot;
                }
            }
        }
    }
    Ok(())
}

/// Marked word `mark` of paragraph `index` of `outline`, with its corrections.
fn correction(
    spelling: &crate::spelling::Spelling,
    outline: &TextOutline,
    index: usize,
    mark: &crate::spelling::Mark,
) -> Option<Correction> {
    let paragraph = outline.document().paragraph(index)?;
    let text = paragraph.text();
    let word = text[mark.range.clone()].to_owned();
    let start = if mark.repeated {
        text[..mark.range.start].trim_end().len()
    } else {
        mark.range.start
    };
    let position = |byte| {
        Some(crate::document::TextPosition {
            paragraph: index,
            offset: paragraph.utf16_offset(byte).ok()?,
        })
    };
    let suggestions = if mark.repeated {
        Vec::new()
    } else {
        let language = paragraph
            .spans()
            .iter()
            .find(|span| span.end > mark.range.start)
            .and_then(|span| span.format.language);
        let mut suggestions = spelling.suggest(&word, language);
        suggestions.truncate(5);
        suggestions
    };
    Some(Correction {
        suggestions,
        repeated: mark.repeated,
        outline: outline.id,
        range: [position(start)?, position(mark.range.end)?].into(),
        word,
    })
}
