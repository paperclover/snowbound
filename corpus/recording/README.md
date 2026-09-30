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

`tools/test_recording.py` checks the rows without a VM.
