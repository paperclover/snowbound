//! A canvas page behind a C surface for native mobile shells: notebooks and their sections
//! (`library`), one page drawn into a `CAMetalLayer`, and the active outline's text as the
//! flat UTF-16 model UIKit's `UITextInput` speaks (see `TextOutline::shown_text`). Every
//! edit a view takes is stored at once through its section.
//!
//! Lengths and positions are in the host's points; the view converts through its display
//! scale. Calls returning `bool` report whether the page or selection changed, after which
//! the host redraws and rereads `sb_view_content`.

mod library;

use library::Shared;
pub use library::{Library, Section, Share};

use canvas::{
    document::TextPosition,
    editor::{Formatting, NoteTag, Selection, TextOutline, Toggle},
    gpu::{Paper, Viewport, page::PageScene},
    date::DateField,
    interaction::{Hit, ObjectFocus, PageView, Request, Response, TextColors},
    layout::TextEngine,
};
use draw::edit::{Key, NamedKey, SelectionUnit};
use onestore::{
    ExGuid,
    op::{Edit, Op, PageOp},
    page::{Image, Page},
};
use parley::{Affinity, BoundingBox};
use std::{
    error::Error,
    ffi::{CStr, CString, c_char, c_void},
    mem::ManuallyDrop,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Wake, Waker},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Host points per document point at the zoom the host calls 100%: OneNote's 100% (96
/// pixels per inch) enlarged by 15%.
const POINT: f32 = 96.0 / 72.0 * 1.15;

/// Set off the main thread when a picture raster lands and the page should be drawn again.
#[derive(Default)]
struct Frame(AtomicBool);

impl Wake for Frame {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::Release);
    }
}

fn report<T>(result: Result<T>) -> Option<T> {
    result.map_err(|error| eprintln!("snowbound: {error}")).ok()
}

fn moved(response: Response) -> bool {
    response.changed || response.moved
}

/// What a touch lands on, so the host routes it before its gesture starts.
#[repr(u8)]
#[derive(Debug, PartialEq)]
enum Target {
    /// Empty page, a picture, a check box or the date: the canvas takes a tap.
    Page = 0,
    /// The text taking input, where the platform's text interaction works.
    ActiveText = 1,
    /// Another outline's text, which a touch focuses before the platform's text interaction
    /// takes over.
    Text = 2,
    /// The focused outline's move or width grip, or the selected picture and its handles.
    Grip = 3,
}

/// Formatting the toolbar shows and applies, by bit in `sb_view_format` and by number in
/// `sb_view_apply`: toggles, lists, indentation, then OneNote's nine default tags from 16.
const FORMATS: [Formatting; 8] = [
    Formatting::Toggle(Toggle::Bold),
    Formatting::Toggle(Toggle::Italic),
    Formatting::Toggle(Toggle::Underline),
    Formatting::Toggle(Toggle::Strikethrough),
    Formatting::Bullets,
    Formatting::Numbering,
    Formatting::Indent,
    Formatting::Outdent,
];
const TAGS: [NoteTag; 9] = [
    NoteTag::ToDo,
    NoteTag::Important,
    NoteTag::Question,
    NoteTag::RememberForLater,
    NoteTag::Definition,
    NoteTag::Highlight,
    NoteTag::Contact,
    NoteTag::Address,
    NoteTag::PhoneNumber,
];

/// The page and its text model, apart from the surface it is drawn on.
struct Canvas {
    page: PageView,
    /// The object space of the page shown, which its edits name.
    space: ExGuid,
    paper: Paper,
    /// Device pixels per host point.
    scale: f32,
    /// The page date field a tap asked to change, for the host's picker.
    asked: Option<DateField>,
}

impl Canvas {
    fn new(space: ExGuid, page: Page, pixels: [u32; 2], scale: f32) -> Result<Self> {
        let mut engine = ENGINES
            .lock()
            .ok()
            .and_then(|mut engines| engines.pop())
            .unwrap_or_default();
        let (scene, editor) = PageScene::from_page(page, &mut engine)?;
        let mut page = PageView::new(
            editor,
            engine,
            Some((scene, [0.0; 2])),
            pixels,
            scale,
            Duration::from_millis(350),
        );
        page.touch = true;
        page.host_viewport = true;
        Ok(Self {
            page,
            space,
            paper: Paper::WHITE,
            scale,
            asked: None,
        })
    }

