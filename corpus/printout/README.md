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
The undone page is written back as it was read: its XPS package as the PictureContainer,
under the file-data type OneNote gives XPS packages (0x8003A), the PNG beside it as the
WebPictureContainer14, IsPrintout, its page number, and the undocumented properties tying
it to its package, kept as stored; without that type or those properties OneNote reads
the page back but draws nothing there. `cold/` is a fresh OneNote 2010 read (with its PDF
export): both pages are printouts and both draw, the moved one lower down; the restored
page's package is a copy, which OneNote counts as a second (`xpsFileIndex="1"`). `tools/test_printout.py` checks this without a VM.
Regenerate with `SNOWBOUND_PRINTOUT_EXPORT` set to a new absolute directory while
running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 2 --collect-notebook --screenshots`.
