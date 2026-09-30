# Deleting between equations

`lab/` holds two COM reads of a disposable OneNote 2010 lab notebook.
`equation-seam.xml` is two equations typed with Alt+=, "aa+bb" and "cc+dd" in
paragraphs of their own, after a selection from "aa+|" into "cc|" was deleted:
OneNote joins them into one equation, `aa++dd`. `equation-per-paragraph.xml` is
Alt+= over a selection across two paragraphs: each paragraph's part becomes an
equation of its own.

`candidate/` is the output of `deleting_between_equations_joins_them` in
`crates/canvas/tests/links_equations.rs` on the page of `corpus/link-edit/candidate`:
the same equations typed and the same selection deleted through the editor, whose
joined equation reads back as one. `cold/` is a fresh OneNote 2010 read of it, whose
MathML is OneNote's own. `tools/test_equation_join.py` checks this without a VM.
Regenerate with `CANVAS_EQUATION_JOIN_EXPORT` set to a new absolute directory while
running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook --screenshots`.
