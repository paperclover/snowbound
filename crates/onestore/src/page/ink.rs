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

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ink {
    pub id: ExGuid,
    pub layout: Layout,
    pub strokes: Vec<InkStroke>,
    /// Nested ink containers, as newer OneNote versions group handwriting.
    pub groups: Vec<Ink>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InkStroke {
    pub id: ExGuid,
    /// Absolute page coordinates in points.
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
            ..
        } = &style.kind
        else {
            return Err(invalid("Ink stroke style has the wrong type"));
        };
        if dimensions.len() % 32 != 0 || dimensions.is_empty() {
            return Err(invalid("Ink dimension table has an invalid length"));
        }
        let guids: Vec<&[u8]> = dimensions.chunks_exact(32).map(|d| &d[..16]).collect();
        let axis = |guid: [u8; 16]| guids.iter().position(|g| **g == guid);
        let (Some(x), Some(y)) = (axis(DIMENSION_X), axis(DIMENSION_Y)) else {
            return Err(invalid("Ink dimension table lacks X and Y"));
        };
        let values = multi_byte(path).ok_or_else(|| invalid("Ink stroke packet is malformed"))?;
        if values.len() % guids.len() != 0 {
            return Err(invalid("Ink stroke packet does not cover its dimensions"));
        }
        let per_dimension = values.len() / guids.len();
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
        Ok(Self {
            id,
            points,
            width: width.unwrap_or(0.0) * 72.0 / 2540.0,
            height: height.unwrap_or(0.0) * 72.0 / 2540.0,
            color: *color,
            transparency: *transparency,
            pen_tip: *pen_tip,
        })
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
    /// The stroke packet OneNote stores: X then Y as ISF multi-byte first differences of the
    /// points rounded to HIMETRIC.
    pub(crate) fn packet(&self) -> Vec<u8> {
        let axis = |index: usize| {
            let mut previous = 0i64;
            self.points.iter().map(move |point| {
                let value = (point[index] * 2540.0 / 72.0).round() as i64;
                let delta = value - previous;
                previous = value;
                delta
            })
        };
        let values: Vec<i64> = axis(0).chain(axis(1)).collect();
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
        };
        let values = multi_byte(&stroke.packet()).unwrap();
        assert_eq!(values.len(), 6);
        assert_eq!(&values[..3], &[3528, 17, -52]);
        assert_eq!(&values[3..], &[1764, -35, 0]);
    }
}
