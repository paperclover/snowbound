//! COLRv1 glyphs, which swash can't paint: skrifa walks the paint graph and this paints it
//! on the CPU, in premultiplied sRGB as browsers blend it, into an image the atlas takes as
//! any colour glyph.

use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    color::{Brush, ColorGlyphFormat, ColorPainter, ColorStop, CompositeMode, Extend, Transform},
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
    raw::TableProvider,
    raw::types::{BoundingBox, F2Dot14},
};
use swash::{
    scale::image::{Content, Image},
    zeno::{Command, Mask, Placement, Vector},
};

/// The colour image of glyph `glyph` at `size` pixels per em, moved right and up by
/// `offset` pixels; `None` where the font has no COLRv1 paint for it.
pub(super) fn paint(
    data: &[u8],
    index: u32,
    glyph: u16,
    size: f32,
    coords: &[i16],
    offset: [f32; 2],
) -> Option<Image> {
    let font = FontRef::from_index(data, index).ok()?;
    let glyph = GlyphId::from(glyph);
    let color = font
        .color_glyphs()
        .get_with_format(glyph, ColorGlyphFormat::ColrV1)?;
    let coords: Vec<F2Dot14> = coords.iter().map(|&c| F2Dot14::from_bits(c)).collect();
    let location = LocationRef::new(&coords);
    let bounds = color
        .bounding_box(location, Size::new(size))
        .or_else(|| font.metrics(Size::new(size), location).bounds)?;
    let left = (bounds.x_min + offset[0]).floor();
    let top = (bounds.y_max + offset[1]).ceil();
    let width = ((bounds.x_max + offset[0]).ceil() - left) as usize;
    let height = (top - (bounds.y_min + offset[1]).floor()) as usize;
    if width == 0 || height == 0 || width * height > 1 << 22 {
        return None;
    }
    let scale = size / f32::from(font.head().ok()?.units_per_em());
    let palette = font
        .color_palettes()
        .get(0)
        .map(|palette| {
            palette
                .colors()
                .iter()
                .map(|c| [c.red, c.green, c.blue, c.alpha].map(|v| f32::from(v) / 255.0))
                .collect()
        })
        .unwrap_or_default();
    let mut painter = Painter {
        font: &font,
        location,
        palette,
        width,
        height,
        transforms: vec![Transform {
            xx: scale,
            yy: -scale,
            dx: offset[0] - left,
            dy: top - offset[1],
            ..Transform::default()
        }],
        clips: vec![vec![1.0; width * height]],
        layers: vec![(vec![[0.0; 4]; width * height], CompositeMode::SrcOver)],
    };
    color.paint(location, &mut painter).ok()?;
    let (pixels, _) = painter.layers.pop()?;
    Some(Image {
        content: Content::Color,
        placement: Placement {
            left: left as i32,
            top: top as i32,
            width: width as u32,
            height: height as u32,
        },
        data: pixels
            .iter()
            .flat_map(|&[r, g, b, a]| {
                let straight = |v: f32| match a > 0.0 {
                    true => ((v / a).clamp(0.0, 1.0) * 255.0).round() as u8,
                    false => 0,
                };
                [
                    straight(r),
                    straight(g),
                    straight(b),
                    (a * 255.0).round() as u8,
                ]
            })
            .collect(),
        ..Image::default()
    })
}

/// Paints into layers of premultiplied pixels; transforms map font units to pixels, y down.
struct Painter<'a> {
    font: &'a FontRef<'a>,
    location: LocationRef<'a>,
    palette: Vec<[f32; 4]>,
    width: usize,
    height: usize,
    transforms: Vec<Transform>,
    /// Coverage, each the intersection of those below it.
    clips: Vec<Vec<f32>>,
    layers: Vec<(Vec<[f32; 4]>, CompositeMode)>,
}

impl Painter<'_> {
    fn transform(&self) -> Transform {
        *self.transforms.last().expect("the pixel transform stays")
    }

    /// Narrows the clip to the path `commands` describes, in pixels.
    fn clip(&mut self, commands: &[Command]) {
        let mut coverage = vec![0; self.width * self.height];
        Mask::new(commands)
            .size(self.width as u32, self.height as u32)
            .render_into(&mut coverage, None);
        let clip = self.clips.last().expect("the whole image stays a clip");
        let clip = clip
            .iter()
            .zip(coverage)
            .map(|(outer, inner)| outer * f32::from(inner) / 255.0)
            .collect();
        self.clips.push(clip);
    }

    /// The premultiplied colour of palette entry `index` at `alpha`; 0xFFFF, the text's
    /// colour, is black, as colour glyphs aren't tinted.
    fn color(&self, index: u16, alpha: f32) -> [f32; 4] {
        let [r, g, b, a] = match index {
            0xFFFF => [0.0, 0.0, 0.0, 1.0],
            _ => self
                .palette
                .get(usize::from(index))
                .copied()
                .unwrap_or_default(),
        };
        let a = a * alpha;
        [r * a, g * a, b * a, a]
    }
}

