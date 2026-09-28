//! OneNote 2010's page template art, recognised by the SHA-256 of OneNote's picture or the
//! marker in Snowbound's, and painted from vector recreations that suit the paper.

use super::{SETTLE, Slot, density, queue};
use crate::gpu::Paper;
use draw::RasterImage;
use resvg::{tiny_skia, usvg};
use sha2::{Digest, Sha256};
use std::{
    task::Waker,
    time::{Duration, Instant},
};

/// One of OneNote 2010's page templates: its art recreated for white paper in `#rrggbb`
/// colours over a view box the size in pixels of the picture OneNote stores, that picture's
/// format, and the SHA-256 of each picture OneNote has shipped for it.
struct Template {
    name: &'static str,
    svg: &'static str,
    jpeg: bool,
    hashes: &'static [&'static str],
}

macro_rules! template {
    ($name:literal, $format:ident, $($hash:literal),+ $(,)?) => {
        Template {
            name: $name,
            svg: include_str!(concat!("../../../assets/backgrounds/", $name, ".svg")),
            jpeg: template!(@$format),
            hashes: &[$($hash),+],
        }
    };
    (@jpeg) => { true };
    (@png) => { false };
}

const TEMPLATES: &[Template] = &[
    template!(
        "bamboo",
        jpeg,
        "a7fa7f092eff43030a56342c39a765f8d5cc48c7db815ddfc8c1e5ec40117fae"
    ),
    template!(
        "binoculars-corner",
        jpeg,
        "74c131777e7c437fd654427417097bc01b0813ba8e1e50e4b937bd50a1bebcdb"
    ),
    template!(
        "black-and-green-title",
        jpeg,
        "9c552717e8d5079bbb226948641ff13532df3d7be434c6ce545f1692fa57d45a"
    ),
    template!(
        "blue-bubbles-corner-margin",
        jpeg,
        "3c68c7ee798e62a4a99c740153f3980d7df029605c843410942c7f85e794823b"
    ),
    template!(
        "blue-bubbles-corner-top",
        jpeg,
        "d0a3a1c3cd63c4023fe5716cbe2c211307d0e277e444d9ef76c7fc097a845fd4"
    ),
    template!(
        "blue-clouds",
        jpeg,
        "7966f3d8a2d61ecb49a35e163781858e052c0b122a18a1238afe27b57e2850e8"
    ),
    template!(
        "blue-dots",
        jpeg,
        "139bb0e79f89c3ddef79b1716a5fbab4c07df5785fb3cdf6b4eeddbf6c078452"
    ),
    template!(
        "blue-flow",
        png,
        "473c90248fa33f8e49b2daf38673d87a9405481eae6a2087cc5384b695b6a4c0",
        "51e05999a1c9f17df28cb474e57dd8e64bdab824874a532c20a23766a01f8967",
    ),
    template!(
        "blue-mist-margin",
        jpeg,
        "83245f217deae4a4143b565e13c045dbb32a9063e8c6b2e43bb15cd76c5f9219"
    ),
    template!(
        "blue-stripe-title",
        jpeg,
        "474f9a8c25d5e21192315397ea995b1e11e2c1608157c6e0277688091bfd136a"
    ),
    template!(
        "blue-stripes",
        jpeg,
        "06e09de80c3f32254da4fe6b2cbad7c05ef144dd54b8c65745e195bbf7317a2e"
    ),
    template!(
        "blue-swirls",
        jpeg,
        "10c833e47be1c8496f949a6b059c2d79212a4dd66bde62116ea337fa4fe0b654"
    ),
    template!(
        "blue-wave-title",
        jpeg,
        "646586cb71742a2369a529876b41af6a472c35cc508d1ae5d8395d55784814f2"
    ),
    template!(
        "blue-waves",
        jpeg,
        "fbb4573e3bee1b337077691bebae15d6fac52432405d31396d526d7694a8283d"
    ),
    template!(
        "books",
        jpeg,
        "1591fd26e7fff5be97431d0ed3d0ade5cfc5fa74e3d7ec282fd242160ce68c1f"
    ),
    template!(
        "bouquet",
        jpeg,
        "bdd997068701ed3a00a224eb694b003c01ac69b857fe7b4147d6c34875b1632b"
    ),
    template!(
        "boxes",
        png,
        "e64fdcd0b108737d8b8f7b677029f924031d6bbaa50585d9c3def7c7e92ecaf2"
    ),
    template!(
        "boxes-shadow",
        png,
        "639eadad468b6b32b9124b1f4395a8da3027ff7258d102173ba070ae2ed541ae"
    ),
    template!(
        "bubbles",
        png,
        "5eebe803e434a845d19bc600df3c75e98bb69bd0de473ceec410d1b3a9154e28"
    ),
    template!(
        "bubbles-margin-margin",
        jpeg,
        "509a6945facfb3ddc7be6ee8b82797ad0c72db5755486ee878125a959cc09b59"
    ),
    template!(
        "bubbles-margin-top",
        jpeg,
        "e9746b4e9ae9ce7b3b0068779db3e113e2dfc9880f25373d745d0e700e69a906"
    ),
    template!(
        "buildings",
        jpeg,
        "505f731cb7707efab2eb06685b392dc7e59265a40b55aae43e5dc15c0a86cba4"
    ),
    template!(
        "chain-title",
        png,
        "edad0f03e6ff99fef9ef8e8b834ce74f26cd23c5f8c067f5cee66f304181e64d"
    ),
    template!(
        "circles",
        png,
        "e1cb1a0ec9be62d5445c73aa84df38234002a7e164ee830c9df24997802cb5d2"
    ),
    template!(
        "clock",
        jpeg,
        "adb1ebbe18d6cd8ff08aa9bf5c83cdb83bf9aa179698e34e93dbcdde12f04d32"
    ),
    template!(
        "columns",
        jpeg,
        "70475431cca3c91a4efa3b8f04864371d2d3a45696674a1a0562fe9cd8db287c"
    ),
    template!(
        "computer",
        jpeg,
        "9301db6d2d87282fcee450189aeace16d85f64273bf62713a3044992b6b7a9e9"
    ),
    template!(
        "cyan-title",
        jpeg,
        "20016e5fa1a32dce5af4e92872597e36432185a7bb2e61c91f362bd68484529b"
    ),
    template!(
        "day-planner",
        jpeg,
        "035b26df61855a3f36dbd30fdab0c157c04c9e8ae2197ea4d4aeb3e82e6a4c2b"
    ),
    template!(
        "fireworks",
        png,
        "b243a24fa13bc8523450e22f408f9eff15301c938f8ca52a57018b58ce6785de"
    ),
    template!(
        "flat-squares",
        png,
        "1262dd23ad54e935cfa10feb1be56648e43bef1116696ca71d87e6e033b1ca7d"
    ),
    template!(
        "flowers-and-hearts",
        jpeg,
        "423b0dd1a93b391d15b1dc8d8757c3bf5725ff2e7a59e6e3140033e2876b67f6"
    ),
    template!(
        "glasses-corner",
        jpeg,
        "131d637cdc5d0b094fb9fad17f4d2a1ace0d03613588155aacaa2d1cb4e16da9"
    ),
    template!(
        "globe",
        jpeg,
        "20abe389c885e42b6ebe9e902976229bb6fd63c8c34cb61aa70b8b746209f90a"
    ),
    template!(
        "graph",
        png,
        "95f59a0433050180d4c0e8858b83363d51bea6752a8b7ca516a8677854d8f5b6"
    ),
    template!(
        "green-stripes",
        jpeg,
        "d56cefe9ee2fae72e31bdba7dd2aa4426ea22e3ceb22ef68c8f63f9f24d5a8bc"
    ),
    template!(
        "hearts",
        png,
        "2035407a0540e1c4f7934db08ba4add750fcb9a62863ddd9553e7871c81a99e3"
    ),
    template!(
        "ivy",
        jpeg,
        "7ce10d1ea660d2f9cf8b704f3fab2966a4ce2627d9858d32c75d857095012098"
    ),
    template!(
        "large-push-pins",
        jpeg,
        "fb12d59b8be911247bbafdd416852e8b74b028005a141cb4dbbba109b4b6ed2c"
    ),
    template!(
        "light-bulb-corner",
        jpeg,
        "7d2b01f17354d9237a6ab99d5b9afdf0e1cc43687125848b0c2dedfb44ce3843"
    ),
    template!(
        "lilies",
        jpeg,
        "140dd943d0f0cfe6aaa98470b7d1a7cb62ca02cb1d8f522dd2ac77433232ef41"
    ),
    template!(
        "math",
        jpeg,
        "1f81dde3b42f23f0666d92ebf14d62893b31b39d72c07aee070eae28c2e6980e"
    ),
    template!(
        "math-science-class-notes",
        jpeg,
        "ec8cd7250f3d82e900e99114869777ee859ec73effabed108815f65742078c3a"
    ),
    template!(
        "networking",
        png,
        "0401451e1d1d7dfdc29ad1b2b68a6c8ac0b706e9868bf22fab26a01cd48620ce"
    ),
    template!(
        "notebook",
        jpeg,
        "7e6a7714a69688d9ffdf16aa942b66064a0c77fcd9b3e469f89730b4b9290c3e"
    ),
    template!(
        "orange-margin",
        jpeg,
        "be4f0e6c89fce91b9ebd2623567f7dfc259e0e3c77c9158742b8f64b724df673"
    ),
    template!(
        "paper",
        jpeg,
        "7354001527ab554c44e7d6981b86dd933b7dc2e0d3dc8512ad3eecd843245c52"
    ),
    template!(
        "paws-cyan",
        png,
        "e9ead0bfc09d32cb366010cdfede1c432a2d1d550cb7332badac1bee9482bc86"
    ),
    template!(
        "pencil-and-notebook",
        jpeg,
        "28f334b77068f71f5f92a95695433b950610204a0e5580ce567db8fad4993ecb"
    ),
    template!(
        "pens",
        jpeg,
        "3fbe3c1c238bd7dbc67f8cff5f3bddfd513c96a9851b9616477947d21dff4b2e"
    ),
    template!(
        "pixel",
        png,
        "5dc2367a80588a7518db5014122510bf0fd784711015ef83a8718336584f82d0"
    ),
    template!(
        "pixel-blends",
        png,
        "edbb86f160050fbf1f9860276802bae292dbfd0bc98e3ea90d43d981e9f0c54a"
    ),
    template!(
        "plain-and-simple",
        jpeg,
        "de35af949d4f83e97ee22f817afe2531cc4b59ff9ee6026dca7ecebc5cf2737f"
    ),
    template!(
        "purple-clouds",
        jpeg,
        "e111f96490755c7d71e87c88acaea38afe55bb865b1a14a83c5bd239648d5e2c"
    ),
    template!(
        "pushpins-corner",
        png,
        "62ad3c277e54f03f1adb44062407346f789e63859b7afabfd64be6af5e9f66ec"
    ),
    template!(
        "rainbow",
        jpeg,
        "f927c7825851974a2149868146970706523a49165133cee6027a43e8c9abdf27"
    ),
    template!(
        "rainbow-border",
        png,
        "0e31da4dfcff4a36c64c1ce940362d2309769f36369e4c43c317d5f2fa15658e"
    ),
    template!(
        "rainbow-mini-border",
        png,
        "2e7aaf26bec32148b96442e8fff1bd2cef2d72630969f23b9a2abedb6cfec93b"
    ),
    template!(
        "recipe",
        jpeg,
        "525a857d0eda855a64d3619df58b1c2d013a73e60fa0d49b155ecfcb2c134c7c"
    ),
    template!(
        "red-and-black-margin",
        jpeg,
        "b4daa90d5a53fcbc85119050b5b76962443c4dd18d7f42cdc6d4e0ad8efad872"
    ),
    template!(
        "side-stripes",
        jpeg,
        "14fa2d16310485aa1ce41f6d774a3d637e8cf8b03c4f72990155df274fdb6bd9"
    ),
    template!(
        "small-push-pins",
        jpeg,
        "d74c3973c8d1f7c77274691afb1aa934940674341d7eee563be75e563281bdfd"
    ),
    template!(
        "sparks",
        jpeg,
        "f091ded5e283af6848670a3172e7c43c6099875d39b3fc69c2bdba914f609602"
    ),
    template!(
        "stars",
        png,
        "7f5b660a1a0bf46c75aaf19b4f77a0e086de003ec03afc1f58d871d55aa5ba9e"
    ),
    template!(
        "swirls",
        png,
        "0908a4cfa23f93011176d47f45843e9ca2973030421996e8e27484781f54b0ec",
        "8b025b80e7d398229ef19384156fe98aeefcf8f69169d69e1eeba73281b24d7e",
    ),
    template!(
        "tiles",
        jpeg,
        "2a4a3530d652e227ddd5adc096a95f6034718f7c380b07db622022d768815059"
    ),
    template!(
        "triangles-title-margin",
        jpeg,
        "d72900e8da72d1a7f3729971aa558e1e9b6e9cf9a0d51e83852e567256dbbfef"
    ),
    template!(
        "triangles-title-top",
        jpeg,
        "2df453410796aec7b9efec00059b6ce64bcf67313a95ae458ba600ea5de14811"
    ),
    template!(
        "tulips",
        jpeg,
        "5b321a4d81bd499b289b1755f6450a42047c494dfbc112dbd56da4ced2c15c1a"
    ),
    template!(
        "window-reflection",
        jpeg,
        "494547e566fb7a63dd429eb0699fe41aa8998f8ea2f758d813fe3d56c3075719"
    ),
    template!(
        "writing",
        jpeg,
        "5a36960df32817e8426bd40a88f88b04fb55b84baef60f1e71e0872217fdb134"
    ),
];

