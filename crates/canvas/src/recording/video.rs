//! Video recordings as AVI files of Motion JPEG pictures and PCM sound, which OneNote 2010
//! plays and lists as recordings (`corpus/recording/video`), and reading them back to play.

use std::ops::Range;

/// Pictures a second in a recording: OneNote 2010's own profiles record at 15.
pub const FPS: u32 = 15;
/// A recording's picture size, OneNote 2010's own.
pub const SIZE: [u32; 2] = [320, 240];

/// Interleaved signed 16-bit little-endian sound.
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub data: Vec<u8>,
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

fn list(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 12);
    chunk(&mut out, b"LIST", &[&kind[..], body].concat());
    out
}

fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// An AVI file of `frames`, JPEG pictures of `size` shown `FPS` a second, where an empty one
/// repeats the picture before it, and `sound` alongside. None past AVI's 4 GiB.
pub fn write(size: [u32; 2], frames: &[Vec<u8>], sound: &Pcm) -> Option<Vec<u8>> {
    let block = u32::from(sound.channels) * 2;
    let bytes_per_second = sound.rate * block;
    let samples = sound.data.len() as u64 / u64::from(block);
    let count = frames.len() as u64;
    // The sound up to frame `index`, whole samples.
    let sound_at = |index: u64| -> usize {
        let before = if index >= count {
            samples
        } else {
            (index * u64::from(sound.rate) / u64::from(FPS)).min(samples)
        };
        (before * u64::from(block)) as usize
    };
    let mut movi = b"movi".to_vec();
    let mut index = Vec::new();
    let [mut largest_frame, mut largest_sound] = [0, 0];
    for (at, frame) in frames.iter().enumerate() {
        let entries: [(&[u8; 4], &[u8]); 2] = [
            (b"00dc", frame),
            (
                b"01wb",
                &sound.data[sound_at(at as u64)..sound_at(at as u64 + 1)],
            ),
        ];
        for (id, body) in entries {
            if id == b"01wb" && body.is_empty() {
                continue;
            }
            // Keyframes: every picture and all sound; a repeat is none.
            let flags: u32 = if body.is_empty() { 0 } else { 0x10 };
            index.extend_from_slice(id);
            index.extend(words(&[
                flags,
                u32::try_from(movi.len()).ok()?,
                body.len() as u32,
            ]));
            chunk(&mut movi, id, body);
            let largest = if id == b"00dc" {
                &mut largest_frame
            } else {
                &mut largest_sound
            };
            *largest = (*largest).max(body.len() as u32);
        }
    }
    let [width, height] = size;
    let frame_count = u32::try_from(count).ok()?;
    let rect = [0u16, 0, width as u16, height as u16]
        .iter()
        .flat_map(|side| side.to_le_bytes())
        .collect::<Vec<u8>>();
    let mut video = Vec::new();
    let strh = [
        &b"vidsMJPG"[..],
        &words(&[0, 0, 0, 1, FPS, 0, frame_count, largest_frame, u32::MAX, 0]),
        &rect,
    ]
    .concat();
    chunk(&mut video, b"strh", &strh);
    let strf = [
        words(&[40, width, height]),
        [&1u16.to_le_bytes()[..], &24u16.to_le_bytes()].concat(),
        b"MJPG".to_vec(),
        words(&[width * height * 3, 0, 0, 0, 0]),
    ]
    .concat();
    chunk(&mut video, b"strf", &strf);
    let mut streams = list(b"strl", &video);
    if samples > 0 {
        let mut audio = Vec::new();
        let strh = [
            &b"auds\0\0\0\0"[..],
            &words(&[
                0,
                0,
                0,
                block,
                bytes_per_second,
                0,
                u32::try_from(samples).ok()?,
                largest_sound,
                u32::MAX,
                block,
            ]),
            &[0; 8],
        ]
        .concat();
        chunk(&mut audio, b"strh", &strh);
        let strf = [
            &1u16.to_le_bytes()[..],
            &sound.channels.to_le_bytes(),
            &sound.rate.to_le_bytes(),
            &bytes_per_second.to_le_bytes(),
            &(block as u16).to_le_bytes(),
            &16u16.to_le_bytes(),
            &0u16.to_le_bytes(),
        ]
        .concat();
        chunk(&mut audio, b"strf", &strf);
        streams.extend(list(b"strl", &audio));
    }
    let mut avih = Vec::new();
    chunk(
        &mut avih,
        b"avih",
        &words(&[
            1_000_000 / FPS,
            largest_frame
                .saturating_add(largest_sound)
                .saturating_mul(FPS),
            0,
            // AVIF_HASINDEX | AVIF_ISINTERLEAVED
            0x110,
            frame_count,
            0,
            if samples > 0 { 2 } else { 1 },
            largest_frame.max(largest_sound),
            width,
            height,
            0,
            0,
            0,
            0,
        ]),
    );
    let header = list(b"hdrl", &[avih, streams].concat());
    let mut body = b"AVI ".to_vec();
    body.extend(header);
    let movi_size = u32::try_from(movi.len()).ok()?;
    body.extend_from_slice(b"LIST");
    body.extend_from_slice(&movi_size.to_le_bytes());
    body.extend(movi);
    chunk(&mut body, b"idx1", &index);
    u32::try_from(body.len()).ok()?;
    let mut out = Vec::with_capacity(body.len() + 8);
    chunk(&mut out, b"RIFF", &body);
    Some(out)
}

