//! A canvas page behind a C surface for native mobile shells: a notebook's sections, a
//! section's pages, one page drawn into a `CAMetalLayer`, and the active outline's text as
//! the flat UTF-16 model UIKit's `UITextInput` speaks (see `TextOutline::shown_text`).
//!
//! Lengths and positions are in the host's points; the view converts through its display
//! scale. Calls returning `bool` report whether the page or selection changed, after which
//! the host redraws and rereads `sb_view_content`.

use canvas::{
    document::TextPosition,
    editor::{Formatting, Selection, TextOutline, Toggle},
    gpu::{Paper, Viewport, page::PageScene},
    interaction::{Hit, PageView, Response, TextColors},
    layout::TextEngine,
};
use draw::edit::{Key, NamedKey, SelectionUnit};
use notebook::discover::{self, Folder, SectionState};
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::Document,
    op::{Edit, Op},
    page::Page,
};
use parley::{Affinity, BoundingBox};
use std::{
    error::Error,
    ffi::{CStr, CString, c_char, c_void},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Wake, Waker},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Host points per document point at 100% zoom, as OneNote's 96 pixels per inch.
const POINT: f32 = 96.0 / 72.0;

/// The tab colour OneNote gives a section that stores none, as the desktop shows it.
const SECTION_COLOR: u32 = 0x00e4_a88a;

#[derive(serde::Serialize)]
struct Listing {
    name: String,
    sections: Vec<Tab>,
}

#[derive(serde::Serialize)]
struct Tab {
    name: String,
    /// The section file's absolute path, for `sb_section_open`.
    path: String,
    /// The section group holding it, `/`-separated; empty at the notebook's top.
    group: String,
    /// The tab colour in sRGB.
    color: [u8; 3],
    /// Password-protected or unreadable sections list but do not open.
    readable: bool,
}

