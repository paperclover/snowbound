mod background;
mod picture;

use super::{Paper, colorref};
use crate::editor::ReadOnlyObject;
use crate::editor::page::Content;
use crate::{
    date::DateField,
    editor::{CanvasEditor, EditorError},
    layout::{LayoutError, TextEngine},
};
use background::Background;
pub use background::{template_art, template_picture};
use draw::Primitive;
use onestore::page::Page;
use picture::Picture;
use std::{
    fmt,
    sync::{Arc, Mutex, OnceLock, mpsc},
    task::Waker,
    time::{Duration, Instant},
};

/// Retained drawing data in the source page's coordinate system.
pub struct PageScene {
    reference: Option<Vec<Content>>,
    /// Pictures and file icons, by the identity of the object that shows them.
    pictures: std::collections::BTreeMap<onestore::ExGuid, Picture>,
    /// Background pictures from OneNote's page templates, painted from their recreations.
    backgrounds: std::collections::BTreeMap<onestore::ExGuid, Background>,
}

/// How long the zoom must hold before a raster is made again for it.
const SETTLE: Duration = Duration::from_millis(150);

/// Where work done off the frame thread lands while its requester keeps it.
type Slot<T> = Arc<Mutex<Option<T>>>;

/// Runs `work` on the scenes' one raster thread once `start` has passed, unless the slot
/// returned has been dropped by then; the result lands in the slot and wakes `waker`.
fn queue<T: Send + 'static>(
    start: Instant,
    waker: &Waker,
    work: impl FnOnce() -> T + Send + 'static,
) -> Slot<T> {
    static WORKER: OnceLock<mpsc::Sender<Box<dyn FnOnce() + Send>>> = OnceLock::new();
    let slot = Slot::default();
    let requester = Arc::downgrade(&slot);
    let waker = waker.clone();
    WORKER
        .get_or_init(|| {
            let (sender, jobs) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
            std::thread::Builder::new()
                .name("page pictures".into())
                .spawn(move || jobs.into_iter().for_each(|job| job()))
                .expect("The picture thread starts");
            sender
        })
        .send(Box::new(move || {
            std::thread::sleep(start.saturating_duration_since(Instant::now()));
            if let Some(slot) = requester.upgrade() {
                *slot.lock().unwrap() = Some(work());
                waker.wake();
            }
        }))
        .expect("The picture thread outlives its jobs");
    slot
}

/// Device pixels per point a raster is made at for `scale`: the next half power of two,
/// so zooming makes rasters again in steps.
fn density(scale: f32) -> f32 {
    2_f32.powf((scale.max(0.25).log2() * 2.0).ceil() / 2.0)
}

#[derive(Debug, PartialEq)]
pub enum SceneHit<T> {
    Outline(T),
    Date(DateField),
    ReadOnly(usize),
    Image(onestore::ExGuid),
}

fn outline_origin(
    mut origin: [f32; 2],
    title: Option<onestore::ExGuid>,
    editor: Option<&CanvasEditor>,
) -> Result<[f32; 2], SceneError> {
    if let Some(id) = title {
        let title = editor
            .and_then(|editor| editor.visible_outlines().find(|outline| outline.id == id))
            .ok_or(SceneError::MissingOutline)?;
        origin[1] += title.bounds().y1 as f32;
    }
    Ok(origin)
}

fn append_ink(ink: &onestore::page::Ink, offset: [f32; 2], primitives: &mut Vec<Primitive<'_>>) {
    for stroke in &ink.strokes {
        let mut color = colorref(stroke.color.unwrap_or(0));
        color[3] = 1.0 - f32::from(stroke.transparency.unwrap_or(0)) / 255.0;
        let width = stroke.width.max(stroke.height);
        let round = stroke.pen_tip != Some(1);
        let mut points = stroke
            .points
            .iter()
            .map(|[x, y]| [x + offset[0], y + offset[1]]);
        let Some(mut from) = points.next() else {
            continue;
        };
        let mut drawn = false;
        for to in points {
            // Pen samples far closer than a pixel add vertices without changing the stroke.
            if (to[0] - from[0]).hypot(to[1] - from[1]) < 0.2 {
                continue;
            }
            primitives.push(Primitive::Segment {
                from,
                to,
                width,
                round,
                color,
            });
            from = to;
            drawn = true;
        }
        if !drawn {
            primitives.push(Primitive::Segment {
                from,
                to: from,
                width,
                round,
                color,
            });
        }
    }
    for group in &ink.groups {
        append_ink(group, offset, primitives);
    }
}

fn append_placeholder(rect: [f32; 4], paper: Paper, primitives: &mut Vec<Primitive<'_>>) {
    primitives.push(Primitive::Rect {
        rect,
        color: paper.shade(colorref(0x00e4ddd6)),
    });
    let inner = [rect[0] + 1.0, rect[1] + 1.0, rect[2] - 1.0, rect[3] - 1.0];
    if inner[0] < inner[2] && inner[1] < inner[3] {
        primitives.push(Primitive::Rect {
            rect: inner,
            color: paper.shade(colorref(0x00faf7f3)),
        });
    }
}

#[derive(Debug)]
pub enum SceneError {
    Layout(LayoutError),
    Editor(EditorError),
    MissingOutline,
    MissingImage,
    InvalidGeometry,
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Layout(error) => error.fmt(f),
            Self::Editor(error) => error.fmt(f),
            Self::MissingOutline => f.write_str("An editable outline is missing from this page."),
            Self::MissingImage => f.write_str("An image is missing its data or dimensions."),
            Self::InvalidGeometry => {
                f.write_str("The page contains invalid object dimensions or positions.")
            }
        }
    }
}
impl std::error::Error for SceneError {}
impl From<LayoutError> for SceneError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

impl From<EditorError> for SceneError {
    fn from(error: EditorError) -> Self {
        match error {
            EditorError::InvalidGeometry => Self::InvalidGeometry,
            error => Self::Editor(error),
        }
    }
}

impl PageScene {
    pub fn new(mut page: Page, engine: &mut TextEngine) -> Result<Self, SceneError> {
        let mut objects = crate::editor::page::build(&mut page, engine, false)
            .map_err(SceneError::from)?
            .objects;
        let pictures = Self::pictures(&objects, None)?;
        pictures.mark_unavailable(&mut objects, engine)?;
        Ok(Self {
            reference: Some(objects),
            ..pictures
        })
    }

    pub fn from_page(
        page: Page,
        engine: &mut TextEngine,
    ) -> Result<(Self, CanvasEditor), SceneError> {
        let mut editor = CanvasEditor::from_page(page, engine).map_err(SceneError::from)?;
        let scene = Self::pictures(&editor.objects, Some(&editor))?;
        scene.mark_unavailable(&mut editor.objects, engine)?;
        Ok((scene, editor))
    }

    /// Follows `editor` after it refreshed from storage, keeping the rasters of pictures
    /// whose bytes did not change so they do not blink.
    pub fn refresh(
        &mut self,
        editor: &mut CanvasEditor,
        engine: &mut TextEngine,
    ) -> Result<(), SceneError> {
        let mut scene = Self::pictures(&editor.objects, Some(editor))?;
        for (id, picture) in &mut scene.pictures {
            if let Some(shown) = self.pictures.remove(id)
                && shown.same(picture)
            {
                *picture = shown;
            }
        }
        for (id, art) in &mut scene.backgrounds {
            if let Some(shown) = self.backgrounds.remove(id)
                && shown.same(art)
            {
                *art = shown;
            }
        }
        scene.mark_unavailable(&mut editor.objects, engine)?;
        *self = scene;
        Ok(())
    }

    /// Turns page pictures the renderer cannot decode into placeholders; their stored data
    /// is kept.
    fn mark_unavailable(
        &self,
        objects: &mut Vec<Content>,
        engine: &mut TextEngine,
    ) -> Result<(), SceneError> {
        *objects = std::mem::take(objects)
            .into_iter()
            .map(|object| match object {
                Content::Image(source)
                    if !self.pictures.contains_key(&source.id)
                        && !self.backgrounds.contains_key(&source.id) =>
                {
                    Content::unavailable(onestore::page::PageObject::Image(source), engine)
                }
                object => Ok(object),
            })
            .collect::<Result<_, _>>()?;
        Ok(())
    }

