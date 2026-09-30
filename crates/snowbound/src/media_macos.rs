//! Audio through AVFoundation: `AVAudioPlayer` plays what Core Audio decodes (WAV, MP3, AAC;
//! not Windows Media), and `AVAudioRecorder` records the default microphone as PCM WAV.

use objc2::{
    msg_send, msg_send_id,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_foundation::{NSString, NSURL};
use std::path::Path;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVFormatIDKey: &'static NSString;
    static AVSampleRateKey: &'static NSString;
    static AVNumberOfChannelsKey: &'static NSString;
    static AVLinearPCMBitDepthKey: &'static NSString;
    static AVLinearPCMIsFloatKey: &'static NSString;
    static AVLinearPCMIsBigEndianKey: &'static NSString;
    static AVMediaTypeAudio: &'static NSString;
}

/// kAudioFormatLinearPCM.
const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");

fn class(name: &str) -> &'static AnyClass {
    AnyClass::get(name).expect("AVFoundation is linked")
}

fn url(path: &Path) -> Option<Retained<NSURL>> {
    Some(unsafe { NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)) })
}

/// A recording or other audio file playing, paused or ended.
pub struct Player(Retained<AnyObject>);

impl Player {
    /// The file at `path`, ready to play; none when Core Audio cannot decode it.
    pub fn open(path: &Path) -> Option<Self> {
        let url = url(path)?;
        unsafe {
            let player: Allocated<AnyObject> = msg_send_id![class("AVAudioPlayer"), alloc];
            let player: Option<Retained<AnyObject>> = msg_send_id![
                player,
                initWithContentsOfURL: &*url,
                error: std::ptr::null_mut::<*mut AnyObject>()
            ];
            let player = player?;
            let ready: bool = msg_send![&player, prepareToPlay];
            ready.then_some(Self(player))
        }
    }

    pub fn play(&mut self) {
        let _: bool = unsafe { msg_send![&self.0, play] };
    }

    pub fn pause(&mut self) {
        let _: () = unsafe { msg_send![&self.0, pause] };
    }

    pub fn playing(&self) -> bool {
        unsafe { msg_send![&self.0, isPlaying] }
    }

    pub fn position_ms(&self) -> u32 {
        let seconds: f64 = unsafe { msg_send![&self.0, currentTime] };
        (seconds * 1000.0) as u32
    }

    pub fn duration_ms(&self) -> u32 {
        let seconds: f64 = unsafe { msg_send![&self.0, duration] };
        (seconds * 1000.0) as u32
    }

    pub fn seek(&mut self, ms: u32) {
        let _: () = unsafe { msg_send![&self.0, setCurrentTime: f64::from(ms) / 1000.0] };
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _: () = unsafe { msg_send![&self.0, stop] };
    }
}

/// The default microphone recording to a file.
pub struct Microphone(Retained<AnyObject>);

impl Microphone {
    /// Records mono 16-bit PCM at `rate` into a WAV file at `path`. The system asks for the
    /// microphone the first time; refused, recording fails with what to do about it.
    pub fn start(path: &Path, rate: u32) -> Result<Self, String> {
        let refused = || {
            "Allow Snowbound to use the microphone in System Settings, Privacy & Security, \
             Microphone."
                .to_owned()
        };
        unsafe {
            // AVAuthorizationStatusRestricted and AVAuthorizationStatusDenied.
            let status: isize = msg_send![
                class("AVCaptureDevice"),
                authorizationStatusForMediaType: AVMediaTypeAudio
            ];
            if matches!(status, 1 | 2) {
                return Err(refused());
            }
            let number = |value: u32| -> Retained<AnyObject> {
                msg_send_id![class("NSNumber"), numberWithUnsignedInt: value]
            };
            let settings: Retained<AnyObject> =
                msg_send_id![class("NSMutableDictionary"), dictionary];
            for (key, value) in [
                (AVFormatIDKey, number(LINEAR_PCM)),
                (AVSampleRateKey, number(rate)),
                (AVNumberOfChannelsKey, number(1)),
                (AVLinearPCMBitDepthKey, number(16)),
                (AVLinearPCMIsFloatKey, number(0)),
                (AVLinearPCMIsBigEndianKey, number(0)),
            ] {
                let _: () = msg_send![&settings, setObject: &*value, forKey: key];
            }
            let url = url(path).ok_or("Try recording again.")?;
            let recorder: Allocated<AnyObject> = msg_send_id![class("AVAudioRecorder"), alloc];
            let recorder: Option<Retained<AnyObject>> = msg_send_id![
                recorder,
                initWithURL: &*url,
                settings: &*settings,
                error: std::ptr::null_mut::<*mut AnyObject>()
            ];
            let recorder = recorder.ok_or("Connect a microphone and try again.")?;
            let recording: bool = msg_send![&recorder, record];
            if !recording {
                return Err(refused());
            }
            Ok(Self(recorder))
        }
    }

    /// Ends the recording, its file complete.
    pub fn stop(self) -> Result<(), String> {
        let _: () = unsafe { msg_send![&self.0, stop] };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Snowbound records, compressed, plays back through Core Audio at its length.
    #[test]
    fn core_audio_plays_the_compressed_recordings() {
        let pcm =
            crate::recording::tests::pcm(crate::recording::RATE, &crate::recording::tests::tone());
        let (bytes, duration) = crate::recording::compress(&pcm).unwrap();
        let folder = std::env::temp_dir().join(format!("snowbound-player-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("tone.wav");
        std::fs::write(&path, bytes).unwrap();
        let mut player = Player::open(&path).unwrap();
        assert!(
            player.duration_ms().abs_diff(duration) < 50,
            "{}",
            player.duration_ms()
        );
        player.seek(1500);
        assert!(player.position_ms().abs_diff(1500) < 50);
        drop(player);
        // OneNote's own recording, Windows Media Audio, stays with the system's player.
        let wma = folder.join("Recorded.wma");
        std::fs::write(
            &wma,
            include_bytes!(
                "../../../corpus/recording/edit/cold/read/c41bd6bd61277bdc4bf3557b5572be5c08451ba5b4665aba662ac1280d907ca3.attachment"
            ),
        )
        .unwrap();
        assert!(Player::open(&wma).is_none());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
