//! Recordings as Snowbound stores them, which OneNote 2010 plays (`corpus/recording`):
//! audio as 16 kHz IMA ADPCM WAV, video as the Motion JPEG AVI files of `video`.

pub mod video;

use onestore::page::{Attachment, Recording};

/// The rate audio is recorded at: wideband speech, as OneNote's own 8 kHz WMA is narrowband.
pub const RATE: u32 = 16_000;

/// The line saying when a recording started, as OneNote writes it from the long `date` and
/// short `time`.
pub fn label(video: bool, date: &str, time: &str) -> String {
    let kind = if video { "Video" } else { "Audio" };
    format!("{kind} recording started: {time} {date}")
}

/// The file recording `id` on page `title` is stored as, from what was recorded: a mono
/// 16-bit PCM WAV file, which is compressed, or a `video` AVI file. It has no icon yet.
pub fn file(
    id: [u8; 16],
    title: &str,
    video: bool,
    recorded: Vec<u8>,
) -> Result<Attachment, onestore::page::text::EditError> {
    let (bytes, duration_ms) = if video {
        let duration = video::Movie::parse(&recorded).map(|movie| movie.duration_ms());
        (recorded, duration)
    } else {
        match compress(&recorded) {
            Some((bytes, duration)) => (bytes, Some(duration)),
            None => {
                let duration = Wave::parse(&recorded).map(|wave| wave.duration_ms());
                (recorded, duration)
            }
        }
    };
    let name: String = title
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = if name.trim().is_empty() {
        if video {
            "Video recording"
        } else {
            "Audio recording"
        }
        .to_owned()
    } else {
        name
    };
    let extension = if video { "avi" } else { "wav" };
    Ok(Attachment {
        id: onestore::page::text::new_id()?,
        filename: format!("{name}.{extension}"),
        source_path: None,
        size: Some(crate::gpu::page::ICON_SIZE),
        layout: Default::default(),
        bytes: Some(bytes.into()),
        preview: None,
        recording: Some(Recording {
            id,
            kind: if video { 2 } else { 1 },
            duration_ms,
        }),
        tags: Vec::new(),
    })
}

/// A recording's sound as a 16-bit PCM WAV file every platform's player decodes: IMA ADPCM
/// decoded, a video's sound taken out of it. None for other files, which play as they are.
pub fn sound(bytes: &[u8]) -> Option<Vec<u8>> {
    let sound = match video::Movie::parse(bytes) {
        Some(movie) => movie
            .wave(bytes)
            .unwrap_or_else(|| silence(movie.duration_ms())),
        None => return decompress(bytes),
    };
    Some(decompress(&sound).unwrap_or(sound))
}

/// Silence as long as `ms`, the clock of a video without sound.
fn silence(ms: u32) -> Vec<u8> {
    wave(8_000, &vec![0; ms as usize * 8])
}

/// A mono 16-bit PCM WAV file of `samples` at `rate`, as the platform recorders write.
pub fn wave(rate: u32, samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    [
        &b"RIFF"[..],
        &(36 + data.len() as u32).to_le_bytes(),
        b"WAVEfmt ",
        &16_u32.to_le_bytes(),
        &1_u16.to_le_bytes(),
        &1_u16.to_le_bytes(),
        &rate.to_le_bytes(),
        &(rate * 2).to_le_bytes(),
        &2_u16.to_le_bytes(),
        &16_u16.to_le_bytes(),
        b"data",
        &(data.len() as u32).to_le_bytes(),
        &data,
    ]
    .concat()
}

/// A recording for a file attached under `filename`, as OneNote 2010 makes one of every
/// audio and video file it attaches, with the length of a WAV file's `bytes`.
pub fn attached(filename: &str, bytes: &[u8]) -> Option<Recording> {
    Some(Recording {
        id: onestore::page::text::new_guid().ok()?,
        kind: Recording::kind_of(filename)?,
        duration_ms: Wave::parse(bytes).map(|wave| wave.duration_ms()),
    })
}

/// The chunks of a RIFF WAVE file this module reads.
struct Wave<'a> {
    format: u16,
    channels: u16,
    rate: u32,
    bytes_per_second: u32,
    block: u16,
    bits: u16,
    /// The sample count a compressed file's `fact` chunk states.
    samples: Option<u32>,
    data: &'a [u8],
}

