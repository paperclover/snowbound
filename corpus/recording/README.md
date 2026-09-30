# Recording audio

## What OneNote 2010 does

Observed in a lab clone on 2026-09-29, with a QEMU HD Audio device on a null
audio backend (it records silence; the clone has no camera, so Record Video
was not observed):

- **Record Audio** (Insert, Recording) with the caret at the end of "Before
  recording": after that paragraph OneNote adds the recording's file, an empty
  paragraph, "Audio recording started: 5:18 PM Tuesday, September 29, 2026" in
  the `cite` quick style (Calibri 9, #595959) and an empty paragraph. The
  contextual Audio & Video tab shows Recording: Record Audio, Record Video,
  Play, Pause, Stop, Rewind 10 Minutes, Rewind 10 Seconds, the position
  ("00:08/00:08"), Fast Forward 10 Seconds, Fast Forward 10 Minutes, See
  Playback and Audio & Video Settings (`native/read/onenote-recording-tab.png`).
- **The file** is named after the page ("Recorded.wma", shown without its
  extension) and is Windows Media Audio 9 Voice, 8 kHz mono: 24,827 bytes for
  34 seconds. It is stored in the section's own file data store beside a
  32-pixel icon, never in `_onefiles`. The attachment carries
  AudioRecordingGuid (`0x1c001c97`), IRecordMedia 1 (`0x14001d24`) and
  AudioRecordingDuration in milliseconds (`0x14001cfd`, 33,903); the page lists
  the recording in AudioRecordingGuids (`0x1c001ca3`, COM's `MediaPlaylist`).
- **Linked notes.** Every paragraph whose text changes while recording is
  linked to that moment: the recording's GUID (`0x1c001c98`) and milliseconds
  since the start (`0x14001c99`, COM's `MediaIndex`) on its text object. A
  paragraph edited during recording is linked too ("Before recording edited",
  20,131 ms); empty paragraphs are not. The line saying when and the file are
  linked at 0 and 151 ms.
- **Playback.** Hovering a linked paragraph, or selecting the recording, shows
  a blue play button in the margin (`native/read/onenote-linked-notes.png`); a
  click plays from the linked moment.
- **Deleting** the recording removes AudioRecordingGuids from the page; the
  notes keep their links (`native/deleted`).
- **Pause** while recording (the Pause button again resumes) leaves the pause
  out of the clock; a note written while paused is not linked
  (`native/read/paused-while-recording.xml`).
- **See Playback** (on by default) highlights, as a selection, the note
  linked last at or before the moment playing; the line saying when, linked at
  0, is not highlighted (`native/read/onenote-see-playback.png`). When the
  note playing is off screen OneNote scrolls just far enough to show it
  (`native/read/onenote-see-playback-scrolls.png`: 60 lines between two
  notes, played from the top of the page). A linked
  note plays from five seconds before its moment, the default of Options,
  Audio & Video (`native/read/onenote-audio-video-options.png`), which also
  shows the default video profile, Windows Media Video 8 for Local Area Network
  (256 Kbps). Control-Alt-P plays and Control-Alt-S stops; Control-Alt-A does
  not record.
- **Record Video** without a camera says "No camera available. Make sure the
  camera is properly installed and connected." (`native/read/onenote-no-camera.png`).
  Installing a virtual camera in the clone was refused, so OneNote's own video
  recording was not observed.
- **0x1c001cc8** is a run list of {u32 character position, u8 state} on text
  objects, OneNote's proofing state: its page date and time and the line saying
  when recording started always carry `00 00 00 00 03` (three recordings, all
  pages of `native/`), typed text none or 0, 1 or 9 (a misspelling), equations
  5. OneNote writes and rereads it itself and reads text without it, so
  Snowbound writes it only on new pages' dates, as before.
- **Attach File** of a .wav makes it a recording as well: identity,
  IRecordMedia 1 and its length (1,000 ms), listed on the page
  (`native/attached`). MS-ONE requires this of .wma, .mp3, .wav, .wmv, .avi and
  .mpg files.

## Rows

`native/` is OneNote's recording (`notebook/`), its COM export
(`read/page-recorded.xml`), and the section after OneNote deleted the
recording (`deleted/`) or attached a .wav (`attached/`).

`candidate/` is `a_recording_stores_its_file_line_and_linked_notes_for_onenote`
in `crates/snowbound/src/recording.rs`: the editor records at the end of
"Before recording", two notes are typed while it records, and Stop puts in
two seconds of a tone as Snowbound stores recordings, 16 kHz IMA ADPCM WAV.
`cold/` is a fresh OneNote 2010 read: the playlist, the file with its bytes,
the line and both notes linked to their moments. Opened in a lab clone, the
note's play button played the recording from there (`played-from-note.png`);
`native/read/onenote-plays-ima-adpcm.png` is OneNote playing a 20-second one.
Regenerate with `SNOWBOUND_RECORDING_EXPORT`, then `tools/native_runner.py DIR
COLD --expected-pages 1 --collect-notebook --screenshots`.

`edit/` is `onenote_s_recording_round_trips_through_deleting_restoring_and_linking`
in `crates/onestore/tests/page_media.rs`: OneNote's recording deleted, put back
as undo does, and a paragraph linked at 25 s, each as its own edit. `edit/cold`
is a fresh OneNote 2010 read: the same playlist, links, WMA bytes and the new
link. Regenerate with `ONESTORE_RECORDING_EDIT_EXPORT`, then
`tools/native_runner.py DIR COLD --expected-pages 2 --collect-notebook
--screenshots`.

`video/` is Record Video. `native/read/attached-video.xml` is OneNote
attaching one file of each kind: .avi, .mpg and .wmv become recordings (kind 2,
with their length), .mp4 and .mov plain files. OneNote played Motion JPEG AVI
with PCM or IMA ADPCM sound, MPEG-1 and WMV 8 with pictures
(`onenote-plays-attached-*.png`), but H.264 in AVI without them. `video/candidate`
is `a_video_recording_stores_its_file_line_and_linked_notes_for_onenote` in
`crates/snowbound/src/recording.rs`: two seconds of Motion JPEG at 320 by 240,
15 a second, with 16 kHz PCM, as Snowbound records video. `video/cold` is a
fresh OneNote 2010 read; OneNote played it from a linked note with See Playback
highlighting it (`video/played-from-note.png`), and played what the macOS
conversion makes of `video/camera.mov` (H.264 and AAC, as a capture session
records) and what GStreamer recorded in a Debian 13 VM from its test sources
(`video/onenote-plays-*.png`). Regenerate with `SNOWBOUND_VIDEO_EXPORT`.

`tools/test_recording.py` checks the rows without a VM.
