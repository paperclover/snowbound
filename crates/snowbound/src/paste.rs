//! Pictures and files from the clipboard, and pictures inserted, as OneNote 2010 places them.

use crate::{State, platform};
use canvas::editor::Piece;
use std::{
    error::Error,
    path::{Path, PathBuf},
};

/// What Paste takes from the clipboard. Files come first, as Finder also offers their names
/// as text and their icons as pictures; then a page holding pictures, as its text alone
/// would lose them; text before a picture, as Office and browsers offer a picture of copied
/// text beside it.
pub(crate) enum Pasted {
    Files(Vec<PathBuf>),
    /// HTML holding a picture.
    Page(String),
    Text(String),
    Picture(Vec<u8>),
}

impl State {
    pub(crate) fn paste(&mut self) -> Result<(), Box<dyn Error>> {
        match self.clipboard.pasted() {
            Some(Pasted::Files(paths)) => {
                for path in paths {
                    self.place_file(&path, None)?;
                }
            }
            Some(Pasted::Page(html)) => {
                // Web pictures come later, so the window never waits on the network.
                let mut fetched = Vec::new();
                let pieces = html_pieces(&html, |source, css| {
                    if source.starts_with("https://") || source.starts_with("http://") {
                        fetched.push((source.to_owned(), css));
                        return Some(Piece::Awaited);
                    }
                    let bytes = load(source)?;
                    let size = sized(&bytes, css)?;
                    Some(Piece::Picture(bytes, size))
                });
                let language = canvas::language::lcid(&platform::input_language());
                let (places, response) = self.view.paste_pieces(pieces, language)?;
                self.respond(response);
                for (place, (source, css)) in places.into_iter().zip(fetched) {
                    let proxy = self.proxy.clone();
                    crate::spawn(move || {
                        let Some(bytes) = fetch(&source) else {
                            return;
                        };
                        let Some(size) = sized(&bytes, css) else {
                            return;
                        };
                        let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                            let response = state.view.insert_awaited(place, bytes, size)?;
                            state.respond(response);
                            Ok(())
                        })));
                    });
                }
            }
            Some(Pasted::Text(text)) => {
                let language = canvas::language::lcid(&platform::input_language());
                let response = self.view.paste(&text, language)?;
                self.respond(response);
            }
            Some(Pasted::Picture(bytes)) => self.insert_picture(bytes, None)?,
            None => {}
        }
        Ok(())
    }

    /// A pasted file, or one dropped at `at`, a window point: a picture file as its picture,
    /// as OneNote 2010 pastes one (lab, 2026-09-30), and any other file attached.
    pub(crate) fn place_file(
        &mut self,
        path: &Path,
        at: Option<[f32; 2]>,
    ) -> Result<(), Box<dyn Error>> {
        match notebook::fs::read(path) {
            Ok(bytes) if draw::RasterImage::measure(&bytes).is_ok() => {
                self.insert_picture(bytes, at)
            }
            _ => self.attach(path, at),
        }
    }

    /// Puts an encoded picture at the caret, or where it is dropped at `at`, a window point,
    /// at the size its resolution gives it, as OneNote 2010 inserts and pastes one.
    pub(crate) fn insert_picture(
        &mut self,
        bytes: Vec<u8>,
        at: Option<[f32; 2]>,
    ) -> Result<(), Box<dyn Error>> {
        let Ok(pixels) = draw::RasterImage::measure(&bytes) else {
            platform::alert(
                "Couldn't insert the picture",
                "Choose a PNG, JPEG or GIF picture.",
            );
            return Ok(());
        };
        let size = picture_size(&bytes, pixels);
        let response = match at {
            Some(point) => self
                .view
                .drop_picture(self.page_point(point), bytes, size)?,
            None => self.view.insert_picture(bytes, size)?,
        };
        self.respond(response);
        Ok(())
    }
}

