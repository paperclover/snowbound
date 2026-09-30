//! Pictures Snowbound draws for tags in place of their symbols. A notebook maps a tag, by
//! the name and symbol its definition stores, to art of its own; OneNote 2010, which knows
//! nothing of the art, draws the symbol.

use super::{CHECKMARK, tag_sources};
use crate::outline::{ParagraphTag, TagIcon};
use resvg::{tiny_skia, usvg};
use std::{collections::BTreeMap, sync::Mutex};

/// The longest side art is drawn from, in pixels: a 24 pt tag at 200% on a 2× display.
const SIDE: u32 = 128;

/// Icon sources drawing a tag's art, unchecked and checked.
type Sources = [&'static [&'static str]; 2];

/// A picked PNG or SVG prepared for keeping as tag art, with its extension: an SVG as it is,
/// a PNG no larger than the art is drawn from. None where it is neither, or draws nothing.
pub fn import(bytes: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    let drawn = drawn(bytes)?;
    Some(match svg(bytes) {
        Some(_) => (bytes.to_vec(), "svg"),
        None => (drawn, "png"),
    })
}

/// The sources drawing art named `art`, made from what `bytes` reads the first time `art`
/// is asked for; none where that is neither a PNG nor an SVG. Art is named by its content,
/// so what one name draws never changes.
pub fn art_sources(art: &str, bytes: impl FnOnce() -> Option<Vec<u8>>) -> Option<Sources> {
    static MADE: Mutex<BTreeMap<String, Option<Sources>>> = Mutex::new(BTreeMap::new());
    let mut made = MADE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(made) = made.get(art) {
        return *made;
    }
    let sources = bytes()
        .and_then(|bytes| drawn(&bytes))
        .and_then(|png| draw::picture_icon(&png))
        .map(|source| {
            // Each art's sources are made once and drawn for the program's life.
            let source: &'static str = source.leak();
            let plain: &'static [&'static str] = Box::leak(Box::new([source]));
            let checked: &'static [&'static str] = Box::leak(Box::new([source, CHECKMARK]));
            [plain, checked]
        });
    made.insert(art.to_owned(), sources);
    sources
}

/// Art as a PNG no larger than `SIDE`: an SVG drawn, a PNG scaled down; none where it is
/// neither, or draws nothing.
fn drawn(bytes: &[u8]) -> Option<Vec<u8>> {
    if let Some(tree) = svg(bytes) {
        return raster(&tree);
    }
    let picture = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let [width, height] = fit([picture.width(), picture.height()]);
    let picture = picture.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    Some(png(&picture.into_rgba8()))
}

/// An SVG document, reading no file it names.
fn svg(bytes: &[u8]) -> Option<usvg::Tree> {
    let mut options = usvg::Options::default();
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    usvg::Tree::from_data(bytes, &options).ok()
}

/// `tree` drawn as a PNG at its proportions no larger than `SIDE`; none where it paints
/// nothing.
fn raster(tree: &usvg::Tree) -> Option<Vec<u8>> {
    let view = tree.size();
    let [width, height] = fit([view.width(), view.height()].map(|side| side.ceil() as u32));
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(
            width as f32 / view.width(),
            height as f32 / view.height(),
        ),
        &mut pixmap.as_mut(),
    );
    let rgba: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    if rgba.chunks_exact(4).all(|pixel| pixel[3] == 0) {
        return None;
    }
    Some(png(&image::RgbaImage::from_raw(width, height, rgba)?))
}

/// A picture of `size` scaled to fit `SIDE` at its proportions, never larger.
fn fit(size: [u32; 2]) -> [u32; 2] {
    let longest = size[0].max(size[1]).max(1);
    if longest <= SIDE {
        return size.map(|side| side.max(1));
    }
    size.map(|side| (u64::from(side) * u64::from(SIDE) / u64::from(longest)).max(1) as u32)
}

fn png(picture: &image::RgbaImage) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, picture.width(), picture.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(picture.as_raw()))
        .expect("A PNG encodes into memory");
    encoded
}

/// The art a notebook's tags draw with, by the name and symbol their definitions store.
#[derive(Clone, Default)]
pub struct TagArt(Vec<Mapped>);

