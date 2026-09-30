use swash::{
    scale::image::{Content, Image},
    zeno::{self, Cap, Fill, Join, Mask, PathBuilder, Placement, Point, Stroke, Style, Transform},
};

/// Colours for an icon's slots, linear RGB: paths of `class="accent"`, `"highlight"` or
/// `"badge"` are recoloured to their slot's colour; an empty slot keeps the art's colours.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Palette {
    pub accent: Option<[f32; 3]>,
    pub highlight: Option<[f32; 3]>,
    pub badge: Option<[f32; 3]>,
}

impl Palette {
    fn slot(&self, class: &str) -> Option<[f32; 3]> {
        match class {
            "accent" => self.accent,
            "highlight" => self.highlight,
            "badge" => self.badge,
            _ => None,
        }
    }

    /// The palette as hashable bits, for caching icons per distinct palette.
    pub(crate) fn bits(&self) -> [Option<[u32; 3]>; 3] {
        [self.accent, self.highlight, self.badge]
            .map(|slot| slot.map(|color| color.map(f32::to_bits)))
    }
}

/// Rasterizes 16×16 SVG sources over each other at a whole `size` of device pixels. Paths are filled and stroked with `#rrggbb`, `currentColor`,
/// which paints the linear `ink`, or two-stop `linearGradient`s with SVG's defaults
/// (`objectBoundingBox` unless `userSpaceOnUse`, `x1`..`y2`, stop offsets, pad spread; no
/// `gradientTransform`). `fill-opacity` and `stroke-opacity` apply, and strokes have round
/// caps and joins. A slot's paths keep the shading among their colours: the hue turn,
/// saturation scale and lightness shift that carry the mean of the slot's colours in a
/// source onto the palette's colour apply to each of them. A source's `image`s of PNG
/// `data:` URLs, as `picture_icon` writes them, paint beneath its paths.
pub(crate) fn rasterize(sources: &[&str], size: u32, ink: [f32; 3], palette: &Palette) -> Image {
    let side = size;
    let size = size as f32;
    let mut pixels = vec![[0.0_f32; 4]; (side * side) as usize];
    for source in sources {
        let svg = roxmltree::Document::parse(source).expect("Bundled icon SVG must be valid");
        assert_eq!(svg.root_element().attribute("viewBox"), Some("0 0 16 16"));
        for image in svg.descendants().filter(|node| node.has_tag_name("image")) {
            picture(image, side, &mut pixels);
        }
        let paint = |value: &str| -> Option<Paint> {
            match value {
                "none" => None,
                "currentColor" => Some(Paint::Flat([ink[0], ink[1], ink[2], 1.0])),
                _ => Some(match value.strip_prefix("url(#") {
                    Some(id) => {
                        let id = id.strip_suffix(')').unwrap();
                        let gradient = svg
                            .descendants()
                            .find(|node| node.attribute("id") == Some(id))
                            .unwrap();
                        Paint::linear(gradient)
                    }
                    None => Paint::Flat(hex(value)),
                }),
            }
        };
        let paths = || svg.descendants().filter(|node| node.has_tag_name("path"));
        let shifts: Vec<_> = ["accent", "highlight", "badge"]
            .into_iter()
            .filter_map(|class| {
                let target = palette.slot(class)?;
                let colors: Vec<_> = paths()
                    .filter(|path| path.attribute("class") == Some(class))
                    .flat_map(|path| {
                        [
                            path.attribute("fill").unwrap_or("#000000"),
                            path.attribute("stroke").unwrap_or("none"),
                        ]
                    })
                    .filter(|value| *value != "currentColor")
                    .filter_map(paint)
                    .flat_map(|paint| paint.colors())
                    .collect();
                Some((class, Shift::new(&colors, target)?))
            })
            .collect();
        for path in paths() {
            let data = path.attribute("d").unwrap();
            let shift = shifts
                .iter()
                .find(|(class, _)| path.attribute("class") == Some(*class))
                .map(|(_, shift)| shift);
            let fill = match path.attribute("fill-rule") {
                Some("evenodd") => Fill::EvenOdd,
                _ => Fill::NonZero,
            };
            let pen = path
                .attribute("stroke-width")
                .map_or(1.0, |width| width.parse().unwrap());
            let opacity = |name| {
                path.attribute(name)
                    .map_or(1.0, |value| value.parse().unwrap())
            };
            let layers = [
                (
                    path.attribute("fill").unwrap_or("#000000"),
                    opacity("fill-opacity"),
                    Style::from(fill),
                ),
                (
                    path.attribute("stroke").unwrap_or("none"),
                    opacity("stroke-opacity"),
                    Style::from(Stroke {
                        start_cap: Cap::Round,
                        end_cap: Cap::Round,
                        join: Join::Round,
                        ..Stroke::new(pen)
                    }),
                ),
            ];
            for (value, opacity, style) in layers {
                let Some(mut paint) = paint(value) else {
                    continue;
                };
                if let Some(shift) = shift.filter(|_| value != "currentColor") {
                    paint.recolor(|color| shift.apply(color));
                }
                let color = paint.shader(data);
                let (mask, _) = Mask::new(data)
                    .style(style)
                    .transform(Some(Transform::scale(size / 16.0, size / 16.0)))
                    .size(side, side)
                    .render();
                for (index, (pixel, coverage)) in pixels.iter_mut().zip(mask).enumerate() {
                    if coverage == 0 {
                        continue;
                    }
                    let alpha = f32::from(coverage) / 255.0 * opacity;
                    // The pixel's centre in icon units.
                    let [x, y] = [index % side as usize, index / side as usize]
                        .map(|device| device as f32 + 0.5)
                        .map(|device| device * 16.0 / size);
                    let color = color([x, y]);
                    for channel in 0..3 {
                        pixel[channel] = color[channel] * alpha + pixel[channel] * (1.0 - alpha);
                    }
                    pixel[3] = alpha + pixel[3] * (1.0 - alpha);
                }
            }
        }
    }
    let data = pixels
        .into_iter()
        .flat_map(|pixel| {
            let alpha = pixel[3];
            let channel = |value: f32| {
                let value = if alpha > 0.0 { value / alpha } else { 0.0 };
                super::srgb_byte(value)
            };
            [
                channel(pixel[0]),
                channel(pixel[1]),
                channel(pixel[2]),
                (alpha * 255.0).round() as u8,
            ]
        })
        .collect();
    Image {
        content: Content::Color,
        placement: Placement {
            left: 0,
            top: 0,
            width: side,
            height: side,
        },
        data,
        ..Image::default()
    }
}

