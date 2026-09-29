pub mod page;
#[cfg(test)]
mod profile;

use crate::{
    layout::{TextBrush, TextLayout},
    outline::{BoxMark, TagIcon},
};
use draw::{GlyphRun, Glyphs, Layer, Primitive, RenderError};
use parley::PositionedLayoutItem;

#[derive(Clone, Copy)]
pub struct Viewport {
    pub size: [u32; 2],
    /// Physical pixels per document point.
    pub scale: f32,
    /// Physical pixel position of the document origin.
    pub origin: [f32; 2],
}

impl Viewport {
    pub fn document_point(&self, point: [f32; 2]) -> [f32; 2] {
        [
            (point[0] - self.origin[0]) / self.scale,
            (point[1] - self.origin[1]) / self.scale,
        ]
    }

    /// Document-point primitives drawn through this viewport.
    pub fn layer<'a>(&self, primitives: &'a [Primitive<'a>]) -> Layer<'a> {
        Layer {
            scale: self.scale,
            origin: self.origin,
            clip: None,
            backdrop: None,
            round: None,
            motion: None,
            primitives,
        }
    }
}

/// The page's paper and the ink that content in OneNote's automatic colour draws in,
/// linear RGBA.
#[derive(Clone, Copy, Debug)]
pub struct Paper {
    pub color: [f32; 4],
    pub ink: [f32; 4],
}

impl Paper {
    pub const WHITE: Self = Self {
        color: [1.0; 4],
        ink: [0.0, 0.0, 0.0, 1.0],
    };

    /// This paper under a page coloured `color` (COLORREF, OneNote's View, Page Color): the
    /// colour as OneNote paints it on white paper, moved onto this paper as a fill is, so
    /// dark paper takes the colour's hue at its own depth. Text keeps its ink; template art
    /// lies over the colour.
    pub fn colored(self, color: Option<u32>) -> Self {
        match color {
            Some(color) => Self {
                color: self.tint(colorref(color)),
                ..self
            },
            None => self,
        }
    }

    /// A fill OneNote draws on white paper, moved onto this paper: as far from it in
    /// OKLab lightness as from white, in the same hue and chroma, so text keeps its
    /// contrast on it.
    pub fn tint(&self, light: [f32; 4]) -> [f32; 4] {
        let [lightness, a, b] = draw::oklab(light);
        let [paper, ..] = draw::oklab(self.color);
        let distance = 1.0 - lightness;
        let lightness = if paper < 0.5 {
            paper + distance
        } else {
            paper - distance
        };
        let [red, green, blue] = draw::from_oklab([lightness, a, b]);
        [red, green, blue, light[3]]
    }

    /// A near-neutral colour OneNote draws on white paper, moved onto this paper: its
    /// darkness becomes ink and its tint tints the paper.
    pub(crate) fn shade(&self, light: [f32; 4]) -> [f32; 4] {
        let ink = 1.0 - (light[0] + light[1] + light[2]) / 3.0;
        let mut shade = light;
        for channel in 0..3 {
            shade[channel] = self.color[channel] * light[channel] + self.ink[channel] * ink;
        }
        shade
    }
}

impl Glyphs for TextLayout {
    fn runs(
        &self,
        paint: &mut dyn FnMut(GlyphRun<'_>) -> Result<(), RenderError>,
    ) -> Result<(), RenderError> {
        for (line, bounds) in self.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                draw::paint_parley_run(
                    &run,
                    bounds.baseline,
                    run.style().brush.rise,
                    [bounds.top, bounds.height],
                    // Automatic text on a highlight stays black, but on a black highlight, which
                    // paints in the ink, it stays ink.
                    |brush: &TextBrush| {
                        brush
                            .color
                            .or(brush.highlight.filter(|&color| color != 0).map(|_| 0))
                            .map(colorref)
                    },
                    run.style()
                        .brush
                        .highlight
                        .filter(|&color| color != 0)
                        .map(colorref),
                    paint,
                )?;
            }
        }
        Ok(())
    }
}

