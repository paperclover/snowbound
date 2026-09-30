//! Record Audio, Record Video and playback, as OneNote 2010 records and plays
//! (`corpus/recording`): a recording starts at the caret and goes on the page when stopped,
//! as a file OneNote plays; a recording, or a note linked to a moment in one, plays in a
//! transport over the page. What Snowbound cannot play opens in the system's player.

use crate::{State, media, platform, video};
use onestore::page::{Attachment, Recording};
use std::{error::Error, ops::Range, path::Path, sync::Arc, time::Duration};
use ui::{Axis, Flags, Spec, Theme, children, fit, px};
use winit::keyboard::NamedKey;

/// The rate audio is recorded at: wideband speech, as OneNote's own 8 kHz WMA is narrowband.
pub(crate) const RATE: u32 = 16_000;

/// What records or plays now.
#[derive(Default)]
pub(crate) enum Media {
    #[default]
    Idle,
    Recording {
        recorder: media::Recorder,
        video: bool,
        path: std::path::PathBuf,
    },
    /// A stopped recording's file being finished, which goes on the page once ready.
    Saving {
        video: bool,
        path: std::path::PathBuf,
        job: std::thread::JoinHandle<Result<Vec<u8>, String>>,
    },
    Playing(Box<Playback>),
}

pub(crate) struct Playback {
    player: media::Player,
    name: String,
    /// The recording's identity, whose linked notes See Playback follows.
    recording: Option<[u8; 16]>,
    video: Option<Video>,
    /// The moment Seek To is taking, as typed.
    seeking: Option<String>,
}

/// A video recording's pictures, and the one last shown.
struct Video {
    bytes: Arc<[u8]>,
    movie: video::Movie,
    shown: Option<(Range<usize>, draw::RasterImage)>,
}

/// The Recording and Playback tabs' commands besides Record Audio and Record Video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Pause,
    Stop,
    /// Seconds to rewind or fast forward.
    Skip(i32),
    SeekTo,
    SeePlayback,
}

fn seek_field() -> ui::Id {
    crate::page().child("transport").child("seek")
}

impl State {
    /// Record Audio or Record Video: starts recording at the caret, or stops the recording
    /// under way.
    pub(crate) fn record(&mut self, video: bool) -> Result<(), Box<dyn Error>> {
        match self.media {
            Media::Recording { .. } => return self.stop_recording(false),
            Media::Saving { .. } => return Ok(()),
            _ => self.media = Media::Idle,
        }
        let folder = std::env::temp_dir().join("Snowbound Recordings");
        std::fs::create_dir_all(&folder)?;
        let extension = if video { "avi" } else { "wav" };
        let path = folder.join(format!("{}.{extension}", std::process::id()));
        let recorder = if video {
            media::Recorder::video(&path)
        } else {
            media::Recorder::audio(&path, RATE)
        };
        let recorder = match recorder {
            Ok(recorder) => recorder,
            Err(detail) => {
                let title = if video {
                    "Couldn't record video"
                } else {
                    "Couldn't record audio"
                };
                platform::alert(title, &detail);
                return Ok(());
            }
        };
        let [date, time] = platform::date_text(crate::filetime());
        let kind = if video { "Video" } else { "Audio" };
        let label = format!("{kind} recording started: {time} {date}");
        let (id, response) = self.view.start_recording(&label)?;
        self.respond(response);
        if id.is_none() {
            recorder.stop()?()?;
            return Ok(());
        }
        self.media = Media::Recording {
            recorder,
            video,
            path,
        };
        Ok(())
    }