impl<'a> Wave<'a> {
    fn parse(bytes: &'a [u8]) -> Option<Self> {
        if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
            return None;
        }
        let u16_at = |data: &[u8], at: usize| {
            Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
        };
        let u32_at = |data: &[u8], at: usize| {
            Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
        };
        let (mut fmt, mut samples, mut data) = (None, None, None);
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let size = u32_at(bytes, at + 4)? as usize;
            let body = &bytes[at + 8..(at + 8).saturating_add(size).min(bytes.len())];
            match &bytes[at..at + 4] {
                b"fmt " => fmt = Some(body),
                b"fact" => samples = u32_at(body, 0),
                b"data" => data = Some(body),
                _ => {}
            }
            at = at + 8 + size + size % 2;
        }
        let fmt = fmt?;
        Some(Self {
            format: u16_at(fmt, 0)?,
            channels: u16_at(fmt, 2)?,
            rate: u32_at(fmt, 4)?,
            bytes_per_second: u32_at(fmt, 8)?,
            block: u16_at(fmt, 12)?,
            bits: u16_at(fmt, 14)?,
            samples,
            data: data?,
        })
    }

    fn duration_ms(&self) -> u32 {
        let ms = match self.samples.filter(|_| self.format != 1) {
            Some(samples) => u64::from(samples) * 1000 / u64::from(self.rate.max(1)),
            None => self.data.len() as u64 * 1000 / u64::from(self.bytes_per_second.max(1)),
        };
        u32::try_from(ms).unwrap_or(u32::MAX)
    }
}

/// IMA ADPCM's step sizes and how each code moves along them.
const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const INDEX_STEPS: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Bytes per block: Windows' IMA ADPCM codec's choice for 16 kHz.
const BLOCK: usize = 512;
/// Samples per block: the header's, then two per remaining byte.
const BLOCK_SAMPLES: usize = (BLOCK - 4) * 2 + 1;

