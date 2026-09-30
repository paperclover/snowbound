//! Audio through the Media Control Interface, which every Windows since 3.1 has: DirectShow's
//! `mpegvideo` device plays whatever codecs are installed (WAV, Windows Media, MP3), and
//! `waveaudio` records the default microphone as PCM WAV. Video isn't recorded here.

use std::{
    path::Path,
    sync::atomic::{AtomicU32, Ordering},
};

#[link(name = "winmm")]
unsafe extern "system" {
    fn mciSendStringW(command: *const u16, answer: *mut u16, length: u32, window: isize) -> u32;
}

/// Sends `command`, answering its return string; none where MCI refuses it.
fn send(command: &str) -> Option<String> {
    let command: Vec<u16> = command.encode_utf16().chain([0]).collect();
    let mut answer = [0u16; 256];
    let error = unsafe {
        mciSendStringW(
            command.as_ptr(),
            answer.as_mut_ptr(),
            answer.len() as u32,
            0,
        )
    };
    let end = answer.iter().position(|&unit| unit == 0).unwrap_or(0);
    (error == 0).then(|| String::from_utf16_lossy(&answer[..end]))
}

/// A device alias no other device of this process has.
fn alias(kind: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!("snowbound_{kind}{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// `path` quoted for a command; none where a quote would end it early.
fn quoted(path: &Path) -> Option<String> {
    let path = path.to_str()?;
    (!path.contains('"')).then(|| format!("\"{path}\""))
}

/// A recording or other audio file playing, paused or ended.
pub struct Player {
    alias: String,
    playing: bool,
}

impl Player {
    /// The file at `path`, ready to play, silent unless `audible`; none when no codec
    /// decodes it.
    pub fn open(path: &Path, audible: bool) -> Option<Self> {
        let alias = alias("player");
        send(&format!(
            "open {} type mpegvideo alias {alias}",
            quoted(path)?
        ))?;
        let player = Self {
            alias,
            playing: false,
        };
        send(&format!("set {} time format milliseconds", player.alias))?;
        if !audible {
            send(&format!("setaudio {} off", player.alias))?;
        }
        Some(player)
    }

    pub fn play(&mut self) {
        self.playing = send(&format!("play {}", self.alias)).is_some();
    }

    pub fn pause(&mut self) {
        send(&format!("pause {}", self.alias));
        self.playing = false;
    }

    pub fn playing(&self) -> bool {
        self.playing && send(&format!("status {} mode", self.alias)).as_deref() == Some("playing")
    }

    fn status(&self, item: &str) -> u32 {
        send(&format!("status {} {item}", self.alias))
            .and_then(|answer| answer.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn position_ms(&self) -> u32 {
        self.status("position")
    }

    pub fn duration_ms(&self) -> u32 {
        self.status("length")
    }

    pub fn seek(&mut self, ms: u32) {
        send(&format!("seek {} to {ms}", self.alias));
        if self.playing {
            self.play();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        send(&format!("close {}", self.alias));
    }
}

/// The default microphone recording to a file.
pub struct Recorder {
    alias: String,
    path: String,
}

impl Recorder {
    /// Records mono 16-bit PCM at `rate` into a WAV file at `path`.
    pub fn audio(path: &Path, rate: u32) -> Result<Self, String> {
        let path = quoted(path).ok_or("Try recording again.")?;
        let alias = alias("recorder");
        send(&format!("open new type waveaudio alias {alias}"))
            .ok_or("Connect a microphone and try again.")?;
        let recorder = Self { alias, path };
        let alias = &recorder.alias;
        send(&format!(
            "set {alias} time format milliseconds bitspersample 16 channels 1 \
             samplespersec {rate} bytespersec {} alignment 2",
            rate * 2
        ))
        .ok_or("Try recording again.")?;
        send(&format!("record {alias}")).ok_or("Connect a microphone and try again.")?;
        Ok(recorder)
    }

    pub fn video(_: &Path) -> Result<Self, String> {
        Err("Snowbound can't record video on Windows yet. Record audio instead.".into())
    }

    /// Pauses or resumes recording.
    pub fn pause(&mut self, paused: bool) {
        let command = if paused { "pause" } else { "resume" };
        send(&format!("{command} {}", self.alias));
    }

    /// Ends the recording and writes its file, with nothing left to run.
    pub fn stop(self) -> Result<impl FnOnce() -> Result<(), String> + Send, String> {
        send(&format!("stop {}", self.alias));
        let saved = send(&format!("save {} {}", self.alias, self.path));
        drop(self);
        match saved {
            Some(_) => Ok(|| Ok(())),
            None => Err("The recording couldn't be saved. Try recording again.".into()),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        send(&format!("close {}", self.alias));
    }
}