/// Names the template a picture Snowbound wrote renders: a PNG `tEXt` keyword, and the
/// prefix of a JPEG comment.
const MARKER: &str = "Snowbound template";

/// A raster holds at most this many bytes; page-sized art stops sharpening past about 2.9
/// device pixels per point.
const MAX_RASTER_BYTES: f32 = 16.0 * 1024.0 * 1024.0;

/// Template art standing in for a page's background picture.
pub(super) struct Background {
    svg: &'static str,
    /// Points; OneNote draws a picture without a stored size at its natural size.
    pub size: [f32; 2],
    /// For light and for dark paper.
    variants: [Variant; 2],
}

#[derive(Default)]
struct Variant {
    /// The raster painted and its device pixels per point.
    shown: Option<(RasterImage, f32)>,
    /// The one raster being made and its device pixels per point; replacing it abandons it.
    pending: Option<(Slot<RasterImage>, f32)>,
}

impl Background {
    /// Whether both recreate the same template picture at the same size.
    pub fn same(&self, other: &Self) -> bool {
        self.svg == other.svg && self.size == other.size
    }

    /// OneNote's template pictures by their hash, and those Snowbound wrote by their marker.
    pub fn recognise(picture: &onestore::page::Image) -> Option<Self> {
        let bytes = picture.bytes.as_deref()?;
        let digest: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let template = TEMPLATES
            .iter()
            .find(|template| template.hashes.contains(&digest.as_str()))
            .or_else(|| {
                let name = marker(bytes)?;
                TEMPLATES.iter().find(|template| template.name == name)
            })?;
        let svg = template.svg;
        let size = match (picture.layout.max_width, picture.layout.max_height) {
            (Some(width), Some(height)) => [width, height],
            _ => {
                let size = parse(svg).size();
                [size.width() * 0.75, size.height() * 0.75]
            }
        };
        Some(Self {
            svg,
            size,
            variants: Default::default(),
        })
    }

