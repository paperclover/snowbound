use super::{EditorError, TextOutline};
use crate::{
    date::PageDate,
    layout::{LayoutError, TextEngine, TextLayout},
    outline::OutlineLayout,
    page::{Image, Outline, Page, PageObject},
    text::Paragraph,
};

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
    Editable(onestore::ExGuid),
    ReadOnly(Box<ReadOnlyObject>),
}

pub struct ReadOnlyObject {
    pub source: PageObject,
    pub message: &'static str,
    pub(crate) label: TextLayout,
}

impl Content {
    pub(super) fn layout(&self) -> Option<(onestore::ExGuid, &onestore::document::Layout)> {
        match self {
            Self::Outline { source, .. } => Some((source.id, &source.layout)),
            Self::Image(source) => Some((source.id, &source.layout)),
            Self::ReadOnly(object) => Some((object.source.id(), object.source.layout())),
            Self::Date { .. } | Self::Editable(_) => None,
        }
    }

    pub(super) fn layout_mut(
        &mut self,
    ) -> Option<(onestore::ExGuid, &mut onestore::document::Layout)> {
        match self {
            Self::Outline { source, .. } => Some((source.id, &mut source.layout)),
            Self::Image(source) => Some((source.id, &mut source.layout)),
            Self::ReadOnly(object) => Some((object.source.id(), object.source.layout_mut())),
            Self::Date { .. } | Self::Editable(_) => None,
        }
    }
}

impl ReadOnlyObject {
    fn new(
        mut source: PageObject,
        margin: [f32; 2],
        message: &'static str,
        engine: &mut TextEngine,
    ) -> Result<Box<Self>, EditorError> {
        let offset = if matches!(source, PageObject::Title(_)) {
            margin
        } else {
            [0.0; 2]
        };
        let layout = source.layout_mut();
        let x = layout.x.unwrap_or(0.0) + offset[0];
        let y = layout.y.unwrap_or(0.0) + offset[1];
        if offset[0] != 0.0 {
            layout.x = Some(x);
        }
        if offset[1] != 0.0 {
            layout.y = Some(y);
        }
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
                    color: Some(0x005d554e),
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
        }))
    }
    pub fn rect(&self) -> [f32; 4] {
        let layout = self.source.layout();
        let x = layout.x.unwrap_or(0.0);
        let y = layout.y.unwrap_or(0.0);
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

#[expect(
    clippy::type_complexity,
    reason = "the three owned parts of an imported page"
)]
pub(crate) fn build(
    page: &mut Page,
    engine: &mut TextEngine,
    editable: bool,
) -> Result<(Vec<Content>, Vec<TextOutline>, Option<PageDate>), EditorError> {
    if page.margin_origin.iter().any(|v| !v.is_finite()) {
        return Err(EditorError::InvalidGeometry);
    }
    let mut objects = Vec::new();
    let mut outlines = Vec::new();
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
                if editable {
                    match TextOutline::from_outline(engine, outline, &page.definitions) {
                        Ok(outline) => {
                            objects.push(Content::Editable(outline.id));
                            outlines.push(outline);
                        }
                        Err(EditorError::Layout(LayoutError::UnsupportedContent)) => {
                            objects.push(Content::ReadOnly(ReadOnlyObject::new(
                                object,
                                page.margin_origin,
                                "Unsupported content\nRead-only",
                                engine,
                            )?))
                        }
                        Err(error) => return Err(error),
                    }
                } else {
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
                                    crate::text::EditError::UnsupportedContent,
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
            PageObject::Unsupported(_) => objects.push(Content::ReadOnly(ReadOnlyObject::new(
                object,
                page.margin_origin,
                "Unsupported content\nRead-only",
                engine,
            )?)),
        }
    }
    Ok((objects, outlines, date))
}
