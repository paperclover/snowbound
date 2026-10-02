//! Audio and video through AVFoundation: `AVAudioPlayer` plays what Core Audio decodes
//! (WAV, MP3, AAC; not Windows Media), `AVAudioRecorder` records the default microphone as
//! PCM WAV, and a capture session records the default camera and microphone as a movie,
//! which `AVAssetReader` reads back into the AVI file OneNote plays.

use canvas::recording::video;
use objc2::{
    msg_send, msg_send_id,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject, ClassBuilder, Sel},
    sel,
};
use objc2_foundation::{NSString, NSURL};
use std::{
    ffi::c_void,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVFormatIDKey: &'static NSString;
    static AVSampleRateKey: &'static NSString;
    static AVNumberOfChannelsKey: &'static NSString;
    static AVLinearPCMBitDepthKey: &'static NSString;
    static AVLinearPCMIsFloatKey: &'static NSString;
    static AVLinearPCMIsBigEndianKey: &'static NSString;
    static AVLinearPCMIsNonInterleaved: &'static NSString;
    static AVMediaTypeAudio: &'static NSString;
    static AVMediaTypeVideo: &'static NSString;
    static AVCaptureSessionPreset320x240: &'static NSString;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: &'static NSString;
    fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *const u8;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
}

/// CMTime.
#[repr(C)]
#[derive(Clone, Copy)]
struct Time {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: CMTime's layout and its Objective-C type encoding, `{?=qiIq}`.
unsafe impl objc2::Encode for Time {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
        "?",
        &[
            objc2::Encoding::LongLong,
            objc2::Encoding::Int,
            objc2::Encoding::UInt,
            objc2::Encoding::LongLong,
        ],
    );
}