/// An AVI file's pictures and sound, as byte ranges of the file.
pub struct Movie {
    /// Each picture's JPEG; an empty range repeats the one before.
    pub frames: Vec<Range<usize>>,
    pub frame_us: u32,
    /// The sound's WAVEFORMATEX and its data, in order.
    pub sound: Option<(Range<usize>, Vec<Range<usize>>)>,
}

impl Movie {
    /// A Motion JPEG AVI file's contents; none for other files.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let mut movie = Self {
            frames: Vec::new(),
            frame_us: 0,
            sound: None,
        };
        let mut kinds = Vec::new();
        let mut first = true;
        for (id, body) in Chunks::new(bytes, 0..bytes.len()) {
            let kind = bytes.get(body.start..body.start + 4)?;
            if id != *b"RIFF" || (kind != b"AVI " && !(kind == b"AVIX" && !first)) {
                return (!first).then_some(movie).filter(|movie| movie.frame_us > 0);
            }
            first = false;
            movie.walk(bytes, body.start + 4..body.end, &mut kinds)?;
        }
        (movie.frame_us > 0).then_some(movie)
    }

    fn walk(&mut self, bytes: &[u8], range: Range<usize>, kinds: &mut Vec<[u8; 4]>) -> Option<()> {
        for (id, body) in Chunks::new(bytes, range) {
            let u32_at = |at: usize| {
                Some(u32::from_le_bytes(
                    bytes
                        .get(body.start + at..body.start + at + 4)?
                        .try_into()
                        .ok()?,
                ))
            };
            match &id {
                b"LIST" => {
                    let inner = body.start + 4..body.end;
                    self.walk(bytes, inner, kinds)?;
                }
                b"strh" => {
                    let kind: [u8; 4] = bytes.get(body.start..body.start + 4)?.try_into().ok()?;
                    if &kind == b"vids" {
                        let [scale, rate] = [u32_at(20)?, u32_at(24)?];
                        self.frame_us =
                            u32::try_from(u64::from(scale) * 1_000_000 / u64::from(rate.max(1)))
                                .ok()?;
                    }
                    kinds.push(kind);
                }
                b"strf" if kinds.last() == Some(b"vids") => {
                    if bytes.get(body.start + 16..body.start + 20)? != b"MJPG" {
                        return None;
                    }
                }
                b"strf" if kinds.last() == Some(b"auds") => {
                    self.sound = Some((body.clone(), Vec::new()));
                }
                [a, b, c, d] if a.is_ascii_digit() && b.is_ascii_digit() => {
                    let stream = usize::from((a - b'0') * 10 + (b - b'0'));
                    match (kinds.get(stream).map(|kind| &kind[..]), [c, d]) {
                        (Some(b"vids"), [b'd', b'c' | b'b']) => self.frames.push(body),
                        (Some(b"auds"), [b'w', b'b']) => {
                            if let Some((_, data)) = &mut self.sound {
                                data.push(body);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Some(())
    }

    pub fn duration_ms(&self) -> u32 {
        let ms = (self.frames.len() as u64 * u64::from(self.frame_us)).div_ceil(1000);
        u32::try_from(ms).unwrap_or(u32::MAX)
    }

    /// The picture showing `at_ms` in: the last stored one at or before it.
    pub fn frame(&self, at_ms: u32) -> Option<Range<usize>> {
        let at = (u64::from(at_ms) * 1000 / u64::from(self.frame_us.max(1))) as usize;
        self.frames[..=at.min(self.frames.len().checked_sub(1)?)]
            .iter()
            .rev()
            .find(|frame| !frame.is_empty())
            .cloned()
    }

    /// The sound as a WAV file, or none where the file has none.
    pub fn wave(&self, bytes: &[u8]) -> Option<Vec<u8>> {
        let (format, data) = self.sound.as_ref()?;
        let format = &bytes[format.clone()];
        let length: usize = data.iter().map(ExactSizeIterator::len).sum();
        let mut out = Vec::with_capacity(length + 64);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((4 + 8 + format.len() + 8 + length) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        chunk(&mut out, b"fmt ", format);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(length as u32).to_le_bytes());
        for range in data {
            out.extend_from_slice(&bytes[range.clone()]);
        }
        Some(out)
    }
}

/// The chunks in `range` of a RIFF file: each one's id and body.
struct Chunks<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> Chunks<'a> {
    fn new(bytes: &'a [u8], range: Range<usize>) -> Self {
        Self {
            bytes,
            at: range.start,
            end: range.end.min(bytes.len()),
        }
    }
}

impl Iterator for Chunks<'_> {
    type Item = ([u8; 4], Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        let header = self.bytes.get(self.at..self.at + 8)?;
        if self.at + 8 > self.end {
            return None;
        }
        let id: [u8; 4] = header[..4].try_into().ok()?;
        let size = u32::from_le_bytes(header[4..].try_into().ok()?) as usize;
        let start = self.at + 8;
        let end = start.saturating_add(size).min(self.end);
        self.at = end + size % 2;
        Some((id, start..end))
    }
}

/// Pictures taken at uneven moments, as a camera takes them, made the `FPS` a second of a
/// recording: each one shows until the next, fitted to `SIZE`.
#[derive(Default)]
pub struct Pictures {
    frames: Vec<Vec<u8>>,
    last: Option<image::RgbImage>,
    /// Whether the last picture is stored yet, which the frames after it repeat.
    stored: bool,
}

impl Pictures {
    /// The picture taken `at_us` microseconds in; the first shows from the start.
    pub fn take(&mut self, at_us: u64, picture: image::RgbImage) {
        self.fill(at_us);
        self.last = Some(fit(picture));
        self.stored = false;
    }

    /// A picture as Core Video gives one: `height` rows of `stride` bytes of BGRA pixels.
    pub fn take_bgra(
        &mut self,
        at_us: u64,
        [width, height]: [usize; 2],
        stride: usize,
        bgra: &[u8],
    ) {
        let rgb = bgra
            .chunks_exact(stride.max(1))
            .take(height)
            .filter_map(|row| row.get(..width * 4))
            .flat_map(|row| {
                row.chunks_exact(4)
                    .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
            })
            .collect();
        if let Some(picture) = image::RgbImage::from_raw(width as u32, height as u32, rgb) {
            self.take(at_us, picture);
        }
    }

    /// The frames of a recording `end_us` microseconds long.
    pub fn finish(mut self, end_us: u64) -> Vec<Vec<u8>> {
        self.fill(end_us);
        self.frames
    }

    /// Frames up to `until_us`, of the last picture.
    fn fill(&mut self, until_us: u64) {
        let Some(last) = &self.last else {
            return;
        };
        while (self.frames.len() as u64) * 1_000_000 / u64::from(FPS) < until_us {
            let frame = if self.stored {
                Vec::new()
            } else {
                self.stored = true;
                let mut jpeg = Vec::new();
                let encoded = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 70)
                    .encode_image(last);
                if encoded.is_err() {
                    jpeg.clear();
                }
                jpeg
            };
            self.frames.push(frame);
        }
    }
}

/// `picture` scaled to fit `SIZE`, centred on black.
fn fit(picture: image::RgbImage) -> image::RgbImage {
    let [width, height] = SIZE;
    if picture.dimensions() == (width, height) {
        return picture;
    }
    let scale =
        (width as f32 / picture.width() as f32).min(height as f32 / picture.height() as f32);
    let fitted = image::imageops::resize(
        &picture,
        ((picture.width() as f32 * scale) as u32).clamp(1, width),
        ((picture.height() as f32 * scale) as u32).clamp(1, height),
        image::imageops::FilterType::Triangle,
    );
    let mut out = image::RgbImage::new(width, height);
    image::imageops::overlay(
        &mut out,
        &fitted,
        i64::from((width - fitted.width()) / 2),
        i64::from((height - fitted.height()) / 2),
    );
    out
}
