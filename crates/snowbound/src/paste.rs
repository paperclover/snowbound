//! The clipboard's formats: what Copy offers, and pictures, files and pages Paste takes from
//! it, with pictures inserted, as OneNote 2010 places them.

use crate::{State, platform};
use canvas::editor::{Clip, Piece};
use std::{
    error::Error,
    path::{Path, PathBuf},
};

/// Every format Copy puts on the clipboard for one selection, which each platform's clipboard
/// offers together: text; HTML, as OneNote 2010 offers it beside text (lab, 2026-10-02); and
/// Snowbound's own format, [`Clip::encode`]'s JSON, which pastes losslessly into Snowbound.
/// Its name is `net.paperclover.snowbound.clip` on macOS, `Snowbound Clip` on Windows and
/// `application/x-snowbound-clip` where formats are MIME types.
pub(crate) struct Copied {
    pub text: String,
    /// A whole page, the copy between `<!--StartFragment-->` and `<!--EndFragment-->`.
    pub html: String,
    pub clip: String,
}

impl Copied {
    pub(crate) fn new(clip: &Clip) -> Self {
        Self {
            text: clip.text(),
            html: clip.html(),
            clip: clip.encode(),
        }
    }
}

/// `html` behind the header Windows' `HTML Format` begins with, which gives the byte offsets
/// of the page and of the fragment its comments mark out.
#[cfg(any(windows, test))]
pub(crate) fn cf_html(html: &str) -> String {
    let header = |offsets: [usize; 4]| {
        format!(
            "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n",
            offsets[0], offsets[1], offsets[2], offsets[3]
        )
    };
    let start = header([0; 4]).len();
    let fragment = html
        .find("<!--StartFragment-->")
        .map_or(0, |at| at + "<!--StartFragment-->".len());
    let end = html.find("<!--EndFragment-->").unwrap_or(html.len());
    header([start, start + html.len(), start + fragment, start + end]) + html
}

/// What Paste takes from the clipboard. Files come first, as Finder also offers their names
/// as text and their icons as pictures; then Snowbound's own copy, then a page, as its text
/// alone would lose its formatting and pictures; text before a picture, as Office and
/// browsers offer a picture of copied text beside it.
pub(crate) enum Pasted {
    Files(Vec<PathBuf>),
    Clip(Clip),
    /// A web page, and the clipboard's text for one that holds nothing to paste.
    Page(String, Option<String>),
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
            Some(Pasted::Clip(clip)) => {
                let response = self.view.paste_clip(clip)?;
                self.respond(response);
            }
            Some(Pasted::Page(html, text)) => {
                // Web pictures come later, so the window never waits on the network.
                let mut fetched = Vec::new();
                let language = canvas::language::lcid(&platform::input_language());
                let pieces = canvas::editor::html_pieces(&html, language, |source, css| {
                    if source.starts_with("https://") || source.starts_with("http://") {
                        fetched.push((source.to_owned(), css));
                        return Some(Piece::Awaited);
                    }
                    let bytes = load(source)?;
                    let size = sized(&bytes, css)?;
                    Some(Piece::Picture(bytes, size))
                });
                if pieces.is_empty() {
                    if let Some(text) = text {
                        let response = self.view.paste(&text, language)?;
                        self.respond(response);
                    }
                    return Ok(());
                }
                let (places, response) = self.view.paste_pieces(pieces)?;
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

    /// The header's offsets find the page and the fragment, counted in bytes.
    #[test]
    fn the_html_format_header_gives_its_offsets() {
        let html =
            "<html><body><!--StartFragment--><p>\u{e9}t\u{e9}</p><!--EndFragment--></body></html>";
        let wrapped = cf_html(html);
        let offset = |name: &str| {
            let at = wrapped.find(name).unwrap() + name.len() + 1;
            wrapped[at..at + 10].parse::<usize>().unwrap()
        };
        assert_eq!(&wrapped[offset("StartHTML")..offset("EndHTML")], html);
        assert_eq!(
            &wrapped[offset("StartFragment")..offset("EndFragment")],
            "<p>\u{e9}t\u{e9}</p>"
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
