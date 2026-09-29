use super::{EditError, EditorError, TextOutline};
use crate::{
    date::PageDate,
    layout::{LayoutError, TextEngine, TextLayout},
    outline::{Arrange, OutlineLayout},
};
use onestore::page::text::Paragraph;
use onestore::page::{Attachment, Image, Ink, Outline, Page, PageObject};
use std::collections::BTreeMap;

/// A title object's own state, plus the child origins `build` replaces with page coordinates.
pub(crate) struct TitleArea {
    pub id: onestore::ExGuid,
    pub date: Option<onestore::ExGuid>,
    pub layout: onestore::document::Layout,
    pub origins: BTreeMap<onestore::ExGuid, [Option<f32>; 2]>,
}

pub(crate) struct Import {
    pub objects: Vec<Content>,
    pub outlines: Vec<TextOutline>,
    pub date: Option<PageDate>,
    pub areas: Vec<TitleArea>,
}

pub(crate) enum Content {
    Date {
        below_title: Option<onestore::ExGuid>,
    },
    Outline {
        source: Outline,
        layout: OutlineLayout,
        below_title: Option<onestore::ExGuid>,
    },
    Image(Image),
    /// A file on the page, laid out relative to its position.
    File {
        source: Attachment,
        layout: Box<crate::outline::ObjectLayout>,
    },
    Ink(Ink),
    Editable(onestore::ExGuid),
    ReadOnly(Box<ReadOnlyObject>),
}

pub struct ReadOnlyObject {
    pub source: PageObject,
    pub message: &'static str,
    pub(crate) label: TextLayout,
    /// Title coordinates are stored relative to the page margin; nothing else is offset.
    offset: [f32; 2],
}

impl Content {
    /// A picture drawn as a placeholder because it has no data, size or decodable pixels.
    pub(crate) fn unavailable(
        source: PageObject,
        engine: &mut TextEngine,
    ) -> Result<Self, EditorError> {
        Ok(Self::ReadOnly(ReadOnlyObject::new(
            source,
            [0.0; 2],
            "Image unavailable\nRead-only",
            engine,
        )?))
    }