#[derive(Clone)]
struct Mapped {
    name: String,
    shape: u16,
    art: String,
    sources: Sources,
}

impl TagArt {
    /// Draws tags named `name` with symbol `shape` with art `art`, drawn by `sources`, in
    /// place of any art they drew with before.
    pub fn map(&mut self, name: &str, shape: u16, art: &str, sources: Sources) {
        self.0
            .retain(|mapped| (mapped.name.as_str(), mapped.shape) != (name, shape));
        self.0.push(Mapped {
            name: name.to_owned(),
            shape,
            art: art.to_owned(),
            sources,
        });
    }

    /// The art tags named `name` with symbol `shape` draw with.
    pub fn art(&self, name: &str, shape: u16) -> Option<&str> {
        self.find(name, shape).map(|mapped| mapped.art.as_str())
    }

    /// What draws `tag`: its art, or its symbol.
    pub fn sources(&self, tag: &ParagraphTag) -> &'static [&'static str] {
        match tag.icon {
            TagIcon::Symbol { shape, checked } => self.find(&tag.label, shape).map_or_else(
                || tag_sources(tag.icon),
                |mapped| mapped.sources[usize::from(checked)],
            ),
            TagIcon::Task { .. } => tag_sources(tag.icon),
        }
    }

    fn find(&self, name: &str, shape: u16) -> Option<&Mapped> {
        self.0
            .iter()
            .find(|mapped| mapped.name == name && mapped.shape == shape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(width: u32, height: u32) -> Vec<u8> {
        png(&image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([200, 40, 40, 255]),
        ))
    }

    #[test]
    fn pictures_keep_their_proportions_within_the_largest_side() {
        let (kept, extension) = import(&encoded(400, 100)).unwrap();
        assert_eq!(extension, "png");
        let picture = image::load_from_memory(&kept).unwrap();
        assert_eq!([picture.width(), picture.height()], [SIDE, SIDE / 4]);
        let small = encoded(20, 30);
        let (kept, _) = import(&small).unwrap();
        let picture = image::load_from_memory(&kept).unwrap();
        assert_eq!([picture.width(), picture.height()], [20, 30]);
        assert!(import(b"neither").is_none());
    }

    #[test]
    fn drawings_are_kept_as_written_unless_they_draw_nothing() {
        let drawing = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#3070f0"/></svg>"##;
        assert_eq!(import(drawing), Some((drawing.to_vec(), "svg")));
        let empty = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"/>"#;
        assert!(import(empty).is_none());
        // A drawing naming a file on this computer reads none of it.
        let named = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><image href="/etc/hosts" width="10" height="10"/></svg>"#;
        assert!(import(named).is_none());
    }

    #[test]
    fn mapped_tags_draw_their_art_checked_or_not() {
        let art = art_sources("test-art", || Some(encoded(8, 8))).unwrap();
        // Made once: a second ask reads nothing.
        assert_eq!(art_sources("test-art", || unreachable!()), Some(art));
        assert_eq!(art_sources("test-broken", || Some(b"x".to_vec())), None);
        let mut map = TagArt::default();
        map.map("Launch", 13, "test-art", art);
        let tag = |label: &str, shape, checked| ParagraphTag {
            icon: TagIcon::Symbol { shape, checked },
            origin: [0.0; 2],
            size: ParagraphTag::SIZE,
            label: label.into(),
            disabled: false,
        };
        assert_eq!(map.sources(&tag("Launch", 13, false)), art[0]);
        assert_eq!(map.sources(&tag("Launch", 13, true)), art[1]);
        assert_eq!(art[1].last(), Some(&CHECKMARK));
        let symbol = TagIcon::of(13, false).unwrap();
        assert_eq!(
            map.sources(&tag("Launch", 15, false)),
            tag_sources(TagIcon::of(15, false).unwrap())
        );
        assert_eq!(map.sources(&tag("Other", 13, false)), tag_sources(symbol));
        assert_eq!(map.art("Launch", 13), Some("test-art"));
        map.map("Launch", 13, "test-other", art);
        assert_eq!(map.art("Launch", 13), Some("test-other"));
    }
}
