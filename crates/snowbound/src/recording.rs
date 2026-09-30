//! Record Audio and playback, as OneNote 2010 records and plays (`corpus/recording`): a
//! recording starts at the caret and goes on the page when stopped, as a WAV file OneNote
//! plays; a recording, or a note linked to a moment in one, plays in a small transport over
//! the page. What the platform cannot decode opens in the system's player.

use crate::{State, media, platform};
use onestore::page::{Attachment, Recording};
use std::{
    error::Error,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use ui::{Axis, Flags, Spec, Theme, fit, px};

/// The rate recordings are made at: wideband speech, as OneNote's own 8 kHz WMA is narrowband.
pub(crate) const RATE: u32 = 16_000;

/// What records or plays now.
#[derive(Default)]
pub(crate) enum Media {
    #[default]
    Idle,
    Recording {
        microphone: media::Microphone,
        id: [u8; 16],
        path: PathBuf,
        started: Instant,
    },
    Playing {
        player: media::Player,
        name: String,
    },
}

impl State {
    /// Record Audio: starts recording at the caret, or stops the recording under way.
    pub(crate) fn record_audio(&mut self) -> Result<(), Box<dyn Error>> {
        if matches!(self.media, Media::Recording { .. }) {
            return self.stop_recording();
        }
        self.media = Media::Idle;
        let folder = std::env::temp_dir().join("Snowbound Recordings");
        std::fs::create_dir_all(&folder)?;
        let path = folder.join(format!("{}.wav", std::process::id()));
        let microphone = match media::Microphone::start(&path, RATE) {
            Ok(microphone) => microphone,
            Err(detail) => {
                platform::alert("Couldn't record audio", &detail);
                return Ok(());
            }
        };
        let [date, time] = platform::date_text(crate::filetime());
        let label = format!("Audio recording started: {time} {date}");
        let (id, response) = self.view.start_recording(&label)?;
        self.respond(response);
        let Some(id) = id else {
            microphone.stop()?;
            return Ok(());
        };
        self.media = Media::Recording {
            microphone,
            id,
            path,
            started: Instant::now(),
        };
        Ok(())
    }

    /// Stop: the recording under way goes on the page where it started.
    pub(crate) fn stop_recording(&mut self) -> Result<(), Box<dyn Error>> {
        let Media::Recording {
            microphone,
            id,
            path,
            ..
        } = std::mem::take(&mut self.media)
        else {
            return Ok(());
        };
        if let Err(detail) = microphone.stop() {
            platform::alert("Couldn't finish the recording", &detail);
            return Ok(());
        }
        let recorded = std::fs::read(&path)?;
        let (bytes, duration_ms) = match compress(&recorded) {
            Some((bytes, duration)) => (bytes, Some(duration)),
            None => (recorded, None),
        };
        let preview = platform::file_icon(&path);
        std::fs::remove_file(&path)?;
        let title = self.view.editor.page()?.title;
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
            "Audio recording".to_owned()
        } else {
            name
        };
        let file = Attachment {
            id: onestore::page::text::new_id()?,
            filename: format!("{name}.wav"),
            source_path: None,
            size: Some(crate::attachment::ICON_SIZE),
            layout: Default::default(),
            bytes: Some(bytes.into()),
            preview: preview.map(Into::into),
            recording: Some(Recording {
                id,
                kind: 1,
                duration_ms,
            }),
        };
        let response = self.view.finish_recording(file)?;
        self.respond(response);
        Ok(())
    }

    /// Plays recording `file` from `at_ms` in the transport, or opens it in the system's
    /// player where the platform cannot decode it.
    pub(crate) fn play(&mut self, file: &Attachment, at_ms: u32) -> Result<(), Box<dyn Error>> {
        if matches!(self.media, Media::Recording { .. }) {
            return Ok(());
        }
        self.media = Media::Idle;
        let Some(path) = self.copy_attachment(file)? else {
            return Ok(());
        };
        let Some(mut player) = media::Player::open(&path) else {
            platform::open_file(&path);
            return Ok(());
        };
        player.seek(at_ms);
        player.play();
        let name = Path::new(&file.filename)
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        self.media = Media::Playing { player, name };
        Ok(())
    }

    /// When the transport's clock next changes what it shows.
    pub(crate) fn media_wake(&self, now: Instant) -> Option<Instant> {
        match &self.media {
            Media::Idle => None,
            Media::Playing { player, .. } if !player.playing() => None,
            _ => Some(now + Duration::from_millis(250)),
        }
    }

    /// The transport over the page's foot while something records or plays: OneNote's
    /// Recording and Playback tabs, cut to what a note taker reaches for.
    pub(crate) fn transport(
        &mut self,
        theme: &Theme,
        page: [f32; 4],
    ) -> Result<(), Box<dyn Error>> {
        let (clock, playing) = match &self.media {
            Media::Idle => return Ok(()),
            Media::Recording { started, .. } => (
                format!("Recording  {}", clock(started.elapsed().as_millis() as u32)),
                None,
            ),
            Media::Playing { player, name } => (
                format!(
                    "{name}  {} / {}",
                    clock(player.position_ms()),
                    clock(player.duration_ms())
                ),
                Some(player.playing()),
            ),
        };
        let height = theme.font_size * 2.0 + 16.0;
        self.ui.open(
            "transport",
            Spec {
                flags: Flags::FLOAT | Flags::CLICKABLE,
                axis: Axis::X,
                size: [ui::children(), px(height)],
                position: [16.0, page[3] - page[1] - height - 16.0],
                fill: Some(theme.popup),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [8.0, 8.0],
                gap: 6.0,
                ..Spec::default()
            },
        );
        let mut chosen = None;
        if let Some(playing) = playing {
            if ui::button(&mut self.ui, "play", if playing { "Pause" } else { "Play" }).clicked {
                chosen = Some(Control::Toggle);
            }
            if ui::button(&mut self.ui, "back", "−10 s").clicked {
                chosen = Some(Control::Skip(-10_000));
            }
            if ui::button(&mut self.ui, "forward", "+10 s").clicked {
                chosen = Some(Control::Skip(10_000));
            }
        }
        if ui::button(&mut self.ui, "stop", "Stop").clicked {
            chosen = Some(Control::Stop);
        }
        self.ui.leaf(
            "clock",
            Spec {
                size: [fit(), px(theme.font_size * 2.0)],
                text: Some(&clock),
                pad: [6.0, 0.0],
                center: true,
                ..Spec::default()
            },
        );
        self.ui.close();
        match (chosen, &mut self.media) {
            (Some(Control::Stop), Media::Recording { .. }) => self.stop_recording()?,
            (Some(Control::Stop), _) => self.media = Media::Idle,
            (Some(Control::Toggle), Media::Playing { player, .. }) => {
                if player.playing() {
                    player.pause();
                } else {
                    if player.position_ms() >= player.duration_ms() {
                        player.seek(0);
                    }
                    player.play();
                }
            }
            (Some(Control::Skip(delta)), Media::Playing { player, .. }) => {
                let at = i64::from(player.position_ms()) + delta;
                player.seek(at.clamp(0, i64::from(player.duration_ms())) as u32);
            }
            _ => {}
        }
        Ok(())
    }
}