    /// The platform's text interaction draws the caret and selection.
    fn colors(&self) -> TextColors {
        TextColors {
            caret: [0.0; 4],
            selection: [0.0; 4],
            paper: self.page_paper(),
        }
    }

    /// The paper the page lies on: the appearance's, in the page's colour.
    fn page_paper(&self) -> Paper {
        self.paper.colored(self.page.editor.page_color())
    }

    /// The desktop's dark page, or white paper.
    fn set_dark(&mut self, dark: bool) {
        self.paper = if dark {
            Paper {
                color: draw::srgb(0x1f, 0x20, 0x22),
                ink: draw::srgb(0xe6, 0xe6, 0xe6),
            }
        } else {
            Paper::WHITE
        };
    }

    /// What the page took since the last call as one edit, or none when it took nothing.
    fn edit(&mut self) -> Result<Option<Edit>> {
        let ops = self.page.editor.take_ops()?;
        if ops.is_empty() {
            return Ok(None);
        }
        Ok(Some(Edit {
            at: library::filetime(),
            ops: ops
                .into_iter()
                .map(|op| Op::Page {
                    space: self.space,
                    op,
                })
                .collect(),
        }))
    }

    fn device(&self, point: [f32; 2]) -> [f32; 2] {
        point.map(|value| value * self.scale)
    }

    /// The page content's extent in document points: the scroll limits of a view with no
    /// size at scale 1.
    fn content(&mut self) -> [f32; 4] {
        let viewport = self.page.viewport;
        self.page.viewport = Viewport {
            size: [0; 2],
            scale: 1.0,
            origin: [0.0; 2],
        };
        let scroll = self.page.scroll();
        self.page.viewport = viewport;
        [scroll.min[0], scroll.min[1], scroll.max[0], scroll.max[1]]
    }

    fn set_transform(&mut self, zoom: f32, corner: [f32; 2]) {
        self.page.viewport.scale = self.scale * POINT * zoom;
        self.page.viewport.origin = corner.map(|value| -value * self.scale * zoom);
    }

    fn outline(&self, id: ExGuid) -> Option<&TextOutline> {
        let editor = &self.page.editor;
        editor
            .visible_outlines()
            .chain(editor.caret_outline())
            .find(|outline| outline.id == id)
    }

    fn target(&self, point: [f32; 2]) -> Target {
        match self.page.hit(self.device(point)) {
            Some(Hit::Handle { .. } | Hit::Resize { .. }) => Target::Grip,
            Some(Hit::Image { id, .. })
                if self.page.object_focus() == Some(ObjectFocus::Image(id)) =>
            {
                Target::Grip
            }
            Some(Hit::Text { id, point })
                if !self
                    .outline(id)
                    .is_some_and(|outline| outline.contains_extension(point)) =>
            {
                if id == self.page.editor.active_outline().id {
                    Target::ActiveText
                } else {
                    Target::Text
                }
            }
            _ => Target::Page,
        }
    }

    /// A finger down at `point`: the start of a tap or of a grip drag.
    fn press(&mut self, point: [f32; 2]) -> Result<bool> {
        let point = self.device(point);
        let changed = moved(self.page.pointer_moved(point)?);
        let pressed = self.page.pointer_pressed(Instant::now())?;
        Ok(self.respond(pressed) || changed)
    }

    /// Whether `response` moved the page, keeping the date field it asks to change.
    fn respond(&mut self, response: Response) -> bool {
        if let Some(Request::EditDate(field)) = response.request {
            self.asked = Some(field);
        }
        moved(response)
    }

    fn drag(&mut self, point: [f32; 2]) -> Result<bool> {
        Ok(moved(self.page.pointer_moved(self.device(point))?))
    }

    fn release(&mut self) -> Result<bool> {
        let released = self.page.pointer_released()?;
        let changed = self.respond(released);
        // A finger leaves no hover behind.
        let _ = self.page.pointer_left();
        Ok(changed)
    }

