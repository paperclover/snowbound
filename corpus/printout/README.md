# File printouts

`native/` is a notebook OneNote 2010 made in the lab (Insert, File Printout of a
two-page Word document, and a shape pasted from Word), cold-read with
`tools/native_runner.py`. Each printout page is a picture whose PictureContainer is the
printout's XPS package, shared by its pages, with IsPrintout, the page shown
(DisplayedPageNumber) and OneNote's PNG rendering of that page as its
WebPictureContainer14 (MS-ONE 2.3.98), which Snowbound draws, framed in grey as OneNote
frames printout pages.

`candidate/` is the output of `printouts_show_onenotes_rendering_and_move_as_pictures`
in `crates/canvas/tests/printouts.rs`: one printout page moved down, the other deleted
and the deletion undone, and text typed before the citation, published as one revision.
Snowbound does not write XPS packages, so the undone page returns as the PNG OneNote
showed of it. `cold/` is a fresh OneNote 2010 read: the moved page is still a printout,
the restored one a picture. `tools/test_printout.py` checks this without a VM.
Regenerate with `SNOWBOUND_PRINTOUT_EXPORT` set to a new absolute directory while
running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 2 --collect-notebook --screenshots`.