    /// Stop: the recording under way stops, and goes on the page where it started once its
    /// file is saved, which happens meanwhile; `wait` waits for it.
    pub(crate) fn stop_recording(&mut self, wait: bool) -> Result<(), Box<dyn Error>> {
        match std::mem::take(&mut self.media) {
            Media::Recording {
                recorder,
                video,
                path,
            } => {
                // Nothing written while the file saves links to the recording.
                self.view.editor.pause_recording(true);
                let finish = match recorder.stop() {
                    Ok(finish) => finish,
                    Err(detail) => {
                        platform::alert("Couldn't finish the recording", &detail);
                        return Ok(());
                    }
                };
                let recorded = path.clone();
                let job = std::thread::spawn(move || {
                    finish()?;
                    let recorded = std::fs::read(&recorded).map_err(|error| error.to_string())?;
                    Ok(if video {
                        recorded
                    } else {
                        compress(&recorded).map_or(recorded, |(bytes, _)| bytes)
                    })
                });
                self.media = Media::Saving { video, path, job };
            }
            other => self.media = other,
        }
        match &self.media {
            Media::Saving { job, .. } if wait || job.is_finished() => {}
            _ => return Ok(()),
        }
        let Media::Saving { video, path, job } = std::mem::take(&mut self.media) else {
            unreachable!("saving, as matched")
        };
        let saved = job
            .join()
            .unwrap_or_else(|_| Err("Try recording again.".into()));
        let Some(id) = self.view.editor.recording() else {
            return Ok(());
        };
        let bytes = match saved {
            Ok(bytes) => bytes,
            Err(detail) => {
                platform::alert("Couldn't finish the recording", &detail);
                return Ok(());
            }
        };
        let duration_ms = if video {
            video::Movie::parse(&bytes).map(|movie| movie.duration_ms())
        } else {
            Wave::parse(&bytes).map(|wave| wave.duration_ms())
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
        let file = Attachment {
            id: onestore::page::text::new_id()?,
            filename: format!("{name}.{extension}"),
            source_path: None,
            size: Some(crate::attachment::ICON_SIZE),
            layout: Default::default(),
            bytes: Some(bytes.into()),
            preview: preview.map(Into::into),
            recording: Some(Recording {
                id,
                kind: if video { 2 } else { 1 },
                duration_ms,
            }),
        };
        let response = self.view.finish_recording(file)?;
        self.respond(response);
        Ok(())
    }

    /// Plays recording `file` from `at_ms` in the transport, or opens it in the system's
    /// player where Snowbound cannot play it.
    pub(crate) fn play(&mut self, file: &Attachment, at_ms: u32) -> Result<(), Box<dyn Error>> {
        if matches!(self.media, Media::Recording { .. } | Media::Saving { .. }) {
            return Ok(());
        }
        self.media = Media::Idle;
        let Some(bytes) = file.bytes.clone() else {
            self.copy_attachment(file)?;
            return Ok(());
        };
        let name = Path::new(&file.filename)
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        let movie = video::Movie::parse(&bytes);
        // The sound as a WAV file every platform plays: IMA ADPCM decoded, a video's sound
        // taken out of it.
        let sound = movie.as_ref().map(|movie| {
            movie
                .wave(&bytes)
                .unwrap_or_else(|| silence(movie.duration_ms()))
        });
        let sound = sound.map_or_else(|| decompress(&bytes), Some);
        let playable = match sound {
            Some(wave) => Attachment {
                filename: format!("{name}.wav"),
                bytes: Some(decompress(&wave).unwrap_or(wave).into()),
                ..file.clone()
            },
            None => file.clone(),
        };
        let Some(path) = self.copy_attachment(&playable)? else {
            return Ok(());
        };
        // Scripted runs stay silent.
        let audible = std::env::var_os("SNOWBOUND_REPLAY").is_none();
        let Some(mut player) = media::Player::open(&path, audible) else {
            if let Some(path) = self.copy_attachment(file)? {
                platform::open_file(&path);
            }
            return Ok(());
        };
        player.seek(at_ms);
        player.play();
        self.media = Media::Playing(Box::new(Playback {
            player,
            name,
            recording: file.recording.map(|recording| recording.id),
            video: movie.map(|movie| Video {
                bytes,
                movie,
                shown: None,
            }),
            seeking: None,
        }));
        Ok(())
    }

    pub(crate) fn transport_status(&self, transport: Transport) -> crate::commands::Status {
        let (recording, playing) = match &self.media {
            Media::Idle | Media::Saving { .. } => (false, None),
            Media::Recording { .. } => (true, None),
            Media::Playing(playback) => (false, Some(playback.player.playing())),
        };
        let (enabled, checked) = match transport {
            Transport::Pause => (
                recording || playing.is_some(),
                Some(self.view.editor.recording_paused() || playing == Some(false)),
            ),
            Transport::Stop => (recording || playing.is_some(), None),
            Transport::Skip(_) | Transport::SeekTo => (playing.is_some(), None),
            Transport::SeePlayback => (true, Some(self.see_playback)),
        };
        crate::commands::Status { enabled, checked }
    }

    /// Runs a transport command: Pause pauses or resumes what records or plays.
    pub(crate) fn run_transport(&mut self, transport: Transport) -> Result<(), Box<dyn Error>> {
        match (transport, &mut self.media) {
            (Transport::SeePlayback, _) => self.see_playback = !self.see_playback,
            (Transport::Stop, Media::Recording { .. }) => self.stop_recording(false)?,
            (Transport::Stop, Media::Playing(_)) => self.media = Media::Idle,
            (Transport::Pause, Media::Recording { recorder, .. }) => {
                let paused = !self.view.editor.recording_paused();
                recorder.pause(paused);
                self.view.editor.pause_recording(paused);
            }
            (Transport::Pause, Media::Playing(playback)) => {
                let player = &mut playback.player;
                if player.playing() {
                    player.pause();
                } else {
                    if player.position_ms() >= player.duration_ms() {
                        player.seek(0);
                    }
                    player.play();
                }
            }
            (Transport::Skip(seconds), Media::Playing(playback)) => {
                let player = &mut playback.player;
                let at = i64::from(player.position_ms()) + i64::from(seconds) * 1000;
                player.seek(at.clamp(0, i64::from(player.duration_ms())) as u32);
            }
            (Transport::SeekTo, Media::Playing(playback)) => {
                playback.seeking = Some(clock(playback.player.position_ms()));
                self.ui.set_focus(Some(seek_field()));
                self.ui.focus_all(seek_field());
            }
            _ => {}
        }
        Ok(())
    }

    /// The transport over the page's foot while something records or plays: OneNote's
    /// Recording and Playback tabs, with a video's pictures above.
    pub(crate) fn transport(
        &mut self,
        theme: &Theme,
        page: [f32; 4],
    ) -> Result<(), Box<dyn Error>> {
        let row = theme.font_size * 2.0;
        if matches!(self.media, Media::Saving { .. }) {
            self.stop_recording(false)?;
        }
        let mut played = None;
        let (clock_text, picture) = match &mut self.media {
            Media::Idle => {
                let response = self.view.set_played(None)?;
                self.respond(response);
                return Ok(());
            }
            Media::Recording { video, .. } => {
                let at = self.view.editor.recording_ms().unwrap_or(0);
                let state = if self.view.editor.recording_paused() {
                    "Paused"
                } else if *video {
                    "Recording video"
                } else {
                    "Recording"
                };
                self.ui.wake_after(Duration::from_millis(250));
                (format!("{state}  {}", clock(at)), None)
            }
            Media::Saving { video, .. } => {
                self.ui.wake_after(Duration::from_millis(100));
                let saving = if *video {
                    "Saving video…"
                } else {
                    "Saving audio…"
                };
                (saving.to_owned(), None)
            }
            Media::Playing(playback) => {
                let at = playback.player.position_ms();
                played = playback
                    .recording
                    .filter(|_| self.see_playback)
                    .and_then(|id| self.view.editor.played_note(id, at))
                    .and_then(|note| canvas::search::paragraph_match(&self.view.editor, note));
                let mut picture = None;
                if let Some(video) = &mut playback.video {
                    let frame = video.movie.frame(at);
                    if video.shown.as_ref().map(|(range, _)| range) != frame.as_ref()
                        && let Some(frame) = frame
                    {
                        let image =
                            draw::RasterImage::decode(&video.bytes[frame.clone()], video::SIZE);
                        video.shown = image.ok().map(|image| (frame, image));
                    }
                    picture = video.shown.as_ref().map(|(_, image)| image.clone());
                }
                if playback.player.playing() {
                    let interval = if playback.video.is_some() {
                        1000 / u64::from(video::FPS)
                    } else {
                        250
                    };
                    self.ui.wake_after(Duration::from_millis(interval));
                }
                (
                    format!(
                        "{}  {} / {}",
                        playback.name,
                        clock(at),
                        clock(playback.player.duration_ms())
                    ),
                    picture,
                )
            }
        };
        let response = self.view.set_played(played)?;
        self.respond(response);
        let picture_height = picture
            .as_ref()
            .map_or(0.0, |image| image.size()[1] as f32 + 6.0);
        let height = row + 16.0 + picture_height;
        self.ui.open(
            "transport",
            Spec {
                flags: Flags::FLOAT | Flags::CLICKABLE,
                axis: Axis::Y,
                size: [children(), px(height)],
                position: [16.0, page[3] - page[1] - height - 16.0],
                fill: Some(theme.popup),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [8.0, 8.0],
                gap: 6.0,
                ..Spec::default()
            },
        );
        if let Some(image) = &picture {
            let [width, height] = image.size().map(|side| side as f32);
            self.ui.leaf(
                "picture",
                Spec {
                    size: [px(width), px(height)],
                    image: Some(image),
                    ..Spec::default()
                },
            );
        }
        self.ui.open(
            "controls",
            Spec {
                axis: Axis::X,
                size: [children(), px(row)],
                gap: 6.0,
                ..Spec::default()
            },
        );
        let paused = self.transport_status(Transport::Pause).checked == Some(true);
        let playing = matches!(self.media, Media::Playing(_));
        let pause = match (playing, paused) {
            (true, true) => "Play",
            (false, true) => "Resume",
            _ => "Pause",
        };
        let saving = matches!(self.media, Media::Saving { .. });
        let mut buttons = if saving {
            Vec::new()
        } else {
            vec![(pause, Transport::Pause)]
        };
        if playing {
            buttons.extend([
                ("−10 min", Transport::Skip(-600)),
                ("−10 s", Transport::Skip(-10)),
            ]);
        }
        let mut chosen = buttons
            .into_iter()
            .filter(|(label, transport)| self.transport_button(label, *transport))
            .last()
            .map(|(_, transport)| transport);
        let seek = self.seek(theme, &clock_text);
        let mut after = Vec::new();
        if playing {
            after.extend([
                ("+10 s", Transport::Skip(10)),
                ("+10 min", Transport::Skip(600)),
            ]);
        }
        for (label, transport) in after {
            if self.transport_button(label, transport) {
                chosen = Some(transport);
            }
        }
        if playing && ui::check_box(&mut self.ui, "see", "See Playback", self.see_playback).clicked
        {
            chosen = Some(Transport::SeePlayback);
        }
        if !saving && ui::button(&mut self.ui, "stop", "Stop").clicked {
            chosen = Some(Transport::Stop);
        }
        self.ui.close();
        self.ui.close();
        if let Some(at) = seek
            && let Media::Playing(playback) = &mut self.media
        {
            playback.player.seek(at.min(playback.player.duration_ms()));
        }
        if let Some(transport) = chosen {
            self.run_transport(transport)?;
        }
        Ok(())
    }

    /// A transport button showing `label`, a skip named by its command's title, which the
    /// label abbreviates; returns whether it was clicked.
    fn transport_button(&mut self, label: &str, transport: Transport) -> bool {
        let clicked = ui::button(&mut self.ui, label, label).clicked;
        if matches!(transport, Transport::Skip(_))
            && let Some(node) = self.ui.access(self.ui.id(label))
        {
            node.set_label(
                crate::commands::command(crate::commands::Id::Transport(transport)).title,
            );
        }
        clicked
    }

    /// The transport's clock, or while seeking the field taking the moment: Enter goes
    /// there, Escape or leaving the field keeps playing where it was. A click on the clock
    /// seeks as Seek To does.
    fn seek(&mut self, theme: &Theme, clock_text: &str) -> Option<u32> {
        let seeking = match &mut self.media {
            Media::Playing(playback) => &mut playback.seeking,
            _ => &mut None,
        };
        let focused = self.ui.focused() == Some(seek_field());
        let Some(text) = seeking.as_mut().filter(|_| focused) else {
            *seeking = None;
            let playing = matches!(self.media, Media::Playing(_));
            let clicked = self
                .ui
                .leaf(
                    "clock",
                    Spec {
                        flags: if playing {
                            Flags::CLICKABLE
                        } else {
                            Flags::default()
                        },
                        size: [fit(), px(theme.font_size * 2.0)],
                        text: Some(clock_text),
                        pad: [6.0, 0.0],
                        center: true,
                        role: playing.then_some(accesskit::Role::Button),
                        ..Spec::default()
                    },
                )
                .clicked;
            if playing && let Some(node) = self.ui.access(self.ui.id("clock")) {
                let title =
                    crate::commands::command(crate::commands::Id::Transport(Transport::SeekTo))
                        .title;
                node.set_label(title.trim_end_matches('…'));
                node.set_value(clock_text);
            }
            if clicked {
                let _ = self.run_transport(Transport::SeekTo);
            }
            return None;
        };
        let keys = ui::popup::navigation(
            &mut self.ui,
            &[seek_field()],
            &[NamedKey::Enter, NamedKey::Escape],
        );
        ui::text_field(
            &mut self.ui,
            seek_field(),
            text,
            "m:ss",
            Spec {
                size: [px(theme.font_size * 6.0), px(theme.font_size * 2.0)],
                fill: Some(theme.base),
                border: Some(theme.accent),
                radius: 4.0,
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
        let at = keys
            .contains(&NamedKey::Enter)
            .then(|| moment(text))
            .flatten();
        if !keys.is_empty() {
            *seeking = None;
            self.ui.set_focus(Some(crate::page()));
        }
        at
    }
}

/// `ms` as the transport shows a moment: minutes and seconds, hours when there are some.
fn clock(ms: u32) -> String {
    let seconds = ms / 1000;
    match seconds / 3600 {
        0 => format!("{}:{:02}", seconds / 60, seconds % 60),
        hours => format!("{hours}:{:02}:{:02}", seconds / 60 % 60, seconds % 60),
    }
}

/// A moment typed as the clock shows one, in milliseconds: seconds, m:ss or h:mm:ss.
fn moment(text: &str) -> Option<u32> {
    let mut seconds = 0u32;
    for part in text.trim().split(':') {
        let part: u32 = part.trim().parse().ok()?;
        seconds = seconds.checked_mul(60)?.checked_add(part)?;
    }
    seconds.checked_mul(1000)
}

/// Silence as long as `ms`, the clock of a video without sound.
fn silence(ms: u32) -> Vec<u8> {
    wave(8_000, &vec![0; ms as usize * 8])
}

/// A mono 16-bit PCM WAV file of `samples` at `rate`, as the platform recorders write.
pub(crate) fn wave(rate: u32, samples: &[i16]) -> Vec<u8> {
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

/// A mono IMA ADPCM WAV file as 16-bit PCM, which every platform's player decodes; none
/// for other files.
pub(crate) fn decompress(wav: &[u8]) -> Option<Vec<u8>> {
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
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

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

    /// A recording made through the editor at the end of "Before recording", two notes
    /// written while it records, stored as the host stores it; `export` names a variable
    /// naming a directory receiving the notebook for a cold read in OneNote 2010.
    fn record_through_the_editor(
        label: &str,
        filename: &str,
        kind: u32,
        bytes: &[u8],
        duration: u32,
        export: &str,
    ) {
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
        let id = editor.start_recording(&mut engine, label).unwrap();
        store(&mut editor);
        std::thread::sleep(Duration::from_millis(40));
        editor.insert(&mut engine, "First linked note").unwrap();
        store(&mut editor);
        editor.insert(&mut engine, "\n").unwrap();
        std::thread::sleep(Duration::from_millis(40));
        editor.insert(&mut engine, "Second linked note").unwrap();
        store(&mut editor);
        let file = Attachment {
            id: onestore::page::text::new_id().unwrap(),
            filename: filename.into(),
            source_path: None,
            size: Some(crate::attachment::ICON_SIZE),
            layout: Default::default(),
            bytes: Some(bytes.into()),
            preview: Some(canvas::gpu::page::file_icon().into()),
            recording: Some(Recording {
                id,
                kind,
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
                        assert_eq!(file.bytes.as_deref(), Some(bytes));
                        assert_eq!(file.recording.unwrap().kind, kind);
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
        let file = format!("[{filename}]");
        assert_eq!(
            texts,
            [
                "Before recording",
                &file,
                "",
                label,
                "First linked note",
                "Second linked note",
            ]
        );
        assert_eq!(shown[1].1, Some(0));
        assert_eq!(shown[3].1, Some(0));
        assert!(shown[4].1.unwrap() >= 40 && shown[5].1.unwrap() > shown[4].1.unwrap());

        if let Some(directory) = std::env::var_os(export) {
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

    /// `SNOWBOUND_RECORDING_EXPORT`: `corpus/recording/candidate`.
    #[test]
    fn a_recording_stores_its_file_line_and_linked_notes_for_onenote() {
        let (bytes, duration) = compress(&wave(RATE, &tone())).unwrap();
        record_through_the_editor(
            "Audio recording started: 5:18 PM Tuesday, September 29, 2026",
            "Recorded.wav",
            1,
            &bytes,
            duration,
            "SNOWBOUND_RECORDING_EXPORT",
        );
    }

    /// Two seconds of moving colour bars with the chord, as a video recording is stored.
    pub(crate) fn clip() -> Vec<u8> {
        let mut pictures = video::Pictures::default();
        // A camera's uneven 20 pictures a second, which the file shows at 15.
        for index in 0..40u32 {
            let picture = image::RgbImage::from_fn(640, 480, |x, y| {
                let bar = (x + index * 16) / 80 % 6;
                let [r, g, b] = [
                    [255, 255, 0],
                    [0, 255, 255],
                    [0, 255, 0],
                    [255, 0, 255],
                    [255, 0, 0],
                    [0, 0, 255],
                ][bar as usize];
                image::Rgb(if y > 400 {
                    [index as u8 * 6; 3]
                } else {
                    [r, g, b]
                })
            });
            pictures.take(u64::from(index) * 50_000, picture);
        }
        let data = tone()
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let sound = video::Pcm {
            rate: RATE,
            channels: 1,
            data,
        };
        video::write(video::SIZE, &pictures.finish(2_000_000), &sound).unwrap()
    }

    /// `SNOWBOUND_VIDEO_EXPORT`: `corpus/recording/video/candidate`.
    #[test]
    fn a_video_recording_stores_its_file_line_and_linked_notes_for_onenote() {
        let bytes = clip();
        let movie = video::Movie::parse(&bytes).unwrap();
        assert_eq!((movie.frames.len(), movie.duration_ms()), (30, 2000));
        record_through_the_editor(
            "Video recording started: 5:18 PM Tuesday, September 29, 2026",
            "Recorded.avi",
            2,
            &bytes,
            movie.duration_ms(),
            "SNOWBOUND_VIDEO_EXPORT",
        );
    }

    #[test]
    fn the_clock_shows_minutes_and_seconds() {
        assert_eq!(clock(8_400), "0:08");
        assert_eq!(clock(754_000), "12:34");
        assert_eq!(clock(3_723_000), "1:02:03");
        // Seek To takes the clock's own forms back.
        assert_eq!(moment("12:34"), Some(754_000));
        assert_eq!(moment(" 1:02:03 "), Some(3_723_000));
        assert_eq!(moment("45"), Some(45_000));
        assert_eq!(moment("1:x"), None);
    }
}
