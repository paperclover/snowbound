use crate::{Primitive, RasterImage, RenderError, colorref};
use one_canvas::{
    editor::{CanvasEditor, EditorError, TextOutline},
    layout::{LayoutError, TextEngine, TextLayout},
    outline::OutlineLayout,
    page::{Page, PageObject},
    text::Paragraph,
};
use std::fmt;

/// Retained drawing data in the source page's coordinate system.
pub struct PageScene {
    objects: Vec<Content>,
}

#[derive(Debug, PartialEq)]
pub enum SceneHit<T> {
    Outline(T),
    ReadOnly(usize),
}

enum Content {
    Outline {
        origin: [f32; 2],
        layout: OutlineLayout,
    },
    Image {
        origin: [f32; 2],
        image: RasterImage,
        size: [f32; 2],
    },
    Editable(onestore::ExGuid),
    ReadOnly(Box<ReadOnlyObject>),
}

pub struct ReadOnlyObject {
    pub source: PageObject,
    pub rect: [f32; 4],
    pub message: &'static str,
    label: TextLayout,
}

impl ReadOnlyObject {
    fn new(
        source: PageObject,
        margin: [f32; 2],
        message: &'static str,
        engine: &mut TextEngine,
    ) -> Result<Box<Self>, SceneError> {
        let (layout, offset) = match &source {
            PageObject::Outline(v) => (&v.layout, [0.0; 2]),
            PageObject::Title(v) => (&v.layout, margin),
            PageObject::Image(v) => (&v.layout, [0.0; 2]),
            PageObject::Unsupported(v) => (&v.layout, [0.0; 2]),
        };
        let x = layout.x.unwrap_or(0.0) + offset[0];
        let y = layout.y.unwrap_or(0.0) + offset[1];
        let width = layout.max_width.unwrap_or(160.0);
        let height = layout.max_height.unwrap_or(42.0);
        if [x, y, width, height].iter().any(|v| !v.is_finite()) || width <= 0.0 || height <= 0.0 {
            return Err(SceneError::InvalidGeometry);
        }
        let width = width.max(160.0);
        let label = engine.layout(
            &Paragraph::new(
                message.into(),
                onestore::document::Format {
                    font: Some("Arial".into()),
                    font_size: Some(11.0),
                    color: Some(0x005d554e),
                    ..Default::default()
                },
            ),
            width - 16.0,
        )?;
        let height = height.max(label.height() + 16.0);
        let rect = [x, y, x + width, y + height];
        if rect.iter().any(|v| !v.is_finite()) {
            return Err(SceneError::InvalidGeometry);
        }
        Ok(Box::new(Self {
            source,
            rect,
            message,
            label,
        }))
    }
}

#[derive(Debug)]
pub enum SceneError {
    Layout(LayoutError),
    Editor(EditorError),
    MissingOutline,
    Image(RenderError),
    MissingImage,
    InvalidGeometry,
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Layout(error) => error.fmt(f),
            Self::Editor(error) => error.fmt(f),
            Self::MissingOutline => f.write_str("An editable outline is missing from this page."),
            Self::Image(error) => write!(f, "Image rendering failed: {error:?}"),
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

impl PageScene {
    pub fn new(mut page: Page, engine: &mut TextEngine) -> Result<Self, SceneError> {
        Self::build(&mut page, engine, false).map(|(scene, _)| scene)
    }

    pub fn from_page(
        mut page: Page,
        engine: &mut TextEngine,
    ) -> Result<(Self, CanvasEditor), SceneError> {
        let (scene, mut outlines) = Self::build(&mut page, engine, true)?;
        if outlines.is_empty() {
            let x = scene
                .read_only()
                .map(|object| object.rect[2])
                .fold(0.0_f32, f32::max);
            outlines.push(
                TextOutline::new(
                    engine,
                    one_canvas::document::TextDocument::new(vec![Paragraph::new(
                        String::new(),
                        Default::default(),
                    )])
                    .map_err(EditorError::from)
                    .map_err(SceneError::Editor)?,
                    240.0,
                    [if x > 0.0 { x + 24.0 } else { 0.0 }, 0.0],
                )
                .map_err(SceneError::Editor)?,
            );
        }
        let editor = CanvasEditor::from_text_outlines(outlines, page.definitions)
            .map_err(SceneError::Editor)?;
        Ok((scene, editor))
    }