/// The layout a text primitive paints.
#[cfg(test)]
pub(crate) fn painted_layout(text: &dyn Glyphs) -> &TextLayout {
    (text as &dyn std::any::Any)
        .downcast_ref()
        .expect("Page text is a TextLayout")
}

const CHECKBOX: &str = include_str!("../../assets/tags/checkbox.svg");
const CHECKMARK: &str = include_str!("../../assets/tags/checkmark.svg");
const STAR: &str = include_str!("../../assets/tags/star.svg");
const QUESTION: &str = include_str!("../../assets/tags/question.svg");
const HIGHLIGHT: &str = include_str!("../../assets/tags/highlight.svg");
const CONTACT: &str = include_str!("../../assets/tags/contact.svg");
const ADDRESS: &str = include_str!("../../assets/tags/address.svg");
const PHONE: &str = include_str!("../../assets/tags/phone.svg");
const MUSIC: &str = include_str!("../../assets/tags/music.svg");
const EXCLAMATION: &str = include_str!("../../assets/tags/exclamation.svg");
const RED_SQUARE: &str = include_str!("../../assets/tags/red-square.svg");
const YELLOW_SQUARE: &str = include_str!("../../assets/tags/yellow-square.svg");
const BLUE_SQUARE: &str = include_str!("../../assets/tags/blue-square.svg");
const FLAG: &str = include_str!("../../assets/tags/flag.svg");
const IDEA: &str = include_str!("../../assets/tags/idea.svg");
const PASSWORD: &str = include_str!("../../assets/tags/password.svg");
const MOVIE: &str = include_str!("../../assets/tags/movie.svg");
const BOOK: &str = include_str!("../../assets/tags/book.svg");
const WEB: &str = include_str!("../../assets/tags/web.svg");
const BLOG: &str = include_str!("../../assets/tags/blog.svg");
const EMAIL: &str = include_str!("../../assets/tags/email.svg");
const BOX_SMALL: &str = include_str!("../../assets/tags/box-small.svg");
const CHECK_SMALL: &str = include_str!("../../assets/tags/check-small.svg");
const MARK_PERSON: &str = include_str!("../../assets/tags/mark-person.svg");
const MARK_MANAGER: &str = include_str!("../../assets/tags/mark-manager.svg");
const MARK_ARROW: &str = include_str!("../../assets/tags/mark-arrow.svg");
const MARK_ONE: &str = include_str!("../../assets/tags/mark-one.svg");
const MARK_TWO: &str = include_str!("../../assets/tags/mark-two.svg");
const MARK_CLIENT: &str = include_str!("../../assets/tags/mark-client.svg");
const TAG: &str = include_str!("../../assets/tags/tag.svg");