/// A notebook folder's sections in its order, or a lone section file as a notebook of one.
fn listing(path: &Path) -> Result<Listing> {
    let stem = |path: &Path| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    if path.is_file() {
        return Ok(Listing {
            name: stem(path),
            sections: vec![Tab {
                name: stem(path),
                path: path.to_string_lossy().into_owned(),
                group: String::new(),
                color: rgb(SECTION_COLOR),
                readable: true,
            }],
        });
    }
    let folder = discover::discover(
        &mut discover::Local::open(path)?,
        discover::Limits {
            entries: 10_000,
            bytes_per_file: 1 << 30,
            depth: 16,
        },
    )?;
    let mut sections = Vec::new();
    fn walk(folder: &Folder, root: &Path, sections: &mut Vec<Tab>) {
        for section in &folder.sections {
            let file = root.join(&section.path);
            let (name, color, readable) = match &section.state {
                SectionState::Readable { name, color } => (name.clone(), *color, true),
                SectionState::Locked | SectionState::Unreadable(_) => (None, None, false),
            };
            sections.push(Tab {
                name: name.unwrap_or_else(|| {
                    file.file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
                path: file.to_string_lossy().into_owned(),
                group: folder.path.clone(),
                color: rgb(color.unwrap_or(SECTION_COLOR)),
                readable,
            });
        }
        for group in &folder.groups {
            // OneNote keeps deleted sections and pages here, out of the notebook's view.
            if !group.path.ends_with("OneNote_RecycleBin") {
                walk(group, root, sections);
            }
        }
    }
    walk(&folder, path, &mut sections);
    Ok(Listing {
        name: stem(path),
        sections,
    })
}

fn rgb(colorref: u32) -> [u8; 3] {
    let [red, green, blue, _] = colorref.to_le_bytes();
    [red, green, blue]
}

pub struct Section {
    /// Each page with the object space holding it.
    pages: Vec<(ExGuid, Page)>,
    /// The page list's title and outline level (1 at the top) for each page.
    headings: Vec<(CString, u32)>,
}

impl Section {
    fn open(bytes: &[u8]) -> Result<Self> {
        let store = Store::parse(bytes)?;
        let index = RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        let mut pages = Vec::new();
        let mut headings = Vec::new();
        for (space, id) in document.pages()? {
            let revision = document.active(space)?;
            let (title, level) = Page::heading(revision, id);
            pages.push((space, Page::from_revision(revision, id)?));
            // Titles keep OneNote's line breaks, which a one-line list shows as spaces.
            let title = title.replace(|char: char| char.is_control(), " ");
            headings.push((CString::new(title)?, level));
        }
        Ok(Self { pages, headings })
    }
}

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
    /// The focused outline's move or width grip.
    Grip = 3,
}

/// The page and its text model, apart from the surface it is drawn on.
struct Canvas {
    page: PageView,
    /// The object space of the page shown, which its edits name.
    space: ExGuid,
    paper: Paper,
}

impl Canvas {
    fn new((space, page): (ExGuid, Page), pixels: [u32; 2], scale: f32) -> Result<Self> {
        let mut engine = TextEngine::default();
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
        Ok(Self {
            page,
            space,
            paper: Paper::WHITE,
        })
    }

    /// The platform's text interaction draws the caret and selection.
    fn colors(&self) -> TextColors {
        TextColors {
            caret: [0.0; 4],
            selection: [0.0; 4],
            paper: self.paper,
        }
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
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
        Ok(Some(Edit {
            // FILETIME: 100 ns ticks since 1601.
            at: (unix.as_secs() + 11_644_473_600) * 10_000_000
                + u64::from(unix.subsec_nanos() / 100),
            ops: ops
                .into_iter()
                .map(|op| Op::Page {
                    space: self.space,
                    op,
                })
                .collect(),
        }))
    }

    /// Device pixels per host point.
    fn display_scale(&self) -> f32 {
        self.page.viewport.scale / (self.page.zoom() * POINT)
    }

    fn device(&self, point: [f32; 2]) -> [f32; 2] {
        point.map(|value| value * self.display_scale())
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
        let scale = self.display_scale();
        self.page.viewport.scale = scale * POINT * zoom;
        self.page.viewport.origin = corner.map(|value| -value * scale * zoom);
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

    /// Runs `change` on the page, then puts the view back where the host's scroll view has
    /// it: the canvas clamps and reveals knowing neither the navigation bar nor the keyboard,
    /// and the host reveals the caret itself.
    fn hold<T>(&mut self, change: impl FnOnce(&mut PageView) -> Result<T>) -> Result<T> {
        let Viewport { scale, origin, .. } = self.page.viewport;
        let result = change(&mut self.page);
        self.page.viewport.scale = scale;
        self.page.viewport.origin = origin;
        result
    }

    /// A finger down at `point`: the start of a tap or of a grip drag.
    fn press(&mut self, point: [f32; 2]) -> Result<bool> {
        let point = self.device(point);
        self.hold(|page| {
            let changed = moved(page.pointer_moved(point)?);
            Ok(moved(page.pointer_pressed(Instant::now())?) || changed)
        })
    }

    fn drag(&mut self, point: [f32; 2]) -> Result<bool> {
        let point = self.device(point);
        self.hold(|page| Ok(moved(page.pointer_moved(point)?)))
    }

    fn release(&mut self) -> Result<bool> {
        self.hold(|page| {
            let changed = moved(page.pointer_released()?);
            // A finger leaves no hover behind.
            let _ = page.pointer_left();
            Ok(changed)
        })
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
        self.hold(|page| Ok(moved(page.compose(text, Some(cursor))?)))
    }

    fn insert(&mut self, text: String) -> Result<bool> {
        self.hold(|page| {
            Ok(moved(if text == "\n" {
                page.key(&Key::Named(NamedKey::Enter), None)?
            } else {
                page.commit_text(text)?
            }))
        })
    }

    /// A rectangle in the active outline's coordinates, in the view's points as `[x, y,
    /// width, height]`.
    fn view_rect(&self, rect: BoundingBox) -> [f32; 4] {
        let viewport = self.page.viewport;
        let points = self.display_scale();
        let origin = self.active().origin();
        let [x, y] = [rect.x0 as f32 + origin[0], rect.y0 as f32 + origin[1]];
        [
            (x * viewport.scale + viewport.origin[0]) / points,
            (y * viewport.scale + viewport.origin[1]) / points,
            rect.width() as f32 * viewport.scale / points,
            rect.height() as f32 * viewport.scale / points,
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

pub struct View {
    canvas: Canvas,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: draw::Renderer,
    frame: Arc<Frame>,
}

impl View {
    fn new(layer: *mut c_void, page: (ExGuid, Page), size: [f32; 2], scale: f32) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        // SAFETY: the host passes a live CAMetalLayer that outlives the view.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        // The simulator's adapter falls short of wgpu's default limits.
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            }))?;
        let pixels = size.map(|side| (side * scale).round().max(1.0) as u32);
        let config = surface
            .get_default_config(&adapter, pixels[0], pixels[1])
            .ok_or("No supported surface configuration")?;
        surface.configure(&device, &config);
        let renderer = draw::Renderer::new(device, queue, config.format);
        Ok(Self {
            canvas: Canvas::new(page, pixels, scale)?,
            surface,
            config,
            renderer,
            frame: Arc::default(),
        })
    }

    fn render(&mut self) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.renderer.device, &self.config);
                return Ok(());
            }
            _ => return Ok(()),
        };
        let canvas = &mut self.canvas;
        canvas
            .page
            .update_pictures(canvas.paper, &Waker::from(self.frame.clone()));
        let primitives = canvas.page.primitives(canvas.colors())?;
        self.renderer
            .draw(
                &frame.texture.create_view(&Default::default()),
                [self.config.width, self.config.height],
                canvas.paper.color,
                &[canvas.page.viewport.layer(&primitives)],
            )
            .map_err(|error| format!("Page drawing failed: {error:?}"))?;
        self.renderer.queue.present(frame);
        Ok(())
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

