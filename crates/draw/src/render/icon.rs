use swash::{
    scale::image::{Content, Image},
    zeno::{Cap, Fill, Join, Mask, Placement, Stroke, Style, Transform, Vector},
};

/// Rasterizes 16×16 SVG sources over each other at `size` device pixels, offset by
/// `phase` quarter pixels. Paths are filled and stroked with `#rrggbb`, two-stop vertical
/// gradients or `currentColor`, which paints white; strokes have round caps and joins.
pub(crate) fn rasterize(sources: &[&str], size: f32, phase: [u8; 2]) -> Image {
    let side = (size + 1.0).ceil() as u32;
    let mut pixels = vec![[0.0_f32; 4]; (side * side) as usize];
    for source in sources {
        let svg = roxmltree::Document::parse(source).expect("Bundled icon SVG must be valid");
        assert_eq!(svg.root_element().attribute("viewBox"), Some("0 0 16 16"));
        let paint = |value: &str| -> Option<[[f32; 4]; 2]> {
            let hex = |value: &str| {
                let [_, r, g, b] = u32::from_str_radix(value.strip_prefix('#').unwrap(), 16)
                    .unwrap()
                    .to_be_bytes();
                super::srgb(r, g, b)
            };
            match value {
                "none" => None,
                "currentColor" => Some([[1.0; 4]; 2]),
                _ => Some(match value.strip_prefix("url(#") {
                    Some(id) => {
                        let id = id.strip_suffix(')').unwrap();
                        let gradient = svg
                            .descendants()
                            .find(|node| node.attribute("id") == Some(id))
                            .unwrap();
                        let stops: Vec<_> = gradient
                            .children()
                            .filter(|node| node.has_tag_name("stop"))
                            .map(|node| hex(node.attribute("stop-color").unwrap()))
                            .collect();
                        stops.try_into().expect("Icon gradients have two stops")
                    }
                    None => [hex(value); 2],
                }),
            }
        };
        for path in svg.descendants().filter(|node| node.has_tag_name("path")) {
            let fill = match path.attribute("fill-rule") {
                Some("evenodd") => Fill::EvenOdd,
                _ => Fill::NonZero,
            };
            let pen = path
                .attribute("stroke-width")
                .map_or(1.0, |width| width.parse().unwrap());
            let layers = [
                (
                    path.attribute("fill").unwrap_or("#000000"),
                    Style::from(fill),
                ),
                (
                    path.attribute("stroke").unwrap_or("none"),
                    Style::from(Stroke {
                        start_cap: Cap::Round,
                        end_cap: Cap::Round,
                        join: Join::Round,
                        ..Stroke::new(pen)
                    }),
                ),
            ];
            for (value, style) in layers {
                let Some([top, bottom]) = paint(value) else {
                    continue;
                };
                let (mask, _) = Mask::new(path.attribute("d").unwrap())
                    .style(style)
                    .transform(Some(Transform::scale(size / 16.0, size / 16.0)))
                    .render_offset(Vector::new(phase[0] as f32 * 0.25, phase[1] as f32 * 0.25))
                    .size(side, side)
                    .render();
                for (index, (pixel, coverage)) in pixels.iter_mut().zip(mask).enumerate() {
                    let alpha = f32::from(coverage) / 255.0;
                    let y = ((index / side as usize) as f32 / size).min(1.0);
                    for channel in 0..3 {
                        let color = top[channel] + (bottom[channel] - top[channel]) * y;
                        pixel[channel] = color * alpha + pixel[channel] * (1.0 - alpha);
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

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="a" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#8090a0"/></linearGradient></defs><path fill="url(#a)" d="M2 2h12v12H2z"/></svg>"##;
    const MARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ff2000"/><stop offset="1" stop-color="#a00000"/></linearGradient></defs><path fill="url(#b)" d="M4 8l3 3 5-7-1-1-4 5-2-2z"/></svg>"##;

    #[test]
    fn current_color_strokes_paint_white_coverage() {
        const LINE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M2 8H14" fill="none" stroke="currentColor" stroke-width="2"/></svg>"##;
        let image = rasterize(&[LINE], 16.0, [0, 0]);
        let pixel = |x: usize, y: usize| &image.data[(y * 17 + x) * 4..][..4];
        assert_eq!(pixel(8, 7), [255, 255, 255, 255]);
        assert_eq!(pixel(8, 2)[3], 0);
        // Round caps reach past the path's ends.
        assert!(pixel(1, 7)[3] > 0);
    }

    #[test]
    fn icons_keep_transparency_and_overlays_across_scales() {
        for size in [8.0, 16.0, 32.0, 57.5] {
            for phase in [[0, 0], [1, 2], [3, 3]] {
                let plain = rasterize(&[BOX], size, phase);
                let marked = rasterize(&[BOX, MARK], size, phase);
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
}