    /// Every picture the page draws, page-level ones and those inside outlines, except the
    /// template backgrounds it recognises; one the renderer cannot decode is left out.
    fn pictures(objects: &[Content], editor: Option<&CanvasEditor>) -> Result<Self, SceneError> {
        fn nested<'a>(
            nodes: &'a [onestore::page::PageParagraph],
            payloads: &mut Vec<(onestore::ExGuid, Option<&'a Arc<[u8]>>)>,
        ) {
            for node in nodes {
                match &node.content {
                    onestore::page::ParagraphContent::Image(image) => {
                        payloads.push((image.id, image.bytes.as_ref()))
                    }
                    // A file without the icon OneNote rendered for it keeps an empty slot.
                    onestore::page::ParagraphContent::Attachment(file) => {
                        if let Some(icon) = file.preview.as_ref() {
                            payloads.push((file.id, Some(icon)))
                        }
                    }
                    onestore::page::ParagraphContent::Table(table) => {
                        for cell in table.rows.iter().flat_map(|row| &row.cells) {
                            nested(&cell.paragraphs, payloads);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut payloads = Vec::new();
        let mut backgrounds = std::collections::BTreeMap::new();
        for object in objects {
            match object {
                Content::Image(source) => {
                    if source.background
                        && let Some(art) = Background::recognise(source)
                    {
                        backgrounds.insert(source.id, art);
                    } else {
                        payloads.push((source.id, source.bytes.as_ref()))
                    }
                }
                Content::Outline { source, .. } => nested(&source.paragraphs, &mut payloads),
                Content::Editable(id) => {
                    if let Some(outline) =
                        editor.and_then(|editor| editor.outlines().iter().find(|o| o.id == *id))
                    {
                        nested(outline.document().nodes(), &mut payloads);
                    }
                }
                Content::Date { .. } | Content::Ink(_) | Content::ReadOnly(_) => {}
            }
        }
        let mut pictures = std::collections::BTreeMap::new();
        for (id, encoded) in payloads {
            let Some(picture) = encoded.and_then(Picture::new) else {
                continue;
            };
            if pictures.insert(id, picture).is_some() {
                return Err(SceneError::InvalidGeometry);
            }
        }
        Ok(Self {
            reference: None,
            pictures,
            backgrounds,
        })
    }

    /// Brings template backgrounds to `paper` and the pictures near `view`, a rectangle in
    /// scene coordinates, to `scale` device pixels per point, rasterizing and decoding off
    /// this thread; `waker` is woken when one lands and the page should be drawn again.
    /// Call before collecting each frame's primitives. True once everything in view shows
    /// its raster for this paper and scale.
    pub fn update_pictures(
        &mut self,
        editor: Option<&CanvasEditor>,
        view: [f32; 4],
        scale: f32,
        paper: Paper,
        waker: &Waker,
    ) -> bool {
        let mut settled = true;
        for art in self.backgrounds.values_mut() {
            settled &= art.update(paper, scale, waker);
        }
        let rects = self.picture_rects(editor);
        picture::update(&mut self.pictures, rects, view, scale, waker) && settled
    }

    /// Waits until the whole page shows its rasters for `paper` at `scale`, for drawing it
    /// offscreen.
    pub fn settle(&mut self, editor: Option<&CanvasEditor>, scale: f32, paper: Paper) {
        let everything = [
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::INFINITY,
        ];
        while !self.update_pictures(editor, everything, scale, paper, Waker::noop()) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The raster a picture or file icon shows, once decoded.
    pub fn image(&self, id: onestore::ExGuid) -> Option<&draw::RasterImage> {
        self.pictures.get(&id)?.image()
    }

    /// Where pictures and file icons sit, in scene coordinates.
    fn picture_rects(&self, editor: Option<&CanvasEditor>) -> Vec<(onestore::ExGuid, [f32; 4])> {
        fn outline(
            layout: &crate::outline::OutlineLayout,
            [x, y]: [f32; 2],
            rects: &mut Vec<(onestore::ExGuid, [f32; 4])>,
        ) {
            rects.extend(layout.objects.iter().map(|object| {
                let [x0, y0, x1, y1] = object.rect;
                (object.id, [x0 + x, y0 + y, x1 + x, y1 + y])
            }));
        }
        let mut rects = Vec::new();
        for content in self.objects(editor).into_iter().flatten() {
            match content {
                Content::Image(source) => {
                    let [x, y] = [
                        source.layout.x.unwrap_or(0.0),
                        source.layout.y.unwrap_or(0.0),
                    ];
                    if let (Some(width), Some(height)) =
                        (source.layout.max_width, source.layout.max_height)
                    {
                        rects.push((source.id, [x, y, x + width, y + height]));
                    }
                }
                Content::Outline {
                    source,
                    layout,
                    below_title,
                } => {
                    let origin = [
                        source.layout.x.unwrap_or(0.0),
                        source.layout.y.unwrap_or(0.0),
                    ];
                    if let Ok(origin) = outline_origin(origin, *below_title, editor) {
                        outline(layout, origin, &mut rects);
                    }
                }
                Content::Editable(id) => {
                    if let Some(editable) =
                        editor.and_then(|editor| editor.visible_outlines().find(|o| o.id == *id))
                    {
                        outline(editable.shaped(), editable.origin(), &mut rects);
                    }
                }
                Content::Date { .. } | Content::Ink(_) | Content::ReadOnly(_) => {}
            }
        }
        rects
    }

    /// Pictures, files and handwriting inside an outline whose origin is `origin`.
    /// `moving` draws one picture at a previewed rectangle, in the same coordinates as `origin`.
    pub fn append_outline_objects<'a>(
        &'a self,
        outline: &'a crate::outline::OutlineLayout,
        origin: [f32; 2],
        moving: Option<(onestore::ExGuid, [f32; 4])>,
        paper: Paper,
        primitives: &mut Vec<Primitive<'a>>,
    ) {
        for object in &outline.objects {
            let [x0, y0, x1, y1] = object.rect;
            let rect = match moving {
                Some((id, rect)) if id == object.id => rect,
                _ => [
                    x0 + origin[0],
                    y0 + origin[1],
                    x1 + origin[0],
                    y1 + origin[1],
                ],
            };
            let picture = self.pictures.get(&object.id);
            if let Some(image) = picture.and_then(Picture::image) {
                primitives.push(Primitive::Image { image, rect });
            }
            match &object.kind {
                crate::outline::ObjectKind::Ink(ink) => {
                    append_ink(ink, [rect[0], rect[1]], primitives)
                }
                crate::outline::ObjectKind::Unsupported(_) => {
                    append_placeholder(rect, paper, primitives)
                }
                crate::outline::ObjectKind::Picture if picture.is_none_or(Picture::failed) => {
                    append_placeholder(rect, paper, primitives)
                }
                crate::outline::ObjectKind::Picture | crate::outline::ObjectKind::File(_) => {}
            }
            if let Some(label) = object.label() {
                primitives.push(Primitive::Text {
                    text: &label.text,
                    origin: [origin[0] + label.origin[0], origin[1] + label.origin[1]],
                    clip: None,
                    ink: paper.ink,
                });
            }
        }
    }

    fn objects<'a>(
        &'a self,
        editor: Option<&'a CanvasEditor>,
    ) -> Result<&'a [Content], SceneError> {
        self.reference
            .as_deref()
            .or_else(|| editor.map(|editor| editor.objects.as_slice()))
            .ok_or(SceneError::MissingOutline)
    }

    pub fn read_only<'a>(
        &'a self,
        editor: Option<&'a CanvasEditor>,
    ) -> impl Iterator<Item = &'a ReadOnlyObject> {
        self.objects(editor)
            .into_iter()
            .flatten()
            .filter_map(|object| match object {
                Content::ReadOnly(object) => Some(object.as_ref()),
                _ => None,
            })
    }

    /// Bounds of noneditable content in page coordinates, each with whether OneNote's view
    /// leaves room beyond it: a picture's corner is as far as the view reaches.
    pub fn content_bounds<'a>(
        &'a self,
        editor: &'a CanvasEditor,
    ) -> impl Iterator<Item = ([f32; 4], bool)> + 'a {
        self.objects(Some(editor))
            .into_iter()
            .flatten()
            .filter_map(move |object| match object {
                Content::Outline {
                    source,
                    layout,
                    below_title,
                } => {
                    let origin = outline_origin(
                        [
                            source.layout.x.unwrap_or(0.0),
                            source.layout.y.unwrap_or(0.0),
                        ],
                        *below_title,
                        Some(editor),
                    )
                    .ok()?;
                    Some((
                        [
                            origin[0],
                            origin[1],
                            origin[0] + layout.size[0],
                            origin[1] + layout.size[1],
                        ],
                        true,
                    ))
                }
                Content::Date { below_title } => {
                    let date = editor.date()?;
                    let origin = outline_origin(
                        [
                            date.source().layout.x.unwrap_or(0.0),
                            date.source().layout.y.unwrap_or(0.0),
                        ],
                        *below_title,
                        Some(editor),
                    )
                    .ok()?;
                    let size = date.layout().size;
                    Some((
                        [
                            origin[0],
                            origin[1],
                            origin[0] + size[0],
                            origin[1] + size[1],
                        ],
                        true,
                    ))
                }
                Content::Image(source) => Some((
                    [
                        source.layout.x.unwrap_or(0.0),
                        source.layout.y.unwrap_or(0.0),
                        source.layout.x.unwrap_or(0.0) + source.layout.max_width?,
                        source.layout.y.unwrap_or(0.0) + source.layout.max_height?,
                    ],
                    false,
                )),
                Content::Ink(ink) => Some((crate::editor::page::ink_bounds(ink)?, true)),
                Content::ReadOnly(object) => Some((object.rect(), true)),
                Content::Editable(_) => None,
            })
    }

    pub fn date_fields(
        &self,
        editor: &CanvasEditor,
    ) -> impl Iterator<Item = (DateField, [f32; 4])> {
        let mut fields = [None; 2];
        if let Some(date) = editor.date()
            && let Some((origin, title)) = self
                .objects(Some(editor))
                .into_iter()
                .flatten()
                .find_map(|object| match object {
                    Content::Date { below_title } => Some((
                        [
                            date.source().layout.x.unwrap_or(0.0),
                            date.source().layout.y.unwrap_or(0.0),
                        ],
                        *below_title,
                    )),
                    _ => None,
                })
            && let Ok(origin) = outline_origin(origin, title, Some(editor))
        {
            for (slot, (field, paragraph)) in fields.iter_mut().zip(
                date.fields()
                    .map(|(field, _)| field)
                    .zip(&date.layout().paragraphs),
            ) {
                let x = origin[0] + paragraph.origin[0];
                let y = origin[1] + paragraph.origin[1];
                let width = paragraph
                    .text
                    .lines()
                    .map(|(line, _)| line.metrics().advance)
                    .fold(0.0_f32, f32::max);
                *slot = Some((
                    field,
                    [x, y, x + width.max(12.0), y + paragraph.text.height()],
                ));
            }
        }
        fields.into_iter().flatten()
    }

    /// The point and outline hit callback use scene coordinates.
    pub fn hit_test<T>(
        &self,
        point: [f32; 2],
        editor: Option<&CanvasEditor>,
        mut outline: impl FnMut(onestore::ExGuid) -> Option<T>,
    ) -> Option<SceneHit<T>> {
        let mut readonly = self.read_only(editor).count();
        for object in self.objects(editor).ok()?.iter().rev() {
            match object {
                Content::Date { .. } => {
                    if let Some(editor) = editor
                        && let Some((field, _)) = self.date_fields(editor).find(|(_, rect)| {
                            (rect[0]..=rect[2]).contains(&point[0])
                                && (rect[1]..=rect[3]).contains(&point[1])
                        })
                    {
                        return Some(SceneHit::Date(field));
                    }
                }
                Content::Editable(id) => {
                    if let Some(hit) = outline(*id) {
                        return Some(SceneHit::Outline(hit));
                    }
                }
                Content::ReadOnly(object) => {
                    readonly -= 1;
                    let [x0, y0, x1, y1] = object.rect();
                    if (x0..=x1).contains(&point[0]) && (y0..=y1).contains(&point[1]) {
                        return Some(SceneHit::ReadOnly(readonly));
                    }
                }
                Content::Image(source) if !source.background => {
                    let [x, y] = [
                        source.layout.x.unwrap_or(0.0),
                        source.layout.y.unwrap_or(0.0),
                    ];
                    if let (Some(width), Some(height)) =
                        (source.layout.max_width, source.layout.max_height)
                        && (x..=x + width).contains(&point[0])
                        && (y..=y + height).contains(&point[1])
                    {
                        return Some(SceneHit::Image(source.id));
                    }
                }
                Content::Outline { .. } | Content::Image(_) | Content::Ink(_) => {}
            }
        }
        None
    }

    /// The offset is in document points.
    pub fn append_primitives<'a>(
        &'a self,
        primitives: &mut Vec<Primitive<'a>>,
        offset: [f32; 2],
        paper: Paper,
    ) -> Result<(), SceneError> {
        self.append_primitives_with(primitives, offset, None, None, paper, |_, _, _| {
            Err(SceneError::MissingOutline)
        })
    }

    /// `moving` draws one picture at a previewed rectangle instead of its stored layout.
    pub fn append_primitives_with<'a, E: From<SceneError>>(
        &'a self,
        primitives: &mut Vec<Primitive<'a>>,
        offset: [f32; 2],
        editor: Option<&'a CanvasEditor>,
        moving: Option<(onestore::ExGuid, [f32; 4])>,
        paper: Paper,
        mut outline: impl FnMut(onestore::ExGuid, [f32; 2], &mut Vec<Primitive<'a>>) -> Result<(), E>,
    ) -> Result<(), E> {
        for content in self.objects(editor)? {
            let origin = match content {
                Content::Outline {
                    source,
                    below_title,
                    ..
                } => outline_origin(
                    [
                        source.layout.x.unwrap_or(0.0),
                        source.layout.y.unwrap_or(0.0),
                    ],
                    *below_title,
                    editor,
                )?,
                Content::Date { below_title } => {
                    let date = editor
                        .and_then(CanvasEditor::date)
                        .ok_or(SceneError::MissingOutline)?;
                    outline_origin(
                        [
                            date.source().layout.x.unwrap_or(0.0),
                            date.source().layout.y.unwrap_or(0.0),
                        ],
                        *below_title,
                        editor,
                    )?
                }
                Content::Image(source) => [
                    source.layout.x.unwrap_or(0.0),
                    source.layout.y.unwrap_or(0.0),
                ],
                Content::ReadOnly(object) => {
                    let [x0, y0, x1, y1] = object.rect();
                    let rect = [
                        x0 + offset[0],
                        y0 + offset[1],
                        x1 + offset[0],
                        y1 + offset[1],
                    ];
                    append_placeholder(rect, paper, primitives);
                    primitives.push(Primitive::Text {
                        clip: None,
                        text: &object.label,
                        origin: [rect[0] + 8.0, rect[1] + 8.0],
                        ink: paper.shade(colorref(0x005d554e)),
                    });
                    continue;
                }
                Content::Editable(id) => {
                    outline(*id, offset, primitives)?;
                    continue;
                }
                Content::Ink(ink) => {
                    let [x, y] = crate::editor::page::ink_offset(ink);
                    append_ink(ink, [offset[0] + x, offset[1] + y], primitives);
                    continue;
                }
            };
            let object_origin = [origin[0] + offset[0], origin[1] + offset[1]];
            match content {
                Content::Image(source) => {
                    // The paper shows until the worker's raster lands.
                    let (image, [width, height]) = match self.backgrounds.get(&source.id) {
                        Some(art) => (art.image(paper), art.size),
                        None => {
                            let picture = self
                                .pictures
                                .get(&source.id)
                                .ok_or(SceneError::MissingImage)?;
                            let size = [
                                source.layout.max_width.ok_or(SceneError::MissingImage)?,
                                source.layout.max_height.ok_or(SceneError::MissingImage)?,
                            ];
                            if picture.failed() {
                                append_placeholder(
                                    [
                                        object_origin[0],
                                        object_origin[1],
                                        object_origin[0] + size[0],
                                        object_origin[1] + size[1],
                                    ],
                                    paper,
                                    primitives,
                                );
                            }
                            (picture.image(), size)
                        }
                    };
                    let Some(image) = image else {
                        continue;
                    };
                    primitives.push(Primitive::Image {
                        image,
                        rect: match moving {
                            Some((id, [x0, y0, x1, y1])) if id == source.id => [
                                x0 + offset[0],
                                y0 + offset[1],
                                x1 + offset[0],
                                y1 + offset[1],
                            ],
                            _ => [
                                object_origin[0],
                                object_origin[1],
                                object_origin[0] + width,
                                object_origin[1] + height,
                            ],
                        },
                    })
                }
                Content::Outline { .. } | Content::Date { .. } => {
                    let outline = match content {
                        Content::Outline { layout, .. } => layout,
                        _ => editor
                            .and_then(CanvasEditor::date)
                            .ok_or(SceneError::MissingOutline)?
                            .layout(),
                    };
                    outline.append_table_primitives(primitives, object_origin, paper);
                    let everything = [f32::NEG_INFINITY, f32::INFINITY];
                    outline.append_background_primitives(
                        primitives,
                        object_origin,
                        everything,
                        paper,
                    );
                    self.append_outline_objects(outline, object_origin, None, paper, primitives);
                    for (index, paragraph) in outline.visible(everything) {
                        outline.append_paragraph_primitives(
                            index,
                            paragraph,
                            object_origin,
                            paper.ink,
                            primitives,
                        );
                    }
                }
                Content::Editable(_) | Content::ReadOnly(_) | Content::Ink(_) => unreachable!(),
            }
        }
        Ok(())
    }
}