/// A web page fragment's text, a line to each block, and its pictures in order, as OneNote
/// 2010 pastes them (lab, 2026-09-30). `load` makes a picture's piece from its source and
/// its `width` and `height` in CSS pixels; one it can't is left out.
fn html_pieces(
    html: &str,
    mut load: impl FnMut(&str, [Option<f32>; 2]) -> Option<Piece>,
) -> Vec<Piece> {
    let html = html
        .split_once("<!--StartFragment-->")
        .map_or(html, |(_, fragment)| fragment);
    let html = html
        .split_once("<!--EndFragment-->")
        .map_or(html, |(fragment, _)| fragment);
    let mut pieces = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, pieces: &mut Vec<Piece>| {
        let trimmed = text.trim_matches(|c: char| c.is_whitespace());
        if !trimmed.is_empty() {
            pieces.push(Piece::Text(trimmed.to_owned()));
        }
        text.clear();
    };
    let line = |text: &mut String| {
        text.truncate(text.trim_end_matches(' ').len());
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
    };
    // Elements whose content is not text: script, style, head and title.
    let mut hidden: Option<String> = None;
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.split_once("-->").map_or("", |(_, after)| after);
            continue;
        }
        let Some(after) = rest.strip_prefix('<') else {
            let end = rest.find('<').unwrap_or(rest.len());
            if hidden.is_none() {
                for c in decode_entities(&rest[..end]).chars() {
                    if c.is_whitespace() && c != '\u{a0}' {
                        if !text.is_empty() && !text.ends_with([' ', '\n']) {
                            text.push(' ');
                        }
                    } else {
                        text.push(if c == '\u{a0}' { ' ' } else { c });
                    }
                }
            }
            rest = &rest[end..];
            continue;
        };
        let end = tag_end(after);
        let tag = &after[..end];
        rest = after.get(end + 1..).unwrap_or("");
        let (closing, tag) = match tag.strip_prefix('/') {
            Some(tag) => (true, tag),
            None => (false, tag),
        };
        let name = tag
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if let Some(until) = &hidden {
            if closing && name == *until {
                hidden = None;
            }
            continue;
        }
        match name.as_str() {
            "script" | "style" | "head" | "title" if !closing => hidden = Some(name),
            "br" => {
                text.truncate(text.trim_end_matches(' ').len());
                text.push('\n');
            }
            "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote"
            | "pre" | "table" | "ul" | "ol" | "dt" | "dd" | "section" | "article" | "header"
            | "footer" | "figure" | "figcaption" => line(&mut text),
            "img" if !closing => {
                let css = |name| {
                    attribute(tag, name)
                        .and_then(|value| value.trim_end_matches("px").parse::<f32>().ok())
                        .filter(|pixels| *pixels > 0.0)
                };
                let Some(picture) = attribute(tag, "src").and_then(|source| {
                    load(&decode_entities(source), [css("width"), css("height")])
                }) else {
                    continue;
                };
                flush(&mut text, &mut pieces);
                pieces.push(picture);
            }
            _ => {}
        }
    }
    flush(&mut text, &mut pieces);
    pieces
}

/// Where a tag's text ends, before its `>`, outside quoted attribute values.
fn tag_end(tag: &str) -> usize {
    let mut quote = None;
    for (at, c) in tag.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '>') => return at,
            _ => {}
        }
    }
    tag.len()
}

/// The value of attribute `name` in `tag`, entities still encoded.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(at) = rest.find('=') {
        let key = rest[..at].trim_end().rsplit(char::is_whitespace).next()?;
        let value = rest[at + 1..].trim_start();
        let (value, after) = match value.chars().next()? {
            quote @ ('"' | '\'') => value[1..].split_once(quote)?,
            _ => value.split_at(value.find(char::is_whitespace).unwrap_or(value.len())),
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
        rest = after;
    }
    None
}

fn decode_entities(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .map(|end| &rest[1..end + 1]);
        let c = entity.and_then(|entity| match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => {
                let number = entity.strip_prefix('#')?;
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (c, entity) {
            (Some(c), Some(entity)) => {
                decoded.push(c);
                rest = &rest[entity.len() + 2..];
            }
            _ => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

/// A picture's size in points from its `width` and `height` in CSS pixels, either giving
/// the other by its proportions, or else from its resolution; none where it doesn't decode.
fn sized(bytes: &[u8], css: [Option<f32>; 2]) -> Option<[f32; 2]> {
    let natural = picture_size(bytes, draw::RasterImage::measure(bytes).ok()?);
    Some(match css {
        [Some(width), Some(height)] => [width * 0.75, height * 0.75],
        [Some(width), None] => [width * 0.75, natural[1] * width * 0.75 / natural[0]],
        [None, Some(height)] => [natural[0] * height * 0.75 / natural[1], height * 0.75],
        [None, None] => natural,
    })
}

/// A picture's bytes from a `data:` address or a local file; none from a relative address,
/// which has no page to resolve against.
fn load(source: &str) -> Option<Vec<u8>> {
    if let Some(data) = source.strip_prefix("data:") {
        let (kind, payload) = data.split_once(',')?;
        return if kind.ends_with(";base64") {
            base64(payload)
        } else {
            Some(percent_decode(payload))
        };
    }
    if let Some(path) = source.strip_prefix("file://") {
        let path = String::from_utf8(percent_decode(path)).ok()?;
        // file:///C:/… on Windows.
        let path = match path.strip_prefix('/') {
            Some(drive) if cfg!(windows) => drive.to_owned(),
            _ => path,
        };
        return notebook::fs::read(path).ok();
    }
    None
}

/// None in the browser, whose pages rarely let another site read their pictures.
#[cfg(target_arch = "wasm32")]
fn fetch(_: &str) -> Option<Vec<u8>> {
    None
}

/// A web picture's bytes, on a thread of its own.
#[cfg(not(target_arch = "wasm32"))]
fn fetch(source: &str) -> Option<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .into();
    agent
        .get(source)
        .call()
        .ok()?
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_vec()
        .ok()
}

pub(crate) fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                at += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    decoded
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut count) = (0_u32, 0);
    for c in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
    {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        bits = bits << 6 | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            decoded.push((bits >> count) as u8);
        }
    }
    Some(decoded)
}