/// The notebook folder at `path`, or the lone section file there, as JSON: `name`, and
/// `sections` in the notebook's order, each with `name`, `path`, `group`, `color` as sRGB
/// bytes and `readable`. Freed with `sb_string_free`; null if it cannot be read.
///
/// # Safety
/// `path` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_notebook(path: *const c_char) -> *mut c_char {
    report(
        listing(Path::new(&string(path))).and_then(|listing| Ok(serde_json::to_string(&listing)?)),
    )
    .map_or(std::ptr::null_mut(), owned)
}

/// # Safety
/// `path` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_open(path: *const c_char) -> *mut Section {
    report(
        std::fs::read(string(path))
            .map_err(Into::into)
            .and_then(|bytes| Section::open(&bytes)),
    )
    .map_or(std::ptr::null_mut(), |section| {
        Box::into_raw(Box::new(section))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_section_count(section: &Section) -> usize {
    section.pages.len()
}

/// The page's title, alive as long as the section.
#[unsafe(no_mangle)]
pub extern "C" fn sb_section_title(section: &Section, index: usize) -> *const c_char {
    section.headings[index].0.as_ptr()
}

/// The page's level in the page list: 1 at the top, 2 for a subpage and so on.
#[unsafe(no_mangle)]
pub extern "C" fn sb_section_level(section: &Section, index: usize) -> u32 {
    section.headings[index].1
}

/// # Safety
/// `section` came from `sb_section_open` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_free(section: *mut Section) {
    drop(unsafe { Box::from_raw(section) });
}

/// A view drawing page `index` of `section` into `layer`, a `CAMetalLayer`, `size` points
/// at `scale` pixels per point; null if it cannot.
///
/// # Safety
/// `layer` is a live `CAMetalLayer` that outlives the view.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_new(
    layer: *mut c_void,
    section: &Section,
    index: usize,
    width: f32,
    height: f32,
    scale: f32,
) -> *mut View {
    report(View::new(
        layer,
        section.pages[index].clone(),
        [width, height],
        scale,
    ))
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
    if scale != view.canvas.display_scale() {
        let _ = report(view.canvas.page.scale_factor_changed(scale));
    }
    let page = &mut view.canvas.page;
    let _ = report(page.resized(pixels));
    view.config.width = pixels[0];
    view.config.height = pixels[1];
    view.surface.configure(&view.renderer.device, &view.config);
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

/// Whether the view has input focus: the focused outline shows its frame and grips only then.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_focus(view: &mut View, focused: bool) {
    let _ = report(view.canvas.hold(|page| page.focus_changed(focused)));
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
/// move or width grip.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_target(view: &View, x: f32, y: f32) -> u8 {
    view.canvas.target([x, y]) as u8
}

/// A finger down: with `sb_view_release` a tap, which places the caret, focuses an outline
/// or starts a new one; with `sb_view_drag` between, a grip moves or widens an outline.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_press(view: &mut View, x: f32, y: f32) -> bool {
    report(view.canvas.press([x, y])).unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_view_drag(view: &mut View, x: f32, y: f32) -> bool {
    report(view.canvas.drag([x, y])).unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_view_release(view: &mut View) -> bool {
    report(view.canvas.release()).unwrap_or(false)
}

/// Undoes the last edit, or with `redo` redoes the last undone one.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_undo(view: &mut View, redo: bool) -> bool {
    report(view.canvas.hold(|page| page.undo(redo).map(moved))).unwrap_or(false)
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

/// Toggles 0 bold, 1 italic or 2 underline on the selection.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_toggle(view: &mut View, toggle: u8) -> bool {
    let toggle = match toggle {
        0 => Toggle::Bold,
        1 => Toggle::Italic,
        _ => Toggle::Underline,
    };
    report(
        view.canvas
            .hold(|page| page.format(Formatting::Toggle(toggle)).map(moved)),
    )
    .unwrap_or(false)
}

/// The edit the page took since the last call, as `onestore::op::Edit` JSON for the
/// section to apply, freed with `sb_string_free`; null when it took none or cannot be
/// stored, in which case the page should be opened again.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_edit(view: &mut View) -> *mut c_char {
    report(view.canvas.edit())
        .flatten()
        .and_then(|edit| serde_json::to_string(&edit).ok())
        .map_or(std::ptr::null_mut(), owned)
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
    report(
        view.canvas
            .set_marked(string(text), [selected_start, selected_end]),
    )
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_unmark(view: &mut View) {
    view.canvas.page.editor.finish_composition();
}

/// Types `text`, a lone `\n` as the Return key.
///
/// # Safety
/// `text` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_insert(view: &mut View, text: *const c_char) -> bool {
    report(view.canvas.insert(string(text))).unwrap_or(false)
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
    report(
        canvas
            .select([start, end])
            .and_then(|()| canvas.hold(|page| Ok(moved(page.commit_text(string(text))?)))),
    )
    .unwrap_or(false)
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
    report(
        view.canvas
            .hold(|page| page.paste(&text, language).map(moved)),
    )
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_delete_backward(view: &mut View) -> bool {
    report(
        view.canvas
            .hold(|page| page.key(&Key::Named(NamedKey::Backspace), None).map(moved)),
    )
    .unwrap_or(false)
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