    /// The outline nearest `point`, framed as its chrome is, in points at 100% zoom.
    fn block(&self, point: [f32; 2]) -> Option<[f32; 4]> {
        let [x, y] = self
            .page
            .viewport
            .document_point(self.device(point))
            .map(f64::from);
        let distance = |rect: &BoundingBox| {
            let dx = (rect.x0 - x).max(x - rect.x1).max(0.0);
            let dy = (rect.y0 - y).max(y - rect.y1).max(0.0);
            dx.hypot(dy)
        };
        let rect = self
            .page
            .editor
            .visible_outlines()
            .map(TextOutline::bounds)
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
        Some([rect.x0, rect.y0, rect.x1, rect.y1].map(|value| value as f32 * POINT))
    }

    fn active(&self) -> &TextOutline {
        self.page.editor.active_outline()
    }

    fn text(&self, range: [u32; 2]) -> String {
        let units: Vec<u16> = self.active().shown_text().encode_utf16().collect();
        let [start, end] = range.map(|offset| (offset as usize).min(units.len()));
        String::from_utf16_lossy(&units[start..end.max(start)])
    }

    fn length(&self) -> u32 {
        self.active().shown_text().encode_utf16().count() as u32
    }

    fn selection(&self) -> Result<[u32; 2]> {
        let [anchor, focus] = self.page.editor.selection().positions;
        let outline = self.active();
        Ok([
            outline.utf16_offset(anchor.min(focus))?,
            outline.utf16_offset(anchor.max(focus))?,
        ])
    }

    fn positions(&self, range: [u32; 2]) -> Result<[TextPosition; 2]> {
        let outline = self.active();
        Ok([
            outline.utf16_position(range[0])?,
            outline.utf16_position(range[1])?,
        ])
    }

    fn select(&mut self, range: [u32; 2]) -> Result<()> {
        let positions = self.positions(range)?;
        Ok(self.page.editor.select(Selection::from(positions))?)
    }

    fn marked(&self) -> Result<Option<[u32; 2]>> {
        let outline = self.active();
        self.page
            .editor
            .marked_range()
            .map(|range| {
                Ok([
                    outline.utf16_offset(range.start)?,
                    outline.utf16_offset(range.end)?,
                ])
            })
            .transpose()
    }

    /// Marked text with its selection in UTF-16 units of `text`.
    fn set_marked(&mut self, text: String, selected: [u32; 2]) -> Result<bool> {
        let byte = |utf16: u32| {
            let mut units = 0;
            text.char_indices()
                .find(|(_, char)| {
                    let found = units >= utf16;
                    units += char.len_utf16() as u32;
                    found
                })
                .map_or(text.len(), |(byte, _)| byte)
        };
        let cursor = (byte(selected[0]), byte(selected[1]));
        Ok(moved(self.page.compose(text, Some(cursor))?))
    }

    /// Types `text`; a lone newline or tab is the Return or Tab key.
    fn insert(&mut self, text: String) -> Result<bool> {
        Ok(moved(match text.as_str() {
            "\n" => self.page.key(&Key::Named(NamedKey::Enter), None)?,
            "\t" => self.page.key(&Key::Named(NamedKey::Tab), None)?,
            _ => self.page.commit_text(text)?,
        }))
    }

    /// The formatting the selection has, as `FORMATS` and `TAGS` bits.
    fn format_bits(&self) -> Result<u32> {
        let state = self.page.editor.format_state()?;
        let mut bits = 0;
        for (bit, format) in FORMATS.iter().enumerate() {
            let on = match format {
                Formatting::Toggle(toggle) => state.toggles.contains(toggle),
                Formatting::Bullets => state.bullets,
                Formatting::Numbering => state.numbering,
                _ => false,
            };
            bits |= u32::from(on) << bit;
        }
        for (bit, tag) in TAGS.iter().enumerate() {
            bits |= u32::from(state.tags.contains(tag)) << (16 + bit);
        }
        Ok(bits)
    }

    fn format(&mut self, command: u8) -> Result<bool> {
        let command = match command {
            16.. => Formatting::Tag(*TAGS.get(usize::from(command - 16)).ok_or("No such tag")?),
            _ => FORMATS
                .get(usize::from(command))
                .ok_or("No such formatting")?
                .clone(),
        };
        Ok(moved(self.page.format(command)?))
    }

