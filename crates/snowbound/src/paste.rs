//! Pictures and files from the clipboard, and pictures inserted, as OneNote 2010 places them.

use crate::{State, platform};
use std::{error::Error, path::PathBuf};

/// What Paste takes from the clipboard. Files come first, as Finder also offers their names
/// as text and their icons as pictures; text before a picture, as Office and browsers offer
/// a picture of copied text beside it.
pub(crate) enum Pasted {
    Files(Vec<PathBuf>),
    Text(String),
    Picture(Vec<u8>),
}

impl State {
    pub(crate) fn paste(&mut self) -> Result<(), Box<dyn Error>> {
        match self.clipboard.pasted() {
            Some(Pasted::Files(paths)) => {
                for path in paths {
                    // OneNote 2010 pastes a picture file as the picture (lab, 2026-09-30).
                    match std::fs::read(&path) {
                        Ok(bytes) if draw::RasterImage::measure(&bytes).is_ok() => {
                            self.insert_picture(bytes)?
                        }
                        _ => self.attach(&path, None)?,
                    }
                }
            }
            Some(Pasted::Text(text)) => {
                let language = canvas::language::lcid(&platform::input_language());
                let response = self.view.paste(&text, language)?;
                self.respond(response);
            }
            Some(Pasted::Picture(bytes)) => self.insert_picture(bytes)?,
            None => {}
        }
        Ok(())
    }

    /// Puts an encoded picture at the caret at the size its resolution gives it, as OneNote
    /// 2010 inserts and pastes one.
    pub(crate) fn insert_picture(&mut self, bytes: Vec<u8>) -> Result<(), Box<dyn Error>> {
        let Ok(pixels) = draw::RasterImage::measure(&bytes) else {
            platform::alert(
                "Couldn't insert the picture",
                "Choose a PNG, JPEG or GIF picture.",
            );
            return Ok(());
        };
        let size = picture_size(&bytes, pixels);
        let response = self.view.insert_picture(bytes, size)?;
        self.respond(response);
        Ok(())
    }
}

/// A picture's size in points: its pixels at the resolution it declares, or at 96 dpi
/// without one, as OneNote 2010 sizes inserted and pasted pictures and pasted bitmaps
/// (lab, 2026-09-30).
fn picture_size(encoded: &[u8], pixels: [u32; 2]) -> [f32; 2] {
    let dpi = resolution(encoded).unwrap_or([96.0; 2]);
    [0, 1].map(|axis| pixels[axis] as f32 * 72.0 / dpi[axis])
}

/// Dots per inch from a PNG's `pHYs` or a JPEG's JFIF density, where they give one.
fn resolution(encoded: &[u8]) -> Option<[f32; 2]> {
    let dpi = if let Some(mut chunks) = encoded.strip_prefix(b"\x89PNG\r\n\x1a\n") {
        loop {
            let length = u32::from_be_bytes(chunks.get(..4)?.try_into().ok()?) as usize;
            let kind = chunks.get(4..8)?;
            let data = chunks.get(8..8 + length)?;
            if kind == b"IDAT" {
                return None;
            }
            // Unit 1 is the metre.
            if kind == b"pHYs" && data.len() == 9 && data[8] == 1 {
                let per_metre =
                    |at: usize| u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
                break [per_metre(0), per_metre(4)].map(|dots| dots as f32 * 0.0254);
            }
            chunks = chunks.get(12 + length..)?;
        }
    } else {
        // Length, identifier, version, units, then the densities.
        let app0 = encoded.strip_prefix(b"\xff\xd8\xff\xe0")?.get(..14)?;
        if &app0[2..7] != b"JFIF\0" {
            return None;
        }
        let density = |at: usize| u16::from_be_bytes([app0[at], app0[at + 1]]) as f32;
        let dots = [density(10), density(12)];
        match app0[9] {
            1 => dots,
            2 => dots.map(|per_cm| per_cm * 2.54),
            _ => return None,
        }
    };
    dpi.iter().all(|dpi| *dpi > 0.0).then_some(dpi)
}

/// A bitmap from the clipboard as the PNG a page stores, without a resolution, so it takes
/// 96 dpi as OneNote 2010 gives a pasted bitmap.
#[cfg(not(target_os = "macos"))]
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
