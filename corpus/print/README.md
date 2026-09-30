# Print

`native/Print.one` is a section the COM API made in OneNote 2010: "Printing test", a
ruled page (horizontal lines every 0.2 points, as OneNote reads the XML's spacing, and a
margin line) with a 70-paragraph outline running past the first sheet, a picture, an
outline near the first sheet's foot and one reaching 867 points across; "Second page";
and "Wide 1200" and "Wide 560", titled pages whose one word sits that far across.
`native/section.pdf` is OneNote's File, Save As, PDF of the section, the same sheets File,
Print prints. `native/rules-section.pdf` is its PDF of `page-background/candidate`.

What they show, on a 612.36 by 790.92 point Letter sheet:

- The page prints between 36-point top and bottom margins, the margin origin at (72, 36).
  Content reaching past the paper's right edge shrinks the whole page to fit it with 9
  points to spare: "Printing test" prints at 0.672, "Wide 560" at 0.978, "Wide 1200" at
  0.484.
- A sheet ends before the first line it would cut: the first holds paragraphs 1 to 37 and
  the second starts at paragraph 38. Content below that on the first sheet waits for the
  next.
- Rule lines print across the paper's whole width, down to the bottom margin of every
  sheet. The page colour does not print. Template art does, and a page longer than a sheet
  only through its art ("VeryLargeGrid") takes a second sheet.
- The footer, "Print Page 1", is the section's name and the sheet's number counted across
  the whole printout, in 10-point Times New Roman centred near the paper's bottom edge; it
  shrinks with the page towards the bottom left. No header prints; the title, date and
  time are the page's own.

Print Preview and Settings offers Print range (Current Page, Page Group, Current Section),
Paper size, Scale content to paper width (on), Orientation, Footer (section and page
number, page number, section, none) and Start page numbering at 1. Save As, PDF offers
Selected Pages, Current Section and Current Notebook.

`crates/canvas/src/print.rs` checks Snowbound's sheets against these without a VM.
