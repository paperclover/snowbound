# Native equation fixtures for the math model

`native-editor/` is OneNote 2010 typing five expressions through its equation
editor (Alt+=) on the Rust-authored link page, driven by
`tools/native_math.py` (`equations.ahk` is the AutoHotkey session,
`equations.png` the desktop afterwards): `a_1+b_2`, `x_i^2`, `(a+b)`,
`\int_0^1 x dx` and `\sum_(i=1)^n i`; `\sqrt(x+1)` did not build up and
left an empty paragraph. `before-read/` is the COM read before typing,
`read/` the read afterwards with each expression exported as MathML, and
`notebook/` the section OneNote saved: the linear text with U+FDD0/FDEE/FDEF
object controls and the text-run data naming each object's kind (29
subscript, 30 sub-superscript, 13 brackets, 21 n-ary with its operator
symbol). `crates/onestore/tests/page_math.rs` renders every expression from
the stored section and compares it with the exported MathML byte for byte;
together with `corpus/m6/native-math-01` (superscripts, a fraction, inline
Greek) that covers the kinds `onestore::page::Math` claims.

`written/candidate` is the page writer's output for
`equations_are_written_and_read_back`: on a fresh section, every native
equation of both fixtures copied as a new paragraph, then two expressions
built from `Math` trees (`x^2+1` and a fraction with a superscript in its
denominator) through `Math::paragraph`. The writer stores each as OneNote
does: the linear text, one run per span with a Cambria Math italic style
carrying the math flags and the math language, the text-run data array
naming each run's object, and the math language marker on the text object.
`written/cold` is the OneNote 2010 read of that section: its exported MathML
equals `expected-mathml.json` (the native fixtures' exports plus the two
built expressions) and `page-000.png` shows the equations rendered.
`tools/test_math_edit.py` checks both without a VM.

Regenerate the editor fixture with `tools/native_math.py
corpus/link-edit/candidate OUTPUT` and the written one with
`ONESTORE_MATH_EXPORT` set while running the test, then
`tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook
--screenshots`.