/// An icon source drawing the PNG `png` centred in the icon at its own proportions; none
/// where it is not a picture.
pub fn picture_icon(png: &[u8]) -> Option<String> {
    use base64::Engine;
    let [width, height] = super::RasterImage::measure(png)
        .ok()?
        .map(|side| side as f32);
    let scale = 16.0 / width.max(height);
    let [width, height] = [width * scale, height * scale];
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><image x="{}" y="{}" width="{width}" height="{height}" href="data:image/png;base64,{}"/></svg>"#,
        (16.0 - width) / 2.0,
        (16.0 - height) / 2.0,
        base64::engine::general_purpose::STANDARD.encode(png),
    ))
}

/// Paints `node`, a picture of `picture_icon`'s, over `pixels`, an icon `side` pixels
/// square, filtered to the whole pixels its box covers.
fn picture(node: roxmltree::Node, side: u32, pixels: &mut [[f32; 4]]) {
    use base64::Engine;
    let scale = side as f32 / 16.0;
    let at = |name| {
        let value: f32 = node.attribute(name).unwrap().parse().unwrap();
        value * scale
    };
    let [x, y] = [at("x"), at("y")];
    let [left, top, right, bottom] = [x, y, x + at("width"), y + at("height")]
        .map(|edge| edge.round().clamp(0.0, side as f32) as u32);
    let decoded = node
        .attribute("href")
        .and_then(|href| href.strip_prefix("data:image/png;base64,"))
        .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
        .and_then(|png| image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok());
    let Some(decoded) = decoded.filter(|_| right > left && bottom > top) else {
        return;
    };
    let shown = image::imageops::resize(
        &decoded.into_rgba8(),
        right - left,
        bottom - top,
        image::imageops::FilterType::Triangle,
    );
    for (column, row, color) in shown.enumerate_pixels() {
        let pixel = &mut pixels[((top + row) * side + left + column) as usize];
        let [r, g, b, a] = color.0;
        let alpha = f32::from(a) / 255.0;
        let color = super::srgb(r, g, b);
        for channel in 0..3 {
            pixel[channel] = color[channel] * alpha + pixel[channel] * (1.0 - alpha);
        }
        pixel[3] = alpha + pixel[3] * (1.0 - alpha);
    }
}

