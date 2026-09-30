//! Ink drawings and handwriting: strokes as page-coordinate polylines in points, decoded from
//! the stroke packets OneNote stores (ISF multi-byte first differences in HIMETRIC).

use super::Error;
use crate::{
    ExGuid,
    document::{Kind, Layout, Revision},
};

/// Points from a HIMETRIC coordinate, the one conversion every stroke coordinate goes through.
fn points(himetric: i64, scale: f32) -> f32 {
    himetric as f32 * scale * 72.0 / 2540.0
}

/// Rounds a page point to the HIMETRIC grid OneNote stores, so a stroke written from it reads
/// back equal.
pub fn snap(value: f32) -> f32 {
    points((value * 2540.0 / 72.0).round() as i64, 1.0)
}
const DIMENSION_X: [u8; 16] = [
    0x8f, 0x6a, 0x8a, 0x59, 0xc0, 0x52, 0xa0, 0x4b, 0x93, 0xaf, 0xaf, 0x35, 0x74, 0x11, 0xa5, 0x61,
];
const DIMENSION_Y: [u8; 16] = [
    0x75, 0x9f, 0x3f, 0xb5, 0xe0, 0x04, 0x98, 0x44, 0xa7, 0xee, 0xc3, 0x0d, 0xbb, 0x5a, 0x90, 0x11,
];
/// The highest pressure level new strokes store, as a 1024-level pen reports it.
const PRESSURE_LEVELS: f32 = 1023.0;

/// Rounds a pressure to the level a stroke stores, so a stroke written from it reads back
/// equal.
pub fn level(pressure: f32) -> f32 {
    (pressure.clamp(0.0, 1.0) * PRESSURE_LEVELS).round() / PRESSURE_LEVELS
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ink {
    pub id: ExGuid,
    /// A drawing's `x` and `y` offset every stroke point, as OneNote moves ink.
    pub layout: Layout,
    pub strokes: Vec<InkStroke>,
    /// Nested ink containers, as newer OneNote versions group handwriting.
    pub groups: Vec<Ink>,
    /// The shape drawn, for a drawing made from Draw's Insert Shapes.
    #[serde(default)]
    pub shape: Option<InkShape>,
}

/// The shapes of Draw's Insert Shapes that Snowbound draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Line,
    Arrow,
    Rectangle,
    Ellipse,
}

impl Ink {
    /// A shape dragged from `from` to `to` in page points, drawn with `pen`'s width and
    /// colour as OneNote 2010 draws it (`corpus/ink-tools`): a rectangle clockwise from its
    /// top left corner, an ellipse in 50 steps clockwise from its right end, and an arrow's
    /// head as a second stroke of two barbs at atan(1/2) to the line, 8 points plus twice
    /// the pen's width long. The result reads back unchanged once stored.
    pub fn drawn(
        kind: ShapeKind,
        from: [f32; 2],
        to: [f32; 2],
        pen: &InkStroke,
    ) -> Result<Self, Error> {
        let [x0, y0] = [from[0].min(to[0]), from[1].min(to[1])];
        let [x1, y1] = [from[0].max(to[0]), from[1].max(to[1])];
        let (shape, paths) = match kind {
            ShapeKind::Line | ShapeKind::Arrow => {
                let mut paths = vec![vec![from, to]];
                let [dx, dy] = [to[0] - from[0], to[1] - from[1]];
                if kind == ShapeKind::Arrow && dx.hypot(dy) > 0.0 {
                    let back = (-dy).atan2(-dx);
                    let length = 8.0 + 2.0 * pen.width;
                    let barb = |turn: f32| {
                        let angle = back + turn;
                        [to[0] + length * angle.cos(), to[1] + length * angle.sin()]
                    };
                    let spread = 0.5f32.atan();
                    paths.push(vec![barb(spread), to, barb(-spread)]);
                }
                (InkShape::Line([from, to]), paths)
            }
            ShapeKind::Rectangle | ShapeKind::Ellipse => {
                let [w, h] = [x1 - x0, y1 - y0];
                let transform = [w, 0.0, 0.0, h, x0, y0];
                let (anchors, path): (&[[f32; 2]], Vec<[f32; 2]>) = if kind == ShapeKind::Rectangle
                {
                    (
                        &[
                            [0.0, 0.0],
                            [0.5, 0.0],
                            [1.0, 0.0],
                            [1.0, 0.5],
                            [1.0, 1.0],
                            [0.5, 1.0],
                            [0.0, 1.0],
                            [0.0, 0.5],
                        ],
                        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]],
                    )
                } else {
                    let [cx, cy, rx, ry] = [x0 + w / 2.0, y0 + h / 2.0, w / 2.0, h / 2.0];
                    (
                        &[[0.5, 0.0], [1.0, 0.5], [0.5, 1.0], [0.0, 0.5]],
                        (0..=50)
                            .map(|step| {
                                let angle = step as f32 * std::f32::consts::TAU / 50.0;
                                [cx + rx * angle.cos(), cy + ry * angle.sin()]
                            })
                            .collect(),
                    )
                };
                (
                    InkShape::Closed {
                        transform,
                        anchors: anchors.to_vec(),
                    },
                    vec![path],
                )
            }
        };
        let mut strokes = Vec::new();
        for path in paths {
            strokes.push(InkStroke {
                id: super::text::new_id()?,
                points: path.into_iter().map(|p| p.map(snap)).collect(),
                pressure: Vec::new(),
                ..pen.clone()
            });
        }
        Ok(Self {
            id: super::text::new_id()?,
            layout: Layout::default(),
            strokes,
            groups: Vec::new(),
            shape: Some(shape.snapped()),
        })
    }
}