impl Time {
    fn micros(self) -> Option<u64> {
        // kCMTimeFlags_Valid
        (self.flags & 1 != 0 && self.timescale > 0).then(|| {
            (i128::from(self.value.max(0)) * 1_000_000 / i128::from(self.timescale)) as u64
        })
    }
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferGetImageBuffer(sample: *mut c_void) -> *mut c_void;
    fn CMSampleBufferGetPresentationTimeStamp(sample: *mut c_void) -> Time;
    fn CMSampleBufferGetDataBuffer(sample: *mut c_void) -> *mut c_void;
    fn CMBlockBufferGetDataLength(buffer: *mut c_void) -> usize;
    fn CMBlockBufferCopyDataBytes(
        buffer: *mut c_void,
        offset: usize,
        length: usize,
        destination: *mut u8,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

/// CMSampleBuffer, as Objective-C names its type.
#[repr(C)]
struct Sample([u8; 0]);

// SAFETY: an opaque struct behind a pointer, `^{opaqueCMSampleBuffer=}`.
unsafe impl objc2::RefEncode for Sample {
    const ENCODING_REF: objc2::Encoding =
        objc2::Encoding::Pointer(&objc2::Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

/// kAudioFormatLinearPCM and kCVPixelFormatType_32BGRA.
const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");

fn class(name: &str) -> &'static AnyClass {
    AnyClass::get(name).expect("AVFoundation is linked")
}

fn url(path: &Path) -> Option<Retained<NSURL>> {
    Some(unsafe { NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)) })
}

fn number(value: u32) -> Retained<AnyObject> {
    unsafe { msg_send_id![class("NSNumber"), numberWithUnsignedInt: value] }
}

fn dictionary(entries: &[(&NSString, Retained<AnyObject>)]) -> Retained<AnyObject> {
    unsafe {
        let dictionary: Retained<AnyObject> =
            msg_send_id![class("NSMutableDictionary"), dictionary];
        for (key, value) in entries {
            let _: () = msg_send![&dictionary, setObject: &**value, forKey: *key];
        }
        dictionary
    }
}

/// Mono 16-bit little-endian PCM at `rate`.
fn pcm_settings(rate: u32) -> Retained<AnyObject> {
    unsafe {
        dictionary(&[
            (AVFormatIDKey, number(LINEAR_PCM)),
            (AVSampleRateKey, number(rate)),
            (AVNumberOfChannelsKey, number(1)),
            (AVLinearPCMBitDepthKey, number(16)),
            (AVLinearPCMIsFloatKey, number(0)),
            (AVLinearPCMIsBigEndianKey, number(0)),
            (AVLinearPCMIsNonInterleaved, number(0)),
        ])
    }
}

/// A recording or other audio file playing, paused or ended.
pub struct Player(Retained<AnyObject>);

impl Player {
    /// The file at `path`, ready to play, silent unless `audible`; none when Core Audio
    /// cannot decode it.
    pub fn open(path: &Path, audible: bool) -> Option<Self> {
        let url = url(path)?;
        unsafe {
            // Mac OS X 10.6 has no AVFoundation.
            let player: Allocated<AnyObject> = msg_send_id![AnyClass::get("AVAudioPlayer")?, alloc];
            let player: Option<Retained<AnyObject>> = msg_send_id![
                player,
                initWithContentsOfURL: &*url,
                error: std::ptr::null_mut::<*mut AnyObject>()
            ];
            let player = player?;
            if !audible {
                let _: () = msg_send![&player, setVolume: 0.0f32];
            }
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

/// The default microphone, or camera and microphone, recording to a file.
pub enum Recorder {
    Audio(Retained<AnyObject>),
    Video {
        session: Retained<AnyObject>,
        output: Retained<AnyObject>,
        /// The movie the session records, which becomes the AVI file at `path`.
        movie: std::path::PathBuf,
        path: std::path::PathBuf,
    },
}

/// Set when the capture session has finished writing its movie.
static MOVIE_WRITTEN: AtomicBool = AtomicBool::new(false);

/// Mac OS X 10.6 has no AVFoundation to record with.
fn recordable() -> Result<(), String> {
    AnyClass::get("AVCaptureDevice")
        .map(|_| ())
        .ok_or_else(|| "To record, use Mac OS X 10.7 or later.".into())
}

/// What to do when the system refused `device` ("microphone" or "camera").
fn refused(device: &str, pane: &str) -> String {
    format!("Allow Snowbound to use the {device} in System Settings, Privacy & Security, {pane}.")
}

/// Whether the system refused media of `kind`: AVAuthorizationStatusRestricted or Denied.
/// Before macOS 10.14 it never asks, and nothing is refused.
fn denied(kind: &NSString) -> bool {
    let device = class("AVCaptureDevice");
    let selector = sel!(authorizationStatusForMediaType:);
    if !unsafe { msg_send![device, respondsToSelector: selector] } {
        return false;
    }
    let status: isize = unsafe { msg_send![device, authorizationStatusForMediaType: kind] };
    matches!(status, 1 | 2)
}

impl Recorder {
    /// Records mono 16-bit PCM at `rate` into a WAV file at `path`. The system asks for the
    /// microphone the first time; refused, recording fails with what to do about it.
    pub fn audio(path: &Path, rate: u32) -> Result<Self, String> {
        recordable()?;
        unsafe {
            if denied(AVMediaTypeAudio) {
                return Err(refused("microphone", "Microphone"));
            }
            let url = url(path).ok_or("Try recording again.")?;
            let recorder: Allocated<AnyObject> = msg_send_id![class("AVAudioRecorder"), alloc];
            let recorder: Option<Retained<AnyObject>> = msg_send_id![
                recorder,
                initWithURL: &*url,
                settings: &*pcm_settings(rate),
                error: std::ptr::null_mut::<*mut AnyObject>()
            ];
            let recorder = recorder.ok_or("Connect a microphone and try again.")?;
            let recording: bool = msg_send![&recorder, record];
            if !recording {
                return Err(refused("microphone", "Microphone"));
            }
            Ok(Self::Audio(recorder))
        }
    }

    /// Records the default camera, with the microphone where there is one, as a Motion JPEG
    /// AVI file at `path` once stopped. The system asks for each device the first time.
    pub fn video(path: &Path) -> Result<Self, String> {
        recordable()?;
        unsafe {
            if denied(AVMediaTypeVideo) {
                return Err(refused("camera", "Camera"));
            }
            let camera: Option<Retained<AnyObject>> = msg_send_id![
                class("AVCaptureDevice"),
                defaultDeviceWithMediaType: AVMediaTypeVideo
            ];
            let camera = camera.ok_or("Connect a camera and try again.")?;
            let session: Retained<AnyObject> = msg_send_id![class("AVCaptureSession"), new];
            let _: () = msg_send![&session, beginConfiguration];
            let small: bool =
                msg_send![&session, canSetSessionPreset: AVCaptureSessionPreset320x240];
            if small {
                let _: () = msg_send![&session, setSessionPreset: AVCaptureSessionPreset320x240];
            }
            let add = |device: &AnyObject| -> bool {
                let input: Option<Retained<AnyObject>> = msg_send_id![
                    class("AVCaptureDeviceInput"),
                    deviceInputWithDevice: device,
                    error: std::ptr::null_mut::<*mut AnyObject>()
                ];
                let Some(input) = input else {
                    return false;
                };
                let fits: bool = msg_send![&session, canAddInput: &*input];
                if fits {
                    let _: () = msg_send![&session, addInput: &*input];
                }
                fits
            };
            if !add(&camera) {
                return Err(refused("camera", "Camera"));
            }
            // Without a microphone, or refused one, the video records silent.
            let microphone: Option<Retained<AnyObject>> = msg_send_id![
                class("AVCaptureDevice"),
                defaultDeviceWithMediaType: AVMediaTypeAudio
            ];
            if let Some(microphone) = microphone.filter(|_| !denied(AVMediaTypeAudio)) {
                add(&microphone);
            }
            let output: Retained<AnyObject> = msg_send_id![class("AVCaptureMovieFileOutput"), new];
            let fits: bool = msg_send![&session, canAddOutput: &*output];
            if !fits {
                return Err("Try recording again.".into());
            }
            let _: () = msg_send![&session, addOutput: &*output];
            let _: () = msg_send![&session, commitConfiguration];
            let _: () = msg_send![&session, startRunning];
            let movie = path.with_extension("mov");
            let _ = std::fs::remove_file(&movie);
            let url = url(&movie).ok_or("Try recording again.")?;
            MOVIE_WRITTEN.store(false, Ordering::SeqCst);
            let _: () = msg_send![
                &output,
                startRecordingToOutputFileURL: &*url,
                recordingDelegate: &*delegate()
            ];
            Ok(Self::Video {
                session,
                output,
                movie,
                path: path.to_owned(),
            })
        }
    }

    /// Pauses or resumes recording.
    pub fn pause(&mut self, paused: bool) {
        unsafe {
            match (self, paused) {
                (Self::Audio(recorder), true) => {
                    let _: () = msg_send![&**recorder, pause];
                }
                (Self::Audio(recorder), false) => {
                    let _: bool = msg_send![&**recorder, record];
                }
                (Self::Video { output, .. }, true) => {
                    let _: () = msg_send![&**output, pauseRecording];
                }
                (Self::Video { output, .. }, false) => {
                    let _: () = msg_send![&**output, resumeRecording];
                }
            }
        }
    }

    /// Ends the recording. Its file is complete once what this returns has run, on any
    /// thread: a video's conversion takes a while.
    pub fn stop(self) -> Result<impl FnOnce() -> Result<(), String> + Send, String> {
        let movie = match self {
            Self::Audio(recorder) => {
                let _: () = unsafe { msg_send![&recorder, stop] };
                None
            }
            Self::Video {
                session,
                output,
                movie,
                path,
            } => {
                unsafe {
                    let _: () = msg_send![&output, stopRecording];
                    // The movie is written once the delegate hears so, on the main thread
                    // or another.
                    let deadline = Instant::now() + Duration::from_secs(30);
                    while !MOVIE_WRITTEN.load(Ordering::SeqCst) && Instant::now() < deadline {
                        let run_loop: Retained<AnyObject> =
                            msg_send_id![class("NSRunLoop"), currentRunLoop];
                        let until: Retained<AnyObject> =
                            msg_send_id![class("NSDate"), dateWithTimeIntervalSinceNow: 0.05f64];
                        let _: () = msg_send![&run_loop, runUntilDate: &*until];
                    }
                    let _: () = msg_send![&session, stopRunning];
                }
                Some((movie, path))
            }
        };
        Ok(move || {
            let Some((movie, path)) = movie else {
                return Ok(());
            };
            let avi = objc2::rc::autoreleasepool(|_| avi(&movie))
                .ok_or("The recording stopped early. Try recording again.");
            let _ = std::fs::remove_file(&movie);
            std::fs::write(&path, avi?).map_err(|error| error.to_string())
        })
    }
}

/// The capture session's recording delegate, which says when the movie is written.
fn delegate() -> Retained<AnyObject> {
    static CLASS: std::sync::OnceLock<&'static AnyClass> = std::sync::OnceLock::new();
    let class = CLASS.get_or_init(|| {
        unsafe extern "C" fn finished(
            _: &AnyObject,
            _: Sel,
            _: *mut AnyObject,
            _: *mut AnyObject,
            _: *mut AnyObject,
            _: *mut AnyObject,
        ) {
            MOVIE_WRITTEN.store(true, Ordering::SeqCst);
        }
        let mut builder = ClassBuilder::new("SnowboundRecordingDelegate", class("NSObject"))
            .expect("Unique recording delegate class");
        unsafe {
            builder.add_method(
                sel!(captureOutput:didFinishRecordingToOutputFileAtURL:fromConnections:error:),
                finished as unsafe extern "C" fn(_, _, _, _, _, _),
            );
        }
        builder.register()
    });
    unsafe { msg_send_id![*class, new] }
}

/// The movie at `path`, as AVFoundation reads it, as a Motion JPEG AVI file with its sound
/// in mono PCM at `canvas::recording::RATE`.
fn avi(path: &Path) -> Option<Vec<u8>> {
    unsafe {
        let asset: Retained<AnyObject> = msg_send_id![class("AVURLAsset"), URLAssetWithURL: &*url(path)?, options: std::ptr::null::<AnyObject>()];
        let duration: Time = msg_send![&asset, duration];
        let mut pictures = video::Pictures::default();
        let video_settings = dictionary(&[(kCVPixelBufferPixelFormatTypeKey, number(BGRA))]);
        read(&asset, AVMediaTypeVideo, &video_settings, |sample| {
            let Some(at) = CMSampleBufferGetPresentationTimeStamp(sample).micros() else {
                return;
            };
            let buffer = CMSampleBufferGetImageBuffer(sample);
            if buffer.is_null() || CVPixelBufferLockBaseAddress(buffer, 1) != 0 {
                return;
            }
            let [width, height, stride] = [
                CVPixelBufferGetWidth(buffer),
                CVPixelBufferGetHeight(buffer),
                CVPixelBufferGetBytesPerRow(buffer),
            ];
            let base = CVPixelBufferGetBaseAddress(buffer);
            if !base.is_null() {
                let rows = std::slice::from_raw_parts(base, stride * height);
                pictures.take_bgra(at, [width, height], stride, rows);
            }
            CVPixelBufferUnlockBaseAddress(buffer, 1);
        })?;
        let mut sound = video::Pcm {
            rate: canvas::recording::RATE,
            channels: 1,
            data: Vec::new(),
        };
        // A movie without sound has no sound track to read.
        let _ = read(
            &asset,
            AVMediaTypeAudio,
            &pcm_settings(sound.rate),
            |sample| {
                let buffer = CMSampleBufferGetDataBuffer(sample);
                if buffer.is_null() {
                    return;
                }
                let length = CMBlockBufferGetDataLength(buffer);
                let at = sound.data.len();
                sound.data.resize(at + length, 0);
                if CMBlockBufferCopyDataBytes(buffer, 0, length, sound.data[at..].as_mut_ptr()) != 0
                {
                    sound.data.truncate(at);
                }
            },
        );
        let frames = pictures.finish(duration.micros()?);
        video::write(video::SIZE, &frames, &sound)
    }
}

/// Reads the first track of `kind` in `asset`, decoded as `settings` asks, a sample at a
/// time; none where there is no such track or it cannot be read.
unsafe fn read(
    asset: &AnyObject,
    kind: &NSString,
    settings: &AnyObject,
    mut sample: impl FnMut(*mut c_void),
) -> Option<()> {
    unsafe {
        let tracks: Retained<AnyObject> = msg_send_id![asset, tracksWithMediaType: kind];
        let track: Option<Retained<AnyObject>> = msg_send_id![&tracks, firstObject];
        let track = track?;
        let reader: Option<Retained<AnyObject>> = msg_send_id![
            class("AVAssetReader"),
            assetReaderWithAsset: asset,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ];
        let reader = reader?;
        let output: Retained<AnyObject> = msg_send_id![
            class("AVAssetReaderTrackOutput"),
            assetReaderTrackOutputWithTrack: &*track,
            outputSettings: settings
        ];
        let fits: bool = msg_send![&reader, canAddOutput: &*output];
        if !fits {
            return None;
        }
        let _: () = msg_send![&reader, addOutput: &*output];
        let started: bool = msg_send![&reader, startReading];
        if !started {
            return None;
        }
        loop {
            let next: *mut Sample = msg_send![&output, copyNextSampleBuffer];
            let next = next.cast::<c_void>();
            if next.is_null() {
                break;
            }
            sample(next);
            CFRelease(next);
        }
        // AVAssetReaderStatusCompleted
        let status: isize = msg_send![&reader, status];
        (status == 2).then_some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Snowbound records, compressed, plays back through Core Audio at its length.
    #[test]
    fn core_audio_plays_the_compressed_recordings() {
        let pcm =
            canvas::recording::wave(canvas::recording::RATE, &crate::recording::tests::tone());
        let (bytes, duration) = canvas::recording::compress(&pcm).unwrap();
        let folder = std::env::temp_dir().join(format!("snowbound-player-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("tone.wav");
        std::fs::write(&path, bytes).unwrap();
        let mut player = Player::open(&path, false).unwrap();
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
        assert!(Player::open(&wma, false).is_none());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// A movie as the capture session writes one, H.264 and AAC in QuickTime, becomes the
    /// AVI file OneNote plays: 15 pictures a second at 320 by 240, and its sound as PCM.
    #[test]
    fn a_camera_movie_becomes_a_motion_jpeg_avi() {
        let folder = std::env::temp_dir().join(format!("snowbound-movie-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let movie = folder.join("camera.mov");
        std::fs::write(
            &movie,
            include_bytes!("../../../corpus/recording/video/camera.mov"),
        )
        .unwrap();
        let bytes = avi(&movie).unwrap();
        std::fs::remove_dir_all(&folder).unwrap();
        let parsed = video::Movie::parse(&bytes).unwrap();
        assert_eq!(parsed.frame_us, 1_000_000 / video::FPS);
        // Two seconds at 30 pictures a second, shown at 15.
        assert!(
            parsed.frames.len().abs_diff(30) <= 1,
            "{}",
            parsed.frames.len()
        );
        assert!(parsed.frames.iter().all(|frame| !frame.is_empty()));
        let picture =
            draw::RasterImage::decode(&bytes[parsed.frames[3].clone()], [1000, 1000]).unwrap();
        assert_eq!(picture.size(), video::SIZE);
        let length: usize = parsed
            .sound
            .unwrap()
            .1
            .iter()
            .map(ExactSizeIterator::len)
            .sum();
        let expected = 2 * canvas::recording::RATE as usize * 2;
        assert!(length.abs_diff(expected) < expected / 20, "{length}");
    }
}