    /// The title outline, where the page has an editable one.
    fn title(&self) -> Option<&TextOutline> {
        self.page
            .editor
            .outlines()
            .iter()
            .find(|outline| outline.title)
    }

    /// Puts the caret at the end of the page title.
    fn focus_title(&mut self) -> Result<bool> {
        let Some(id) = self.title().map(|title| title.id) else {
            return Ok(false);
        };
        self.page.editor.focus_outline(id)?;
        let end = self.length();
        self.select([end, end])?;
        Ok(true)
    }

    /// Selects the first place `query` occurs on the page, ignoring case.
    fn find(&mut self, query: &str) -> Result<bool> {
        let lower: Vec<u16> = query.to_lowercase().encode_utf16().collect();
        if lower.is_empty() {
            return Ok(false);
        }
        let found = self.page.editor.outlines().iter().find_map(|outline| {
            let text: Vec<u16> = outline.shown_text().to_lowercase().encode_utf16().collect();
            let at = text
                .windows(lower.len())
                .position(|window| window == lower)?;
            // Lowercasing can change the length; the match is then only found in place.
            (text.len() == outline.shown_text().encode_utf16().count())
                .then_some((outline.id, at as u32))
        });
        let Some((id, at)) = found else {
            return Ok(false);
        };
        self.page.editor.focus_outline(id)?;
        self.select([at, at + lower.len() as u32])?;
        Ok(true)
    }

