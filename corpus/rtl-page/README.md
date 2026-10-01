# Editing a right-to-left page

OneNote 2010 keeps a right-to-left page's positions in the same left-to-right frame
as any page, from a margin origin that `corpus/m6/native-page-direction-03` stores at
x 10763.25: native XML x is stored x minus the origin minus 36. It opens the page
scrolled to the right end of its content and keeps the view's right edge when the
window resizes. A click on blank page puts an outline's left edge on the grid
anchored at the margin origin, as on any page; only an outline laid out wider than
its stored width keeps its right edge. A new drawing stores page-frame strokes with
a zero offset (Win7 lab, 2026-10-01).

`candidate/` is the output of `crates/canvas/tests/rtl_page.rs` on that page through
the page view: it opens at the drawings' right edge, a click and typing add an outline,
a pen stroke adds a drawing, and a header drag moves the positioned outline two grid
columns right, each published as a revision. `cold/` is a fresh OneNote 2010 read of
it: the new outline at x 126, the moved one at 180, the stroke where the pen went.
`tools/test_rtl_page.py` checks this without a VM. Regenerate with
`SNOWBOUND_RTL_EXPORT` set to a new absolute directory while running the test, then
cold-open it with `tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook --screenshots`.
