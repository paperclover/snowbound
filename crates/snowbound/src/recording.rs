//! Record Audio, Record Video and playback, as OneNote 2010 records and plays
//! (`corpus/recording`): a recording starts at the caret and goes on the page when stopped,
//! as a file OneNote plays; a recording, or a note linked to a moment in one, plays in a
//! transport over the page. What Snowbound cannot play opens in the system's player.

use crate::{State, media, platform};
use canvas::recording::{RATE, video};
use onestore::page::Attachment;
use std::{error::Error, ops::Range, path::Path, sync::Arc, time::Duration};
use ui::{Axis, Flags, Spec, Theme, children, fill, fit, px};
use winit::keyboard::NamedKey;

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
        job: std::thread::JoinHandle<Result<Attachment, String>>,
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
        notebook::fs::create_dir_all(&folder)?;
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
        let label = canvas::recording::label(video, &date, &time);
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
                let id = self.view.editor.recording();
                let title = self.view.editor.page()?.title;
                let job = std::thread::spawn(move || {
                    finish()?;
                    let recorded =
                        notebook::fs::read(&recorded).map_err(|error| error.to_string())?;
                    let id = id.ok_or("Try recording again.")?;
                    canvas::recording::file(id, &title, video, recorded)
                        .map_err(|error| error.to_string())
                });
                self.media = Media::Saving { video, path, job };
            }
            other => self.media = other,
        }
        match &self.media {
            Media::Saving { job, .. } if wait || job.is_finished() => {}
            _ => return Ok(()),
        }
        let Media::Saving { path, job, .. } = std::mem::take(&mut self.media) else {
            unreachable!("saving, as matched")
        };
        let saved = job
            .join()
            .unwrap_or_else(|_| Err("Try recording again.".into()));
        if self.view.editor.recording().is_none() {
            return Ok(());
        }
        let mut file = match saved {
            Ok(file) => file,
            Err(detail) => {
                platform::alert("Couldn't finish the recording", &detail);
                return Ok(());
            }
        };
        file.preview = platform::file_icon(&path).map(Into::into);
        notebook::fs::remove_file(&path)?;
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
        let playable = match canvas::recording::sound(&bytes) {
            Some(wave) => Attachment {
                filename: format!("{name}.wav"),
                bytes: Some(wave.into()),
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
            video: video::Movie::parse(&bytes).map(|movie| Video {
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
    pub(crate) fn transport(&mut self, theme: &Theme) -> Result<(), Box<dyn Error>> {
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
        // In the page's lower leading corner.
        self.ui.open(
            "corner",
            Spec {
                flags: Flags::FLOAT,
                axis: Axis::Y,
                size: [fill(), fill()],
                pad: [16.0, 16.0],
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "room",
            Spec {
                size: [px(0.0), fill()],
                ..Spec::default()
            },
        );
        self.ui.open(
            "transport",
            Spec {
                flags: Flags::CLICKABLE,
                axis: Axis::Y,
                size: [children(), px(height)],
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
                node.set_label(title);
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use canvas::recording::{compress, wave};
    use onestore::page::Recording;
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
            size: Some(canvas::gpu::page::ICON_SIZE),
            layout: Default::default(),
            bytes: Some(bytes.into()),
            preview: Some(canvas::gpu::page::file_icon().into()),
            recording: Some(Recording {
                id,
                kind,
                duration_ms: Some(duration),
            }),
            tags: Vec::new(),
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
            notebook::fs::create_dir_all(&directory).unwrap();
            notebook::fs::write(directory.join("Recorded.one"), &image).unwrap();
            let file_id = Store::parse(&image).unwrap().header.file_id;
            notebook::fs::write(
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