/// What OneNote keeps beside a drawn shape's strokes to edit it by (`corpus/ink-tools`), in
/// page points before the drawing's offset.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InkShape {
    /// A line or arrow from one end to the other.
    Line([[f32; 2]; 2]),
    /// A closed shape: `transform` (`[a, b, c, d, x, y]`) takes its anchors, in the unit
    /// square, onto the page.
    Closed {
        transform: [f32; 6],
        anchors: Vec<[f32; 2]>,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InkStroke {
    pub id: ExGuid,
    /// Page coordinates in points, before the drawing's offset.
    pub points: Vec<[f32; 2]>,
    /// Pen width and height in points.
    pub width: f32,
    pub height: f32,
    /// COLORREF; absent means the window text colour.
    pub color: Option<u32>,
    /// 0 is opaque, 255 fully transparent.
    pub transparency: Option<u8>,
    /// 0 is a round (ball) tip, 1 a rectangle.
    pub pen_tip: Option<u8>,
    /// The ISF raster operation: 9 (MaskPen) for a highlighter, whose colour multiplies
    /// what lies beneath.
    #[serde(default)]
    pub raster_operation: Option<u8>,
    /// The pen's pressure at each point, from none at 0 to full at 1, for a stroke whose
    /// width follows it; empty where the pen ignores pressure, as a mouse's does.
    #[serde(default)]
    pub pressure: Vec<f32>,
}

impl Ink {
    pub(crate) fn read(
        revision: &Revision<'_>,
        id: ExGuid,
        node: &crate::document::Element<'_>,
    ) -> Result<Self, Error> {
        let invalid = |message| Error { offset: 0, message };
        let Kind::Ink {
            data,
            scale_x,
            scale_y,
            shape_kind,
            line,
            anchors,
        } = &node.kind
        else {
            unreachable!()
        };
        let scale = [scale_x.unwrap_or(1.0), scale_y.unwrap_or(1.0)];
        let mut strokes = Vec::new();
        if let Some(data) = data {
            let data = revision
                .nodes
                .get(data)
                .ok_or_else(|| invalid("Missing ink data"))?;
            let Kind::InkData { strokes: ids, .. } = &data.kind else {
                return Err(invalid("Ink data has the wrong type"));
            };
            for stroke_id in ids {
                let stroke = revision
                    .nodes
                    .get(stroke_id)
                    .ok_or_else(|| invalid("Missing ink stroke"))?;
                strokes.push(InkStroke::read(revision, *stroke_id, stroke, scale)?);
            }
        }
        let mut groups = Vec::new();
        for child in &node.content {
            let element = revision
                .nodes
                .get(child)
                .ok_or_else(|| invalid("Missing nested ink"))?;
            if matches!(element.kind, Kind::Ink { .. }) {
                groups.push(Ink::read(revision, *child, element)?);
            }
        }
        Ok(Self {
            id,
            layout: node.layout.clone(),
            strokes,
            groups,
            shape: InkShape::read(*shape_kind, line.as_deref(), anchors.as_deref()),
        })
    }

    /// The extent of every stroke point, as `[left, top, width, height]` in points.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let mut extent: Option<[f32; 4]> = None;
        for point in self
            .strokes
            .iter()
            .flat_map(|stroke| stroke.points.iter())
            .copied()
            .chain(
                self.groups
                    .iter()
                    .filter_map(Ink::bounds)
                    .flat_map(|[x, y, w, h]| [[x, y], [x + w, y + h]]),
            )
        {
            extent = Some(match extent {
                None => [point[0], point[1], point[0], point[1]],
                Some([x0, y0, x1, y1]) => [
                    x0.min(point[0]),
                    y0.min(point[1]),
                    x1.max(point[0]),
                    y1.max(point[1]),
                ],
            });
        }
        extent.map(|[x0, y0, x1, y1]| [x0, y0, x1 - x0, y1 - y0])
    }
}