/// The tag's artwork, drawn in order.
pub fn tag_sources(icon: TagIcon) -> &'static [&'static str] {
    match icon {
        TagIcon::CheckBox {
            checked: false,
            mark: None,
        } => &[CHECKBOX],
        TagIcon::CheckBox {
            checked: true,
            mark: None,
        } => &[CHECKBOX, CHECKMARK],
        TagIcon::CheckBox {
            checked: false,
            mark: Some(BoxMark::Person),
        } => &[BOX_SMALL, MARK_PERSON],
        TagIcon::CheckBox {
            checked: true,
            mark: Some(BoxMark::Person),
        } => &[BOX_SMALL, CHECK_SMALL, MARK_PERSON],
        TagIcon::CheckBox {
            checked: false,
            mark: Some(BoxMark::Manager),
        } => &[BOX_SMALL, MARK_MANAGER],
        TagIcon::CheckBox {
            checked: true,
            mark: Some(BoxMark::Manager),
        } => &[BOX_SMALL, CHECK_SMALL, MARK_MANAGER],
        TagIcon::CheckBox {
            checked: false,
            mark: Some(BoxMark::Arrow),
        } => &[BOX_SMALL, MARK_ARROW],
        TagIcon::CheckBox {
            checked: true,
            mark: Some(BoxMark::Arrow),
        } => &[BOX_SMALL, CHECK_SMALL, MARK_ARROW],
        TagIcon::CheckBox {
            checked: false,
            mark: Some(BoxMark::One),
        } => &[BOX_SMALL, MARK_ONE],
        TagIcon::CheckBox {
            checked: true,
            mark: Some(BoxMark::One),
        } => &[BOX_SMALL, CHECK_SMALL, MARK_ONE],
        TagIcon::CheckBox {
            checked: false,
            mark: Some(BoxMark::Two),
        } => &[BOX_SMALL, MARK_TWO],
        TagIcon::CheckBox {
            checked: true,
            mark: Some(BoxMark::Two),
        } => &[BOX_SMALL, CHECK_SMALL, MARK_TWO],
        TagIcon::CheckBox {
            checked: false,
            mark: Some(BoxMark::Client),
        } => &[BOX_SMALL, MARK_CLIENT],
        TagIcon::CheckBox {
            checked: true,
            mark: Some(BoxMark::Client),
        } => &[BOX_SMALL, CHECK_SMALL, MARK_CLIENT],
        TagIcon::Star => &[STAR],
        TagIcon::Question => &[QUESTION],
        TagIcon::Highlight => &[HIGHLIGHT],
        TagIcon::Contact => &[CONTACT],
        TagIcon::Address => &[ADDRESS],
        TagIcon::Phone => &[PHONE],
        TagIcon::Music => &[MUSIC],
        TagIcon::Exclamation => &[EXCLAMATION],
        TagIcon::RedSquare => &[RED_SQUARE],
        TagIcon::YellowSquare => &[YELLOW_SQUARE],
        TagIcon::BlueSquare => &[BLUE_SQUARE],
        TagIcon::Flag => &[FLAG],
        TagIcon::Idea => &[IDEA],
        TagIcon::Password => &[PASSWORD],
        TagIcon::Movie => &[MOVIE],
        TagIcon::Book => &[BOOK],
        TagIcon::Web => &[WEB],
        TagIcon::Blog => &[BLOG],
        TagIcon::Email => &[EMAIL],
        TagIcon::Other => &[TAG],
    }
}

