pub mod page;
#[cfg(test)]
mod profile;
mod tag_art;
pub use tag_art::{TagArt, art_sources, import as import_tag_art};

use crate::{
    layout::{TextBrush, TextLayout},
    outline::TagIcon,
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

const CHECKMARK: &str = include_str!("../../assets/tags/checkmark.svg");
const BOX_SMALL: &str = include_str!("../../assets/tags/box-small.svg");
const CHECK_SMALL: &str = include_str!("../../assets/tags/check-small.svg");
const TAG: &str = include_str!("../../assets/tags/tag.svg");

/// How a tag symbol's artwork takes a check: a large box under it, a small box with the
/// check between the box and its badge, or art that shows none.
enum Art {
    Box(&'static str),
    Badged(&'static str),
    Plain(&'static str),
}

/// The artwork of symbol `shape` of MS-ONE's NoteTagShape, as OneNote 2010 draws it on the
/// page. A shape MS-ONE does not list shows a plain tag.
fn art(shape: u16) -> Art {
    use Art::{Badged, Box, Plain};
    macro_rules! tag {
        ($name:literal) => {
            include_str!(concat!("../../assets/tags/", $name, ".svg"))
        };
    }
    match shape {
        1 => Box(tag!("checkbox-green")),
        2 => Box(tag!("checkbox-yellow")),
        3 => Box(tag!("checkbox")),
        4 => Badged(tag!("mark-star-green")),
        5 => Badged(tag!("mark-star-yellow")),
        6 => Badged(tag!("mark-star-blue")),
        7 => Badged(tag!("mark-exclamation-green")),
        8 => Badged(tag!("mark-exclamation-yellow")),
        9 => Badged(tag!("mark-exclamation-blue")),
        10 => Badged(tag!("mark-arrow-green")),
        11 => Badged(tag!("mark-arrow-yellow")),
        12 => Badged(tag!("mark-arrow-blue")),
        13 => Plain(tag!("star")),
        14 => Plain(tag!("follow-up")),
        15 => Plain(tag!("question")),
        16 => Plain(tag!("arrow-right-blue")),
        17 => Plain(tag!("exclamation")),
        18 => Plain(tag!("phone")),
        19 => Plain(tag!("calendar")),
        20 => Plain(tag!("clock")),
        21 => Plain(tag!("idea")),
        22 => Plain(tag!("pushpin")),
        23 => Plain(tag!("address")),
        24 => Plain(tag!("blog")),
        25 => Plain(tag!("smiley")),
        26 => Plain(tag!("ribbon")),
        27 => Plain(tag!("key")),
        28 => Badged(tag!("mark-one-blue")),
        29 => Plain(tag!("circle-1-blue")),
        30 => Badged(tag!("mark-two-blue")),
        31 => Plain(tag!("circle-2-blue")),
        32 => Badged(tag!("mark-three-blue")),
        33 => Plain(tag!("circle-3-blue")),
        34 => Plain(tag!("star8-blue")),
        35 => Plain(tag!("tick-blue")),
        36 => Plain(tag!("circle-blue")),
        37 => Plain(tag!("arrow-down-blue")),
        38 => Plain(tag!("arrow-left-blue")),
        39 => Plain(tag!("solid-target-blue")),
        40 => Plain(tag!("star-blue")),
        41 => Plain(tag!("sun-blue")),
        42 => Plain(tag!("target-blue")),
        43 => Plain(tag!("triangle-blue")),
        44 => Plain(tag!("umbrella-blue")),
        45 => Plain(tag!("arrow-up-blue")),
        46 => Plain(tag!("x-dots-blue")),
        47 => Plain(tag!("x-blue")),
        48 => Badged(tag!("mark-one-green")),
        49 => Plain(tag!("circle-1-green")),
        50 => Badged(tag!("mark-two-green")),
        51 => Plain(tag!("circle-2-green")),
        52 => Badged(tag!("mark-three-green")),
        53 => Plain(tag!("circle-3-green")),
        54 => Plain(tag!("star8-green")),
        55 => Plain(tag!("tick-green")),
        56 => Plain(tag!("circle-green")),
        57 => Plain(tag!("arrow-down-green")),
        58 => Plain(tag!("arrow-left-green")),
        59 => Plain(tag!("arrow-right-green")),
        60 => Plain(tag!("solid-target-green")),
        61 => Plain(tag!("star-green")),
        62 => Plain(tag!("sun-green")),
        63 => Plain(tag!("target-green")),
        64 => Plain(tag!("triangle-green")),
        65 => Plain(tag!("umbrella-green")),
        66 => Plain(tag!("arrow-up-green")),
        67 => Plain(tag!("x-dots-green")),
        68 => Plain(tag!("x-green")),
        69 => Badged(tag!("mark-one-yellow")),
        70 => Plain(tag!("circle-1-yellow")),
        71 => Badged(tag!("mark-two-yellow")),
        72 => Plain(tag!("circle-2-yellow")),
        73 => Badged(tag!("mark-three-yellow")),
        74 => Plain(tag!("circle-3-yellow")),
        75 => Plain(tag!("star8-yellow")),
        76 => Plain(tag!("tick-yellow")),
        77 => Plain(tag!("circle-yellow")),
        78 => Plain(tag!("arrow-down-yellow")),
        79 => Plain(tag!("arrow-left-yellow")),
        80 => Plain(tag!("arrow-right-yellow")),
        81 => Plain(tag!("solid-target-yellow")),
        82 => Plain(tag!("sun-yellow")),
        83 => Plain(tag!("target-yellow")),
        84 => Plain(tag!("triangle-yellow")),
        85 => Plain(tag!("umbrella-yellow")),
        86 => Plain(tag!("arrow-up-yellow")),
        87 => Plain(tag!("x-dots-yellow")),
        88 => Plain(tag!("x-yellow")),
        89 => Plain(tag!("flag-today")),
        90 => Plain(tag!("flag-tomorrow")),
        91 => Plain(tag!("flag-this-week")),
        92 => Plain(tag!("flag-next-week")),
        93 => Plain(tag!("flag-no-date")),
        94 => Badged(tag!("mark-person-blue")),
        95 => Badged(tag!("mark-person-yellow")),
        96 => Badged(tag!("mark-person-green")),
        97 => Badged(tag!("mark-flag-blue")),
        98 => Badged(tag!("mark-flag-yellow")),
        99 => Badged(tag!("mark-flag-green")),
        100 => Plain(tag!("red-square")),
        101 => Plain(tag!("yellow-square")),
        102 => Plain(tag!("blue-square")),
        103 => Plain(tag!("green-square")),
        104 => Plain(tag!("orange-square")),
        105 => Plain(tag!("pink-square")),
        106 => Plain(tag!("email")),
        107 => Plain(tag!("envelope")),
        108 => Plain(tag!("envelope-open")),
        109 => Plain(tag!("mobile")),
        110 => Plain(tag!("phone-clock")),
        111 => Plain(tag!("question-balloon")),
        112 => Plain(tag!("paperclip")),
        113 => Plain(tag!("frown")),
        114 => Plain(tag!("im-contact")),
        115 => Plain(tag!("person")),
        116 => Plain(tag!("people")),
        117 => Plain(tag!("bell")),
        118 => Plain(tag!("contact")),
        119 => Plain(tag!("rose")),
        120 => Plain(tag!("date")),
        121 => Plain(tag!("music")),
        122 => Plain(tag!("movie")),
        123 => Plain(tag!("quote")),
        124 => Plain(tag!("globe")),
        125 => Plain(tag!("web")),
        126 => Plain(tag!("laptop")),
        127 => Plain(tag!("plane")),
        128 => Plain(tag!("car")),
        129 => Plain(tag!("binoculars")),
        130 => Plain(tag!("presentation")),
        131 => Plain(tag!("password")),
        132 => Plain(tag!("book")),
        133 => Plain(tag!("notebook")),
        134 => Plain(tag!("paper")),
        135 => Plain(tag!("research")),
        136 => Plain(tag!("highlight")),
        137 => Plain(tag!("dollar")),
        138 => Plain(tag!("coins")),
        139 => Plain(tag!("schedule")),
        140 => Plain(tag!("lightning")),
        141 => Plain(tag!("cloud")),
        142 => Plain(tag!("heart")),
        143 => Plain(tag!("sunflower")),
        _ => Plain(TAG),
    }
}

/// Symbols MS-ONE lists.
const SYMBOLS: u16 = 143;

/// The tag's artwork, drawn in order.
pub fn tag_sources(icon: TagIcon) -> &'static [&'static str] {
    static SOURCES: std::sync::OnceLock<Vec<[Vec<&'static str>; 2]>> = std::sync::OnceLock::new();
    let (shape, checked) = match icon {
        TagIcon::Symbol { shape, checked } => (shape, checked),
        TagIcon::Task { shape } => (shape, false),
    };
    let sources = SOURCES.get_or_init(|| {
        (0..=SYMBOLS)
            .map(|shape| match art(shape) {
                Art::Box(art) => [vec![art], vec![art, CHECKMARK]],
                Art::Badged(badge) => [vec![BOX_SMALL, badge], vec![BOX_SMALL, CHECK_SMALL, badge]],
                Art::Plain(art) => [vec![art], vec![art]],
            })
            .collect()
    });
    match sources.get(usize::from(shape)) {
        Some(sources) => &sources[usize::from(checked)],
        None => &[TAG],
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

    /// OneNote 2010 draws paragraph `shape N` of `corpus/custom-tags/native/shapes.one` with
    /// the art `DRAWN`'s Nth word names (its render: `shapes-1-72.png`, `shapes-73-143.png`);
    /// each paragraph's tag definition stores the NoteTagShape.
    #[test]
    fn symbols_draw_the_art_onenote_draws_for_their_number() {
        use onestore::page::{PageObject, ParagraphContent};
        use onestore::{RevisionIndex, Store, document::Document, document::Kind, page::Page};
        const DRAWN: &str = "\
            checkbox-green checkbox-yellow checkbox mark-star-green mark-star-yellow \
            mark-star-blue mark-exclamation-green mark-exclamation-yellow mark-exclamation-blue \
            mark-arrow-green mark-arrow-yellow mark-arrow-blue star follow-up question \
            arrow-right-blue exclamation phone calendar clock idea pushpin address blog smiley \
            ribbon key mark-one-blue circle-1-blue mark-two-blue circle-2-blue mark-three-blue \
            circle-3-blue star8-blue tick-blue circle-blue arrow-down-blue arrow-left-blue \
            solid-target-blue star-blue sun-blue target-blue triangle-blue umbrella-blue \
            arrow-up-blue x-dots-blue x-blue mark-one-green circle-1-green mark-two-green \
            circle-2-green mark-three-green circle-3-green star8-green tick-green circle-green \
            arrow-down-green arrow-left-green arrow-right-green solid-target-green star-green \
            sun-green target-green triangle-green umbrella-green arrow-up-green x-dots-green \
            x-green mark-one-yellow circle-1-yellow mark-two-yellow circle-2-yellow \
            mark-three-yellow circle-3-yellow star8-yellow tick-yellow circle-yellow \
            arrow-down-yellow arrow-left-yellow arrow-right-yellow solid-target-yellow \
            sun-yellow target-yellow triangle-yellow umbrella-yellow arrow-up-yellow \
            x-dots-yellow x-yellow flag-today flag-tomorrow flag-this-week flag-next-week \
            flag-no-date mark-person-blue mark-person-yellow mark-person-green mark-flag-blue \
            mark-flag-yellow mark-flag-green red-square yellow-square blue-square green-square \
            orange-square pink-square email envelope envelope-open mobile phone-clock \
            question-balloon paperclip frown im-contact person people bell contact rose date \
            music movie quote globe web laptop plane car binoculars presentation password book \
            notebook paper research highlight dollar coins schedule lightning cloud heart \
            sunflower";
        let bytes = include_bytes!("../../../../corpus/custom-tags/native/shapes.one");
        let store = Store::parse(bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let files: Vec<&str> = DRAWN.split_whitespace().collect();
        let mut seen = Vec::new();
        for (space, _) in document.pages().unwrap() {
            let page = Page::from_space(&document, space).unwrap();
            if !page.title.starts_with("Shapes") {
                continue;
            }
            for object in &page.objects {
                let PageObject::Outline(outline) = object else {
                    continue;
                };
                for paragraph in &outline.paragraphs {
                    let ParagraphContent::Text(text) = &paragraph.content else {
                        continue;
                    };
                    let label: usize = text.text.text()["shape ".len()..].parse().unwrap();
                    let definition = text.tags[0].definition.unwrap();
                    let Kind::TagDefinition {
                        shape: Some(shape), ..
                    } = page.definitions[&definition].kind
                    else {
                        panic!("shape {label} stores no symbol");
                    };
                    let (Art::Box(drawn) | Art::Badged(drawn) | Art::Plain(drawn)) = art(shape);
                    let file = files[label - 1];
                    let expected = std::fs::read_to_string(format!(
                        "{}/assets/tags/{file}.svg",
                        env!("CARGO_MANIFEST_DIR")
                    ))
                    .unwrap();
                    assert_eq!(
                        drawn, expected,
                        "shape {label}, stored {shape}, draws {file}"
                    );
                    seen.push(label);
                }
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=143).collect::<Vec<_>>());
    }

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
        let icons: Vec<_> = [
            (3, false),
            (3, true),
            (13, false),
            (15, false),
            (136, false),
            (118, false),
            (23, false),
            (18, false),
            (121, false),
            (17, false),
            (100, false),
            (101, false),
            (102, false),
            (999, false),
        ]
        .into_iter()
        .map(|(shape, checked)| TagIcon::of(shape, checked).unwrap())
        .chain([TagIcon::Task { shape: 91 }])
        .collect();
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
        let mut encoder = renderer
            .device()
            .create_command_encoder(&Default::default());
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
        renderer.queue().submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        renderer
            .device()
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