    /// The latest raster for this paper, if one has landed.
    pub fn image(&self, paper: Paper) -> Option<&RasterImage> {
        let (image, _) = self.variants[usize::from(dark(paper))].shown.as_ref()?;
        Some(image)
    }

    /// Takes a finished raster and asks the worker for one at `scale` device pixels per
    /// point when the shown one differs; `waker` hears when it lands. True once the raster
    /// shown matches.
    pub fn update(&mut self, paper: Paper, scale: f32, waker: &Waker) -> bool {
        let dark = dark(paper);
        let [width, height] = self.size.map(|side| side.max(1.0));
        let density = density(scale)
            .min((MAX_RASTER_BYTES / (4.0 * width * height)).sqrt())
            .min(4096.0 / width.max(height));
        let variant = &mut self.variants[usize::from(dark)];
        let landed = variant
            .pending
            .as_ref()
            .and_then(|(slot, made)| Some((slot.lock().unwrap().take()?, *made)));
        if let Some((image, made)) = landed {
            if made == density || variant.shown.is_none() {
                variant.shown = Some((image, made));
            }
            variant.pending = None;
        }
        let shown = variant.shown.as_ref().map(|(_, made)| *made);
        if shown != Some(density)
            && variant.pending.as_ref().map(|(_, made)| *made) != Some(density)
        {
            let svg = self.svg;
            let paper = dark.then_some(paper);
            let size = [width, height].map(|side| (side * density).round().max(1.0) as u32);
            let start = Instant::now()
                + if shown.is_some() {
                    SETTLE
                } else {
                    Duration::ZERO
                };
            let slot = queue(start, waker, move || rasterize(svg, paper, size));
            variant.pending = Some((slot, density));
        }
        shown == Some(density)
    }
}