/// A picture's size in points: its pixels at the resolution it declares, or at 96 dpi
/// without one, as OneNote 2010 sizes inserted and pasted pictures and pasted bitmaps
/// (lab, 2026-09-30).
fn picture_size(encoded: &[u8], pixels: [u32; 2]) -> [f32; 2] {
    let dpi = resolution(encoded).unwrap_or([96.0; 2]);
    [0, 1].map(|axis| pixels[axis] as f32 * 72.0 / dpi[axis])
}

/// Dots per inch from a PNG's `pHYs`, or a JPEG's JFIF density or else its Exif
/// resolution, where they give one.
fn resolution(encoded: &[u8]) -> Option<[f32; 2]> {
    let dpi = match encoded.strip_prefix(b"\x89PNG\r\n\x1a\n") {
        Some(chunks) => png_resolution(chunks)?,
        None => jpeg_resolution(encoded.strip_prefix(b"\xff\xd8")?)?,
    };
    dpi.iter().all(|dpi| *dpi > 0.0).then_some(dpi)
}

fn png_resolution(mut chunks: &[u8]) -> Option<[f32; 2]> {
    loop {
        let length = u32::from_be_bytes(chunks.get(..4)?.try_into().ok()?) as usize;
        let kind = chunks.get(4..8)?;
        let data = chunks.get(8..8 + length)?;
        if kind == b"IDAT" {
            return None;
        }
        // Unit 1 is the metre.
        if kind == b"pHYs" && data.len() == 9 && data[8] == 1 {
            let per_metre = |at: usize| u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
            return Some([per_metre(0), per_metre(4)].map(|dots| dots as f32 * 0.0254));
        }
        chunks = chunks.get(12 + length..)?;
    }
}

/// `segments` follow the start-of-image marker.
fn jpeg_resolution(mut segments: &[u8]) -> Option<[f32; 2]> {
    let mut exif = None;
    while let [0xff, marker, high, low, rest @ ..] = segments {
        let length = usize::from(u16::from_be_bytes([*high, *low])).checked_sub(2)?;
        let data = rest.get(..length)?;
        match (*marker, data) {
            (
                0xe0,
                [
                    b'J',
                    b'F',
                    b'I',
                    b'F',
                    0,
                    _,
                    _,
                    units @ (1 | 2),
                    x0,
                    x1,
                    y0,
                    y1,
                    ..,
                ],
            ) => {
                let dots = [[*x0, *x1], [*y0, *y1]].map(|dots| f32::from(u16::from_be_bytes(dots)));
                if dots.iter().all(|dots| *dots > 0.0) {
                    return Some(if *units == 1 {
                        dots
                    } else {
                        dots.map(|per_cm| per_cm * 2.54)
                    });
                }
            }
            (0xe1, [b'E', b'x', b'i', b'f', 0, 0, tiff @ ..]) => exif = exif_resolution(tiff),
            // The scan follows.
            (0xda, _) => break,
            _ => {}
        }
        segments = &rest[length..];
    }
    exif
}

