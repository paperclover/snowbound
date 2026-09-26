//! A canvas page behind a C surface for native mobile shells: a section's pages, one drawn
//! into a `CAMetalLayer`, and the active outline's text as flat UTF-16 offsets, paragraphs
//! joined by `\n`, the model UIKit's `UITextInput` speaks.
//!
//! Lengths and positions are in the host's points; the view converts through its display
//! scale. Calls returning `bool` report whether the page or selection changed, after which
//! the host redraws and rereads `sb_view_content`.

use canvas::{
    document::TextPosition,
    editor::Selection,
    gpu::{Paper, Viewport, page::PageScene},
    interaction::{Hit, PageView, Response, TextColors, page_hit_test},
    layout::TextEngine,
};
use draw::edit::{Key, NamedKey, SelectionUnit};
use onestore::{RevisionIndex, Store, document::Document, page::Page};
use parley::{Affinity, BoundingBox};
use std::{
    error::Error,
    ffi::{CStr, CString, c_char, c_void},
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

/// The platform's text interaction draws the caret and selection.
const COLORS: TextColors = TextColors {
    caret: [0.0; 4],
    selection: [0.0; 4],
    paper: Paper::WHITE,
};

pub struct Section {
    pages: Vec<Page>,
    titles: Vec<CString>,
}

impl Section {
    fn open(bytes: &[u8]) -> Result<Self> {
        let store = Store::parse(bytes)?;
        let index = RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        let pages = document
            .pages()?
            .into_iter()
            .map(|(space, id)| Page::from_revision(document.active(space)?, id))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let titles = pages
            .iter()
            .map(|page| CString::new(page.title.replace('\0', "")))
            .collect::<std::result::Result<_, _>>()?;
        Ok(Self { pages, titles })
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

pub struct View {
    page: PageView,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: draw::Renderer,
    frame: Arc<Frame>,
}

fn report<T>(result: Result<T>) -> Option<T> {
    result.map_err(|error| eprintln!("snowbound: {error}")).ok()
}

fn moved(response: Response) -> bool {
    response.changed || response.moved
}

impl View {
    fn new(layer: *mut c_void, page: Page, size: [f32; 2], scale: f32) -> Result<Self> {
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
        let mut engine = TextEngine::default();
        let (scene, editor) = PageScene::from_page(page, &mut engine)?;
        Ok(Self {
            page: PageView::new(
                editor,
                engine,
                Some((scene, [0.0; 2])),
                pixels,
                scale,
                Duration::from_millis(350),
            ),
            surface,
            config,
            renderer,
            frame: Arc::default(),
        })
    }

    /// Device pixels per host point.
    fn display_scale(&self) -> f32 {
        self.page.viewport.scale / (self.page.zoom() * POINT)
    }

    fn device(&self, point: [f32; 2]) -> [f32; 2] {
        point.map(|value| value * self.display_scale())
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
        self.page
            .update_pictures(COLORS.paper, &Waker::from(self.frame.clone()));
        let primitives = self.page.primitives(COLORS)?;
        self.renderer
            .draw(
                &frame.texture.create_view(&Default::default()),
                [self.config.width, self.config.height],
                COLORS.paper.color,
                &[self.page.viewport.layer(&primitives)],
            )
            .map_err(|error| format!("Page drawing failed: {error:?}"))?;
        self.renderer.queue.present(frame);
        Ok(())
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

    fn tap(&mut self, point: [f32; 2]) -> Result<bool> {
        let mut changed = moved(self.page.pointer_moved(self.device(point))?);
        changed |= moved(self.page.pointer_pressed(Instant::now())?);
        changed |= moved(self.page.pointer_released()?);
        // A finger leaves no hover behind.
        let _ = self.page.pointer_left();
        Ok(changed)
    }

    /// Whether `point` lands on the text of the outline taking input, where the platform's
    /// own text interaction places the caret.
    fn in_active_text(&self, point: [f32; 2]) -> bool {
        let point = self.page.viewport.document_point(self.device(point));
        let pixel = self.display_scale() / self.page.viewport.scale;
        matches!(
            page_hit_test(&self.page.editor, self.page.scene.as_ref(), point, pixel),
            Some(Hit::Text { id, .. }) if id == self.page.editor.active_outline().id
        )
    }

    /// UTF-16 lengths of the active outline's text paragraphs.
    fn lengths(&self) -> Result<Vec<u32>> {
        self.page
            .editor
            .active_outline()
            .document()
            .paragraphs()
            .map(|paragraph| Ok(paragraph.utf16_offset(paragraph.text().len())?))
            .collect()
    }

    fn position(&self, flat: u32) -> Result<TextPosition> {
        let lengths = self.lengths()?;
        let mut rest = flat;
        for (paragraph, length) in lengths.iter().enumerate() {
            if rest <= *length {
                return Ok(TextPosition {
                    paragraph,
                    offset: rest,
                });
            }
            rest -= length + 1;
        }
        Ok(TextPosition {
            paragraph: lengths.len() - 1,
            offset: lengths[lengths.len() - 1],
        })
    }

    fn flat(&self, position: TextPosition) -> Result<u32> {
        let before: u32 = self.lengths()?[..position.paragraph]
            .iter()
            .map(|length| length + 1)
            .sum();
        Ok(before + position.offset)
    }

    fn length(&self) -> Result<u32> {
        let lengths = self.lengths()?;
        Ok(lengths.iter().sum::<u32>() + lengths.len() as u32 - 1)
    }

    fn text(&self, range: [u32; 2]) -> Result<String> {
        let range = self.position(range[0])?..self.position(range[1])?;
        Ok(self
            .page
            .editor
            .active_outline()
            .document()
            .slice(range)?
            .iter()
            .map(|paragraph| paragraph.text())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn selection(&self) -> Result<[u32; 2]> {
        let [anchor, focus] = self.page.editor.selection().positions;
        Ok([self.flat(anchor.min(focus))?, self.flat(anchor.max(focus))?])
    }

    fn select(&mut self, range: [u32; 2]) -> Result<()> {
        let positions = [self.position(range[0])?, self.position(range[1])?];
        Ok(self.page.editor.select(Selection::from(positions))?)
    }

    fn marked(&self) -> Result<Option<[u32; 2]>> {
        self.page
            .editor
            .marked_range()
            .map(|range| Ok([self.flat(range.start)?, self.flat(range.end)?]))
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

    fn insert(&mut self, text: String) -> Result<bool> {
        let response = if text == "\n" {
            self.page.key(&Key::Named(NamedKey::Enter), None)?
        } else {
            self.page.commit_text(text)?
        };
        Ok(moved(response))
    }

    /// A document rectangle in the active outline, in the view's points as `[x, y, width,
    /// height]`.
    fn view_rect(&self, rect: BoundingBox, origin: [f32; 2]) -> [f32; 4] {
        let viewport = self.page.viewport;
        let points = self.display_scale();
        let [x, y] = [rect.x0 as f32 + origin[0], rect.y0 as f32 + origin[1]];
        [
            (x * viewport.scale + viewport.origin[0]) / points,
            (y * viewport.scale + viewport.origin[1]) / points,
            rect.width() as f32 * viewport.scale / points,
            rect.height() as f32 * viewport.scale / points,
        ]
    }

    fn caret_rect(&self, flat: u32) -> Result<[f32; 4]> {
        let position = self.position(flat)?;
        let outline = self.page.editor.active_outline();
        let paragraph = outline.paragraph_layout(position.paragraph)?;
        let visible = paragraph.projection.visible_offset(position.offset)?;
        let byte = paragraph.projection.text().byte_offset(visible)?;
        let caret = paragraph
            .text
            .caret(paragraph.text.cursor(byte, Affinity::Downstream), 1.0);
        let origin = outline.origin();
        Ok(self.view_rect(
            caret,
            [
                origin[0] + paragraph.origin[0],
                origin[1] + paragraph.origin[1],
            ],
        ))
    }

    /// The selection rectangles of a range, one or more per line.
    fn range_rects(&self, range: [u32; 2]) -> Result<Vec<[f32; 4]>> {
        let [start, end] = [self.position(range[0])?, self.position(range[1])?];
        let outline = self.page.editor.active_outline();
        let origin = outline.origin();
        let mut rects = Vec::new();
        for (index, paragraph) in outline.layouts() {
            if index < start.paragraph || index > end.paragraph {
                continue;
            }
            let cursor = |offset: u32, affinity| -> Result<_> {
                let visible = paragraph.projection.visible_offset(offset)?;
                let byte = paragraph.projection.text().byte_offset(visible)?;
                Ok(paragraph.text.cursor(byte, affinity))
            };
            let first = cursor(
                if index == start.paragraph {
                    start.offset
                } else {
                    0
                },
                Affinity::Downstream,
            )?;
            let last = if index == end.paragraph {
                cursor(end.offset, Affinity::Upstream)?
            } else {
                paragraph
                    .text
                    .cursor(paragraph.projection.text().text().len(), Affinity::Upstream)
            };
            let origin = [
                origin[0] + paragraph.origin[0],
                origin[1] + paragraph.origin[1],
            ];
            for rect in paragraph
                .text
                .selection(parley::editing::Selection::new(first, last))
            {
                rects.push(self.view_rect(rect, origin));
            }
        }
        Ok(rects)
    }

    fn closest(&self, point: [f32; 2]) -> Result<u32> {
        let point = self.page.viewport.document_point(self.device(point));
        let origin = self.page.editor.active_outline().origin();
        let selection = self.page.editor.selection_at(
            point[0] - origin[0],
            point[1] - origin[1],
            SelectionUnit::Grapheme,
        )?;
        self.flat(selection.positions[1])
    }
}

/// # Safety
/// `path` is a NUL-terminated path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_open(path: *const c_char) -> *mut Section {
    let path = unsafe { CStr::from_ptr(path) };
    report(
        std::fs::read(path.to_str().unwrap_or_default())
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
    section.titles[index].as_ptr()
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
    if scale != view.display_scale() {
        let _ = report(view.page.scale_factor_changed(scale));
    }
    let _ = report(view.page.resized(pixels));
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

/// The page content's `[left, top, right, bottom]` in points at 100% zoom, bottom and right
/// including the margin OneNote scrolls past the last object.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_content(view: &mut View, bounds: &mut [f32; 4]) {
    *bounds = view.content().map(|value| value * POINT);
}

/// Zooms and scrolls so the page point `x`, `y` sits at the view's corner; the host's
/// scroll view owns the limits and rubber-banding.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_set_transform(view: &mut View, zoom: f32, x: f32, y: f32) {
    view.set_transform(zoom, [x, y]);
}

/// A tap: places the caret, focuses an outline or starts a new one.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_tap(view: &mut View, x: f32, y: f32) -> bool {
    report(view.tap([x, y])).unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_view_in_active_text(view: &View, x: f32, y: f32) -> bool {
    view.in_active_text([x, y])
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_text_length(view: &View) -> u32 {
    report(view.length()).unwrap_or(0)
}

/// The text in `start..end`, freed with `sb_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn sb_text(view: &View, start: u32, end: u32) -> *mut c_char {
    report(view.text([start, end]))
        .and_then(|text| CString::new(text.replace('\0', "")).ok())
        .map_or(std::ptr::null_mut(), CString::into_raw)
}

/// # Safety
/// `text` came from `sb_text` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_string_free(text: *mut c_char) {
    drop(unsafe { CString::from_raw(text) });
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_selection(view: &View, range: &mut [u32; 2]) {
    if let Some(selection) = report(view.selection()) {
        *range = selection;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_select(view: &mut View, start: u32, end: u32) -> bool {
    report(view.select([start, end])).is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_marked(view: &View, range: &mut [u32; 2]) -> bool {
    report(view.marked())
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
    let text = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    report(view.set_marked(text, [selected_start, selected_end])).unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_unmark(view: &mut View) {
    view.page.editor.finish_composition();
}

/// Types `text`, a lone `\n` as the Return key.
///
/// # Safety
/// `text` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_insert(view: &mut View, text: *const c_char) -> bool {
    let text = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    report(view.insert(text)).unwrap_or(false)
}

/// Replaces `start..end` with `text`, as autocorrection and dictation do.
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
    let text = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    report(
        view.select([start, end])
            .and_then(|()| Ok(moved(view.page.commit_text(text)?))),
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
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let language = unsafe { CStr::from_ptr(language) }.to_string_lossy();
    report(
        view.page
            .paste(&text, canvas::language::lcid(&language))
            .map(moved),
    )
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_delete_backward(view: &mut View) -> bool {
    report(
        view.page
            .key(&Key::Named(NamedKey::Backspace), None)
            .map(moved),
    )
    .unwrap_or(false)
}

/// The caret at `offset` as `[x, y, width, height]` in the view's points.
#[unsafe(no_mangle)]
pub extern "C" fn sb_caret_rect(view: &View, offset: u32, rect: &mut [f32; 4]) -> bool {
    report(view.caret_rect(offset))
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
    let found = report(view.range_rects([start, end])).unwrap_or_default();
    for (index, rect) in found.iter().take(capacity).enumerate() {
        unsafe { rects.add(index).write(*rect) };
    }
    found.len()
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_closest(view: &View, x: f32, y: f32) -> u32 {
    report(view.closest([x, y])).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_section_lists_its_pages() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../corpus/media-edit/candidate/Features.one"
        );
        let section = Section::open(&std::fs::read(path).unwrap()).unwrap();
        for title in &section.titles {
            eprintln!("{title:?}");
        }
        assert!(!section.pages.is_empty());
    }
}