    /// The page's text, outline by outline, as the desktop puts a refused edit's on the
    /// clipboard.
    fn page_text(&self) -> String {
        let editor = &self.page.editor;
        editor
            .visible_outlines()
            .chain(editor.caret_outline())
            .map(|outline| {
                outline
                    .document()
                    .paragraphs()
                    .map(library::shown)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Puts a picture of `size` points after the caret's paragraph.
    fn insert_picture(&mut self, bytes: &[u8], size: [f32; 2]) -> Result<bool> {
        let image = Image {
            id: onestore::page::text::new_id()?,
            layout: onestore::document::Layout {
                max_width: Some(size[0]),
                max_height: Some(size[1]),
                ..Default::default()
            },
            size: Some(size),
            bytes: Some(bytes.into()),
            alt: None,
            background: false,
        };
        let page = &mut self.page;
        page.editor.insert_picture(&mut page.engine, image)?;
        Ok(true)
    }

    /// Gives the page date `seconds` since 1970 (keeping the stored fraction of a second),
    /// titled `text`: the long date and the short time as the host formats them.
    fn change_date(&mut self, seconds: i64, text: [String; 2]) -> Result<bool> {
        let date = self.page.editor.date().ok_or("The page has no date")?;
        let timestamp = u64::try_from(seconds + 11_644_473_600)?
            .checked_mul(10_000_000)
            .and_then(|ticks| ticks.checked_add(date.timestamp() % 10_000_000))
            .ok_or("The date is out of range")?;
        Ok(moved(self.page.change_date(timestamp, text)?))
    }

    /// A rectangle in the active outline's coordinates, in the view's points as `[x, y,
    /// width, height]`.
    fn view_rect(&self, rect: BoundingBox) -> [f32; 4] {
        let viewport = self.page.viewport;
        let origin = self.active().origin();
        let [x, y] = [rect.x0 as f32 + origin[0], rect.y0 as f32 + origin[1]];
        [
            (x * viewport.scale + viewport.origin[0]) / self.scale,
            (y * viewport.scale + viewport.origin[1]) / self.scale,
            rect.width() as f32 * viewport.scale / self.scale,
            rect.height() as f32 * viewport.scale / self.scale,
        ]
    }

    fn caret_rect(&self, offset: u32) -> Result<[f32; 4]> {
        let position = self.active().utf16_position(offset)?;
        let caret = self
            .page
            .editor
            .caret_at(position, Affinity::Downstream, 1.0)?;
        Ok(self.view_rect(caret))
    }

    /// The selection rectangles of a range, one or more per line.
    fn range_rects(&self, range: [u32; 2]) -> Result<Vec<[f32; 4]>> {
        let rects = self.page.editor.range_rects(Selection {
            positions: self.positions(range)?,
            affinities: [Affinity::Downstream, Affinity::Upstream],
        })?;
        Ok(rects.into_iter().map(|rect| self.view_rect(rect)).collect())
    }

    fn closest(&self, point: [f32; 2]) -> Result<u32> {
        let point = self.page.viewport.document_point(self.device(point));
        let origin = self.active().origin();
        let selection = self.page.editor.selection_at(
            point[0] - origin[0],
            point[1] - origin[1],
            SelectionUnit::Grapheme,
        )?;
        Ok(self.active().utf16_offset(selection.positions[1])?)
    }
}

/// The GPU every page view draws with, and the renderer whose glyph atlas they share.
struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    renderer: draw::Renderer,
}

/// Made with the first view's surface, whose adapter every later layer can use.
static GPU: Mutex<Option<Gpu>> = Mutex::new(None);

fn gpu() -> std::sync::MutexGuard<'static, Option<Gpu>> {
    GPU.lock().unwrap_or_else(|error| error.into_inner())
}

/// Text engines of closed views, whose fonts and shaping caches the next view takes over
/// rather than enumerating the system's fonts again.
static ENGINES: Mutex<Vec<TextEngine>> = Mutex::new(Vec::new());

pub struct View {
    /// Taken apart on drop, which returns its text engine to `ENGINES`.
    canvas: ManuallyDrop<Canvas>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    frame: Arc<Frame>,
    section: Arc<Shared>,
    /// A conflict page, which shows what is stored and takes no edits.
    read_only: bool,
}

impl View {
    fn new(
        layer: *mut c_void,
        section: &Section,
        space: ExGuid,
        size: [f32; 2],
        scale: f32,
    ) -> Result<Self> {
        let pixels = size.map(|side| (side * scale).round().max(1.0) as u32);
        let mut gpu = gpu();
        let instance = match &*gpu {
            Some(gpu) => gpu.instance.clone(),
            None => wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()),
        };
        // SAFETY: the host passes a live CAMetalLayer that outlives the view.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }?;
        if gpu.is_none() {
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&surface),
                    ..Default::default()
                }))?;
            // The simulator's adapter falls short of wgpu's default limits.
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                }))?;
            let format = surface
                .get_default_config(&adapter, pixels[0], pixels[1])
                .ok_or("No supported surface configuration")?
                .format;
            *gpu = Some(Gpu {
                instance,
                renderer: draw::Renderer::new(device, queue, format),
                adapter,
            });
        }
        let gpu = gpu.as_ref().ok_or("No GPU")?;
        let config = surface
            .get_default_config(&gpu.adapter, pixels[0], pixels[1])
            .ok_or("No supported surface configuration")?;
        surface.configure(&gpu.renderer.device, &config);
        let (page, read_only) = section.shared.page(space)?;
        Ok(Self {
            canvas: ManuallyDrop::new(Canvas::new(space, page, pixels, scale)?),
            surface,
            config,
            frame: Arc::default(),
            section: Arc::clone(&section.shared),
            read_only,
        })
    }

    fn render(&mut self) -> Result<()> {
        let mut gpu = gpu();
        let renderer = &mut gpu.as_mut().ok_or("No GPU")?.renderer;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&renderer.device, &self.config);
                return Ok(());
            }
            _ => return Ok(()),
        };
        let canvas: &mut Canvas = &mut self.canvas;
        let paper = canvas.page_paper();
        canvas
            .page
            .update_pictures(paper, &Waker::from(self.frame.clone()));
        let primitives = canvas.page.primitives(canvas.colors())?;
        renderer
            .draw(
                &frame.texture.create_view(&Default::default()),
                [self.config.width, self.config.height],
                paper.color,
                &[canvas.page.viewport.layer(&primitives)],
            )
            .map_err(|error| format!("Page drawing failed: {error:?}"))?;
        renderer.queue.present(frame);
        Ok(())
    }

    /// Hands the section what the page took, after `result`, the change that took it. A
    /// conflict page returns to what is stored instead.
    fn stored(&mut self, result: Result<bool>) -> bool {
        let changed = report(result).unwrap_or(false);
        match report(self.canvas.edit()) {
            Some(Some(_)) if self.read_only => {
                let _ = report(self.reload());
            }
            Some(Some(edit)) => {
                // Pictures that came or went, by insertion or undo, are drawn or dropped.
                if edit.ops.iter().any(|op| {
                    matches!(
                        op,
                        Op::Page {
                            op: PageOp::Insert { .. } | PageOp::Add { .. } | PageOp::Delete { .. },
                            ..
                        }
                    )
                }) {
                    let page = &mut self.canvas.page;
                    if let Some((scene, _)) = &mut page.scene {
                        let refreshed = scene.refresh(&mut page.editor, &mut page.engine);
                        let _ = report(refreshed.map_err(Into::into));
                    }
                }
                let _ = report(self.section.apply(edit));
            }
            _ => {}
        }
        changed
    }

    /// Shows the page as stored, in place: the caret, selection and drawn pictures stay.
    /// Waits for marked text to end.
    fn reload(&mut self) -> Result<bool> {
        if self.canvas.page.editor.marked_range().is_some() {
            return Ok(false);
        }
        let (page, read_only) = self.section.page(self.canvas.space)?;
        self.read_only = read_only;
        Ok(moved(self.canvas.page.refresh(page)?))
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: `canvas` is not used again.
        let canvas = unsafe { ManuallyDrop::take(&mut self.canvas) };
        if let Ok(mut engines) = ENGINES.lock() {
            engines.push(canvas.page.engine);
        }
    }
}