/// XResolution and YResolution from an Exif block's first directory, in their unit.
fn exif_resolution(tiff: &[u8]) -> Option<[f32; 2]> {
    let big = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let bytes = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let bytes = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        })
    };
    let directory = u32_at(4)? as usize;
    let (mut x, mut y, mut unit) = (None, None, 2);
    for entry in 0..usize::from(u16_at(directory)?) {
        let at = directory + 2 + entry * 12;
        let rational = || {
            let offset = u32_at(at + 8)? as usize;
            let [numerator, denominator] = [u32_at(offset)?, u32_at(offset + 4)?];
            (denominator > 0).then(|| numerator as f32 / denominator as f32)
        };
        match u16_at(at)? {
            0x011a => x = rational(),
            0x011b => y = rational(),
            0x0128 => unit = u16_at(at + 8)?,
            _ => {}
        }
    }
    let dots = [x?, y?];
    match unit {
        2 => Some(dots),
        3 => Some(dots.map(|per_cm| per_cm * 2.54)),
        _ => None,
    }
}

/// A bitmap from the clipboard as the PNG a page stores, without a resolution, so it takes
/// 96 dpi as OneNote 2010 gives a pasted bitmap.
#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
pub(crate) fn bitmap(image: arboard::ImageData) -> Option<Vec<u8>> {
    png(
        [image.width, image.height].map(|side| side as u32),
        &image.bytes,
    )
    .ok()
}

