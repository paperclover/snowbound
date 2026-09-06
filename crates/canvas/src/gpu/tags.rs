use crate::outline::TagIcon;
use swash::{
    scale::image::{Content, Image},
    zeno::{Mask, Placement, Transform, Vector},
};

pub(crate) fn rasterize(icon: TagIcon, size: f32, phase: [u8; 2]) -> Image {
    let (source, overlay) = match icon {
        TagIcon::CheckBox { checked } => (
            include_str!("../../assets/tags/checkbox.svg"),
            checked.then_some(include_str!("../../assets/tags/checkmark.svg")),
        ),
        TagIcon::Question => (include_str!("../../assets/tags/question.svg"), None),
        TagIcon::Music => (include_str!("../../assets/tags/music.svg"), None),
    };
    let width = (size + 1.0).ceil() as u32;
    let mut pixels = vec![[0.0_f32; 4]; (width * width) as usize];
    for source in std::iter::once(source).chain(overlay) {
        let svg = roxmltree::Document::parse(source).expect("Bundled tag SVG must be valid");
        assert_eq!(svg.root_element().attribute("viewBox"), Some("0 0 16 16"));
        for path in svg.descendants().filter(|node| node.has_tag_name("path")) {
            let gradient = path
                .attribute("fill")
                .unwrap()
                .strip_prefix("url(#")
                .unwrap()
                .strip_suffix(')')
                .unwrap();
            let gradient = svg
                .descendants()
                .find(|node| node.attribute("id") == Some(gradient))
                .unwrap();
            let mut stops = gradient
                .children()
                .filter(|node| node.has_tag_name("stop"))
                .map(|node| {
                    let color = u32::from_str_radix(
                        node.attribute("stop-color")
                            .unwrap()
                            .strip_prefix('#')
                            .unwrap(),
                        16,
                    )
                    .unwrap();
                    let [_, r, g, b] = color.to_be_bytes();
                    super::colorref(u32::from_le_bytes([r, g, b, 0]))
                });
            let top = stops.next().unwrap();
            let bottom = stops.next().unwrap();
            assert!(stops.next().is_none());
            let (mask, _) = Mask::new(path.attribute("d").unwrap())
                .transform(Some(Transform::scale(size / 16.0, size / 16.0)))
                .render_offset(Vector::new(phase[0] as f32 * 0.25, phase[1] as f32 * 0.25))
                .size(width, width)
                .render();
            for (index, (pixel, coverage)) in pixels.iter_mut().zip(mask).enumerate() {
                let alpha = f32::from(coverage) / 255.0;
                let y = ((index / width as usize) as f32 / size).min(1.0);
                for channel in 0..3 {
                    let color = top[channel] + (bottom[channel] - top[channel]) * y;
                    pixel[channel] = color * alpha + pixel[channel] * (1.0 - alpha);
                }
                pixel[3] = alpha + pixel[3] * (1.0 - alpha);
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
            width,
            height: width,
        },
        data,
        ..Image::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_masks_keep_transparency_and_checked_state_across_scales() {
        for size in [8.0, 16.0, 32.0, 57.5] {
            for phase in [[0, 0], [1, 2], [3, 3]] {
                let mut masks = Vec::new();
                for icon in [
                    TagIcon::CheckBox { checked: false },
                    TagIcon::CheckBox { checked: true },
                    TagIcon::Question,
                    TagIcon::Music,
                ] {
                    let image = rasterize(icon, size, phase);
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
                    masks.push(image.data);
                }
                assert_ne!(masks[0], masks[1]);
                assert!(masks[1].chunks_exact(4).any(|p| p[0] > p[2] && p[3] > 0));
            }
        }
    }
}