fn string(text: *const c_char) -> String {
    // SAFETY: every caller's contract makes `text` NUL-terminated.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

fn owned(text: String) -> *mut c_char {
    CString::new(text.replace('\0', "")).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// A view drawing page `id` of `section` into `layer`, a `CAMetalLayer`, `size` points at
/// `scale` pixels per point; null if it cannot.
///
/// # Safety
/// `layer` is a live `CAMetalLayer` that outlives the view; `id` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_new(
    layer: *mut c_void,
    section: &Section,
    id: *const c_char,
    width: f32,
    height: f32,
    scale: f32,
) -> *mut View {
    report(
        string(id)
            .parse()
            .map_err(Into::into)
            .and_then(|space| View::new(layer, section, space, [width, height], scale)),
    )
    .map_or(std::ptr::null_mut(), |view| Box::into_raw(Box::new(view)))
}

/// # Safety
/// `view` came from `sb_view_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_free(view: *mut View) {
    drop(unsafe { Box::from_raw(view) });
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_view_resize(view: &mut View, width: f32, height: f32, scale: f32) {
    let pixels = [width, height].map(|side| (side * scale).round().max(1.0) as u32);
    let canvas = &mut view.canvas;
    if scale != canvas.scale {
        canvas.scale = scale;
        let _ = report(canvas.page.scale_factor_changed(scale));
    }
    let _ = report(canvas.page.resized(pixels));
    view.config.width = pixels[0];
    view.config.height = pixels[1];
    if let Some(gpu) = &*gpu() {
        view.surface.configure(&gpu.renderer.device, &view.config);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_view_render(view: &mut View) -> bool {
    report(view.render()).is_some()
}

/// Whether a picture landed since the last call, so the page should be drawn again.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_frame_pending(view: &View) -> bool {
    view.frame.0.swap(false, Ordering::Acquire)
}

/// Draws the page on the desktop's dark paper, or on white.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_set_dark(view: &mut View, dark: bool) {
    view.canvas.set_dark(dark);
}

/// A conflict page: it shows what is stored, and any edit returns it there.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_read_only(view: &View) -> bool {
    view.read_only
}

/// Shows the page as stored after a change made elsewhere, keeping the caret; with
/// `discard`, after the section refused an edit, drops what the page took since.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_reload(view: &mut View, discard: bool) -> bool {
    if discard {
        let _ = view.canvas.page.editor.take_ops();
    }
    report(view.reload()).unwrap_or(false)
}

/// Whether the view has input focus: the focused outline shows its frame and grips only then.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_focus(view: &mut View, focused: bool) {
    let result = view.canvas.page.focus_changed(focused).map(moved);
    view.stored(result);
}

/// The page content's `[left, top, right, bottom]` in points at 100% zoom, bottom and right
/// including the margin OneNote scrolls past the last object.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_content(view: &mut View, bounds: &mut [f32; 4]) {
    *bounds = view.canvas.content().map(|value| value * POINT);
}