fn hex(value: &str) -> [f32; 4] {
    let [_, r, g, b] = u32::from_str_radix(value.strip_prefix('#').unwrap(), 16)
        .unwrap()
        .to_be_bytes();
    super::srgb(r, g, b)
}

/// A number or a percentage of `whole`.
fn number(value: &str, whole: f32) -> f32 {
    match value.strip_suffix('%') {
        Some(percent) => percent.parse::<f32>().unwrap() / 100.0 * whole,
        None => value.parse().unwrap(),
    }
}

enum Paint {
    Flat([f32; 4]),
    Linear {
        /// Colour and offset of each stop.
        stops: [([f32; 4], f32); 2],
        /// Where offsets 0 and 1 lie, in icon units or fractions of the shape's box.
        line: [[f32; 2]; 2],
        bounding: bool,
    },
}

impl Paint {
    fn colors(&self) -> Vec<[f32; 4]> {
        match self {
            Self::Flat(color) => vec![*color],
            Self::Linear { stops, .. } => stops.iter().map(|(color, _)| *color).collect(),
        }
    }

    fn recolor(&mut self, change: impl Fn([f32; 4]) -> [f32; 4]) {
        match self {
            Self::Flat(color) => *color = change(*color),
            Self::Linear { stops, .. } => {
                for (color, _) in stops {
                    *color = change(*color);
                }
            }
        }
    }

    fn linear(gradient: roxmltree::Node) -> Self {
        assert!(
            gradient.attribute("gradientTransform").is_none(),
            "Icon gradients have no gradientTransform"
        );
        let stops: Vec<_> = gradient
            .children()
            .filter(|node| node.has_tag_name("stop"))
            .map(|node| {
                (
                    hex(node.attribute("stop-color").unwrap()),
                    node.attribute("offset")
                        .map_or(0.0, |offset| number(offset, 1.0)),
                )
            })
            .collect();
        let bounding = gradient.attribute("gradientUnits") != Some("userSpaceOnUse");
        // Percentages are of the shape's box or of the icon.
        let whole = if bounding { 1.0 } else { 16.0 };
        let at = |name, default| {
            gradient
                .attribute(name)
                .map_or(default * whole, |value| number(value, whole))
        };
        Self::Linear {
            stops: stops.try_into().expect("Icon gradients have two stops"),
            line: [
                [at("x1", 0.0), at("y1", 0.0)],
                [at("x2", 1.0), at("y2", 0.0)],
            ],
            bounding,
        }
    }

    /// The colour at each point of `data` in icon units.
    fn shader(&self, data: &str) -> impl Fn([f32; 2]) -> [f32; 4] {
        let (stops, [from, to], frame) = match *self {
            Self::Flat(color) => ([(color, 0.0), (color, 1.0)], [[0.0; 2], [1.0, 0.0]], None),
            Self::Linear {
                stops,
                line,
                bounding,
            } => (stops, line, bounding.then(|| bounds(data))),
        };
        let direction = [to[0] - from[0], to[1] - from[1]];
        let length = direction[0] * direction[0] + direction[1] * direction[1];
        move |point| {
            let [x, y] = match frame {
                // A flat box has no extent to spread across on that axis.
                Some([min, max]) => std::array::from_fn(|axis| {
                    let extent = max[axis] - min[axis];
                    if extent > 0.0 {
                        (point[axis] - min[axis]) / extent
                    } else {
                        0.0
                    }
                }),
                None => point,
            };
            // A gradient of no length paints its last stop.
            let along = if length > 0.0 {
                ((x - from[0]) * direction[0] + (y - from[1]) * direction[1]) / length
            } else {
                1.0
            };
            let [(first, start), (last, end)] = stops;
            let t = if end > start {
                ((along - start) / (end - start)).clamp(0.0, 1.0)
            } else {
                f32::from(along >= start)
            };
            std::array::from_fn(|channel| first[channel] + (last[channel] - first[channel]) * t)
        }
    }
}

/// A turn of hue, scale of saturation and shift of lightness in sRGB's HSL.
struct Shift([f32; 3]);