impl crate::outline::OutlineLayout {
    /// Paragraphs that may paint between outline-local `rows`, with their indices.
    pub fn visible(
        &self,
        rows: [f32; 2],
    ) -> impl Iterator<Item = (usize, &crate::outline::ParagraphLayout)> {
        // Markers, tags and overhanging ink stay within this of a paragraph's lines.
        const OVERHANG: f32 = 72.0;
        self.paragraphs
            .iter()
            .enumerate()
            .filter(move |(_, paragraph)| {
                let [start, end] = [
                    paragraph.origin[1] - OVERHANG,
                    paragraph.origin[1] + paragraph.text.height() + OVERHANG,
                ];
                end >= rows[0] && start <= rows[1]
            })
    }

    /// Highlights behind the paragraphs `visible` finds between outline-local `rows`. A black
    /// highlight paints in the paper's ink, censoring the automatic text on it in any theme.
    /// A paragraph's band spans its outline, or its cell, tinted for the paper.
    pub fn append_background_primitives(
        &self,
        primitives: &mut Vec<Primitive<'_>>,
        origin: [f32; 2],
        rows: [f32; 2],
        paper: Paper,
    ) {
        for (index, paragraph) in self.visible(rows) {
            if let Some(band) = paragraph.band {
                let mut rect = parley::BoundingBox::new(
                    0.0,
                    f64::from(paragraph.origin[1]),
                    f64::from(self.size[0]),
                    f64::from(paragraph.origin[1] + paragraph.text.height()),
                );
                let cell = self.paragraph_cell(index);
                if let Some(clipped) = cell.map_or(Some(rect), |cell| cell.clip(rect)) {
                    rect = clipped;
                    primitives.push(Primitive::Rect {
                        rect: [
                            rect.x0 as f32 + origin[0],
                            rect.y0 as f32 + origin[1],
                            rect.x1 as f32 + origin[0],
                            rect.y1 as f32 + origin[1],
                        ],
                        color: paper.tint(colorref(band)),
                    });
                }
            }
            for (mut rect, color) in paragraph.text.backgrounds() {
                rect.x0 += f64::from(paragraph.origin[0]);
                rect.x1 += f64::from(paragraph.origin[0]);
                rect.y0 += f64::from(paragraph.origin[1]);
                rect.y1 += f64::from(paragraph.origin[1]);
                if let Some(cell) = self.paragraph_cell(index) {
                    let Some(clipped) = cell.clip(rect) else {
                        continue;
                    };
                    rect = clipped;
                }
                primitives.push(Primitive::Rect {
                    rect: [
                        rect.x0 as f32 + origin[0],
                        rect.y0 as f32 + origin[1],
                        rect.x1 as f32 + origin[0],
                        rect.y1 as f32 + origin[1],
                    ],
                    color: if color == 0 {
                        paper.ink
                    } else {
                        colorref(color)
                    },
                });
            }
        }
    }