impl InkStroke {
    fn read(
        revision: &Revision<'_>,
        id: ExGuid,
        node: &crate::document::Element<'_>,
        scale: [f32; 2],
    ) -> Result<Self, Error> {
        let invalid = |message| Error { offset: 0, message };
        let Kind::InkStroke { path, style, .. } = &node.kind else {
            return Err(invalid("Ink stroke has the wrong type"));
        };
        let style = style
            .and_then(|style| revision.nodes.get(&style))
            .ok_or_else(|| invalid("Missing ink stroke style"))?;
        let Kind::InkStyle {
            dimensions,
            width,
            height,
            color,
            transparency,
            pen_tip,
            raster_operation,
            ignore_pressure,
            ..
        } = &style.kind
        else {
            return Err(invalid("Ink stroke style has the wrong type"));
        };
        if dimensions.len() % 32 != 0 || dimensions.is_empty() {
            return Err(invalid("Ink dimension table has an invalid length"));
        }
        let entries: Vec<&[u8]> = dimensions.chunks_exact(32).collect();
        let axis = |guid: &[u8]| entries.iter().position(|entry| entry[..16] == *guid);
        let (Some(x), Some(y)) = (axis(&DIMENSION_X), axis(&DIMENSION_Y)) else {
            return Err(invalid("Ink dimension table lacks X and Y"));
        };
        let values = multi_byte(path).ok_or_else(|| invalid("Ink stroke packet is malformed"))?;
        if values.len() % entries.len() != 0 {
            return Err(invalid("Ink stroke packet does not cover its dimensions"));
        }
        let per_dimension = values.len() / entries.len();
        let coordinates = |dimension: usize, factor: f32| {
            let mut position = 0i64;
            values[dimension * per_dimension..(dimension + 1) * per_dimension]
                .iter()
                .map(move |delta| {
                    position += delta;
                    points(position, factor)
                })
        };
        let points = coordinates(x, scale[0])
            .zip(coordinates(y, scale[1]))
            .map(|(x, y)| [x, y])
            .collect();
        let mut pressure = Vec::new();
        if let Some(dimension) = axis(&PRESSURE_DIMENSION[..16])
            && *ignore_pressure != Some(true)
        {
            // In f32, as `level` divides, so a level written reads back equal.
            let [lower, upper] = [16, 20].map(|at| {
                i32::from_le_bytes(entries[dimension][at..at + 4].try_into().unwrap()) as f32
            });
            if upper > lower {
                let mut level = 0i64;
                pressure = values[dimension * per_dimension..(dimension + 1) * per_dimension]
                    .iter()
                    .map(|delta| {
                        level += delta;
                        (level as f32 - lower) / (upper - lower)
                    })
                    .collect();
            }
        }
        Ok(Self {
            id,
            points,
            width: width.unwrap_or(0.0) * 72.0 / 2540.0,
            height: height.unwrap_or(0.0) * 72.0 / 2540.0,
            color: *color,
            transparency: *transparency,
            pen_tip: *pen_tip,
            raster_operation: *raster_operation,
            pressure,
        })
    }