impl ColorPainter for Painter<'_> {
    fn push_transform(&mut self, transform: Transform) {
        self.transforms.push(self.transform() * transform);
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }

    fn push_clip_glyph(&mut self, glyph: GlyphId) {
        let mut pen = Pen {
            transform: self.transform(),
            commands: Vec::new(),
        };
        if let Some(outline) = self.font.outline_glyphs().get(glyph) {
            let _ = outline.draw(
                DrawSettings::unhinted(Size::unscaled(), self.location),
                &mut pen,
            );
        }
        self.clip(&pen.commands);
    }

    fn push_clip_box(&mut self, clip: BoundingBox<f32>) {
        let mut pen = Pen {
            transform: self.transform(),
            commands: Vec::new(),
        };
        pen.move_to(clip.x_min, clip.y_min);
        pen.line_to(clip.x_max, clip.y_min);
        pen.line_to(clip.x_max, clip.y_max);
        pen.line_to(clip.x_min, clip.y_max);
        pen.close();
        self.clip(&pen.commands);
    }

    fn pop_clip(&mut self) {
        if self.clips.len() > 1 {
            self.clips.pop();
        }
    }

    fn fill(&mut self, brush: Brush<'_>) {
        let shade = Shade::new(self, brush);
        let inverse = invert(self.transform());
        let clip = self.clips.last().expect("the whole image stays a clip");
        let (layer, _) = self.layers.last_mut().expect("the image stays a layer");
        for (i, (pixel, &coverage)) in layer.iter_mut().zip(clip).enumerate() {
            if coverage <= 0.0 {
                continue;
            }
            let (x, y) = ((i % self.width) as f32 + 0.5, (i / self.width) as f32 + 0.5);
            let Some(source) = shade.at(inverse.transform(x, y)) else {
                continue;
            };
            let behind = 1.0 - source[3] * coverage;
            *pixel = std::array::from_fn(|c| source[c] * coverage + pixel[c] * behind);
        }
    }

    fn push_layer(&mut self, mode: CompositeMode) {
        self.layers
            .push((vec![[0.0; 4]; self.width * self.height], mode));
    }

    fn pop_layer(&mut self) {
        if self.layers.len() < 2 {
            return;
        }
        let (source, mode) = self.layers.pop().expect("checked above");
        let (backdrop, _) = self.layers.last_mut().expect("checked above");
        for (b, s) in backdrop.iter_mut().zip(source) {
            *b = composite(mode, s, *b);
        }
    }
}

/// `source` over `backdrop` in `mode`, both premultiplied.
fn composite(mode: CompositeMode, s: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    use CompositeMode::*;
    let (sa, ba) = (s[3], b[3]);
    let porter_duff = |fs: f32, fb: f32| std::array::from_fn(|c| s[c] * fs + b[c] * fb);
    // W3C Compositing: a separable blend of unpremultiplied channels, then source-over.
    let blend = |f: fn(f32, f32) -> f32| {
        let mut out: [f32; 4] = std::array::from_fn(|c| {
            let mixed = match sa > 0.0 && ba > 0.0 {
                true => sa * ba * f(s[c] / sa, b[c] / ba),
                false => 0.0,
            };
            s[c] * (1.0 - ba) + b[c] * (1.0 - sa) + mixed
        });
        out[3] = sa + ba - sa * ba;
        out
    };
    match mode {
        Clear => [0.0; 4],
        Src => s,
        Dest => b,
        DestOver => porter_duff(1.0 - ba, 1.0),
        SrcIn => porter_duff(ba, 0.0),
        DestIn => porter_duff(0.0, sa),
        SrcOut => porter_duff(1.0 - ba, 0.0),
        DestOut => porter_duff(0.0, 1.0 - sa),
        SrcAtop => porter_duff(ba, 1.0 - sa),
        DestAtop => porter_duff(1.0 - ba, sa),
        Xor => porter_duff(1.0 - ba, 1.0 - sa),
        Plus => std::array::from_fn(|c| (s[c] + b[c]).min(1.0)),
        Screen => blend(|s, b| s + b - s * b),
        Multiply => blend(|s, b| s * b),
        Darken => blend(f32::min),
        Lighten => blend(f32::max),
        Difference => blend(|s, b| (s - b).abs()),
        Exclusion => blend(|s, b| s + b - 2.0 * s * b),
        Overlay => blend(|s, b| hard_light(b, s)),
        HardLight => blend(hard_light),
        SoftLight => blend(|s, b| {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = match b <= 0.25 {
                    true => ((16.0 * b - 12.0) * b + 4.0) * b,
                    false => b.sqrt(),
                };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }),
        ColorDodge => blend(|s, b| match s < 1.0 {
            true => (b / (1.0 - s)).min(1.0),
            false => f32::from(b > 0.0),
        }),
        ColorBurn => blend(|s, b| match s > 0.0 {
            true => 1.0 - ((1.0 - b) / s).min(1.0),
            false => f32::from(b >= 1.0),
        }),
        // The HSL modes, rare in emoji, and unknown modes paint as source-over.
        _ => porter_duff(1.0, 1.0 - sa),
    }
}