enum Control {
    Toggle,
    Stop,
    Skip(i64),
}

/// `ms` as the transport shows a moment: minutes and seconds, hours when there are some.
fn clock(ms: u32) -> String {
    let seconds = ms / 1000;
    match seconds / 3600 {
        0 => format!("{}:{:02}", seconds / 60, seconds % 60),
        hours => format!("{hours}:{:02}:{:02}", seconds / 60 % 60, seconds % 60),
    }
}

/// A recording for a file attached under `filename`, as OneNote 2010 makes one of every
/// audio and video file it attaches, with the length of a WAV file's `bytes`.
pub(crate) fn attached(filename: &str, bytes: &[u8]) -> Option<Recording> {
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
pub(crate) fn compress(wav: &[u8]) -> Option<(Vec<u8>, u32)> {
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A mono 16-bit PCM WAV file of `samples` at `rate`, as the platform recorder writes.
    pub(crate) fn pcm(rate: u32, samples: &[i16]) -> Vec<u8> {
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

    /// Two seconds of a spoken-pitch chord.
    pub(crate) fn tone() -> Vec<i16> {
        (0..2 * RATE)
            .map(|n| {
                let t = n as f32 / RATE as f32;
                let wave = (t * 220.0 * std::f32::consts::TAU).sin() * 0.5
                    + (t * 330.0 * std::f32::consts::TAU).sin() * 0.3;
                (wave * 12_000.0) as i16
            })
            .collect()
    }

    /// The reference decoder: each block's header sample and index, then its codes.
    fn decode(wave: &Wave<'_>) -> Vec<i16> {
        let mut out = Vec::new();
        for block in wave.data.chunks(BLOCK) {
            let mut predicted = i32::from(i16::from_le_bytes([block[0], block[1]]));
            let mut index = i32::from(block[2]);
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
        out.truncate(wave.samples.unwrap() as usize);
        out
    }

    #[test]
    fn a_recording_compresses_to_ima_adpcm_that_decodes_close_to_what_was_heard() {
        let samples = tone();
        let (bytes, duration) = compress(&pcm(RATE, &samples)).unwrap();
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
            let mut bytes = pcm(RATE, &[0; 64]);
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

    /// A recording made through the editor, stored as the host stores it:
    /// `SNOWBOUND_RECORDING_EXPORT` names a directory receiving the notebook for a cold
    /// read in OneNote 2010 (`corpus/recording/candidate`).
    #[test]
    fn a_recording_stores_its_file_line_and_linked_notes_for_onenote() {
        use canvas::{editor::CanvasEditor, layout::TextEngine};
        use onestore::{
            Arena, Section, Store,
            op::{Edit, Op},
            page::{MediaIndex, PageObject, ParagraphContent},
        };
        let source =
            onestore::create_section("Recorded.one", "Before recording", "Snowbound").unwrap();
        let arena = Arena::default();
        let mut section = Section::open(&arena, source.clone()).unwrap();
        let (space, ..) = section.pages().unwrap()[0].clone();
        let mut engine = TextEngine::default();
        let mut editor =
            CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
        let body = editor
            .outlines()
            .iter()
            .find(|outline| !outline.title)
            .unwrap()
            .id;
        editor.focus_outline(body).unwrap();
        editor
            .move_selection(&mut engine, draw::edit::Movement::DocumentEnd, false)
            .unwrap();
        let mut at = 134_000_000_000_000_000;
        let mut store = |editor: &mut CanvasEditor| {
            at += 10_000_000;
            let ops = editor.take_ops().unwrap();
            let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
            section.apply("Snowbound", &Edit { at, ops }).unwrap();
        };
        let id = editor
            .start_recording(
                &mut engine,
                "Audio recording started: 5:18 PM Tuesday, September 29, 2026",
            )
            .unwrap();
        store(&mut editor);
        std::thread::sleep(Duration::from_millis(40));
        editor.insert(&mut engine, "First linked note").unwrap();
        store(&mut editor);
        editor.insert(&mut engine, "\n").unwrap();
        std::thread::sleep(Duration::from_millis(40));
        editor.insert(&mut engine, "Second linked note").unwrap();
        store(&mut editor);
        let (bytes, duration) = compress(&pcm(RATE, &tone())).unwrap();
        let file = Attachment {
            id: onestore::page::text::new_id().unwrap(),
            filename: "Recorded.wav".into(),
            source_path: None,
            size: Some(crate::attachment::ICON_SIZE),
            layout: Default::default(),
            bytes: Some(bytes.clone().into()),
            preview: Some(canvas::gpu::page::file_icon().into()),
            recording: Some(Recording {
                id,
                kind: 1,
                duration_ms: Some(duration),
            }),
        };
        editor.finish_recording(&mut engine, file).unwrap();
        store(&mut editor);
        let mut image = source;
        section.seal().unwrap().unwrap().apply(&mut image).unwrap();

        let arena = Arena::default();
        let page = Section::open(&arena, image.clone())
            .unwrap()
            .page(space)
            .unwrap();
        let outline = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) if !outline.title => Some(outline),
                _ => None,
            })
            .unwrap();
        let shown: Vec<(String, Option<u32>)> = outline
            .paragraphs
            .iter()
            .map(|paragraph| {
                let text = match &paragraph.content {
                    ParagraphContent::Text(text) => text.text.text().to_owned(),
                    ParagraphContent::Attachment(file) => {
                        assert_eq!(file.bytes.as_deref(), Some(bytes.as_slice()));
                        format!("[{}]", file.filename)
                    }
                    _ => unreachable!(),
                };
                if paragraph.media != MediaIndex::default() {
                    assert_eq!(paragraph.media.recordings, [id]);
                }
                (text, paragraph.media.time_ms)
            })
            .collect();
        let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Before recording",
                "[Recorded.wav]",
                "",
                "Audio recording started: 5:18 PM Tuesday, September 29, 2026",
                "First linked note",
                "Second linked note",
            ]
        );
        assert_eq!(shown[1].1, Some(0));
        assert_eq!(shown[3].1, Some(0));
        assert!(shown[4].1.unwrap() >= 40 && shown[5].1.unwrap() > shown[4].1.unwrap());

        if let Some(directory) = std::env::var_os("SNOWBOUND_RECORDING_EXPORT") {
            let directory = PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("Recorded.one"), &image).unwrap();
            let file_id = Store::parse(&image).unwrap().header.file_id;
            std::fs::write(
                directory.join("Open Notebook.onetoc2"),
                onestore::create_table_of_contents(
                    "Open Notebook.onetoc2",
                    &[("Recorded.one", file_id)],
                )
                .unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn the_clock_shows_minutes_and_seconds() {
        assert_eq!(clock(8_400), "0:08");
        assert_eq!(clock(754_000), "12:34");
        assert_eq!(clock(3_723_000), "1:02:03");
    }
}