    /// How many times its pen's size the stroke draws at point `index`, as OneNote 2010
    /// draws pressure (`corpus/ink-pressure`): a quarter at none, all of it at half and 1.75
    /// times at full.
    pub fn thickness(&self, index: usize) -> f32 {
        self.pressure
            .get(index)
            .map_or(1.0, |pressure| 0.25 + 1.5 * pressure)
    }
}

impl InkShape {
    /// Any geometry this does not recognise reads as no shape: the strokes still draw it.
    fn read(kind: Option<u8>, line: Option<&[u8]>, anchors: Option<&[u8]>) -> Option<Self> {
        let floats = |bytes: &[u8]| -> Vec<f32> {
            bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect()
        };
        match (kind?, line, anchors) {
            (11, Some(line), _) if line.len() == 16 => {
                let v = floats(line);
                Some(Self::Line(
                    [[v[0], v[1]], [v[2], v[3]]].map(|end| end.map(|v| v * 36.0)),
                ))
                .filter(Self::finite)
            }
            (12, _, Some(bytes)) if bytes.len() >= 24 && (bytes.len() - 24) % 8 == 0 => {
                let v = floats(bytes);
                Some(Self::Closed {
                    transform: std::array::from_fn(|i| v[i] * 36.0),
                    anchors: v[6..].chunks_exact(2).map(|p| [p[0], p[1]]).collect(),
                })
                .filter(Self::finite)
            }
            _ => None,
        }
    }

    fn finite(&self) -> bool {
        match self {
            Self::Line(ends) => ends.iter().flatten().all(|v| v.is_finite()),
            Self::Closed { transform, anchors } => transform
                .iter()
                .chain(anchors.iter().flatten())
                .all(|v| v.is_finite()),
        }
    }

    /// The kind, property and bytes OneNote stores the shape as.
    pub(crate) fn stored(&self) -> (u8, u32, Vec<u8>) {
        let half = |v: &f32| (v / 36.0).to_le_bytes();
        match self {
            Self::Line(ends) => (
                11,
                0x1c001dac,
                ends.iter().flatten().flat_map(half).collect(),
            ),
            Self::Closed { transform, anchors } => (
                12,
                0x1c001daa,
                transform
                    .iter()
                    .flat_map(half)
                    .chain(anchors.iter().flatten().flat_map(|v| v.to_le_bytes()))
                    .collect(),
            ),
        }
    }

    /// This shape as it reads back once stored.
    pub fn snapped(&self) -> Self {
        let half = |v: f32| (v / 36.0) * 36.0;
        match self {
            Self::Line(ends) => Self::Line(ends.map(|end| end.map(half))),
            Self::Closed { transform, anchors } => Self::Closed {
                transform: transform.map(half),
                anchors: anchors.clone(),
            },
        }
    }
}

/// ISF multi-byte decoding: a count, then that many 7-bit little-endian varints whose low bit
/// is the sign.
fn multi_byte(bytes: &[u8]) -> Option<Vec<i64>> {
    let mut cursor = 0;
    let mut next = || {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = *bytes.get(cursor)?;
            cursor += 1;
            if shift >= 64 {
                return None;
            }
            value |= u64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
    };
    let count = next()? >> 1;
    let mut values = Vec::with_capacity(usize::try_from(count).ok()?.min(1 << 20));
    for _ in 0..count {
        let raw = next()?;
        let magnitude = i64::try_from(raw >> 1).ok()?;
        values.push(if raw & 1 == 1 { -magnitude } else { magnitude });
    }
    (cursor == bytes.len()).then_some(values)
}

