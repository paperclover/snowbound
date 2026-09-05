# Cell shading on OneNote 2010

The COM schema rejects `shadingColor`. This separate Rust-authored storage control
replaces one cell's four-byte modification timestamp property with documented
`CellShadingColor` (`0x14001e26`) and the COLORREF bytes `ff ff 00 00` (yellow).
`input/probe.rs` records the bounded transformation, including object-digest repair;
`input/generation.log` records its execution. The timestamp is deliberately absent
on that cell; other table content and geometry remain unchanged.

A fresh OneNote 14.0.4763.1000 cache opens the file successfully. Its XML omits cell
shading and its PDF renders the cell white, as shown in `shading-2.png`. The Rust
model retains the documented yellow value. This establishes a native 2010
rendering limitation for the tested property, not native rendering parity for
cell shading. The report's color follows the documented stored value.