/// Straight RGBA pixels as a PNG without a resolution.
pub(crate) fn png(size: [u32; 2], rgba: &[u8]) -> Result<Vec<u8>, png::EncodingError> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, size[0], size[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_at(pixels_per_metre: Option<u32>) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 200, 100);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_pixel_dims(pixels_per_metre.map(|dots| png::PixelDimensions {
            xppu: dots,
            yppu: dots,
            unit: png::Unit::Meter,
        }));
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[0; 200 * 100 * 4])
            .unwrap();
        bytes
    }

    fn jfif(units: u8, density: u16) -> Vec<u8> {
        let mut bytes = b"\xff\xd8\xff\xe0\x00\x10JFIF\0\x01\x01".to_vec();
        bytes.push(units);
        bytes.extend(density.to_be_bytes());
        bytes.extend(density.to_be_bytes());
        bytes.extend([0, 0]);
        bytes
    }

    /// A JPEG header whose JFIF segment gives no density and whose Exif block gives `dpi`
    /// in `unit`, little-endian.
    fn exif(dpi: u32, unit: u16) -> Vec<u8> {
        let mut tiff = b"II*\0\x08\0\0\0".to_vec();
        tiff.extend(3u16.to_le_bytes());
        let values = 8 + 2 + 3 * 12 + 4;
        for (tag, kind, value) in [
            (0x011a_u16, 5_u16, values),
            (0x011b, 5, values + 8),
            (0x0128, 3, u32::from(unit)),
        ] {
            tiff.extend(tag.to_le_bytes());
            tiff.extend(kind.to_le_bytes());
            tiff.extend(1u32.to_le_bytes());
            tiff.extend(value.to_le_bytes());
        }
        tiff.extend(0u32.to_le_bytes());
        for _ in 0..2 {
            tiff.extend(dpi.to_le_bytes());
            tiff.extend(1u32.to_le_bytes());
        }
        let mut bytes = jfif(0, 1);
        bytes.extend([0xff, 0xe1]);
        bytes.extend(u16::try_from(2 + 6 + tiff.len()).unwrap().to_be_bytes());
        bytes.extend(b"Exif\0\0");
        bytes.extend(tiff);
        bytes.extend([0xff, 0xda, 0, 2]);
        bytes
    }

    fn shown(pieces: &[Piece]) -> Vec<String> {
        pieces
            .iter()
            .map(|piece| match piece {
                Piece::Text(text) => text.clone(),
                Piece::Picture(bytes, size) => format!("[{} {size:?}]", bytes.len()),
                Piece::Awaited => "[awaited]".to_owned(),
            })
            .collect()
    }

    /// As OneNote 2010 pasted `<p>before <img> after</p>` (lab, 2026-09-30): the text on
    /// either side of the picture, the picture at the size its attributes give.
    #[test]
    fn a_pasted_page_gives_its_text_and_pictures_in_order() {
        let html = "Version:0.9\r\n<html><head><style>p{}</style><title>T</title></head>\
            <body><!--StartFragment--><p>before <img alt=\"a > b\" \
            src=\"file:///C:/work/pic.png\" width=\"200\" height=\"100px\"> after</p>\
            <p>Fish &amp; chips&nbsp;&#x2014;&#8212;</p><ul><li>one<li>two<br>three</ul>\
            <img src=\"missing.png\"><IMG SRC='https://example.invalid/a.png' width=40>\
            <!--EndFragment--></body></html>";
        let mut asked = Vec::new();
        let pieces = html_pieces(html, |source, css| {
            asked.push((source.to_owned(), css));
            match source {
                "missing.png" => None,
                "https://example.invalid/a.png" => Some(Piece::Awaited),
                _ => Some(Piece::Picture(vec![0; 3], [150.0, 75.0])),
            }
        });
        assert_eq!(
            asked,
            [
                (
                    "file:///C:/work/pic.png".to_owned(),
                    [Some(200.0), Some(100.0)]
                ),
                ("missing.png".to_owned(), [None, None]),
                (
                    "https://example.invalid/a.png".to_owned(),
                    [Some(40.0), None]
                ),
            ]
        );
        assert_eq!(
            shown(&pieces),
            [
                "before",
                "[3 [150.0, 75.0]]",
                "after\nFish & chips \u{2014}\u{2014}\none\ntwo\nthree",
                "[awaited]",
            ]
        );
    }

    /// Attributes size a picture, the missing one by its proportions.
    #[test]
    fn page_pictures_take_the_size_their_attributes_give() {
        let picture = png_at(None);
        assert_eq!(sized(&picture, [None, None]), Some([150.0, 75.0]));
        assert_eq!(sized(&picture, [Some(100.0), None]), Some([75.0, 37.5]));
        assert_eq!(sized(&picture, [None, Some(50.0)]), Some([75.0, 37.5]));
        assert_eq!(sized(&picture, [Some(8.0), Some(4.0)]), Some([6.0, 3.0]));
        assert_eq!(sized(b"not a picture", [None, None]), None);
    }

    #[test]
    fn picture_sources_decode() {
        assert_eq!(
            load("data:image/png;base64,iVBORw0KGgo=").unwrap(),
            b"\x89PNG\r\n\x1a\n"
        );
        assert_eq!(load("data:text/plain,a%20b").unwrap(), b"a b");
        assert_eq!(load("relative/pic.png"), None);
        let directory =
            std::env::temp_dir().join(format!("snowbound-paste-{}", std::process::id()));
        notebook::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("a picture.png");
        notebook::fs::write(&file, b"bytes").unwrap();
        let address = format!("file://{}", file.to_str().unwrap().replace(' ', "%20"));
        assert_eq!(load(&address).unwrap(), b"bytes");
        notebook::fs::remove_dir_all(&directory).unwrap();
    }

    fn close(size: [f32; 2], expected: [f32; 2]) -> bool {
        (0..2).all(|axis| (size[axis] - expected[axis]).abs() < 0.01)
    }

    /// OneNote 2010 read these back from a 200 by 100 pixel picture that GDI+ saved at 72
    /// and 48 dpi (lab, 2026-09-30).
    #[test]
    fn pictures_take_the_size_their_resolution_gives() {
        let pixels = [200, 100];
        let size = |bytes: &[u8]| picture_size(bytes, pixels);
        assert!(close(size(&png_at(Some(2834))), [200.0456, 100.0228]));
        assert!(close(size(&png_at(Some(1889))), [300.1212, 150.0606]));
        assert!(close(size(&png_at(None)), [150.0, 75.0]));
        assert!(close(size(&jfif(1, 48)), [300.0, 150.0]));
        assert!(close(size(&jfif(2, 20)), [283.4646, 141.7323]));
        assert!(close(size(&jfif(0, 1)), [150.0, 75.0]));
        assert!(close(size(&jfif(1, 0)), [150.0, 75.0]));
        assert!(close(size(b"GIF89a"), [150.0, 75.0]));
        assert!(close(size(&exif(72, 2)), [200.0, 100.0]));
        assert!(close(size(&exif(20, 3)), [283.4646, 141.7323]));
        assert!(close(size(&exif(72, 1)), [150.0, 75.0]));
    }

    #[test]
    fn a_pasted_bitmap_is_stored_as_a_png_at_96_dpi() {
        let rgba: Vec<u8> = (0..3 * 2 * 4).map(|byte| byte as u8).collect();
        let encoded = png([3, 2], &rgba).unwrap();
        assert_eq!(draw::RasterImage::measure(&encoded).unwrap(), [3, 2]);
        assert!(close(picture_size(&encoded, [3, 2]), [2.25, 1.5]));
        let mut reader = png::Decoder::new(std::io::Cursor::new(&encoded))
            .read_info()
            .unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut decoded).unwrap();
        assert_eq!(decoded, rgba);
    }
}