impl InkStroke {
    /// The stroke packet OneNote stores: X, Y and any pressure as ISF multi-byte first
    /// differences, of the points rounded to HIMETRIC and the pressure's levels.
    pub(crate) fn packet(&self) -> Vec<u8> {
        let differences = |values: Vec<i64>| {
            let mut previous = 0i64;
            values.into_iter().map(move |value| {
                let delta = value - previous;
                previous = value;
                delta
            })
        };
        let axis = |index: usize| {
            differences(
                self.points
                    .iter()
                    .map(|point| (point[index] * 2540.0 / 72.0).round() as i64)
                    .collect(),
            )
        };
        let pressure = differences(
            self.pressure
                .iter()
                .map(|pressure| (pressure.clamp(0.0, 1.0) * PRESSURE_LEVELS).round() as i64)
                .collect(),
        );
        let values: Vec<i64> = axis(0).chain(axis(1)).chain(pressure).collect();
        let mut out = Vec::new();
        let mut push = |raw: u64| {
            let mut raw = raw;
            loop {
                let byte = (raw & 0x7f) as u8;
                raw >>= 7;
                if raw == 0 {
                    out.push(byte);
                    break;
                }
                out.push(byte | 0x80);
            }
        };
        push((values.len() as u64) << 1);
        for value in values {
            push(((value.unsigned_abs()) << 1) | u64::from(value < 0));
        }
        out
    }
}

/// The dimension table OneNote 2010 writes for mouse ink: X and Y in HIMETRIC with the limits
/// and resolution of the authoring screen.
pub(crate) const DIMENSIONS: [u8; 64] = [
    0x8f, 0x6a, 0x8a, 0x59, 0xc0, 0x52, 0xa0, 0x4b, 0x93, 0xaf, 0xaf, 0x35, 0x74, 0x11, 0xa5, 0x61,
    0x00, 0x00, 0x00, 0x00, 0x80, 0x07, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x21, 0xe2, 0xe2, 0x41,
    0x75, 0x9f, 0x3f, 0xb5, 0xe0, 0x04, 0x98, 0x44, 0xa7, 0xee, 0xc3, 0x0d, 0xbb, 0x5a, 0x90, 0x11,
    0x00, 0x00, 0x00, 0x00, 0x38, 0x04, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x8b, 0xc5, 0xe2, 0x41,
];

/// The entry a pressure-sensitive pen adds to `DIMENSIONS`: NormalPressure from 0 to 1023, of
/// no unit and resolution 1, as OneNote keeps it (`corpus/ink-pressure`).
pub(crate) const PRESSURE_DIMENSION: [u8; 32] = [
    0x2d, 0x50, 0x07, 0x73, 0xf4, 0xf9, 0x18, 0x4e, 0xb3, 0xf2, 0x2c, 0xe1, 0xb1, 0xa3, 0x61, 0x0c,
    0x00, 0x00, 0x00, 0x00, 0xff, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f,
];

/// The dimension table OneNote 2010 writes for a drawn shape's pen: X and Y in HIMETRIC over
/// the whole 32-bit range, at a resolution of 1000.
pub(crate) const SHAPE_DIMENSIONS: [u8; 64] = [
    0x8f, 0x6a, 0x8a, 0x59, 0xc0, 0x52, 0xa0, 0x4b, 0x93, 0xaf, 0xaf, 0x35, 0x74, 0x11, 0xa5, 0x61,
    0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0x7f, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7a, 0x44,
    0x75, 0x9f, 0x3f, 0xb5, 0xe0, 0x04, 0x98, 0x44, 0xa7, 0xee, 0xc3, 0x0d, 0xbb, 0x5a, 0x90, 0x11,
    0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0x7f, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7a, 0x44,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_round_trip_through_the_multi_byte_coding() {
        let stroke = InkStroke {
            id: ExGuid::default(),
            points: vec![[100.0, 50.0], [100.5, 49.0], [99.0, 49.0]],
            width: 1.0,
            height: 1.0,
            color: None,
            transparency: None,
            pen_tip: None,
            raster_operation: None,
            pressure: Vec::new(),
        };
        let values = multi_byte(&stroke.packet()).unwrap();
        assert_eq!(values.len(), 6);
        assert_eq!(&values[..3], &[3528, 17, -52]);
        assert_eq!(&values[3..], &[1764, -35, 0]);
    }
}