    /// One paragraph of this outline, whose origin is `origin`: its text or equation, list
    /// markers and tags.
    pub fn append_paragraph_primitives<'a>(
        &'a self,
        index: usize,
        paragraph: &'a crate::outline::ParagraphLayout,
        origin: [f32; 2],
        ink: [f32; 4],
        primitives: &mut Vec<Primitive<'a>>,
    ) {
        let [x, y] = [
            origin[0] + paragraph.origin[0],
            origin[1] + paragraph.origin[1],
        ];
        let clip = self.paragraph_cell(index).map(|cell| {
            let [left, top, right, bottom] = cell.text_bounds();
            [
                left + origin[0],
                top + origin[1],
                right + origin[0],
                bottom + origin[1],
            ]
        });
        primitives.push(Primitive::Text {
            clip,
            text: &paragraph.text,
            origin: [x, y],
            ink,
        });
        for (index, [left, baseline]) in paragraph.text.spaces() {
            let math = &paragraph.math[index];
            let [x, y] = [x + left, y + baseline - math.baseline];
            for item in &math.items {
                primitives.push(match item {
                    crate::math::MathItem::Text { layout, origin } => Primitive::Text {
                        clip,
                        text: layout,
                        origin: [x + origin[0], y + origin[1]],
                        ink,
                    },
                    // As pen strokes, so hairline rules keep a device pixel.
                    crate::math::MathItem::Rule([x0, y0, x1, y1]) => {
                        let width = (x1 - x0).min(y1 - y0);
                        let [from, to] = if x1 - x0 >= y1 - y0 {
                            let middle = (y0 + y1) / 2.0;
                            [[x0 + width / 2.0, middle], [x1 - width / 2.0, middle]]
                        } else {
                            let middle = (x0 + x1) / 2.0;
                            [[middle, y0 + width / 2.0], [middle, y1 - width / 2.0]]
                        };
                        Primitive::Segment {
                            from: [x + from[0], y + from[1]],
                            to: [x + to[0], y + to[1]],
                            width,
                            round: false,
                            color: math.color.map_or(ink, colorref),
                        }
                    }
                    crate::math::MathItem::Stroke { from, to, width } => Primitive::Segment {
                        from: [x + from[0], y + from[1]],
                        to: [x + to[0], y + to[1]],
                        width: *width,
                        round: true,
                        color: math.color.map_or(ink, colorref),
                    },
                });
            }
        }
        for (layout, marker) in &paragraph.markers {
            primitives.push(Primitive::Text {
                clip,
                text: layout,
                origin: [origin[0] + marker[0], y + marker[1]],
                ink,
            });
        }
        for tag in &paragraph.tags {
            primitives.push(Primitive::Icon {
                sources: super::tag_sources(tag.icon),
                origin: [
                    origin[0] + self.tag_column_offset() + tag.origin[0],
                    y + tag.origin[1],
                ],
                size: tag.size,
                tint: [1.0, 1.0, 1.0, if tag.disabled { 0.45 } else { 1.0 }],
                palette: draw::Palette::default(),
            });
        }
    }

    pub fn append_table_primitives(
        &self,
        primitives: &mut Vec<Primitive<'_>>,
        origin: [f32; 2],
        paper: Paper,
    ) {
        for table in &self.tables {
            let (Some(first), Some(last)) = (table.cells.first(), table.cells.last()) else {
                continue;
            };
            if !table.borders {
                continue;
            }
            let bounds = [first.rect[0], first.rect[1], last.rect[2], last.rect[3]];
            let color = paper.shade(colorref(0x00a3a3a3));
            primitives.push(Primitive::RoundedRect {
                rect: [
                    bounds[0] + origin[0] - 0.375,
                    bounds[1] + origin[1] - 0.375,
                    bounds[2] + origin[0] + 0.375,
                    bounds[3] + origin[1] + 0.375,
                ],
                radius: [3.6; 2],
                stroke: Some(draw::Stroke::Solid(0.75)),
                color,
            });
            for cell in &table.cells {
                let [left, top, right, bottom] = cell.rect;
                if right < bounds[2] {
                    primitives.push(Primitive::Rect {
                        rect: [
                            right + origin[0] - 0.375,
                            top + origin[1],
                            right + origin[0] + 0.375,
                            bottom + origin[1],
                        ],
                        color,
                    });
                }
                if bottom < bounds[3] {
                    primitives.push(Primitive::Rect {
                        rect: [
                            left + origin[0],
                            bottom + origin[1] - 0.375,
                            right + origin[0],
                            bottom + origin[1] + 0.375,
                        ],
                        color,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::TextOutline;
    use crate::gpu::painted_layout;
    use onestore::page::text::Paragraph;
    use onestore::page::{Image, PageObject};
    use onestore::{ExGuid, document::Layout};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn title_editing_keeps_identity_and_date_geometry_through_composition_and_undo() {
        use crate::document::TextDocument;
        use onestore::page::Title;
        let mut engine = TextEngine::default();
        let mut fields = ["Header", "September 8, 2026"].map(|text| {
            let document =
                TextDocument::new(vec![Paragraph::new(text.into(), Default::default())]).unwrap();
            TextOutline::new(&mut engine, document, 468.0, [0.0; 2])
                .unwrap()
                .snapshot()
        });
        for field in &mut fields {
            field.layout.max_width = None;
            field.layout.width_set_by_user = None;
            field.layout.max_height = Some(21.6);
        }
        fields[0].title = true;
        fields[0].min_width = Some(162.0);
        fields[1].layout.x = Some(2.0);
        fields[1].layout.y = Some(3.0);
        let id = fields[0].id;
        let source = fields[0].paragraphs.clone();
        let page = Page {
            identity: None,
            created: Some(1),
            title: "Header".into(),
            margin_origin: [36.0, 14.4],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects: vec![PageObject::Title(Title {
                date: Some(fields[1].id),
                id: ExGuid::default(),
                layout: Layout {
                    x: Some(10.0),
                    y: Some(20.0),
                    ..Default::default()
                },
                outlines: fields.into(),
            })],
        };
        let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
        assert!(editor.active_outline().title);
        assert_eq!(editor.active_outline().origin(), [46.0, 34.4]);
        assert_eq!(editor.active_outline().layout().max_width, None);
        let bounds = |editor: &CanvasEditor| {
            scene
                .content_bounds(editor)
                .map(|(rect, _)| rect)
                .collect::<Vec<_>>()
        };
        let before = bounds(&editor);
        assert_eq!(before[0][0], 48.0);
        assert!((before[0][1] - 62.6).abs() < 0.00001);
        let text = "A long title with multiple wrapped lines ".repeat(10);
        editor.select_all().unwrap();
        let end = text.encode_utf16().count() as u32;
        editor.compose(&mut engine, text.clone(), end..end).unwrap();
        let wrapped = bounds(&editor);
        assert!(wrapped[0][1] > before[0][1] + 20.0);
        assert_eq!(
            wrapped[0][1],
            editor.active_outline().bounds().y1 as f32 + 3.0 + 3.6
        );
        editor.cancel_composition(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document().nodes(), source);
        assert_eq!(bounds(&editor), before);
        editor.commit_text(&mut engine, text).unwrap();
        assert_eq!(bounds(&editor), wrapped);
        assert_eq!(editor.active_outline().id, id);
        assert_eq!(
            editor.active_outline().document().nodes()[0].id,
            source[0].id
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document().nodes(), source);
        assert_eq!(bounds(&editor), before);
        editor.redo(&mut engine).unwrap();
        editor.select_all().unwrap();
        editor.insert(&mut engine, "").unwrap();
        assert_eq!(editor.outlines().len(), 1);
        assert!(editor.caret_outline().is_none());
        assert_eq!(editor.active_outline().id, id);
        assert_eq!(bounds(&editor), before);
        assert!(editor.move_outline(id, [0.0; 2]).is_err());
        assert!(editor.resize(&mut engine, 90.0).is_err());
        editor.undo(&mut engine).unwrap();
        let primitives = paint_editor(&scene, &editor);
        let Primitive::Text { origin, .. } = primitives.last().unwrap() else {
            panic!()
        };
        assert_eq!(origin[1], wrapped[0][1]);
        let title = editor.active_outline().document().clone();
        editor
            .change_date(
                &mut engine,
                2,
                ["A date label that wraps ".repeat(30), String::new()],
            )
            .unwrap();
        let changed = bounds(&editor);
        assert_eq!(changed[0][1], wrapped[0][1]);
        assert!(changed[0][3] > wrapped[0][3]);
        let (field, rect) = scene.date_fields(&editor).next().unwrap();
        assert_eq!(field, DateField::Date);
        assert_eq!(rect[1], changed[0][1]);
        assert_eq!(rect[3], changed[0][3]);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.date().unwrap().timestamp(), 1);
        assert_eq!(bounds(&editor), wrapped);
        assert_eq!(editor.active_outline().document(), &title);
    }

    #[test]
    #[ignore = "requires the native baseline section via CANVAS_TEST_SECTION"]
    fn native_title_metrics_and_exit() {
        let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let store = onestore::Store::parse(&bytes).unwrap();
        let index = onestore::RevisionIndex::parse(&store).unwrap();
        let document = onestore::document::Document::parse(&index).unwrap();
        let page = Page::from_document(&document, "Baseline anchors").unwrap();
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
        let title_id = editor
            .outlines()
            .iter()
            .find(|outline| outline.title)
            .unwrap()
            .id;
        for (text, lines, y) in [
            ("Header", 1, 86.4),
            (
                "A page title that is intentionally long enough to wrap across several lines at this window size while retaining its own date and time below the text",
                3,
                140.4,
            ),
        ] {
            editor.focus_outline(title_id).unwrap();
            editor.select_all().unwrap();
            editor.insert(&mut engine, text).unwrap();
            let paragraph = editor.active_outline().layouts().next().unwrap().1;
            assert_eq!(paragraph.text.lines().count(), lines);
            assert!((paragraph.text.lines().next().unwrap().1.height - 20.751953).abs() < 0.001);
            editor.leave_title(&mut engine).unwrap();
            assert_eq!(editor.caret_outline().unwrap().origin(), [36.0, y]);
            editor.undo(&mut engine).unwrap();
        }
        let date = editor.date().unwrap();
        let timestamp = date.timestamp();
        let source = date.source().paragraphs.clone();
        assert_eq!(
            date.fields().map(|(field, _)| field).collect::<Vec<_>>(),
            [DateField::Date, DateField::Time]
        );
        let positions = editor
            .outlines()
            .iter()
            .map(TextOutline::origin)
            .collect::<Vec<_>>();
        editor
            .change_date(
                &mut engine,
                timestamp + 2 * 86_400 * 10_000_000,
                ["Wednesday, September 09, 2026".into(), "6:14 AM".into()],
            )
            .unwrap();
        assert_eq!(
            editor
                .outlines()
                .iter()
                .map(TextOutline::origin)
                .collect::<Vec<_>>(),
            positions
        );
        assert_eq!(editor.date().unwrap().source().paragraphs[1], source[1]);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.date().unwrap().timestamp(), timestamp);
        assert_eq!(editor.date().unwrap().source().paragraphs, source);
        for (run_size, base_size, screen_y, paragraphs) in [
            (11.0, 11.0, 235.0, Some(2)),
            (11.0, 11.0, 252.0, Some(3)),
            (11.0, 11.0, 256.0, Some(3)),
            (11.0, 11.0, 260.0, None),
            (22.0, 11.0, 306.0, Some(3)),
            (22.0, 22.0, 306.0, Some(2)),
            (22.0, 22.0, 324.0, None),
        ] {
            let style = ExGuid {
                n: 900,
                ..Default::default()
            };
            let format = onestore::document::Format {
                font: Some("Calibri".into()),
                font_size: Some(run_size),
                ..Default::default()
            };
            let mut nodes = crate::document::TextDocument::new(vec![Paragraph::new(
                "Anchor0".into(),
                format.clone(),
            )])
            .unwrap()
            .nodes()
            .to_vec();
            nodes[0].style = Some(style);
            let outline = TextOutline::new(
                &mut engine,
                crate::document::TextDocument::from_nodes(nodes).unwrap(),
                72.0,
                [120.0, 90.0],
            )
            .unwrap();
            let id = outline.id;
            let source = outline.document().clone();
            let mut editor = CanvasEditor::from_text_outlines(
                vec![outline],
                BTreeMap::from([(
                    style,
                    onestore::page::Definition {
                        kind: onestore::document::Kind::Style {
                            name: Some("p".into()),
                        },
                        format: onestore::document::Format {
                            font_size: Some(base_size),
                            ..format
                        },
                    },
                )]),
                None,
            )
            .unwrap();
            let point = [
                (220.0 - 48.0) * 0.75 - 120.0,
                (screen_y - 83.0) * 0.75 - 90.0,
            ];
            assert_eq!(
                editor.select_below(&mut engine, id, point).unwrap(),
                paragraphs.is_some(),
                "run={run_size} base={base_size} y={screen_y}"
            );
            assert_eq!(editor.outlines()[0].document(), &source);
            if let Some(count) = paragraphs {
                editor.insert(&mut engine, "X").unwrap();
                assert_eq!(editor.active_outline().document().nodes().len(), count);
                assert_eq!(editor.active_outline().origin(), [120.0, 90.0]);
                assert_eq!(
                    editor
                        .active_outline()
                        .document()
                        .paragraphs()
                        .last()
                        .unwrap()
                        .text(),
                    "X"
                );
                assert!(
                    (editor
                        .active_outline()
                        .layouts()
                        .last()
                        .unwrap()
                        .1
                        .text
                        .height()
                        - base_size * (12.207_031 / 10.0))
                        .abs()
                        < 0.001
                );
            }
        }
    }

    #[test]
    #[ignore = "requires the native baseline section via CANVAS_TEST_SECTION"]
    fn native_title_flow() {
        use crate::document::TextDocument;
        use onestore::page::Image;
        let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let store = onestore::Store::parse(&bytes).unwrap();
        let index = onestore::RevisionIndex::parse(&store).unwrap();
        let document = onestore::document::Document::parse(&index).unwrap();
        let mut engine = TextEngine::default();
        let long = "A page title that is intentionally long enough to wrap across several lines at this window size while retaining its own date and time below the text";
        for (font_size, text, x, y, expected) in [
            (17.0, long, 120.0, 90.0, 116.18992),
            (17.0, long, 120.0, 110.0, 116.18992),
            (17.0, long, 120.0, 150.0, 150.0),
            (17.0, long, 120.0, 73.8, 73.8),
            (17.0, long, 120.0, 74.0, 116.18992),
            (17.0, long, 120.0, 40.0, 40.0),
            (17.0, long, 540.0, 90.0, 116.18992),
            (17.0, long, 550.0, 90.0, 90.0),
            (17.0, long, 600.0, 90.0, 90.0),
            (17.0, long, 0.0, 90.0, 116.18992),
            (17.0, long, -50.0, 90.0, 116.18992),
            (17.0, long, -100.0, 90.0, 90.0),
            (17.0, "Header\u{000b}Second", 120.0, 90.0, 95.43796),
            (17.0, "Header\u{000b}Second", 250.0, 90.0, 95.43796),
            (17.0, "Header\u{000b}Second", 540.0, 90.0, 95.43796),
            (17.0, "Header\u{000b}Second", 550.0, 90.0, 90.0),
            (11.0, long, 120.0, 60.0, 60.0),
            (11.0, long, 120.0, 70.0, 80.78945),
            (22.0, long, 120.0, 80.0, 80.0),
            (22.0, long, 120.0, 90.0, 134.5003),
        ] {
            let mut page = Page::from_document(&document, "Baseline anchors").unwrap();
            page.objects
                .retain(|object| matches!(object, PageObject::Title(_)));
            let PageObject::Title(title) = &mut page.objects[0] else {
                unreachable!()
            };
            let title = title
                .outlines
                .iter_mut()
                .find(|outline| outline.title)
                .unwrap();
            title.paragraphs[0].text_mut().unwrap().text = Paragraph::new(
                "Header".into(),
                onestore::document::Format {
                    font: Some("Calibri".into()),
                    font_size: Some(font_size),
                    ..Default::default()
                },
            );
            let body = TextOutline::new(
                &mut engine,
                TextDocument::new(vec![Paragraph::new("Anchor".into(), Default::default())])
                    .unwrap(),
                72.0,
                [x, y],
            )
            .unwrap()
            .snapshot();
            let id = body.id;
            page.objects.push(PageObject::Outline(body));
            let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
            editor.select_all().unwrap();
            editor.insert(&mut engine, text).unwrap();
            let position = |editor: &CanvasEditor| {
                editor
                    .outlines()
                    .iter()
                    .find(|outline| outline.id == id)
                    .unwrap()
                    .origin()
            };
            assert_eq!(position(&editor)[0], x);
            assert!(
                (position(&editor)[1] - expected).abs() < 0.002,
                "({x}, {y}) after {text}: {:?}, expected {expected}",
                position(&editor)
            );
            editor.select_all().unwrap();
            editor.insert(&mut engine, "Header").unwrap();
            assert!((position(&editor)[1] - expected).abs() < 0.002);
            editor.undo(&mut engine).unwrap();
            editor.undo(&mut engine).unwrap();
            assert_eq!(position(&editor), [x, y]);
            editor.redo(&mut engine).unwrap();
            assert!((position(&editor)[1] - expected).abs() < 0.002);
        }
        for trigger in [true, false] {
            let mut page = Page::from_document(&document, "Baseline anchors").unwrap();
            let image_bytes = page
                .objects
                .iter()
                .find_map(|object| match object {
                    PageObject::Image(image) => image.bytes.clone(),
                    _ => None,
                })
                .unwrap();
            page.objects
                .retain(|object| matches!(object, PageObject::Title(_)));
            for (x, y, background) in [
                (120.0, 90.0, !trigger),
                (600.0, 150.0, false),
                (600.0, 90.0, true),
                (120.0, 60.0, false),
            ] {
                page.objects.push(PageObject::Image(Image {
                    size: None,
                    id: onestore::page::text::new_id().unwrap(),
                    bytes: Some(image_bytes.clone()),
                    layout: Layout {
                        x: Some(x),
                        y: Some(y),
                        max_width: Some(72.0),
                        max_height: Some(80.0),
                        ..Default::default()
                    },
                    alt: None,
                    background,
                }));
            }
            let (mut scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
            scene.settle(Some(&editor), 1.0, Paper::WHITE);
            let images = |editor: &CanvasEditor| {
                let mut primitives = Vec::new();
                scene
                    .append_primitives_with::<SceneError>(
                        &mut primitives,
                        [0.0; 2],
                        Some(editor),
                        None,
                        Paper::WHITE,
                        |_, _, _| Ok(()),
                    )
                    .unwrap();
                primitives
                    .into_iter()
                    .filter_map(|primitive| match primitive {
                        Primitive::Image { image, rect } => Some((image.id(), rect)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            let before = images(&editor);
            editor.select_all().unwrap();
            editor.insert(&mut engine, "Header").unwrap();
            editor.select_all().unwrap();
            editor.insert(&mut engine, long).unwrap();
            let after = images(&editor);
            for (index, ((old_id, old), (new_id, new))) in before.iter().zip(&after).enumerate() {
                assert_eq!(old_id, new_id);
                let delta = if trigger && index < 2 { 26.18992 } else { 0.0 };
                assert_eq!([old[0], old[2]], [new[0], new[2]]);
                assert!((old[1] + delta - new[1]).abs() < 0.002);
                assert!((old[3] + delta - new[3]).abs() < 0.002);
                assert!(
                    scene
                        .content_bounds(&editor)
                        .any(|(bounds, _)| bounds == *new)
                );
            }
            editor.undo(&mut engine).unwrap();
            assert_eq!(images(&editor), before);
            editor.redo(&mut engine).unwrap();
            assert_eq!(images(&editor), after);
        }
    }

    #[test]
    fn title_exit_places_an_uncommitted_body_caret_below_the_date() {
        use crate::document::TextDocument;
        use onestore::page::Title;
        let mut engine = TextEngine::default();
        let fields =
            [("Header", 20.751953), ("Date\u{000b}Time", 12.207031)].map(|(text, line_spacing)| {
                let document = TextDocument::new(vec![Paragraph::new(
                    text.into(),
                    onestore::document::Format {
                        font: Some("Arial".into()),
                        font_size: Some(8.0),
                        line_spacing: Some(line_spacing),
                        ..Default::default()
                    },
                )])
                .unwrap();
                TextOutline::new(&mut engine, document, 468.0, [0.0; 2])
                    .unwrap()
                    .snapshot()
            });
        let mut fields = Vec::from(fields);
        fields[0].title = true;
        fields[0].min_width = Some(162.0);
        let title_id = fields[0].id;
        let page = Page {
            identity: None,
            created: None,
            title: "Header".into(),
            margin_origin: [36.0, 14.4],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects: vec![PageObject::Title(Title {
                date: None,
                id: ExGuid::default(),
                layout: Layout::default(),
                outlines: fields,
            })],
        };
        let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
        let original = editor.active_outline().document().clone();
        editor.leave_title(&mut engine).unwrap();
        assert_eq!(editor.caret_outline().unwrap().origin(), [36.0, 86.4]);
        assert_eq!(editor.outlines().len(), 1);
        assert!(!editor.undo(&mut engine).unwrap());
        editor.insert(&mut engine, "body").unwrap();
        let body_id = editor.active_outline().id;
        editor.focus_outline(title_id).unwrap();
        editor.leave_title(&mut engine).unwrap();
        assert_eq!(editor.active_outline().id, body_id);
        assert!(editor.caret_outline().is_none());
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.outlines().len(), 1);
        editor.focus_outline(title_id).unwrap();
        editor.select_all().unwrap();
        editor
            .insert(&mut engine, "Header\u{000b}Second\u{000b}Third")
            .unwrap();
        assert_eq!(editor.active_outline().document().nodes().len(), 1);
        assert_eq!(
            editor
                .active_outline()
                .layouts()
                .next()
                .unwrap()
                .1
                .text
                .lines()
                .count(),
            3
        );
        editor.leave_title(&mut engine).unwrap();
        assert_eq!(editor.caret_outline().unwrap().origin(), [36.0, 140.4]);
        assert_eq!(editor.outlines().len(), 1);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().id, title_id);
        assert_eq!(editor.active_outline().document(), &original);
    }

    #[test]
    fn reverse_hit_order_keeps_read_only_focus_indices() {
        let objects = [0.0, 20.0]
            .into_iter()
            .map(|position| {
                PageObject::Unsupported(onestore::page::Unsupported {
                    id: ExGuid {
                        guid: [position as u8; 16],
                        n: 1,
                    },
                    jcid: 0xdead,
                    layout: Layout {
                        x: Some(position),
                        y: Some(position),
                        max_width: Some(160.0),
                        max_height: Some(80.0),
                        ..Default::default()
                    },
                })
            })
            .collect();
        let page = Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects,
        };
        let scene = PageScene::new(page, &mut TextEngine::default()).unwrap();
        assert_eq!(
            scene.hit_test::<()>([30.0, 30.0], None, |_| unreachable!()),
            Some(SceneHit::ReadOnly(1))
        );
        assert_eq!(
            scene.hit_test::<()>([5.0, 5.0], None, |_| unreachable!()),
            Some(SceneHit::ReadOnly(0))
        );
        assert_eq!(
            scene.hit_test::<()>([300.0, 300.0], None, |_| unreachable!()),
            None
        );
    }

    #[test]
    fn unsupported_outlines_and_titles_leave_supported_outlines_editable() {
        use crate::document::TextDocument;
        use onestore::document::{Kind, Tag};
        use onestore::page::{Definition, Outline, Title};
        for title in [false, true] {
            let [tag, list] = [9, 10].map(|byte| ExGuid {
                guid: [byte; 16],
                n: 1,
            });
            let make_outline = |text: &str| {
                let document =
                    TextDocument::new(vec![Paragraph::new(text.into(), Default::default())])
                        .unwrap();
                Outline {
                    title: false,
                    min_width: None,
                    id: ExGuid {
                        guid: [u8::from(text == "Editable") + 1; 16],
                        n: 1,
                    },
                    layout: Layout {
                        max_width: Some(200.0),
                        ..Default::default()
                    },
                    indents: vec![18.0, 0.0],
                    paragraphs: document.nodes().to_vec(),
                    unsupported: Vec::new(),
                }
            };
            let mut numbered = make_outline("Numbered");
            numbered.paragraphs[0].lists.push(list);
            let unsupported = if title {
                PageObject::Title(Title {
                    date: None,
                    id: ExGuid::default(),
                    layout: Layout {
                        x: Some(10.0),
                        y: Some(20.0),
                        ..Default::default()
                    },
                    outlines: vec![numbered],
                })
            } else {
                PageObject::Outline(numbered)
            };
            // A tag symbol without artwork draws as a plain tag and stays editable.
            let mut editable = make_outline("Editable");
            editable.paragraphs[0].tags.push(Tag {
                definition: Some(tag),
                status: 1,
                action_type: None,
                created: None,
                completed: None,
                start: None,
                due: None,
                task_id: None,
                extra_set: 0,
            });
            let page = Page {
                identity: None,
                created: None,
                title: String::new(),
                margin_origin: [36.0, 14.0],
                color: None,
                rule_lines: None,
                objects: vec![unsupported, PageObject::Outline(editable)],
                definitions: BTreeMap::from([
                    (
                        tag,
                        Definition {
                            kind: Kind::TagDefinition {
                                shape: Some(999),
                                label: Some("Unknown icon".into()),
                                color: None,
                                highlight: None,
                                action_type: None,
                            },
                            format: Default::default(),
                        },
                    ),
                    (
                        // A numbering sequence OneNote 2010 does not offer.
                        list,
                        Definition {
                            kind: Kind::List {
                                font: None,
                                format: Some("\u{fffd}\u{63}.".into()),
                                restart: None,
                                bullet: None,
                            },
                            format: Default::default(),
                        },
                    ),
                ]),
            };
            let mut engine = TextEngine::default();
            let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
            assert_eq!(editor.outlines().len(), 1);
            let readonly = scene.read_only(Some(&editor)).next().unwrap();
            assert_eq!(scene.read_only(Some(&editor)).count(), 1);
            if title {
                assert_eq!(&readonly.rect()[..2], &[46.0, 34.0]);
            }
            editor.insert(&mut engine, "Still ").unwrap();
            assert_eq!(
                editor
                    .active_outline()
                    .document()
                    .paragraphs()
                    .next()
                    .unwrap()
                    .text(),
                "Still Editable"
            );
            editor.undo(&mut engine).unwrap();
            assert_eq!(
                editor
                    .active_outline()
                    .document()
                    .paragraphs()
                    .next()
                    .unwrap()
                    .text(),
                "Editable"
            );
        }
    }

    #[test]
    fn unsupported_sources_stay_owned_and_read_only_across_edit_and_undo() {
        use crate::document::TextDocument;
        use onestore::page::{Outline, Unsupported};
        let document = TextDocument::new(vec![Paragraph::new(
            "preserved text".into(),
            Default::default(),
        )])
        .unwrap();
        let original = document.nodes().to_vec();
        let unknown = Unsupported {
            id: ExGuid {
                guid: [8; 16],
                n: 1,
            },
            jcid: 0xdead,
            layout: Layout::default(),
        };
        let page = Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects: vec![
                PageObject::Outline(Outline {
                    title: false,
                    min_width: None,
                    id: ExGuid {
                        guid: [7; 16],
                        n: 1,
                    },
                    layout: Layout {
                        x: Some(50.0),
                        y: Some(70.0),
                        max_width: Some(200.0),
                        ..Default::default()
                    },
                    indents: vec![18.0, 0.0],
                    paragraphs: original.clone(),
                    unsupported: vec![unknown.clone()],
                }),
                PageObject::Image(Image {
                    size: None,
                    id: ExGuid::default(),
                    layout: Layout {
                        y: Some(150.0),
                        ..Default::default()
                    },
                    bytes: None,
                    alt: Some("missing diagram".into()),
                    background: false,
                }),
                PageObject::Unsupported(unknown.clone()),
            ],
        };
        let mut engine = TextEngine::default();
        let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
        assert_eq!(scene.read_only(Some(&editor)).count(), 3);
        assert!(editor.outlines().is_empty());
        assert!(editor.caret_outline().is_some());
        assert!(
            editor.active_outline().origin()[0]
                > scene
                    .read_only(Some(&editor))
                    .map(|object| object.rect()[2])
                    .fold(0.0_f32, f32::max)
        );
        let labels: Vec<_> = scene
            .read_only(Some(&editor))
            .map(|object| object.label.id())
            .collect();
        for edit in [true, false] {
            if edit {
                editor.insert(&mut engine, "new annotation").unwrap();
            } else {
                editor.undo(&mut engine).unwrap();
            }
            assert_eq!(editor.caret_outline().is_none(), edit);
            assert_eq!(editor.outlines().len(), usize::from(edit));
            let readonly: Vec<_> = scene.read_only(Some(&editor)).collect();
            let PageObject::Outline(outline) = &readonly[0].source else {
                panic!()
            };
            assert_eq!(outline.paragraphs, original);
            assert_eq!(
                outline.unsupported.as_slice(),
                std::slice::from_ref(&unknown)
            );
            let PageObject::Image(image) = &readonly[1].source else {
                panic!()
            };
            assert_eq!(image.alt.as_deref(), Some("missing diagram"));
            assert!(image.bytes.is_none());
            let PageObject::Unsupported(source) = &readonly[2].source else {
                panic!()
            };
            assert_eq!(source, &unknown);
            assert_eq!(
                readonly
                    .iter()
                    .map(|object| object.label.id())
                    .collect::<Vec<_>>(),
                labels
            );
            let primitives = paint_editor(&scene, &editor);
            assert_eq!(primitives.len(), 9);
            assert_eq!(
                primitives
                    .iter()
                    .filter(|p| matches!(p, Primitive::Text { .. }))
                    .count(),
                3
            );
        }
        assert_eq!(
            editor
                .active_outline()
                .document()
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            ""
        );
    }

    fn paint_editor<'a>(scene: &'a PageScene, editor: &'a CanvasEditor) -> Vec<Primitive<'a>> {
        let mut primitives = Vec::new();
        scene
            .append_primitives_with(
                &mut primitives,
                [0.0; 2],
                Some(editor),
                None,
                Paper::WHITE,
                |id, offset, primitives| {
                    let outline = editor
                        .outlines()
                        .iter()
                        .find(|outline| outline.id == id)
                        .ok_or(SceneError::MissingOutline)?;
                    for (_, paragraph) in outline.layouts() {
                        primitives.push(Primitive::Text {
                            clip: None,
                            text: &paragraph.text,
                            ink: Paper::WHITE.ink,
                            origin: [
                                outline.origin()[0] + paragraph.origin[0] + offset[0],
                                outline.origin()[1] + paragraph.origin[1] + offset[1],
                            ],
                        });
                    }
                    Ok::<_, SceneError>(())
                },
            )
            .unwrap();
        primitives
    }

    #[test]
    fn editable_outlines_keep_paint_order_and_images_across_edit_and_undo() {
        use crate::document::TextDocument;
        use onestore::page::Outline;
        use onestore::page::text::Paragraph;
        let outline = |x, text: &str| {
            let document =
                TextDocument::new(vec![Paragraph::new(text.into(), Default::default())]).unwrap();
            PageObject::Outline(Outline {
                title: false,
                min_width: None,
                id: ExGuid {
                    guid: [x as u8; 16],
                    n: 1,
                },
                layout: Layout {
                    x: Some(x),
                    max_width: Some(180.0),
                    width_set_by_user: Some(true),
                    ..Default::default()
                },
                indents: vec![18.0, 0.0],
                paragraphs: document.nodes().to_vec(),
                unsupported: Vec::new(),
            })
        };
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[17, 34, 51, 255])
                .unwrap();
        }
        let page = || Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects: vec![
                outline(2.0, "first"),
                PageObject::Image(Image {
                    size: None,
                    id: ExGuid::default(),
                    layout: Layout {
                        x: Some(5.0),
                        y: Some(7.0),
                        max_width: Some(40.0),
                        max_height: Some(30.0),
                        ..Default::default()
                    },
                    bytes: Some(Arc::from(bytes.clone())),
                    alt: None,
                    background: false,
                }),
                outline(50.0, "last"),
            ],
        };
        let mut engine = TextEngine::default();
        let mut reference = PageScene::new(page(), &mut engine).unwrap();
        let (mut scene, mut editor) = PageScene::from_page(page(), &mut engine).unwrap();
        reference.settle(None, 1.0, Paper::WHITE);
        scene.settle(Some(&editor), 1.0, Paper::WHITE);
        let mut reference_primitives = Vec::new();
        reference
            .append_primitives(&mut reference_primitives, [0.0; 2], Paper::WHITE)
            .unwrap();
        let image_id;
        {
            let actual = paint_editor(&scene, &editor);
            assert_eq!(actual.len(), 3);
            for (actual, expected) in actual.iter().zip(&reference_primitives) {
                match (actual, expected) {
                    (
                        Primitive::Text {
                            clip: None,
                            text: a,
                            origin: x,
                            ..
                        },
                        Primitive::Text {
                            clip: None,
                            text: b,
                            origin: y,
                            ..
                        },
                    ) => {
                        assert_eq!(x, y);
                        assert_eq!(
                            painted_layout(*a)
                                .lines()
                                .next()
                                .unwrap()
                                .0
                                .metrics()
                                .advance,
                            painted_layout(*b)
                                .lines()
                                .next()
                                .unwrap()
                                .0
                                .metrics()
                                .advance
                        );
                    }
                    (
                        Primitive::Image { image: a, rect: x },
                        Primitive::Image { image: b, rect: y },
                    ) => {
                        assert_eq!(x, y);
                        assert_eq!(a.pixels(), b.pixels());
                    }
                    _ => panic!("paint order changed"),
                }
            }
            let Primitive::Image { image, .. } = &actual[1] else {
                panic!()
            };
            image_id = image.id();
        }
        let original = editor.active_outline().document().clone();
        editor.insert(&mut engine, "new words ").unwrap();
        {
            let actual = paint_editor(&scene, &editor);
            let Primitive::Text { text, .. } = &actual[0] else {
                panic!()
            };
            let Primitive::Text { text: before, .. } = &reference_primitives[0] else {
                panic!()
            };
            assert!(
                painted_layout(*text)
                    .lines()
                    .next()
                    .unwrap()
                    .0
                    .metrics()
                    .advance
                    > painted_layout(*before)
                        .lines()
                        .next()
                        .unwrap()
                        .0
                        .metrics()
                        .advance
            );
            let Primitive::Image { image, .. } = &actual[1] else {
                panic!()
            };
            assert_eq!(image.id(), image_id);
        }
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document(), &original);
        let actual = paint_editor(&scene, &editor);
        let Primitive::Text { text, .. } = &actual[0] else {
            panic!()
        };
        let Primitive::Text { text: before, .. } = &reference_primitives[0] else {
            panic!()
        };
        assert_eq!(
            painted_layout(*text)
                .lines()
                .next()
                .unwrap()
                .0
                .metrics()
                .advance,
            painted_layout(*before)
                .lines()
                .next()
                .unwrap()
                .0
                .metrics()
                .advance
        );
        assert!(matches!(
            scene.append_primitives(&mut Vec::new(), [0.0; 2], Paper::WHITE),
            Err(SceneError::MissingOutline)
        ));
    }

    #[test]
    fn ink_draws_every_stroke_and_bounds_include_the_pen() {
        use onestore::page::{Ink, InkStroke};
        let stroke = |points: Vec<[f32; 2]>, color| InkStroke {
            id: ExGuid::default(),
            points,
            width: 2.0,
            height: 2.0,
            color,
            transparency: Some(51),
            pen_tip: None,
        };
        let ink = Ink {
            id: ExGuid::default(),
            layout: Layout::default(),
            // A straight vertical line has no width of its own.
            strokes: vec![stroke(vec![[10.0, 20.0], [10.0, 20.1], [10.0, 60.0]], None)],
            groups: vec![Ink {
                id: ExGuid::default(),
                layout: Layout::default(),
                strokes: vec![stroke(vec![[40.0, 30.0]], Some(0x0000ff))],
                groups: Vec::new(),
            }],
        };
        let page = Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects: vec![PageObject::Ink(ink.clone())],
        };
        let mut engine = TextEngine::default();
        let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
        assert!(
            scene
                .content_bounds(&editor)
                .any(|(bounds, _)| bounds == [9.0, 19.0, 41.0, 61.0])
        );
        let mut primitives = Vec::new();
        scene
            .append_primitives_with::<SceneError>(
                &mut primitives,
                [5.0, 0.0],
                Some(&editor),
                None,
                Paper::WHITE,
                |_, _, _| Ok(()),
            )
            .unwrap();
        let segments: Vec<_> = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Segment {
                    from, to, color, ..
                } => Some((*from, *to, *color)),
                _ => None,
            })
            .collect();
        let red = colorref(0x0000ff);
        assert_eq!(
            segments,
            [
                ([15.0, 20.0], [15.0, 60.0], [0.0, 0.0, 0.0, 0.8]),
                ([45.0, 30.0], [45.0, 30.0], [red[0], red[1], red[2], 0.8]),
            ]
        );
        assert_eq!(editor.page().unwrap().objects, [PageObject::Ink(ink)]);
    }

    #[test]
    fn scene_owns_decoded_images_and_reuses_identity_across_translated_frames() {
        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoded, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[255, 0, 0, 255])
                .unwrap();
        }
        let page = |width| Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects: vec![PageObject::Image(Image {
                size: None,
                id: ExGuid::default(),
                layout: Layout {
                    x: Some(2.0),
                    y: Some(5.0),
                    max_width: Some(width),
                    max_height: Some(4.0),
                    ..Default::default()
                },
                bytes: Some(Arc::from(encoded.clone())),
                alt: None,
                background: false,
            })],
        };
        let mut engine = TextEngine::default();
        let mut scene = PageScene::new(page(3.0), &mut engine).unwrap();
        scene.settle(None, 1.0, Paper::WHITE);
        for invalid in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert!(matches!(
                PageScene::new(page(invalid), &mut engine),
                Err(SceneError::InvalidGeometry)
            ));
        }
        let mut primitives = Vec::new();
        scene
            .append_primitives(&mut primitives, [0.0; 2], Paper::WHITE)
            .unwrap();
        scene
            .append_primitives(&mut primitives, [10.0, -20.0], Paper::WHITE)
            .unwrap();
        let [
            Primitive::Image {
                image: first,
                rect: a,
            },
            Primitive::Image {
                image: second,
                rect: b,
            },
        ] = primitives.as_slice()
        else {
            panic!()
        };
        assert_eq!(*a, [2.0, 5.0, 5.0, 9.0]);
        assert_eq!(*b, [12.0, -15.0, 15.0, -11.0]);
        assert_eq!(first.id(), second.id());
        assert_eq!(first.pixels(), [255, 0, 0, 255]);
    }

    #[test]
    fn foreground_pictures_take_hits_and_draw_at_their_moving_preview() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[17, 34, 51, 255])
                .unwrap();
        }
        let image = |x: f32, background| Image {
            size: None,
            id: onestore::page::text::new_id().unwrap(),
            layout: Layout {
                x: Some(x),
                y: Some(0.0),
                max_width: Some(100.0),
                max_height: Some(100.0),
                ..Default::default()
            },
            bytes: Some(Arc::from(bytes.clone())),
            alt: None,
            background,
        };
        let [background, picture] = [image(0.0, true), image(50.0, false)];
        let id = picture.id;
        let page = Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects: vec![PageObject::Image(background), PageObject::Image(picture)],
        };
        let mut engine = TextEngine::default();
        let (mut scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
        scene.settle(Some(&editor), 1.0, Paper::WHITE);
        let hit = |x| scene.hit_test::<()>([x, 10.0], Some(&editor), |_| None);
        assert_eq!(hit(60.0), Some(SceneHit::Image(id)));
        assert_eq!(hit(10.0), None);
        let mut primitives = Vec::new();
        scene
            .append_primitives_with::<SceneError>(
                &mut primitives,
                [0.0; 2],
                Some(&editor),
                Some((id, [70.0, 5.0, 120.0, 55.0])),
                Paper::WHITE,
                |_, _, _| Ok(()),
            )
            .unwrap();
        let rects: Vec<_> = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(rects, [[0.0, 0.0, 100.0, 100.0], [70.0, 5.0, 120.0, 55.0]]);
    }

    #[test]
    fn pictures_past_the_budget_open_decode_in_view_and_keep_their_bytes() {
        let side = 1024;
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, side, side);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[40, 90, 160, 255].repeat((side * side) as usize))
                .unwrap();
        }
        let bytes: Arc<[u8]> = Arc::from(bytes);
        let pictures: Vec<_> = (0..20)
            .map(|index| Image {
                size: None,
                id: onestore::page::text::new_id().unwrap(),
                layout: Layout {
                    x: Some(0.0),
                    y: Some(index as f32 * 600.0),
                    max_width: Some(512.0),
                    max_height: Some(512.0),
                    ..Default::default()
                },
                bytes: Some(Arc::clone(&bytes)),
                alt: None,
                background: false,
            })
            .collect();
        assert!(20 * u64::from(side * side) * 4 > draw::MAX_IMAGE_BYTES);
        let page = Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            color: None,
            rule_lines: None,
            definitions: BTreeMap::new(),
            objects: pictures.iter().cloned().map(PageObject::Image).collect(),
        };
        let mut engine = TextEngine::default();
        let (mut scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
        let kept = |scene: &PageScene| -> u64 {
            let shown = scene.pictures.values().filter_map(Picture::image);
            shown.map(|image| image.pixels().len() as u64).sum()
        };
        let shown = |scene: &PageScene, index: usize| {
            scene.image(pictures[index].id).map(|image| image.size())
        };
        let settle = |scene: &mut PageScene, view: [f32; 4]| {
            let give_up = Instant::now() + Duration::from_secs(20);
            while !scene.update_pictures(Some(&editor), view, 2.0, Paper::WHITE, Waker::noop()) {
                assert!(
                    Instant::now() < give_up,
                    "the pictures in view never landed"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(kept(scene) <= picture::BUDGET, "{}", kept(scene));
        };
        for top in (0..20).map(|index| index as f32 * 600.0) {
            let view = [0.0, top, 600.0, top + 1200.0];
            settle(&mut scene, view);
            let index = (top / 600.0) as usize;
            assert_eq!(shown(&scene, index), Some([side; 2]));
            let mut primitives = Vec::new();
            scene
                .append_primitives_with::<SceneError>(
                    &mut primitives,
                    [0.0; 2],
                    Some(&editor),
                    None,
                    Paper::WHITE,
                    |_, _, _| Ok(()),
                )
                .unwrap();
            assert!(primitives.iter().any(|primitive| matches!(
                primitive,
                Primitive::Image { rect, .. } if rect[1] == top
            )));
        }
        assert_eq!(shown(&scene, 0), None, "the first picture was let go");
        // Every picture in view at once shrinks to fit rather than failing the frame.
        settle(&mut scene, [0.0, 0.0, 600.0, 12_000.0]);
        for index in 0..20 {
            let [width, height] = shown(&scene, index).unwrap();
            assert!(width < side && width == height, "{width}×{height}");
        }
        for object in editor.page().unwrap().objects {
            let PageObject::Image(picture) = object else {
                panic!()
            };
            assert!(Arc::ptr_eq(picture.bytes.as_ref().unwrap(), &bytes));
        }
    }
}