    /// The page-level picture shown, drawn or as a placeholder.
    pub(super) fn picture(&self) -> Option<&Image> {
        match self {
            Self::Image(image) => Some(image),
            Self::ReadOnly(object) => match &object.source {
                PageObject::Image(image) => Some(image),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn layout(&self) -> Option<(onestore::ExGuid, &onestore::document::Layout)> {
        match self {
            Self::Outline { source, .. } => Some((source.id, &source.layout)),
            Self::Image(source) => Some((source.id, &source.layout)),
            Self::File { source, .. } => Some((source.id, &source.layout)),
            Self::Ink(source) => Some((source.id, &source.layout)),
            Self::ReadOnly(object) => Some((object.source.id(), object.source.layout())),
            Self::Date { .. } | Self::Editable(_) => None,
        }
    }

    /// A file on the page, where its column's `[x0, y0, x1, y1]` lie in page points.
    pub(crate) fn file(&self) -> Option<(&Attachment, [f32; 4])> {
        let Self::File { source, layout } = self else {
            return None;
        };
        let [x, y] = [source.layout.x, source.layout.y].map(|v| v.unwrap_or(0.0));
        let [x0, y0, x1, y1] = layout.bounds();
        Some((source, [x + x0, y + y0, x + x1, y + y1]))
    }

    pub(super) fn layout_mut(
        &mut self,
    ) -> Option<(onestore::ExGuid, &mut onestore::document::Layout)> {
        match self {
            Self::Outline { source, .. } => Some((source.id, &mut source.layout)),
            Self::Image(source) => Some((source.id, &mut source.layout)),
            Self::File { source, .. } => Some((source.id, &mut source.layout)),
            Self::Ink(source) => Some((source.id, &mut source.layout)),
            Self::ReadOnly(object) => Some((object.source.id(), object.source.layout_mut())),
            Self::Date { .. } | Self::Editable(_) => None,
        }
    }
}

impl ReadOnlyObject {
    fn new(
        source: PageObject,
        margin: [f32; 2],
        message: &'static str,
        engine: &mut TextEngine,
    ) -> Result<Box<Self>, EditorError> {
        let offset = if matches!(source, PageObject::Title(_)) {
            margin
        } else {
            [0.0; 2]
        };
        let layout = source.layout();
        let x = layout.x.unwrap_or(0.0) + offset[0];
        let y = layout.y.unwrap_or(0.0) + offset[1];
        let width = layout.max_width.unwrap_or(160.0);
        let height = layout.max_height.unwrap_or(42.0);
        if [x, y, width, height].iter().any(|v| !v.is_finite()) || width <= 0.0 || height <= 0.0 {
            return Err(EditorError::InvalidGeometry);
        }
        let width = width.max(160.0);
        let label = engine.layout(
            &Paragraph::new(
                message.into(),
                onestore::document::Format {
                    font: Some("Arial".into()),
                    font_size: Some(11.0),
                    ..Default::default()
                },
            ),
            width - 16.0,
        )?;
        let height = height.max(label.height() + 16.0);
        let rect = [x, y, x + width, y + height];
        if rect.iter().any(|v| !v.is_finite()) {
            return Err(EditorError::InvalidGeometry);
        }
        Ok(Box::new(Self {
            source,
            message,
            label,
            offset,
        }))
    }
    pub fn rect(&self) -> [f32; 4] {
        let layout = self.source.layout();
        let x = layout.x.unwrap_or(0.0) + self.offset[0];
        let y = layout.y.unwrap_or(0.0) + self.offset[1];
        [
            x,
            y,
            x + layout.max_width.unwrap_or(160.0).max(160.0),
            y + layout
                .max_height
                .unwrap_or(42.0)
                .max(self.label.height() + 16.0),
        ]
    }
}

pub(crate) fn build(
    page: &mut Page,
    engine: &mut TextEngine,
    editable: bool,
) -> Result<Import, EditorError> {
    if page.margin_origin.iter().any(|v| !v.is_finite()) {
        return Err(EditorError::InvalidGeometry);
    }
    let mut objects = Vec::new();
    let mut outlines = Vec::new();
    let mut areas = Vec::new();
    let mut date = None;
    for object in std::mem::take(&mut page.objects) {
        match &object {
            PageObject::Outline(outline) => {
                let origin = [
                    outline.layout.x.unwrap_or(0.0),
                    outline.layout.y.unwrap_or(0.0),
                ];
                if origin.iter().any(|v| !v.is_finite()) {
                    return Err(EditorError::InvalidGeometry);
                }
                let unsupported = |error: &EditorError| {
                    matches!(
                        error,
                        EditorError::Layout(LayoutError::UnsupportedContent)
                            | EditorError::Edit(EditError::UnsupportedContent)
                    )
                };
                if editable {
                    match TextOutline::from_outline(engine, outline, &page.definitions) {
                        Ok(outline) => {
                            objects.push(Content::Editable(outline.id));
                            outlines.push(outline);
                            continue;
                        }
                        Err(error) if !unsupported(&error) => return Err(error),
                        // Drawn as stored when the editor cannot hold it.
                        Err(_) => {}
                    }
                }
                match outline.layout(engine, &page.definitions) {
                    Ok(layout) => objects.push(Content::Outline {
                        source: outline.clone(),
                        layout,
                        below_title: None,
                    }),
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
                areas.push(TitleArea {
                    id: title.id,
                    date: title.date,
                    layout: title.layout.clone(),
                    origins: title
                        .outlines
                        .iter()
                        .map(|outline| (outline.id, [outline.layout.x, outline.layout.y]))
                        .collect(),
                });
                let mut anchor = if editable {
                    title
                        .outlines
                        .iter()
                        .zip(&layouts)
                        .find(|(source, _)| source.title)
                        .map(|(source, (origin, layout))| {
                            (
                                source.id,
                                title.layout.y.unwrap_or(0.0)
                                    + page.margin_origin[1]
                                    + origin[1]
                                    + layout.size[1].max(source.layout.max_height.unwrap_or(0.0)),
                            )
                        })
                } else {
                    None
                };
                for (index, (origin, layout)) in layouts.into_iter().enumerate() {
                    let mut origin = [
                        title.layout.x.unwrap_or(0.0) + origin[0] + page.margin_origin[0],
                        title.layout.y.unwrap_or(0.0) + origin[1] + page.margin_origin[1],
                    ];
                    if origin.iter().any(|v| !v.is_finite()) {
                        return Err(EditorError::InvalidGeometry);
                    }
                    if editable && title.outlines[index].title {
                        let mut source = title.outlines[index].clone();
                        source.layout.x = Some(origin[0]);
                        source.layout.y = Some(origin[1]);
                        let outline =
                            TextOutline::from_outline(engine, &source, &page.definitions)?;
                        anchor = Some((outline.id, outline.bounds().y1 as f32));
                        objects.push(Content::Editable(outline.id));
                        outlines.push(outline);
                    } else {
                        if let Some((_, bottom)) = anchor {
                            origin[1] -= bottom;
                        }
                        if editable
                            && title.date == Some(title.outlines[index].id)
                            && let Some(timestamp) = page.created
                        {
                            let mut source = title.outlines[index].clone();
                            source.layout.x = Some(origin[0]);
                            source.layout.y = Some(origin[1]);
                            match PageDate::new(timestamp, source, engine, &page.definitions) {
                                Ok(value) => {
                                    if date.replace(value).is_some() {
                                        return Err(EditorError::InvalidGeometry);
                                    }
                                    objects.push(Content::Date {
                                        below_title: anchor.map(|(id, _)| id),
                                    });
                                    continue;
                                }
                                Err(EditorError::Edit(
                                    onestore::page::text::EditError::UnsupportedContent,
                                )) => {}
                                Err(error) => return Err(error),
                            }
                        }
                        let mut source = title.outlines[index].clone();
                        source.layout.x = Some(origin[0]);
                        source.layout.y = Some(origin[1]);
                        objects.push(Content::Outline {
                            source,
                            layout,
                            below_title: anchor.map(|(id, _)| id),
                        });
                    }
                }
            }
            PageObject::Image(source) => {
                if source.bytes.is_none()
                    || source.layout.max_width.is_none()
                    || source.layout.max_height.is_none()
                {
                    objects.push(Content::unavailable(object, engine)?);
                    continue;
                }
                let origin = [
                    source.layout.x.unwrap_or(0.0),
                    source.layout.y.unwrap_or(0.0),
                ];
                let size = [
                    source
                        .layout
                        .max_width
                        .ok_or(EditorError::InvalidGeometry)?,
                    source
                        .layout
                        .max_height
                        .ok_or(EditorError::InvalidGeometry)?,
                ];
                if origin.iter().any(|v| !v.is_finite())
                    || size.iter().any(|v| !v.is_finite() || *v <= 0.0)
                    || !(origin[0] + size[0]).is_finite()
                    || !(origin[1] + size[1]).is_finite()
                {
                    return Err(EditorError::InvalidGeometry);
                }
                let PageObject::Image(source) = object else {
                    unreachable!()
                };
                objects.push(Content::Image(source));
            }
            PageObject::Attachment(source) => {
                if [source.layout.x, source.layout.y]
                    .iter()
                    .any(|v| !v.unwrap_or(0.0).is_finite())
                {
                    return Err(EditorError::InvalidGeometry);
                }
                let layout = Box::new(crate::outline::page_file(engine, source)?);
                let PageObject::Attachment(source) = object else {
                    unreachable!()
                };
                objects.push(Content::File { source, layout });
            }
            PageObject::Ink(ink) => {
                if ink_bounds(ink).is_some_and(|b| b.iter().any(|v| !v.is_finite())) {
                    return Err(EditorError::InvalidGeometry);
                }
                let PageObject::Ink(ink) = object else {
                    unreachable!()
                };
                objects.push(Content::Ink(ink))
            }
            PageObject::Unsupported(_) => objects.push(Content::ReadOnly(ReadOnlyObject::new(
                object,
                page.margin_origin,
                "Unsupported content\nRead-only",
                engine,
            )?)),
        }
    }
    Ok(Import {
        objects,
        outlines,
        date,
        areas,
    })
}

/// The painted extent of a page's ink drawing, pen included, as `[x0, y0, x1, y1]` page
/// points.
pub(crate) fn ink_bounds(ink: &Ink) -> Option<[f32; 4]> {
    let [x, y] = ink_offset(ink);
    stroke_bounds(ink).map(|[x0, y0, x1, y1]| [x0 + x, y0 + y, x1 + x, y1 + y])
}

pub(crate) fn ink_offset(ink: &Ink) -> [f32; 2] {
    [ink.layout.x.unwrap_or(0.0), ink.layout.y.unwrap_or(0.0)]
}

/// The painted extent of every stroke before the drawing's offset.
fn stroke_bounds(ink: &Ink) -> Option<[f32; 4]> {
    ink.strokes
        .iter()
        .flat_map(|stroke| {
            let r = stroke.width.max(stroke.height) * 0.5;
            stroke
                .points
                .iter()
                .map(move |[x, y]| [x - r, y - r, x + r, y + r])
        })
        .chain(ink.groups.iter().filter_map(stroke_bounds))
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
}