fn dark(paper: Paper) -> bool {
    let [lightness, ..] = draw::oklab(paper.color);
    let [ink, ..] = draw::oklab(paper.ink);
    ink > lightness
}

fn rasterize(svg: &str, dark: Option<Paper>, size: [u32; 2]) -> RasterImage {
    let pixels = match dark {
        Some(paper) => render(&onto(svg, paper), size),
        None => render(svg, size),
    };
    RasterImage::new(size, pixels).expect("Art is within the image budget")
}

/// Straight-alpha RGBA rows of `svg` stretched to `width` by `height` pixels.
fn render(svg: &str, [width, height]: [u32; 2]) -> Vec<u8> {
    let tree = parse(svg);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("Art has a size");
    let view = tree.size();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(
            width as f32 / view.width(),
            height as f32 / view.height(),
        ),
        &mut pixmap.as_mut(),
    );
    pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect()
}

/// Template art `name` for a preview: drawn `size` points big, or at its natural size, at
/// `density` pixels per point for `paper`, with the size in points it was drawn at.
pub fn template_art(
    name: &str,
    size: Option<[f32; 2]>,
    density: f32,
    paper: Paper,
) -> Option<(RasterImage, [f32; 2])> {
    let template = TEMPLATES.iter().find(|template| template.name == name)?;
    let size = size.unwrap_or_else(|| {
        let view = parse(template.svg).size();
        [view.width() * 0.75, view.height() * 0.75]
    });
    let pixels = size.map(|side| (side * density).round().max(1.0) as u32);
    Some((
        rasterize(template.svg, dark(paper).then_some(paper), pixels),
        size,
    ))
}