/// The frame of the outline nearest the view point `x`, `y`, as `[left, top, right,
/// bottom]` in points at 100% zoom; false on a page with none.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_block(view: &View, x: f32, y: f32, rect: &mut [f32; 4]) -> bool {
    view.canvas
        .block([x, y])
        .map(|block| *rect = block)
        .is_some()
}

/// Zooms and scrolls so the page point `x`, `y` sits at the view's corner; the host's
/// scroll view owns the limits and rubber-banding.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_set_transform(view: &mut View, zoom: f32, x: f32, y: f32) {
    view.canvas.set_transform(zoom, [x, y]);
}

/// What the view point `x`, `y` lands on: 0 for the page, where the canvas takes a tap; 1
/// for the text taking input; 2 for another outline's text; 3 for the focused outline's
/// move or width grip, or the selected picture and its handles.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_target(view: &View, x: f32, y: f32) -> u8 {
    view.canvas.target([x, y]) as u8
}

/// A finger down: with `sb_view_release` a tap, which places the caret, focuses an outline
/// or starts a new one; with `sb_view_drag` between, a grip moves or widens an outline.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_press(view: &mut View, x: f32, y: f32) -> bool {
    let result = view.canvas.press([x, y]);
    view.stored(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_view_drag(view: &mut View, x: f32, y: f32) -> bool {
    report(view.canvas.drag([x, y])).unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_view_release(view: &mut View) -> bool {
    let result = view.canvas.release();
    view.stored(result)
}

/// Undoes the last edit, or with `redo` redoes the last undone one.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_undo(view: &mut View, redo: bool) -> bool {
    let result = view.canvas.page.undo(redo).map(moved);
    view.stored(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_view_can_undo(view: &View, redo: bool) -> bool {
    let editor = &view.canvas.page.editor;
    if redo {
        editor.can_redo()
    } else {
        editor.can_undo()
    }
}

/// The selection's formatting as bits: 0 bold, 1 italic, 2 underline, 3 strikethrough, 4
/// bulleted, 5 numbered, and from 16 OneNote's default tags in order, To Do first.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_format(view: &View) -> u32 {
    report(view.canvas.format_bits()).unwrap_or(0)
}

/// Applies formatting to the selection: 0 bold, 1 italic, 2 underline, 3 strikethrough, 4
/// bullets, 5 numbering, 6 indent, 7 outdent, each toggling where it can; from 16 toggles
/// OneNote's default tag `command - 16`, To Do first.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_apply(view: &mut View, command: u8) -> bool {
    let result = view.canvas.format(command);
    view.stored(result)
}

/// The page title's text; null on a page without an editable title.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_title(view: &View) -> *mut c_char {
    view.canvas
        .title()
        .map_or(std::ptr::null_mut(), |title| owned(title.shown_text()))
}

/// Puts the caret at the end of the page title; false on a page without one.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_focus_title(view: &mut View) -> bool {
    report(view.canvas.focus_title()).unwrap_or(false)
}

/// Selects the first place `query` occurs on the page, ignoring case.
///
/// # Safety
/// `query` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_find(view: &mut View, query: *const c_char) -> bool {
    report(view.canvas.find(&string(query))).unwrap_or(false)
}

/// The page's text, for the clipboard when an edit is refused; freed with `sb_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_page_text(view: &View) -> *mut c_char {
    owned(view.canvas.page_text())
}

/// Puts the picture file `bytes` (JPEG or PNG), `width` × `height` points, after the
/// caret's paragraph.
///
/// # Safety
/// `bytes` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_insert_picture(
    view: &mut View,
    bytes: *const u8,
    length: usize,
    width: f32,
    height: f32,
) -> bool {
    // SAFETY: the caller's contract.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    let result = view.canvas.insert_picture(bytes, [width, height]);
    view.stored(result)
}

/// The page date field the last tap asked to change, taken: 0 the date, 1 the time, or -1
/// for none; `seconds` gets the page's date in seconds since 1970.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_date_request(view: &mut View, seconds: &mut i64) -> i8 {
    let Some(field) = view.canvas.asked.take() else {
        return -1;
    };
    let Some(date) = view.canvas.page.editor.date() else {
        return -1;
    };
    *seconds = (date.timestamp() / 10_000_000) as i64 - 11_644_473_600;
    field as i8
}