/// A COLORREF (0x00BBGGRR) as linear RGBA.
pub fn colorref(color: u32) -> [f32; 4] {
    let [red, green, blue, _] = color.to_le_bytes();
    draw::srgb(red, green, blue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TextEngine;
    use onestore::document::Format;
    use onestore::page::text::Paragraph;
    use std::time::Duration;

    /// A page colour paints as OneNote paints it on white paper, and on dark paper as a
    /// dark paper of its hue; no colour leaves the paper as it was.
    #[test]
    fn page_colours_colour_the_paper() {
        let teal = 0x00f2f9d4;
        assert_eq!(
            Paper::WHITE
                .colored(Some(teal))
                .color
                .map(|c| (c * 1000.0).round()),
            colorref(teal).map(|c| (c * 1000.0).round())
        );
        let dark = Paper {
            color: draw::srgb(0x1f, 0x20, 0x22),
            ink: draw::srgb(0xe6, 0xe6, 0xe6),
        };
        let colored = dark.colored(Some(teal));
        let [lightness, a, b] = draw::oklab(colored.color);
        let [paper, ..] = draw::oklab(dark.color);
        let [_, ta, tb] = draw::oklab(colorref(teal));
        assert!(lightness > paper && lightness < 0.5);
        assert!((a - ta).abs() < 1e-3 && (b - tb).abs() < 1e-3);
        assert_eq!(colored.ink, dark.ink);
        assert_eq!(dark.colored(None).color, dark.color);
    }

    #[test]
    fn runs_carry_their_highlight_as_what_lies_behind_them() {
        let run = |highlight, color| {
            (
                "text".into(),
                Format {
                    highlight,
                    color,
                    ..Format::default()
                },
            )
        };
        let text = TextEngine::default()
            .layout(
                &Paragraph::from_runs([
                    run(None, None),
                    run(Some(0x0000ffff), None),
                    run(Some(0x0000ffff), Some(0x000000ff)),
                    run(Some(0), None),
                ]),
                400.0,
            )
            .unwrap();
        let mut painted = Vec::new();
        text.runs(&mut |run| {
            painted.push((run.color, run.backdrop));
            Ok(())
        })
        .unwrap();
        let [black, red, yellow] = [0, 0x000000ff, 0x0000ffff].map(colorref);
        assert_eq!(
            painted,
            [
                (None, None),
                (Some(black), Some(yellow)),
                (Some(red), Some(yellow)),
                // A black highlight paints in the ink, as its automatic text does.
                (None, None),
            ]
        );
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn tags_and_page_text_paint_through_the_renderer() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let size = [448, 64];
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Tag readback test"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tag readback"),
            size: u64::from(size[0] * size[1] * 4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut renderer = draw::Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let icons = [
            TagIcon::CheckBox {
                checked: false,
                mark: None,
            },
            TagIcon::CheckBox {
                checked: true,
                mark: None,
            },
            TagIcon::Star,
            TagIcon::Question,
            TagIcon::Highlight,
            TagIcon::Contact,
            TagIcon::Address,
            TagIcon::Phone,
            TagIcon::Music,
            TagIcon::Exclamation,
            TagIcon::RedSquare,
            TagIcon::YellowSquare,
            TagIcon::BlueSquare,
            TagIcon::Flag,
            TagIcon::Other,
        ];
        let text = TextEngine::default()
            .layout(
                &Paragraph::from_runs([
                    (
                        "Tagged".into(),
                        Format {
                            color: Some(0x000000ff),
                            underline: Some(true),
                            ..Format::default()
                        },
                    ),
                    (" automatic".into(), Format::default()),
                    (
                        " lit".into(),
                        Format {
                            highlight: Some(0x0000ffff),
                            ..Format::default()
                        },
                    ),
                ]),
                100.0,
            )
            .unwrap();
        let mut primitives: Vec<_> = icons
            .iter()
            .enumerate()
            .map(|(index, icon)| Primitive::Icon {
                sources: tag_sources(*icon),
                origin: [2.0 + 16.0 * index as f32, 2.0],
                size: crate::outline::ParagraphTag::SIZE,
                tint: [1.0; 4],
                palette: draw::Palette::default(),
            })
            .collect();
        primitives.push(Primitive::Text {
            text: &text,
            origin: [2.0, 20.0],
            clip: None,
            ink: [0.0, 1.0, 0.0, 1.0],
        });
        let viewport = Viewport {
            size,
            scale: 2.0,
            origin: [0.0; 2],
        };
        renderer
            .draw(
                &target.create_view(&Default::default()),
                size,
                [1.0; 4],
                &[viewport.layer(&primitives)],
            )
            .unwrap();
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size[0] * 4),
                    rows_per_image: Some(size[1]),
                },
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        renderer.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        renderer
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(5)),
            })
            .unwrap();
        let pixels = readback.get_mapped_range(..).unwrap().to_vec();
        let pixel = |x: u32, y: u32| {
            let offset = ((y * size[0] + x) * 4) as usize;
            [pixels[offset], pixels[offset + 1], pixels[offset + 2]]
        };
        for index in 0..icons.len() as u32 {
            let painted = (4..28)
                .flat_map(|y| (4 + 32 * index..28 + 32 * index).map(move |x| (x, y)))
                .filter(|(x, y)| pixel(*x, *y).iter().any(|v| *v < 200))
                .count();
            assert!(painted > 20, "tag {index} was not painted");
        }
        let count = |color: [u8; 3]| {
            (40..64)
                .flat_map(|y| (4..size[0]).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    pixel(*x, *y)
                        .iter()
                        .zip(color)
                        .all(|(v, c)| v.abs_diff(c) < 80)
                })
                .count()
        };
        assert!(
            count([255, 0, 0]) > 50,
            "text colour or underline was not painted"
        );
        assert!(
            count([0, 255, 0]) > 50,
            "automatic text did not paint in the primitive's ink"
        );
        assert!(
            count([0, 0, 0]) > 10,
            "automatic text on a highlight did not stay black"
        );
    }
}