/// Template `name`'s art as Snowbound stores it for a page: in the format and pixel size of
/// OneNote's picture, marked so that recognising it paints the art.
pub fn template_picture(name: &str) -> Option<Vec<u8>> {
    let template = TEMPLATES.iter().find(|template| template.name == name)?;
    let view = parse(template.svg).size();
    let [width, height] = [view.width(), view.height()].map(|side| side.round() as u32);
    // Over white paper, as OneNote's pictures are drawn.
    let rgb: Vec<u8> = render(template.svg, [width, height])
        .chunks_exact(4)
        .flat_map(|pixel| {
            let alpha = u16::from(pixel[3]);
            [0, 1, 2].map(|channel| {
                ((u16::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
            })
        })
        .collect();
    let mut encoded = Vec::new();
    if template.jpeg {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 90)
            .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
            .expect("Art encodes");
        // The comment follows the JFIF header, which must lead.
        let [_, _, _, _, high, low, ..] = encoded[..] else {
            unreachable!("A JPEG starts with its JFIF header")
        };
        let at = 4 + usize::from(u16::from_be_bytes([high, low]));
        let comment = format!("{MARKER} {name}");
        let length = u16::try_from(comment.len() + 2).expect("A short comment");
        let segment = [&[0xff, 0xfe][..], &length.to_be_bytes(), comment.as_bytes()].concat();
        encoded.splice(at..at, segment);
    } else {
        // OneNote's PNG templates hold 256 colours.
        let rgba: Vec<u8> = rgb
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect();
        let quantized = color_quant::NeuQuant::new(10, 256, &rgba);
        let indices: Vec<u8> = rgba
            .chunks_exact(4)
            .map(|pixel| quantized.index_of(pixel) as u8)
            .collect();
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(quantized.color_map_rgb());
        encoder.set_compression(png::Compression::High);
        // Palette indices compress best unfiltered.
        encoder.set_filter(png::Filter::NoFilter);
        encoder
            .add_text_chunk(MARKER.into(), name.into())
            .expect("A Latin-1 keyword");
        let mut writer = encoder.write_header().expect("Art encodes");
        writer.write_image_data(&indices).expect("Art encodes");
        writer.finish().expect("Art encodes");
    }
    Some(encoded)
}

/// The template a picture Snowbound wrote names: a PNG's text chunk before its image data,
/// or a JPEG's comment before its scan.
fn marker(picture: &[u8]) -> Option<&str> {
    fn named(text: &[u8], separator: u8) -> Option<&str> {
        let name = text
            .strip_prefix(MARKER.as_bytes())?
            .strip_prefix(&[separator])?;
        std::str::from_utf8(name).ok()
    }
    if let Some(mut rest) = picture.strip_prefix(b"\x89PNG\r\n\x1a\n") {
        while let [a, b, c, d, kind @ ..] = rest {
            let length = u32::from_be_bytes([*a, *b, *c, *d]) as usize;
            match kind.get(..4)? {
                b"IDAT" => return None,
                b"tEXt" => {
                    if let Some(name) = named(kind.get(4..4 + length)?, 0) {
                        return Some(name);
                    }
                }
                _ => {}
            }
            rest = rest.get(12 + length..)?;
        }
        return None;
    }
    let mut rest = picture.strip_prefix(&[0xff, 0xd8])?;
    while let [0xff, kind, high, low, ..] = *rest {
        let length = usize::from(u16::from_be_bytes([high, low]));
        match kind {
            0xda => return None,
            0xfe => {
                if let Some(name) = named(rest.get(4..2 + length)?, b' ') {
                    return Some(name);
                }
            }
            _ => {}
        }
        rest = rest.get(2 + length..)?;
    }
    None
}

fn parse(svg: &str) -> usvg::Tree {
    usvg::Tree::from_str(svg, &usvg::Options::default()).expect("Bundled template art is valid")
}

/// Recolours `#rrggbb` art drawn for white paper onto dark `paper`: lightness turns about
/// the paper towards its ink, easing off so the art stays behind the text, and hue and
/// chroma carry over. Keywords, such as a mask's `white`, stay as they are.
fn onto(svg: &str, paper: Paper) -> String {
    let [lightness, a, b] = draw::oklab(paper.color);
    let [ink, ..] = draw::oklab(paper.ink);
    let mut pieces = svg.split('#');
    let mut recoloured = pieces.next().unwrap_or_default().to_owned();
    for piece in pieces {
        let color = piece
            .get(..6)
            .filter(|_| !piece[6..].starts_with(|c: char| c.is_ascii_alphanumeric()))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok());
        let Some(color) = color else {
            recoloured.push('#');
            recoloured.push_str(piece);
            continue;
        };
        let [_, red, green, blue] = color.to_be_bytes();
        let [l, ca, cb] = draw::oklab(draw::srgb(red, green, blue));
        let turned = lightness + 0.7 * (1.0 - l).max(0.0).sqrt() * (ink - lightness);
        let [red, green, blue] = draw::from_oklab([turned, a + ca, b + cb]).map(|value| {
            let encoded = if value <= 0.003_130_8 {
                value * 12.92
            } else {
                1.055 * value.powf(1.0 / 2.4) - 0.055
            };
            (encoded * 255.0).round() as u8
        });
        recoloured.push_str(&format!("#{red:02x}{green:02x}{blue:02x}"));
        recoloured.push_str(&piece[6..]);
    }
    recoloured
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gpu::page::PageScene, layout::TextEngine};
    use onestore::{
        ExGuid,
        document::Layout,
        page::{Image, Page, PageObject},
    };
    use std::{collections::BTreeMap, sync::Arc};

    const DARK: Paper = Paper {
        color: [0.0137, 0.0144, 0.0159, 1.0],
        ink: [0.791, 0.791, 0.791, 1.0],
    };

    fn picture(bytes: &[u8]) -> Image {
        Image {
            id: ExGuid::default(),
            layout: Layout {
                x: Some(0.0),
                y: Some(0.0),
                max_width: Some(64.0),
                max_height: Some(48.0),
                ..Default::default()
            },
            size: None,
            bytes: Some(Arc::from(bytes)),
            alt: None,
            background: true,
        }
    }

    fn red_png() -> Vec<u8> {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 255])
            .unwrap();
        encoded
    }

    fn ivy() -> Background {
        Background {
            svg: TEMPLATES.iter().find(|t| t.name == "ivy").unwrap().svg,
            size: [64.0, 48.0],
            variants: Default::default(),
        }
    }

    fn settle(mut update: impl FnMut() -> bool) {
        let give_up = Instant::now() + Duration::from_secs(10);
        while !update() {
            assert!(Instant::now() < give_up, "the worker never delivered");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn rasters_land_from_the_worker_and_follow_the_zoom() {
        let mut art = ivy();
        assert!(!art.update(DARK, 1.0, Waker::noop()));
        assert!(art.image(DARK).is_none());
        settle(|| art.update(DARK, 1.0, Waker::noop()));
        let size = |art: &Background| art.image(DARK).map(|image| image.pixels().len() / 4);
        assert_eq!(size(&art), Some(64 * 48));
        assert!(
            art.image(Paper::WHITE).is_none(),
            "each paper has its own raster"
        );
        // Zooming keeps the last raster up while one for the new scale is made, and a
        // newer scale abandons the older request.
        assert!(!art.update(DARK, 4.0, Waker::noop()));
        let abandoned = Arc::downgrade(&art.variants[1].pending.as_ref().unwrap().0);
        assert!(!art.update(DARK, 2.0, Waker::noop()));
        assert!(abandoned.upgrade().is_none());
        assert_eq!(size(&art), Some(64 * 48));
        settle(|| art.update(DARK, 2.0, Waker::noop()));
        assert_eq!(size(&art), Some(128 * 96));
        assert!(art.variants[1].pending.is_none());
    }

    #[test]
    fn every_recreation_parses_for_light_and_dark_paper_under_distinct_names_and_hashes() {
        let mut names = std::collections::BTreeSet::new();
        let mut hashes = std::collections::BTreeSet::new();
        for template in TEMPLATES {
            assert!(
                names.insert(template.name),
                "{} is listed twice",
                template.name
            );
            for hash in template.hashes {
                assert!(
                    hash.len() == 64
                        && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                );
                assert!(hashes.insert(hash), "{hash} is listed twice");
            }
            for svg in [template.svg.to_string(), onto(template.svg, DARK)] {
                usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap();
            }
        }
    }

    #[test]
    fn our_template_pictures_are_recognised_as_the_template() {
        for (name, magic, size) in [
            ("ivy", &b"\xff\xd8\xff\xe0"[..], [216, 792]),
            ("hearts", &b"\x89PNG"[..], [171, 552]),
        ] {
            let bytes = template_picture(name).unwrap();
            assert!(bytes.starts_with(magic), "{name} keeps OneNote's format");
            assert_eq!(draw::RasterImage::measure(&bytes).unwrap(), size);
            assert_eq!(marker(&bytes), Some(name));
            let art = Background::recognise(&picture(&bytes)).unwrap();
            let template = TEMPLATES.iter().find(|t| t.name == name).unwrap();
            assert_eq!(art.svg, template.svg);
            assert!(!template.hashes.iter().any(|hash| {
                let digest: String = Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                digest == *hash
            }));
        }
        assert!(template_picture("not a template").is_none());
        assert_eq!(marker(&red_png()), None);
    }

    #[test]
    fn the_sample_notebook_carries_our_ivy() {
        let bytes = std::fs::read("../../docs/sample-notebook/Personal/Garden.one").unwrap();
        let store = onestore::Store::parse(&bytes).unwrap();
        let index = onestore::RevisionIndex::parse(&store).unwrap();
        let document = onestore::document::Document::parse(&index).unwrap();
        let mut backgrounds = 0;
        for (space, id) in document.pages().unwrap() {
            let page = Page::from_revision(document.active(space).unwrap(), id).unwrap();
            for object in &page.objects {
                if let PageObject::Image(image) = object
                    && image.background
                {
                    assert_eq!(marker(image.bytes.as_deref().unwrap()), Some("ivy"));
                    assert!(Background::recognise(image).is_some());
                    backgrounds += 1;
                }
            }
        }
        assert_eq!(backgrounds, 1);
    }

    #[test]
    fn other_pictures_keep_their_own_bytes() {
        assert!(Background::recognise(&picture(&red_png())).is_none());
        assert!(
            Background::recognise(&Image {
                bytes: None,
                ..picture(&[])
            })
            .is_none()
        );
    }

    #[test]
    fn dark_paper_takes_white_to_the_paper_and_keeps_hue() {
        assert_eq!(onto(r##"fill="#ffffff""##, DARK), r##"fill="#1f2022""##);
        assert_eq!(
            onto("href=\"#beads\" stop-color=\"white\"", DARK),
            "href=\"#beads\" stop-color=\"white\""
        );
        let leaf = onto("#8fce73", DARK);
        let [red, green, blue] =
            [1, 3, 5].map(|at| u8::from_str_radix(&leaf[at..at + 2], 16).unwrap());
        assert!(green > red && green > blue && green < 0xce, "{leaf}");
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION pointing to a section whose pages use OneNote's page templates"]
    fn recognises_template_backgrounds_in_a_section() {
        let bytes = std::fs::read(std::env::var("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let store = onestore::Store::parse(&bytes).unwrap();
        let index = onestore::RevisionIndex::parse(&store).unwrap();
        let document = onestore::document::Document::parse(&index).unwrap();
        let mut recognised = 0;
        for (space, id) in document.pages().unwrap() {
            let page = Page::from_revision(document.active(space).unwrap(), id).unwrap();
            for object in &page.objects {
                if let PageObject::Image(image) = object
                    && image.background
                {
                    assert!(Background::recognise(image).is_some(), "{:?}", image.id);
                    recognised += 1;
                }
            }
        }
        assert!(recognised > 0);
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn dark_paper_paints_the_dark_recreation_instead_of_the_embedded_picture() {
        let background = picture(&red_png());
        let id = background.id;
        let page = Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            color: None,
            definitions: BTreeMap::new(),
            objects: vec![PageObject::Image(background)],
        };
        let mut scene = PageScene::new(page, &mut TextEngine::default()).unwrap();
        scene.backgrounds.insert(id, ivy());
        let mut before = Vec::new();
        scene
            .append_primitives(&mut before, [0.0; 2], DARK)
            .unwrap();
        assert!(before.is_empty(), "the paper shows until the raster lands");
        drop(before);
        scene.settle(None, 1.0, DARK);
        let mut primitives = Vec::new();
        scene
            .append_primitives(&mut primitives, [0.0; 2], DARK)
            .unwrap();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let size = [64_u32, 48];
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Background readback test"),
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
            label: Some("Background readback"),
            size: u64::from(size[0] * size[1] * 4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut renderer = draw::Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let viewport = crate::gpu::Viewport {
            size,
            scale: 1.0,
            origin: [0.0; 2],
        };
        renderer
            .draw(
                &target.create_view(&Default::default()),
                size,
                DARK.color,
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
        let pixels: Vec<[u8; 3]> = pixels.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect();
        assert!(
            pixels
                .iter()
                .all(|[r, g, b]| !(*r > 200 && *g < 80 && *b < 80)),
            "the embedded picture was painted"
        );
        // Leaves on dark paper stay green, darker than they are on white.
        assert!(
            pixels
                .iter()
                .any(|[r, g, b]| g > r && g > b && *g > 0x40 && *g < 0xce),
            "the dark recreation was not painted"
        );
        assert!(
            pixels.iter().all(|[_, g, _]| *g < 0xe0),
            "the art kept white paper"
        );
    }
}