/// Gives the page date `seconds` since 1970, shown as `date` and `time`: the long date and
/// the short time, as OneNote titles a page.
///
/// # Safety
/// `date` and `time` are NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_change_date(
    view: &mut View,
    seconds: i64,
    date: *const c_char,
    time: *const c_char,
) -> bool {
    let result = view.canvas.change_date(seconds, [string(date), string(time)]);
    view.stored(result)
}

/// # Safety
/// `text` came from this library and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_string_free(text: *mut c_char) {
    drop(unsafe { CString::from_raw(text) });
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_text_length(view: &View) -> u32 {
    view.canvas.length()
}

/// The text in `start..end`, freed with `sb_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn sb_text(view: &View, start: u32, end: u32) -> *mut c_char {
    owned(view.canvas.text([start, end]))
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_selection(view: &View, range: &mut [u32; 2]) {
    if let Some(selection) = report(view.canvas.selection()) {
        *range = selection;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_select(view: &mut View, start: u32, end: u32) -> bool {
    report(view.canvas.select([start, end])).is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_marked(view: &View, range: &mut [u32; 2]) -> bool {
    report(view.canvas.marked())
        .flatten()
        .map(|marked| *range = marked)
        .is_some()
}

/// # Safety
/// `text` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_set_marked(
    view: &mut View,
    text: *const c_char,
    selected_start: u32,
    selected_end: u32,
) -> bool {
    let result = view
        .canvas
        .set_marked(string(text), [selected_start, selected_end]);
    view.stored(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_unmark(view: &mut View) {
    view.canvas.page.editor.finish_composition();
    view.stored(Ok(false));
}

/// Types `text`, a lone `\n` as the Return key and a lone `\t` as the Tab key.
///
/// # Safety
/// `text` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_insert(view: &mut View, text: *const c_char) -> bool {
    let result = view.canvas.insert(string(text));
    view.stored(result)
}

/// Replaces `start..end` with `text`, as autocorrection, dictation and cutting do.
///
/// # Safety
/// `text` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_replace(
    view: &mut View,
    start: u32,
    end: u32,
    text: *const c_char,
) -> bool {
    let canvas = &mut view.canvas;
    let result = canvas
        .select([start, end])
        .and_then(|()| Ok(moved(canvas.page.commit_text(string(text))?)));
    view.stored(result)
}

/// Pastes clipboard `text` in `language`, the keyboard's BCP-47 tag.
///
/// # Safety
/// `text` and `language` are NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_paste(
    view: &mut View,
    text: *const c_char,
    language: *const c_char,
) -> bool {
    let (text, language) = (string(text), canvas::language::lcid(&string(language)));
    let result = view.canvas.page.paste(&text, language).map(moved);
    view.stored(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_delete_backward(view: &mut View) -> bool {
    let result = view
        .canvas
        .page
        .key(&Key::Named(NamedKey::Backspace), None)
        .map(moved);
    view.stored(result)
}

/// The caret at `offset` as `[x, y, width, height]` in the view's points.
#[unsafe(no_mangle)]
pub extern "C" fn sb_caret_rect(view: &View, offset: u32, rect: &mut [f32; 4]) -> bool {
    report(view.canvas.caret_rect(offset))
        .map(|caret| *rect = caret)
        .is_some()
}

/// Writes up to `capacity` rectangles covering `start..end`; returns how many there are.
///
/// # Safety
/// `rects` has room for `capacity` rectangles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_range_rects(
    view: &View,
    start: u32,
    end: u32,
    rects: *mut [f32; 4],
    capacity: usize,
) -> usize {
    let found = report(view.canvas.range_rects([start, end])).unwrap_or_default();
    for (index, rect) in found.iter().take(capacity).enumerate() {
        unsafe { rects.add(index).write(*rect) };
    }
    found.len()
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_closest(view: &View, x: f32, y: f32) -> u32 {
    report(view.canvas.closest([x, y])).unwrap_or(0)
}

#[cfg(test)]
mod tests;