impl Shift {
    /// The shift carrying the mean of linear `colors` onto linear `target`.
    fn new(colors: &[[f32; 4]], target: [f32; 3]) -> Option<Self> {
        let count = colors.len() as f32;
        let mean: [f32; 3] = std::array::from_fn(|channel| {
            colors
                .iter()
                .map(|color| encode(color[channel]))
                .sum::<f32>()
                / count
        });
        let [from, to] = [mean, target.map(encode)].map(hsl);
        (count > 0.0).then(|| {
            Self([
                to[0] - from[0],
                if from[1] > 0.0 { to[1] / from[1] } else { 0.0 },
                to[2] - from[2],
            ])
        })
    }

    fn apply(&self, color: [f32; 4]) -> [f32; 4] {
        let [hue, saturation, lightness] = hsl([color[0], color[1], color[2]].map(encode));
        let [turn, scale, lift] = self.0;
        let [r, g, b] = rgb([
            (hue + turn).rem_euclid(360.0),
            (saturation * scale).clamp(0.0, 1.0),
            (lightness + lift).clamp(0.0, 1.0),
        ])
        .map(decode);
        [r, g, b, color[3]]
    }
}

fn encode(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

fn decode(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Hue in degrees, saturation and lightness of an encoded sRGB colour.
fn hsl([r, g, b]: [f32; 3]) -> [f32; 3] {
    let [max, min] = [r.max(g).max(b), r.min(g).min(b)];
    let lightness = (max + min) / 2.0;
    let range = max - min;
    if range == 0.0 {
        return [0.0, 0.0, lightness];
    }
    let saturation = range / (1.0 - (2.0 * lightness - 1.0).abs());
    let sector = if max == r {
        (g - b) / range
    } else if max == g {
        (b - r) / range + 2.0
    } else {
        (r - g) / range + 4.0
    };
    [(sector * 60.0).rem_euclid(360.0), saturation, lightness]
}

fn rgb([hue, saturation, lightness]: [f32; 3]) -> [f32; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    [0.0, 8.0, 4.0].map(|offset: f32| {
        let k = (offset + hue / 30.0).rem_euclid(12.0);
        lightness - chroma / 2.0 * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    })
}

/// The tight box around a path's geometry, curves included, as SVG's bounding box is.
fn bounds(data: &str) -> [[f32; 2]; 2] {
    struct Sampled {
        current: Point,
        min: [f32; 2],
        max: [f32; 2],
    }
    impl Sampled {
        fn add(&mut self, point: Point) {
            self.min = [self.min[0].min(point.x), self.min[1].min(point.y)];
            self.max = [self.max[0].max(point.x), self.max[1].max(point.y)];
        }
    }
    impl PathBuilder for Sampled {
        fn current_point(&self) -> Point {
            self.current
        }
        fn move_to(&mut self, to: impl Into<Point>) -> &mut Self {
            self.current = to.into();
            self.add(self.current);
            self
        }
        fn line_to(&mut self, to: impl Into<Point>) -> &mut Self {
            self.move_to(to)
        }
        fn quad_to(&mut self, control: impl Into<Point>, to: impl Into<Point>) -> &mut Self {
            let [start, control, to] = [self.current, control.into(), to.into()];
            let lift = |end: Point| end + (control - end) * (2.0 / 3.0);
            self.curve_to(lift(start), lift(to), to)
        }
        fn curve_to(
            &mut self,
            first: impl Into<Point>,
            second: impl Into<Point>,
            to: impl Into<Point>,
        ) -> &mut Self {
            let [start, first, second, to] = [self.current, first.into(), second.into(), to.into()];
            for step in 1..=32 {
                let t = step as f32 / 32.0;
                let u = 1.0 - t;
                self.add(
                    start * (u * u * u)
                        + first * (3.0 * u * u * t)
                        + second * (3.0 * u * t * t)
                        + to * (t * t * t),
                );
            }
            self.current = to;
            self
        }
        fn close(&mut self) -> &mut Self {
            self
        }
    }
    let mut sampled = Sampled {
        current: Point::ZERO,
        min: [f32::INFINITY; 2],
        max: [f32::NEG_INFINITY; 2],
    };
    zeno::apply(data, Fill::NonZero, None, &mut sampled);
    [sampled.min, sampled.max]
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="a" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#8090a0"/></linearGradient></defs><path fill="url(#a)" d="M2 2h12v12H2z"/></svg>"##;
    const MARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ff2000"/><stop offset="1" stop-color="#a00000"/></linearGradient></defs><path fill="url(#b)" d="M4 8l3 3 5-7-1-1-4 5-2-2z"/></svg>"##;

    fn gradient(attributes: &str, stops: &str) -> String {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="g" {attributes}>{stops}</linearGradient></defs><path fill="url(#g)" d="M0 4H16V12H0Z"/></svg>"##
        )
    }

    const BLACK_WHITE: &str =
        r##"<stop offset="0" stop-color="#000000"/><stop offset="1" stop-color="#ffffff"/>"##;

    /// The grey painted at a pixel of `source` rasterized at 16 pixels, and the grey a
    /// black-to-white ramp paints at fraction `t`.
    fn grey(source: &str, x: usize, y: usize) -> u8 {
        rasterize(&[source], 16, [1.0; 3], &Palette::default()).data[(y * 16 + x) * 4]
    }

    fn ramp(t: f32) -> u8 {
        super::super::srgb_byte(t)
    }

    #[test]
    fn gradients_span_the_shape_unless_in_user_space() {
        let vertical = gradient(r#"x2="0" y2="1""#, BLACK_WHITE);
        // Pixel centres 4.5 and 11.5 lie 0.5 inside the shape's 8-unit box.
        assert_eq!(grey(&vertical, 8, 4), ramp(0.5 / 8.0));
        assert_eq!(grey(&vertical, 8, 11), ramp(7.5 / 8.0));
        let user = gradient(
            r#"gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="16""#,
            BLACK_WHITE,
        );
        assert_eq!(grey(&user, 8, 4), ramp(4.5 / 16.0));
        assert_eq!(grey(&user, 8, 11), ramp(11.5 / 16.0));
    }

    #[test]
    fn gradients_default_to_horizontal_and_pad_past_their_stops() {
        let across = gradient("", BLACK_WHITE);
        assert_eq!(grey(&across, 3, 4), ramp(3.5 / 16.0));
        assert_eq!(grey(&across, 3, 11), ramp(3.5 / 16.0));
        let inner = gradient(
            r#"x2="0" y2="1""#,
            r##"<stop offset="25%" stop-color="#000000"/><stop offset="75%" stop-color="#ffffff"/>"##,
        );
        assert_eq!(grey(&inner, 8, 4), 0);
        assert_eq!(grey(&inner, 8, 11), 255);
        assert_eq!(grey(&inner, 8, 7), ramp((3.5 / 8.0 - 0.25) / 0.5));
    }

    #[test]
    fn bounds_follow_curves_not_their_control_points() {
        let [min, max] = bounds("M2 8A6 6 0 0 1 14 8Q8 14 2 8Z");
        assert!((min[1] - 2.0).abs() < 0.01, "{min:?}");
        assert!((max[1] - 11.0).abs() < 0.01, "{max:?}");
        assert!((min[0] - 2.0).abs() < 0.01 && (max[0] - 14.0).abs() < 0.01);
    }

    #[test]
    fn slots_take_the_palette_and_keep_their_shading() {
        // Two blues of one hue in the accent slot, a red outside it.
        const ART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path class="accent" d="M0 0H16V4H0Z" fill="#80a0ff"/><path class="accent" d="M0 4H16V8H0Z" fill="#0040e0"/><path d="M0 8H16V12H0Z" fill="#ff0000"/><path class="accent" d="M0 12H16V16H0Z" fill="currentColor"/></svg>"##;
        let pixel = |palette: &Palette, y: usize| {
            let image = rasterize(&[ART], 16, [0.0; 3], palette);
            let at = (y * 16 + 8) * 4;
            let [r, g, b] = [0, 1, 2].map(|channel| f32::from(image.data[at + channel]) / 255.0);
            hsl([r, g, b])
        };
        let plain = Palette::default();
        let green = Palette {
            accent: Some([0.0, 1.0, 0.0].map(decode)),
            ..plain
        };
        let [light, dark] = [2, 6].map(|y| pixel(&green, y));
        // Both turn green, the lighter above the darker, centred on the palette's colour.
        for [hue, ..] in [light, dark] {
            assert!((hue - 120.0).abs() < 2.0, "{hue}");
        }
        assert!(light[2] > dark[2]);
        assert!(((light[2] + dark[2]) / 2.0 - 0.5).abs() < 0.02);
        assert_eq!(pixel(&green, 10), pixel(&plain, 10));
        assert_eq!(pixel(&green, 14), [0.0, 0.0, 0.0]);
        assert!((pixel(&plain, 2)[0] - hsl([0.5, 0.627, 1.0])[0]).abs() < 2.0);
    }

    #[test]
    fn current_color_paints_the_ink_and_fixed_colours_keep_theirs() {
        const LINE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M2 8H14" fill="none" stroke="currentColor" stroke-width="2"/><path d="M2 12H14" fill="none" stroke="#ff0000" stroke-width="2"/></svg>"##;
        let ink = super::super::srgb(0x20, 0x60, 0xa0);
        let image = rasterize(&[LINE], 16, [ink[0], ink[1], ink[2]], &Palette::default());
        let pixel = |x: usize, y: usize| &image.data[(y * 16 + x) * 4..][..4];
        assert_eq!(pixel(8, 7), [0x20, 0x60, 0xa0, 255]);
        assert_eq!(pixel(8, 11), [255, 0, 0, 255]);
        assert_eq!(pixel(8, 2)[3], 0);
        // Round caps reach past the path's ends.
        assert!(pixel(1, 7)[3] > 0);
    }

    #[test]
    fn opacity_scales_each_paint_on_its_own() {
        const HALF: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M2 2H14V14H2Z" fill="#ffffff" fill-opacity="0.5" stroke="#000000" stroke-width="2" stroke-opacity="0.25"/></svg>"##;
        let image = rasterize(&[HALF], 16, [1.0; 3], &Palette::default());
        let alpha = |x: usize, y: usize| image.data[(y * 16 + x) * 4 + 3];
        assert_eq!(alpha(8, 8), 128);
        // The stroke's outer half covers only what the fill leaves.
        assert_eq!(alpha(1, 8), 64);
    }

    /// A 2×1 PNG, red then half-transparent blue, fits the icon's width, centred, and paints
    /// beneath the paths of later sources.
    #[test]
    fn pictures_fill_their_box_at_their_proportions() {
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 255, 0, 0, 255, 128])
            .unwrap();
        let source = picture_icon(&png).unwrap();
        let image = rasterize(&[&source, MARK], 32, [1.0; 3], &Palette::default());
        let pixel = |x: usize, y: usize| &image.data[(y * 32 + x) * 4..][..4];
        // The picture spans rows 8 to 24 of 32.
        assert_eq!(pixel(2, 4)[3], 0);
        assert_eq!(pixel(2, 12), [255, 0, 0, 255]);
        assert_eq!(pixel(30, 20), [0, 0, 255, 128]);
        assert_eq!(pixel(30, 28)[3], 0);
        let marked = rasterize(&[MARK], 32, [1.0; 3], &Palette::default());
        let covered: Vec<_> = (0..32 * 32)
            .filter(|at| marked.data[at * 4 + 3] == 255)
            .collect();
        assert!(!covered.is_empty());
        for at in covered {
            assert_eq!(image.data[at * 4..][..4], marked.data[at * 4..][..4]);
        }
        assert!(picture_icon(b"not a picture").is_none());
    }

    #[test]
    fn icons_keep_transparency_and_overlays_across_scales() {
        for size in [8, 16, 32, 57] {
            let plain = rasterize(&[BOX], size, [1.0; 3], &Palette::default());
            let marked = rasterize(&[BOX, MARK], size, [1.0; 3], &Palette::default());
            for image in [&plain, &marked] {
                assert_eq!(
                    image.data.len(),
                    (image.placement.width * image.placement.height * 4) as usize
                );
                assert!(image.data.chunks_exact(4).any(|p| p[3] > 0));
                assert!(image.data.chunks_exact(4).any(|p| p[3] < 255));
                assert!(
                    image
                        .data
                        .chunks_exact(4)
                        .filter(|p| p[3] == 0)
                        .all(|p| p[..3] == [0; 3])
                );
            }
            assert_ne!(plain.data, marked.data);
            assert!(marked.data.chunks_exact(4).any(|p| p[0] > p[2] && p[3] > 0));
        }
    }
}
