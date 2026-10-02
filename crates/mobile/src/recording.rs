//! Record Audio, Record Video and playback, stored as the desktop stores them
//! (`canvas::recording`): the host records with the platform's devices and hands over a PCM
//! WAV file, or a video's pictures and sound as `Movie` takes them; a recording plays from
//! the files `sb_view_play_request` writes.

use super::*;
use canvas::recording::{self, video};
use std::path::Path;

impl Canvas {
    /// Starts recording at the caret, its line saying when from the long `date` and short
    /// `time`; whether it started.
    pub(crate) fn start_recording(&mut self, video: bool, date: &str, time: &str) -> Result<bool> {
        let label = recording::label(video, date, time);
        Ok(self.page.start_recording(&label)?.0.is_some())
    }

    /// Puts the recording made, `recorded`, where it started: a PCM WAV file, or with
    /// `video` an AVI file from `Movie`. None forgets the recording, whose file never came.
    pub(crate) fn finish_recording(
        &mut self,
        recorded: Option<Vec<u8>>,
        video: bool,
    ) -> Result<bool> {
        let editor = &mut self.page.editor;
        let Some(id) = editor.recording() else {
            return Ok(false);
        };
        let Some(recorded) = recorded else {
            editor.cancel_recording();
            return Ok(false);
        };
        let file = recording::file(id, &editor.page()?.title, video, recorded)?;
        Ok(moved(self.page.finish_recording(file)?))
    }

    /// The recording a tap asked to play, taken, with its files written to `folder` to play
    /// from: see `sb_view_play_request`.
    pub(crate) fn take_play(&mut self, folder: &Path) -> Result<Option<serde_json::Value>> {
        let Some((file, at)) = self.play.take() else {
            return Ok(None);
        };
        let bytes = file.bytes.clone().ok_or("The recording has no data")?;
        let stem = Path::new(&file.filename)
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        std::fs::create_dir_all(folder)?;
        let sound = match recording::sound(&bytes) {
            Some(wave) => {
                let path = folder.join(format!("{stem}.wav"));
                std::fs::write(&path, wave)?;
                path
            }
            None => {
                let path = folder.join(file.filename.replace('/', "_"));
                std::fs::write(&path, &bytes)?;
                path
            }
        };
        let movie = video::Movie::parse(&bytes)
            .map(|movie| -> Result<_> {
                let path = folder.join(format!("{stem}.avi"));
                std::fs::write(&path, &bytes)?;
                // An empty frame repeats the picture before it.
                let mut shown = [0; 2];
                let frames: Vec<[usize; 2]> = movie
                    .frames
                    .iter()
                    .map(|range| {
                        if !range.is_empty() {
                            shown = [range.start, range.end];
                        }
                        shown
                    })
                    .collect();
                Ok(serde_json::json!({
                    "path": path,
                    "frames": frames,
                    "frame_us": movie.frame_us,
                    "size": video::SIZE,
                }))
            })
            .transpose()?;
        self.playing = file.recording.map(|recording| recording.id);
        Ok(Some(serde_json::json!({
            "name": stem,
            "sound": sound,
            "video": movie,
            "at_ms": at,
        })))
    }

    /// See Playback: highlights the note linked last at or before `at_ms` into the recording
    /// playing, or with none ends playback.
    pub(crate) fn played(&mut self, at_ms: Option<u32>) -> Result<bool> {
        if at_ms.is_none() {
            self.playing = None;
        }
        let note = self
            .playing
            .zip(at_ms)
            .and_then(|(id, at)| self.page.editor.played_note(id, at))
            .and_then(|note| canvas::search::paragraph_match(&self.page.editor, note));
        Ok(moved(self.page.set_played(note)?))
    }
}

/// Starts recording audio, or with `video` video, at the caret, its line saying when from
/// the long `date` and short `time`; whether it started.
///
/// # Safety
/// `date` and `time` are NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_start_recording(
    view: &mut View,
    video: bool,
    date: *const c_char,
    time: *const c_char,
) -> bool {
    if view.read_only || view.reading.is_some() {
        return false;
    }
    let started = view
        .canvas
        .start_recording(video, &string(date), &string(time));
    let started = report(started).unwrap_or(false);
    view.stored(Ok(started));
    started
}

/// How far the recording under way has got in milliseconds, its pauses left out, or -1;
/// `paused` gets whether it is paused.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_recording(view: &View, paused: &mut bool) -> i64 {
    let editor = &view.canvas.page.editor;
    *paused = editor.recording_paused();
    editor.recording_ms().map_or(-1, i64::from)
}

