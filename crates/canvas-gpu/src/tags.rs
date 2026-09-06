use one_canvas::outline::TagIcon;
use swash::{
    scale::image::{Content, Image},
    zeno::{Mask, Placement, Transform, Vector},
};

pub(crate) fn rasterize(icon: TagIcon, size: f32, phase: [u8; 2]) -> Image {
    let layers = match icon {
        TagIcon::CheckBox { checked } => {
            let mut layers = vec![
                ("M0 0H16V16H0Z", 0xf7b691, 0xef9864),
                ("M1 1H15V15H1Z", 0xffffff, 0xffffff),
                ("M2 2H14V14H2Z", 0xf6af7c, 0xe98755),
                ("M3 3H13V13H3Z", 0xffffff, 0xf5e6d6),
            ];
            if checked {
                layers.push(("M2 8L6 11L13 3L14 5L6 14L1 10Z", 0x778cf4, 0x2544c8));
            }
            layers
        }
        TagIcon::Question => vec![(
            "M8 5C8 1 15 1 15 5C15 7 11 8 11 10H9C9 6 13 6 13 5C13 3 10 3 10 5ZM9 12H11V14H9Z",
            0xe83ace,
            0x8f008f,
        )],
        TagIcon::Music => vec![
            (
                "M7 1C10 1 9 3 12 4C14 5 15 7 13 9C13 6 10 7 9 5V11C9 15 2 17 1 13C0 10 4 8 7 9Z",
                0xf0bb9c,
                0xa35546,
            ),
            ("M2 11C3 9 6 9 7 10C7 11 4 13 2 12Z", 0xffe8dc, 0xda9786),
        ],
    };
    let width = (size + 1.0).ceil() as u32;
    let mut pixels = vec![[0.0_f32; 4]; (width * width) as usize];
    for (path, top, bottom) in layers {
        let (mask, _) = Mask::new(path)
            .transform(Some(Transform::scale(size / 16.0, size / 16.0)))
            .render_offset(Vector::new(phase[0] as f32 * 0.25, phase[1] as f32 * 0.25))
            .size(width, width)
            .render();
        let top = super::colorref(top);
        let bottom = super::colorref(bottom);
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