fn hard_light(s: f32, b: f32) -> f32 {
    match s <= 0.5 {
        true => b * 2.0 * s,
        false => {
            let s = 2.0 * s - 1.0;
            b + s - b * s
        }
    }
}

fn invert(m: Transform) -> Transform {
    let det = m.xx * m.yy - m.xy * m.yx;
    if det.abs() < f32::EPSILON {
        return Transform {
            xx: 0.0,
            yy: 0.0,
            ..Transform::default()
        };
    }
    let (xx, xy, yx, yy) = (m.yy / det, -m.xy / det, -m.yx / det, m.xx / det);
    Transform {
        xx,
        yx,
        xy,
        yy,
        dx: -(xx * m.dx + xy * m.dy),
        dy: -(yx * m.dx + yy * m.dy),
    }
}

/// A brush resolved against the palette, evaluated at points in its own space.
enum Shade {
    Solid([f32; 4]),
    Linear {
        p0: [f32; 2],
        along: [f32; 2],
        line: Line,
    },
    Radial {
        c0: [f32; 2],
        r0: f32,
        cd: [f32; 2],
        dr: f32,
        line: Line,
    },
    Sweep {
        c0: [f32; 2],
        start: f32,
        span: f32,
        line: Line,
    },
}

impl Shade {
    fn new(painter: &Painter, brush: Brush<'_>) -> Self {
        let line = |stops: &[ColorStop], extend| Line {
            stops: {
                let mut stops: Vec<(f32, [f32; 4])> = stops
                    .iter()
                    .map(|s| (s.offset, painter.color(s.palette_index, s.alpha)))
                    .collect();
                stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                stops
            },
            extend,
        };
        match brush {
            Brush::Solid {
                palette_index,
                alpha,
            } => Shade::Solid(painter.color(palette_index, alpha)),
            Brush::LinearGradient {
                p0,
                p1,
                color_stops,
                extend,
            } => {
                let d = [p1.x - p0.x, p1.y - p0.y];
                let length = d[0] * d[0] + d[1] * d[1];
                Shade::Linear {
                    p0: [p0.x, p0.y],
                    along: match length > 0.0 {
                        true => d.map(|v| v / length),
                        false => [0.0; 2],
                    },
                    line: line(color_stops, extend),
                }
            }
            Brush::RadialGradient {
                c0,
                r0,
                c1,
                r1,
                color_stops,
                extend,
            } => Shade::Radial {
                c0: [c0.x, c0.y],
                r0,
                cd: [c1.x - c0.x, c1.y - c0.y],
                dr: r1 - r0,
                line: line(color_stops, extend),
            },
            Brush::SweepGradient {
                c0,
                start_angle,
                end_angle,
                color_stops,
                extend,
            } => Shade::Sweep {
                c0: [c0.x, c0.y],
                start: start_angle,
                span: end_angle - start_angle,
                line: line(color_stops, extend),
            },
        }
    }

