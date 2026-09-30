# Pressure ink

OneNote 2010 draws a pen's pressure but stores only what the pen reports. With a mouse, or
with File > Options > Advanced > Pen > "Use pen pressure sensitivity" off (the default,
`HKCU\Software\Microsoft\Office\14.0\OneNote\Options\Pen\InkPressure`), a pen's drawing
attributes carry IgnorePressure (`0x08003411`) and its dimension table holds X and Y. With the
option on, the attributes leave IgnorePressure out, and a pen that reports pressure adds it.
The lab's Windows 7 has no digitizer, so the pressure strokes here come from ISF: WPF builds
strokes with NormalPressure (and one with X and Y tilt) and OneNote takes each through
`UpdatePageContent` (`tools/native/ink-pressure.ps1`).

- `native` is that run: page "Levels" holds two drawings of nine 24 pt strokes at pressures 0
  to 1 in eighths, one pen reporting 0..1023 and the other 0..255; "Strokes" a ramp from no to
  full pressure, a wave, a pen ignoring the pressure it recorded, a stroke with tilt and a
  4 by 24 rectangular tip.

What it shows, and `crates/onestore/tests/page_ink.rs` pins:

- The dimension table (`0x1c00340a`) keeps every packet property the pen reported, each a
  GUID, lower and upper limit, unit and resolution: X and Y, NormalPressure
  (`7307502d-f9f4-4e18-b3f2-2ce1b1a3610c`), tilt and the rest. The stroke's path holds one
  block of multi-byte first differences per dimension, in the table's order.
- OneNote draws each point as a stamp of the pen's tip `0.25 + 1.5 * p` times its width and
  height, where `p` is the pressure over the pen's range, `(value - lower) / (upper -
  lower)`: a quarter of the pen at none, all of it at half, 1.75 times at full. Consecutive
  stamps are joined by the quadrilateral across their ends. Tilt changes nothing drawn, and a
  pen with IgnorePressure draws at its width whatever it recorded.

Snowbound reads pressure the same way and draws it as OneNote does. A stroke drawn with a pen
that reports pressure (a tablet on macOS, the Apple Pencil) stores X, Y and NormalPressure
from 0 to 1023 without IgnorePressure, as OneNote does with the option on; a mouse's or a
finger's stroke stays as OneNote's mouse ink.

`candidate` is `pressure_ink_is_written_as_onenote_keeps_it_and_survives_edits` in
`page_ink.rs` (`ONESTORE_INK_PRESSURE_EXPORT`): the native file with the first drawing's
no-pressure stroke erased, four Snowbound drawings of a 500 HIMETRIC pen below it (none, half
and full pressure, and a ramp), and on "Strokes" the ramp moved to (72, 108) and the wave
deleted. `cold` is a fresh OneNote 2010 read: every width as drawn above, in its PDF export
and `read/page-*.png`.

`tools/test_ink_pressure.py` checks both from the PDF exports without a VM. Regenerate with
`tools/native_runner.py EMPTY_DIRECTORY OUTPUT --author tools/native/ink-pressure.ps1
--collect-notebook --screenshots --pdf`, and cold-open a candidate with
`tools/native_runner.py CANDIDATE COLD --expected-pages 2 --collect-notebook --screenshots
--pdf`.