/// Pauses or resumes the recording: nothing written while it is paused links to it.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_pause_recording(view: &mut View, paused: bool) {
    view.canvas.page.editor.pause_recording(paused);
}

/// Stops recording: the file recorded, `length` bytes of mono 16-bit PCM WAV or with
/// `video` the AVI file `sb_movie_finish` made, goes where the recording started. Null
/// `bytes` forgets the recording.
///
/// # Safety
/// `bytes` is null or holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_finish_recording(
    view: &mut View,
    bytes: *const u8,
    length: usize,
    video: bool,
) -> bool {
    // SAFETY: the caller's contract.
    let recorded = (!bytes.is_null()).then(|| unsafe { std::slice::from_raw_parts(bytes, length) });
    let result = view
        .canvas
        .finish_recording(recorded.map(<[u8]>::to_vec), video);
    view.stored(result)
}

/// The recording the last tap asked to play, taken, as JSON: its `name`, the `sound` file
/// written to `folder` that the platform's player plays, from `at_ms`; and for a video its
/// `video` file `path`, the byte range of the JPEG picture each of its `frames` shows,
/// `frame_us` apart, and their `size`. Null when none was asked for.
///
/// # Safety
/// `folder` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_view_play_request(
    view: &mut View,
    folder: *const c_char,
) -> *mut c_char {
    let request = view.canvas.take_play(Path::new(&string(folder)));
    report(request)
        .flatten()
        .map_or(std::ptr::null_mut(), |json| owned(json.to_string()))
}

/// The recording last asked to play is at `at_ms`, whose linked note the page highlights;
/// -1 ends playback. Whether the page changed.
#[unsafe(no_mangle)]
pub extern "C" fn sb_view_played(view: &mut View, at_ms: i64) -> bool {
    let at = u32::try_from(at_ms).ok();
    report(view.canvas.played(at)).unwrap_or(false)
}

/// The rate the host records sound at, as mono 16-bit PCM: the desktop's.
#[unsafe(no_mangle)]
pub extern "C" fn sb_recording_rate() -> u32 {
    recording::RATE
}

/// A video recording's pictures and sound as the camera gave them, which become the AVI
/// file the desktop records.
#[derive(Default)]
pub struct Movie {
    pictures: video::Pictures,
    sound: Vec<u8>,
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_movie_new() -> *mut Movie {
    Box::into_raw(Box::default())
}

/// The picture taken `at_us` microseconds in: `height` rows of `stride` bytes of BGRA.
///
/// # Safety
/// `bgra` holds `height` × `stride` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_movie_picture(
    movie: &mut Movie,
    at_us: u64,
    bgra: *const u8,
    width: u32,
    height: u32,
    stride: usize,
) {
    // SAFETY: the caller's contract.
    let bgra = unsafe { std::slice::from_raw_parts(bgra, height as usize * stride) };
    movie.pictures.take_bgra(
        at_us,
        [width, height].map(|side| side as usize),
        stride,
        bgra,
    );
}

/// More of the sound, mono 16-bit little-endian PCM at `canvas::recording::RATE`.
///
/// # Safety
/// `pcm` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_movie_sound(movie: &mut Movie, pcm: *const u8, length: usize) {
    // SAFETY: the caller's contract.
    movie
        .sound
        .extend_from_slice(unsafe { std::slice::from_raw_parts(pcm, length) });
}

/// The AVI file of a movie `duration_us` long, `length` bytes freed with `sb_bytes_free`,
/// or null when it has no pictures or passes 4 GiB.
///
/// # Safety
/// `movie` came from `sb_movie_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_movie_finish(
    movie: *mut Movie,
    duration_us: u64,
    length: &mut usize,
) -> *mut u8 {
    // SAFETY: the caller's contract.
    let movie = unsafe { Box::from_raw(movie) };
    let frames = movie.pictures.finish(duration_us);
    if frames.is_empty() {
        return std::ptr::null_mut();
    }
    let sound = video::Pcm {
        rate: recording::RATE,
        channels: 1,
        data: movie.sound,
    };
    let Some(avi) = video::write(video::SIZE, &frames, &sound) else {
        return std::ptr::null_mut();
    };
    *length = avi.len();
    Box::into_raw(avi.into_boxed_slice()).cast()
}