    fn at(&self, (x, y): (f32, f32)) -> Option<[f32; 4]> {
        match self {
            Shade::Solid(color) => Some(*color),
            Shade::Linear { p0, along, line } => {
                Some(line.at((x - p0[0]) * along[0] + (y - p0[1]) * along[1]))
            }
            Shade::Radial {
                c0,
                r0,
                cd,
                dr,
                line,
            } => {
                // The greatest t whose circle, of radius r0 + t·dr ≥ 0, passes through the
                // point: a two-point conical gradient, as Skia and the spec draw it.
                let p = [x - c0[0], y - c0[1]];
                let a = cd[0] * cd[0] + cd[1] * cd[1] - dr * dr;
                let b = p[0] * cd[0] + p[1] * cd[1] + r0 * dr;
                let c = p[0] * p[0] + p[1] * p[1] - r0 * r0;
                let radius = |t: f32| r0 + t * dr >= 0.0;
                let t = if a.abs() < 1e-6 {
                    Some(c / (2.0 * b)).filter(|t| t.is_finite() && radius(*t))
                } else {
                    let discriminant = b * b - a * c;
                    (discriminant >= 0.0)
                        .then(|| {
                            let root = discriminant.sqrt();
                            let (t1, t2) = ((b + root) / a, (b - root) / a);
                            let (high, low) = (t1.max(t2), t1.min(t2));
                            [high, low].into_iter().find(|t| radius(*t))
                        })
                        .flatten()
                };
                t.map(|t| line.at(t))
            }
            Shade::Sweep {
                c0,
                start,
                span,
                line,
            } => {
                let angle = (y - c0[1]).atan2(x - c0[0]).to_degrees().rem_euclid(360.0);
                (*span != 0.0).then(|| line.at((angle - start) / span))
            }
        }
    }
}

/// A colour line: stops by offset, extended past 0 and 1 by `extend`.
struct Line {
    stops: Vec<(f32, [f32; 4])>,
    extend: Extend,
}

impl Line {
    fn at(&self, t: f32) -> [f32; 4] {
        let t = match self.extend {
            Extend::Repeat => t.rem_euclid(1.0),
            Extend::Reflect => 1.0 - (t.rem_euclid(2.0) - 1.0).abs(),
            _ => t,
        };
        let Some(first) = self.stops.first() else {
            return [0.0; 4];
        };
        if t <= first.0 {
            return first.1;
        }
        for pair in self.stops.windows(2) {
            let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
            if t <= t1 {
                let f = match t1 > t0 {
                    true => (t - t0) / (t1 - t0),
                    false => 1.0,
                };
                return std::array::from_fn(|c| c0[c] + (c1[c] - c0[c]) * f);
            }
        }
        self.stops.last().expect("checked above").1
    }
}

/// Collects an outline as pixel-space path commands.
struct Pen {
    transform: Transform,
    commands: Vec<Command>,
}

impl Pen {
    fn point(&self, x: f32, y: f32) -> Vector {
        let (x, y) = self.transform.transform(x, y);
        Vector::new(x, y)
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::MoveTo(self.point(x, y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::LineTo(self.point(x, y)));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.commands
            .push(Command::QuadTo(self.point(cx0, cy0), self.point(x, y)));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.commands.push(Command::CurveTo(
            self.point(cx0, cy0),
            self.point(cx1, cy1),
            self.point(x, y),
        ));
    }

    fn close(&mut self) {
        self.commands.push(Command::Close);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn colrv1_glyphs_paint_their_gradients_in_colour() {
        // Noto Color Emoji's 😀 alone (assets/NotoColorEmoji-OFL.txt).
        let font = include_bytes!("../../assets/NotoColorEmoji-Grinning.ttf");
        let image = super::paint(font, 0, 1, 64.0, &[], [0.0, 0.0]).expect("a COLRv1 glyph");
        assert_eq!(image.content, swash::scale::image::Content::Color);
        let p = image.placement;
        let pixel = |x: u32, y: u32| {
            let i = ((y * p.width + x) * 4) as usize;
            [0, 1, 2, 3].map(|c| image.data[i + c])
        };
        // The face's radial gradient: yellow, opaque, lighter above than below.
        let [r, g, b, a] = pixel(p.width / 2, p.height / 6);
        assert!(
            a == 255 && r > 240 && g > 180 && b < 100,
            "{:?}",
            [r, g, b, a]
        );
        assert!(pixel(p.width / 2, p.height / 6)[1] > pixel(p.width / 8, p.height * 3 / 5)[1]);
        assert_eq!(pixel(0, 0)[3], 0, "outside the face is clear");
    }
}