    fn build(
        page: &mut Page,
        engine: &mut TextEngine,
        editable: bool,
    ) -> Result<(Self, Vec<TextOutline>), SceneError> {
        if page.margin_origin.iter().any(|v| !v.is_finite()) {
            return Err(SceneError::InvalidGeometry);
        }
        let mut objects = Vec::new();
        let mut outlines = Vec::new();
        let mut image_bytes = 0_u64;
        for object in std::mem::take(&mut page.objects) {
            match &object {
                PageObject::Outline(outline) => {
                    let origin = [
                        outline.layout.x.unwrap_or(0.0),
                        outline.layout.y.unwrap_or(0.0),
                    ];
                    if origin.iter().any(|v| !v.is_finite()) {
                        return Err(SceneError::InvalidGeometry);
                    }
                    if editable {
                        match TextOutline::from_outline(engine, outline, &page.definitions) {
                            Ok(outline) => {
                                objects.push(Content::Editable(outline.id));
                                outlines.push(outline);
                            }
                            Err(EditorError::Layout(LayoutError::UnsupportedContent)) => objects
                                .push(Content::ReadOnly(ReadOnlyObject::new(
                                    object,
                                    page.margin_origin,
                                    "Unsupported content\nRead-only",
                                    engine,
                                )?)),
                            Err(error) => return Err(SceneError::Editor(error)),
                        }
                    } else {
                        match outline.layout(engine, &page.definitions) {
                            Ok(layout) => objects.push(Content::Outline { origin, layout }),
                            Err(LayoutError::UnsupportedContent) => {
                                objects.push(Content::ReadOnly(ReadOnlyObject::new(
                                    object,
                                    page.margin_origin,
                                    "Unsupported content\nRead-only",
                                    engine,
                                )?))
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                PageObject::Title(title) => {
                    let layouts = match title.layout(engine, &page.definitions) {
                        Ok(layouts) => layouts,
                        Err(LayoutError::UnsupportedContent) => {
                            objects.push(Content::ReadOnly(ReadOnlyObject::new(
                                object,
                                page.margin_origin,
                                "Unsupported content\nRead-only",
                                engine,
                            )?));
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    for (origin, layout) in layouts {
                        let origin = [
                            title.layout.x.unwrap_or(0.0) + origin[0] + page.margin_origin[0],
                            title.layout.y.unwrap_or(0.0) + origin[1] + page.margin_origin[1],
                        ];
                        if origin.iter().any(|v| !v.is_finite()) {
                            return Err(SceneError::InvalidGeometry);
                        }
                        objects.push(Content::Outline { origin, layout });
                    }
                }
                PageObject::Image(source) => {
                    if source.bytes.is_none()
                        || source.layout.max_width.is_none()
                        || source.layout.max_height.is_none()
                    {
                        objects.push(Content::ReadOnly(ReadOnlyObject::new(
                            object,
                            page.margin_origin,
                            "Image unavailable\nRead-only",
                            engine,
                        )?));
                        continue;
                    }
                    let origin = [
                        source.layout.x.unwrap_or(0.0),
                        source.layout.y.unwrap_or(0.0),
                    ];
                    let size = [
                        source.layout.max_width.ok_or(SceneError::MissingImage)?,
                        source.layout.max_height.ok_or(SceneError::MissingImage)?,
                    ];
                    if origin.iter().any(|v| !v.is_finite())
                        || size.iter().any(|v| !v.is_finite() || *v <= 0.0)
                    {
                        return Err(SceneError::InvalidGeometry);
                    }
                    let image = RasterImage::decode(
                        source.bytes.as_deref().ok_or(SceneError::MissingImage)?,
                    )
                    .map_err(SceneError::Image)?;
                    image_bytes += image.pixels.as_ref().len() as u64;
                    if image_bytes > crate::MAX_IMAGE_BYTES {
                        return Err(SceneError::Image(RenderError::ImageBudget));
                    }
                    objects.push(Content::Image {
                        origin,
                        image,
                        size,
                    });
                }
                PageObject::Unsupported(_) => objects.push(Content::ReadOnly(ReadOnlyObject::new(
                    object,
                    page.margin_origin,
                    "Unsupported content\nRead-only",
                    engine,
                )?)),
            }
        }
        Ok((Self { objects }, outlines))
    }

    pub fn read_only(&self) -> impl Iterator<Item = &ReadOnlyObject> {
        self.objects.iter().filter_map(|object| match object {
            Content::ReadOnly(object) => Some(object.as_ref()),
            _ => None,
        })
    }

    pub fn contains_outline(&self, id: onestore::ExGuid) -> bool {
        self.objects
            .iter()
            .any(|object| matches!(object, Content::Editable(candidate) if *candidate == id))
    }

    /// The point and outline hit callback use scene coordinates.
    pub fn hit_test<T>(
        &self,
        point: [f32; 2],
        mut outline: impl FnMut(onestore::ExGuid) -> Option<T>,
    ) -> Option<SceneHit<T>> {
        let mut readonly = self.read_only().count();
        for object in self.objects.iter().rev() {
            match object {
                Content::Editable(id) => {
                    if let Some(hit) = outline(*id) {
                        return Some(SceneHit::Outline(hit));
                    }
                }
                Content::ReadOnly(object) => {
                    readonly -= 1;
                    let [x0, y0, x1, y1] = object.rect;
                    if (x0..=x1).contains(&point[0]) && (y0..=y1).contains(&point[1]) {
                        return Some(SceneHit::ReadOnly(readonly));
                    }
                }
                Content::Outline { .. } | Content::Image { .. } => {}
            }
        }
        None
    }

    /// The offset is in document points.
    pub fn append_primitives<'a>(
        &'a self,
        primitives: &mut Vec<Primitive<'a>>,
        offset: [f32; 2],
    ) -> Result<(), SceneError> {
        self.append_primitives_with(primitives, offset, |_, _, _| {
            Err(SceneError::MissingOutline)
        })
    }

    pub fn append_primitives_with<'a, E>(
        &'a self,
        primitives: &mut Vec<Primitive<'a>>,
        offset: [f32; 2],
        mut outline: impl FnMut(onestore::ExGuid, [f32; 2], &mut Vec<Primitive<'a>>) -> Result<(), E>,
    ) -> Result<(), E> {
        for content in &self.objects {
            let origin = match content {
                Content::Outline { origin, .. } | Content::Image { origin, .. } => origin,
                Content::ReadOnly(object) => {
                    let [x0, y0, x1, y1] = object.rect;
                    let rect = [
                        x0 + offset[0],
                        y0 + offset[1],
                        x1 + offset[0],
                        y1 + offset[1],
                    ];
                    primitives.push(Primitive::Rect {
                        rect,
                        color: colorref(0x00e4ddd6),
                    });
                    primitives.push(Primitive::Rect {
                        rect: [rect[0] + 1.0, rect[1] + 1.0, rect[2] - 1.0, rect[3] - 1.0],
                        color: colorref(0x00faf7f3),
                    });
                    primitives.push(Primitive::Text {
                        layout: &object.label,
                        origin: [rect[0] + 8.0, rect[1] + 8.0],
                    });
                    continue;
                }
                Content::Editable(id) => {
                    outline(*id, offset, primitives)?;
                    continue;
                }
            };
            let object_origin = [origin[0] + offset[0], origin[1] + offset[1]];
            match content {
                Content::Image { image, size, .. } => primitives.push(Primitive::Image {
                    image,
                    rect: [
                        object_origin[0],
                        object_origin[1],
                        object_origin[0] + size[0],
                        object_origin[1] + size[1],
                    ],
                }),
                Content::Outline {
                    layout: outline, ..
                } => {
                    for paragraph in &outline.paragraphs {
                        let origin = [
                            object_origin[0] + paragraph.origin[0],
                            object_origin[1] + paragraph.origin[1],
                        ];
                        for (rect, color) in paragraph.text.backgrounds() {
                            primitives.push(Primitive::Rect {
                                rect: [
                                    rect.x0 as f32 + origin[0],
                                    rect.y0 as f32 + origin[1],
                                    rect.x1 as f32 + origin[0],
                                    rect.y1 as f32 + origin[1],
                                ],
                                color: colorref(color),
                            });
                        }
                        primitives.push(Primitive::Text {
                            layout: &paragraph.text,
                            origin,
                        });
                        for (layout, origin) in &paragraph.markers {
                            primitives.push(Primitive::Text {
                                layout,
                                origin: [
                                    object_origin[0] + origin[0],
                                    object_origin[1] + (origin[1] + paragraph.origin[1]),
                                ],
                            });
                        }
                        for tag in &paragraph.tags {
                            primitives.push(Primitive::Tag {
                                tag,
                                origin: [object_origin[0], object_origin[1] + paragraph.origin[1]],
                            });
                        }
                    }
                }
                Content::Editable(_) | Content::ReadOnly(_) => unreachable!(),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use one_canvas::page::Image;
    use onestore::{ExGuid, document::Layout};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn reverse_hit_order_keeps_read_only_focus_indices() {
        let objects = [0.0, 20.0]
            .into_iter()
            .map(|position| {
                PageObject::Unsupported(one_canvas::page::Unsupported {
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
            title: String::new(),
            margin_origin: [0.0; 2],
            definitions: BTreeMap::new(),
            objects,
        };
        let scene = PageScene::new(page, &mut TextEngine::default()).unwrap();
        assert_eq!(
            scene.hit_test::<()>([30.0, 30.0], |_| unreachable!()),
            Some(SceneHit::ReadOnly(1))
        );
        assert_eq!(
            scene.hit_test::<()>([5.0, 5.0], |_| unreachable!()),
            Some(SceneHit::ReadOnly(0))
        );
        assert_eq!(
            scene.hit_test::<()>([300.0, 300.0], |_| unreachable!()),
            None
        );
    }

    #[test]
    fn unsupported_tags_and_titles_leave_supported_outlines_editable() {
        use one_canvas::{
            document::TextDocument,
            page::{Definition, Outline, Title},
        };
        use onestore::document::{Kind, Tag};
        for title in [false, true] {
            let definition = ExGuid {
                guid: [9; 16],
                n: 1,
            };
            let make_outline = |text: &str| {
                let document =
                    TextDocument::new(vec![Paragraph::new(text.into(), Default::default())])
                        .unwrap();
                Outline {
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
            let mut tagged = make_outline("Tagged");
            tagged.paragraphs[0].tags.push(Tag {
                definition: Some(definition),
                status: 1,
                action_type: None,
                created: None,
                completed: None,
                start: None,
                due: None,
                task_id: None,
                extra_set: 0,
            });
            let unsupported = if title {
                PageObject::Title(Title {
                    id: ExGuid::default(),
                    layout: Layout {
                        x: Some(10.0),
                        y: Some(20.0),
                        ..Default::default()
                    },
                    outlines: vec![tagged],
                })
            } else {
                PageObject::Outline(tagged)
            };
            let page = Page {
                title: String::new(),
                margin_origin: [36.0, 14.0],
                objects: vec![unsupported, PageObject::Outline(make_outline("Editable"))],
                definitions: BTreeMap::from([(
                    definition,
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
                )]),
            };
            let mut engine = TextEngine::default();
            let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
            assert_eq!(editor.outlines().len(), 1);
            let readonly = scene.read_only().next().unwrap();
            assert_eq!(scene.read_only().count(), 1);
            if title {
                assert_eq!(&readonly.rect[..2], &[46.0, 34.0]);
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
        use one_canvas::{
            document::TextDocument,
            page::{Outline, Unsupported},
        };
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
            title: String::new(),
            margin_origin: [0.0; 2],
            definitions: BTreeMap::new(),
            objects: vec![
                PageObject::Outline(Outline {
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
        assert_eq!(scene.read_only().count(), 3);
        assert!(
            editor.active_outline().origin()[0]
                > scene
                    .read_only()
                    .map(|object| object.rect[2])
                    .fold(0.0_f32, f32::max)
        );
        let labels: Vec<_> = scene.read_only().map(|object| object.label.id()).collect();
        for edit in [true, false] {
            if edit {
                editor.insert(&mut engine, "new annotation").unwrap();
            } else {
                editor.undo(&mut engine).unwrap();
            }
            let readonly: Vec<_> = scene.read_only().collect();
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
            .append_primitives_with(&mut primitives, [0.0; 2], |id, offset, primitives| {
                let outline = editor
                    .outlines()
                    .iter()
                    .find(|outline| outline.id == id)
                    .ok_or(SceneError::MissingOutline)?;
                for (_, paragraph) in outline.layouts() {
                    primitives.push(Primitive::Text {
                        layout: &paragraph.text,
                        origin: [
                            outline.origin()[0] + paragraph.origin[0] + offset[0],
                            outline.origin()[1] + paragraph.origin[1] + offset[1],
                        ],
                    });
                }
                Ok::<_, SceneError>(())
            })
            .unwrap();
        primitives
    }

    #[test]
    fn editable_outlines_keep_paint_order_and_images_across_edit_and_undo() {
        use one_canvas::{document::TextDocument, page::Outline, text::Paragraph};
        let outline = |x, text: &str| {
            let document =
                TextDocument::new(vec![Paragraph::new(text.into(), Default::default())]).unwrap();
            PageObject::Outline(Outline {
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
            title: String::new(),
            margin_origin: [0.0; 2],
            definitions: BTreeMap::new(),
            objects: vec![
                outline(2.0, "first"),
                PageObject::Image(Image {
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
        let reference = PageScene::new(page(), &mut engine).unwrap();
        let (scene, mut editor) = PageScene::from_page(page(), &mut engine).unwrap();
        let mut reference_primitives = Vec::new();
        reference
            .append_primitives(&mut reference_primitives, [0.0; 2])
            .unwrap();
        let image_id;
        {
            let actual = paint_editor(&scene, &editor);
            assert_eq!(actual.len(), 3);
            for (actual, expected) in actual.iter().zip(&reference_primitives) {
                match (actual, expected) {
                    (
                        Primitive::Text {
                            layout: a,
                            origin: x,
                        },
                        Primitive::Text {
                            layout: b,
                            origin: y,
                        },
                    ) => {
                        assert_eq!(x, y);
                        assert_eq!(
                            a.lines().next().unwrap().0.metrics().advance,
                            b.lines().next().unwrap().0.metrics().advance
                        );
                    }
                    (
                        Primitive::Image { image: a, rect: x },
                        Primitive::Image { image: b, rect: y },
                    ) => {
                        assert_eq!(x, y);
                        assert_eq!(a.pixels.as_ref(), b.pixels.as_ref());
                    }
                    _ => panic!("paint order changed"),
                }
            }
            let Primitive::Image { image, .. } = &actual[1] else {
                panic!()
            };
            image_id = image.pixels.id();
        }
        let original = editor.active_outline().document().clone();
        editor.insert(&mut engine, "new words ").unwrap();
        {
            let actual = paint_editor(&scene, &editor);
            let Primitive::Text { layout, .. } = &actual[0] else {
                panic!()
            };
            let Primitive::Text { layout: before, .. } = &reference_primitives[0] else {
                panic!()
            };
            assert!(
                layout.lines().next().unwrap().0.metrics().advance
                    > before.lines().next().unwrap().0.metrics().advance
            );
            let Primitive::Image { image, .. } = &actual[1] else {
                panic!()
            };
            assert_eq!(image.pixels.id(), image_id);
        }
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document(), &original);
        let actual = paint_editor(&scene, &editor);
        let Primitive::Text { layout, .. } = &actual[0] else {
            panic!()
        };
        let Primitive::Text { layout: before, .. } = &reference_primitives[0] else {
            panic!()
        };
        assert_eq!(
            layout.lines().next().unwrap().0.metrics().advance,
            before.lines().next().unwrap().0.metrics().advance
        );
        assert!(matches!(
            scene.append_primitives(&mut Vec::new(), [0.0; 2]),
            Err(SceneError::MissingOutline)
        ));
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
            title: String::new(),
            margin_origin: [0.0; 2],
            definitions: BTreeMap::new(),
            objects: vec![PageObject::Image(Image {
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
        let scene = PageScene::new(page(3.0), &mut engine).unwrap();
        for invalid in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert!(matches!(
                PageScene::new(page(invalid), &mut engine),
                Err(SceneError::InvalidGeometry)
            ));
        }
        let mut primitives = Vec::new();
        scene.append_primitives(&mut primitives, [0.0; 2]).unwrap();
        scene
            .append_primitives(&mut primitives, [10.0, -20.0])
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
        assert_eq!(first.pixels.id(), second.pixels.id());
        assert_eq!(first.pixels.as_ref(), [255, 0, 0, 255]);
    }
}