/// A mono 16-bit PCM WAV file as 4-bit IMA ADPCM (WAVE_FORMAT_DVI_ADPCM), which Windows
/// and macOS decode, a quarter of the size; and its length. None for other files.
pub fn compress(wav: &[u8]) -> Option<(Vec<u8>, u32)> {
    let wave = Wave::parse(wav)?;
    if wave.format != 1 || wave.channels != 1 || wave.bits != 16 {
        return None;
    }
    let samples: Vec<i16> = wave
        .data
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let mut data = Vec::with_capacity(samples.len() / 2 + BLOCK);
    let mut index = 0_i32;
    for block in samples.chunks(BLOCK_SAMPLES) {
        let mut predicted = i32::from(block[0]);
        data.extend_from_slice(&block[0].to_le_bytes());
        data.extend_from_slice(&[index as u8, 0]);
        // A short last block is padded with its last sample; `fact` gives the true length.
        let last = *block.last().unwrap();
        let rest = block[1..]
            .iter()
            .copied()
            .chain(std::iter::repeat(last))
            .take(BLOCK_SAMPLES - 1)
            .collect::<Vec<_>>();
        for pair in rest.chunks_exact(2) {
            let mut byte = 0;
            for (shift, sample) in [0, 4].into_iter().zip(pair) {
                let code = encode(i32::from(*sample), &mut predicted, &mut index);
                byte |= code << shift;
            }
            data.push(byte);
        }
    }
    let count = u32::try_from(samples.len()).ok()?;
    let bytes_per_second = wave.rate * BLOCK as u32 / BLOCK_SAMPLES as u32;
    let mut out = Vec::with_capacity(data.len() + 60);
    let fmt: Vec<u8> = [
        &0x11_u16.to_le_bytes()[..],
        &1_u16.to_le_bytes(),
        &wave.rate.to_le_bytes(),
        &bytes_per_second.to_le_bytes(),
        &(BLOCK as u16).to_le_bytes(),
        &4_u16.to_le_bytes(),
        &2_u16.to_le_bytes(),
        &(BLOCK_SAMPLES as u16).to_le_bytes(),
    ]
    .concat();
    let riff = 4 + (8 + fmt.len()) + (8 + 4) + (8 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&u32::try_from(riff).ok()?.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(&fmt);
    out.extend_from_slice(b"fact");
    out.extend_from_slice(&4_u32.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::try_from(data.len()).ok()?.to_le_bytes());
    out.extend_from_slice(&data);
    let duration = u32::try_from(u64::from(count) * 1000 / u64::from(wave.rate.max(1))).ok()?;
    Some((out, duration))
}

/// The 4-bit code nearest `sample` from `predicted`, which it and `index` then follow as a
/// decoder does.
fn encode(sample: i32, predicted: &mut i32, index: &mut i32) -> u8 {
    let step = STEPS[*index as usize];
    let mut difference = sample - *predicted;
    let mut code = 0;
    if difference < 0 {
        code = 8;
        difference = -difference;
    }
    let mut delta = step >> 3;
    for (bit, part) in [(4, step), (2, step >> 1), (1, step >> 2)] {
        if difference >= part {
            code |= bit;
            difference -= part;
            delta += part;
        }
    }
    *predicted = if code & 8 != 0 {
        *predicted - delta
    } else {
        *predicted + delta
    }
    .clamp(-32768, 32767);
    *index = (*index + INDEX_STEPS[usize::from(code & 7)]).clamp(0, 88);
    code
}

/// A mono IMA ADPCM WAV file as 16-bit PCM, which every platform's player decodes; none
/// for other files.
pub fn decompress(wav: &[u8]) -> Option<Vec<u8>> {
    let wave = Wave::parse(wav).filter(|wave| wave.format == 0x11 && wave.channels == 1)?;
    Some(self::wave(wave.rate, &decode(&wave)))
}

/// Each block's header sample and index, then its codes.
fn decode(wave: &Wave<'_>) -> Vec<i16> {
    let mut out = Vec::new();
    for block in wave.data.chunks(usize::from(wave.block.max(4))) {
        let Some(header) = block.get(..4) else {
            break;
        };
        let mut predicted = i32::from(i16::from_le_bytes([header[0], header[1]]));
        let mut index = i32::from(header[2]).clamp(0, 88);
        out.push(predicted as i16);
        for byte in &block[4..] {
            for code in [byte & 15, byte >> 4] {
                let step = STEPS[index as usize];
                let mut delta = step >> 3;
                for (bit, part) in [(4, step), (2, step >> 1), (1, step >> 2)] {
                    if code & bit != 0 {
                        delta += part;
                    }
                }
                predicted = if code & 8 != 0 {
                    predicted - delta
                } else {
                    predicted + delta
                }
                .clamp(-32768, 32767);
                index = (index + INDEX_STEPS[usize::from(code & 7)]).clamp(0, 88);
                out.push(predicted as i16);
            }
        }
    }
    if let Some(samples) = wave.samples {
        out.truncate(samples as usize);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two seconds of a spoken-pitch chord.
    fn tone() -> Vec<i16> {
        (0..2 * RATE)
            .map(|n| {
                let t = n as f32 / RATE as f32;
                let wave = (t * 220.0 * std::f32::consts::TAU).sin() * 0.5
                    + (t * 330.0 * std::f32::consts::TAU).sin() * 0.3;
                (wave * 12_000.0) as i16
            })
            .collect()
    }

    #[test]
    fn a_recording_compresses_to_ima_adpcm_that_decodes_close_to_what_was_heard() {
        let samples = tone();
        let (bytes, duration) = compress(&wave(RATE, &samples)).unwrap();
        assert_eq!(duration, 2000);
        let wave = Wave::parse(&bytes).unwrap();
        assert_eq!(
            (wave.format, wave.channels, wave.rate, wave.bits),
            (0x11, 1, RATE, 4)
        );
        assert_eq!(wave.data.len() % BLOCK, 0);
        assert_eq!(wave.duration_ms(), 2000);
        assert!(bytes.len() * 3 < samples.len() * 2);
        let decoded = decode(&wave);
        assert_eq!(
            Wave::parse(&decompress(&bytes).unwrap())
                .unwrap()
                .data
                .len(),
            decoded.len() * 2
        );
        assert_eq!(decoded.len(), samples.len());
        let noise: f64 = samples
            .iter()
            .zip(&decoded)
            .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
            .sum();
        let signal: f64 = samples.iter().map(|a| f64::from(*a).powi(2)).sum();
        let snr = 10.0 * (signal / noise).log10();
        assert!(snr > 20.0, "{snr} dB");
    }

    #[test]
    fn only_mono_16_bit_pcm_compresses_and_attached_wav_files_know_their_length() {
        let stereo = {
            let mut bytes = wave(RATE, &[0; 64]);
            bytes[22] = 2;
            bytes
        };
        assert!(compress(&stereo).is_none());
        assert!(compress(b"not a wave").is_none());
        let silence = include_bytes!(
            "../../../corpus/m6/native-features-01/notebook/fixture-data/silence.wav"
        );
        // OneNote 2010 stored 1000 ms for this file when attached (`corpus/recording`).
        let recording = attached("silence.WAV", silence).unwrap();
        assert_eq!((recording.kind, recording.duration_ms), (1, Some(1000)));
        assert_eq!(attached("clip.wmv", b"").unwrap().kind, 2);
        assert!(attached("notes.txt", b"").is_none());
    }

    #[test]
    fn a_recording_is_stored_named_after_its_page_and_plays_as_pcm() {
        let id = [7; 16];
        let audio = file(id, "Lecture 3/4", false, wave(RATE, &tone())).unwrap();
        assert_eq!(audio.filename, "Lecture 3_4.wav");
        let recording = audio.recording.unwrap();
        assert_eq!(
            (recording.id, recording.kind, recording.duration_ms),
            (id, 1, Some(2000))
        );
        let stored = audio.bytes.unwrap();
        assert_eq!(Wave::parse(&stored).unwrap().format, 0x11);
        let played = sound(&stored).unwrap();
        assert_eq!(Wave::parse(&played).unwrap().format, 1);
        assert_eq!(
            file(id, " ", true, Vec::new()).unwrap().filename,
            "Video recording.avi"
        );
        assert_eq!(
            label(false, "Tuesday, September 29, 2026", "5:18 PM"),
            "Audio recording started: 5:18 PM Tuesday, September 29, 2026"
        );
    }
}
