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

`native-editor-2/` is a second session (`ONESTORE_MATH_EQUATIONS` on
`tools/native_math.py`): `\sqrt x+1`, `\cbrt(x)`, `\sqrt(n&x)`, `a/b`,
`\lim_(x\to 0) f(x)` (the `\lim` stayed literal), `\prod_(k=1)^n k`,
`[a+b]`, `\overline(x)` and `x\hat`; `\sqrt (x+1)` again left an empty
paragraph. Its MathML adds radicals (25: `msqrt`, or `mroot` with the
degree second), a lower limit (19: `munder`, with the upright `lim` as one
`mi` in a row), a product, named fences (`mfenced open="[" close="]"`), an
overbar (23: `mover accent="false"` with a stretchy bar) and an accent (10:
`mover accent="true"` with the combining circumflex exported as `^`).

`native-editor-3/` is a third session: `\matrix(1&2@3&4)` (exported inline,
its trailing `(\matrix(a&b@c&d))` staying literal text), `\eqarray(x&=1@y&=2)`,
`x\above 2`, `x\below 2`, `\box(x)`, `\rect(x)`, `\underline(x)` (a literal
`▱` before fences), `\iint x dx dy` (a bare operator), `f(x)/(x^2+1)` and
`\sum^n x`. Its MathML adds matrices (20: `mtable` of cells row by row, the
column count on the opening run's `0x0c003451`), equation arrays (15: rows
with `maligngroup` and a `malignmark` for each `&`), an upper limit (33:
`mover`), boxes (11: `mpadded`; 12: `menclose notation="box"`) and an n-ary
operator with one empty limit (`mover`, or `msup` for integrals).

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

`native-editor-4/` is a fourth `tools/native_math.py` session repeating most
of the forms above and adding `e^(x+1)`, `\alpha+\beta` (which stays linear
text: one run between objects, Greek in mathematical italic),
`(a+b)/(c+d)`, `x^2+y^2=z^2`, `lim_(n\to\infty) a_n` (a function-apply
object, 17 with U+2061, around a limit; not built by the linear reading
here) and `{a+b}`; `x_i^2` kept the editor's “Type equation here.”
placeholder (11 with `⬚`), as the editor was still opening.

`native-linear/` is the linear form OneNote 2010 shows and stores after
Equation Tools, Design, Linear, applied to each equation of a session like the
fourth one plus `x^2+1` and `(a+b)/(c+d)`. It was authored interactively in a
lab clone through its desktop agent (clicking each equation, then Linear), as
the key tips and the equation's context menu would not switch it from a
script; `notebook/links.one` is the section OneNote saved on closing. Each
equation becomes one run of math between objects: letters in mathematical
italic, arguments of more than one factor in parentheses, an n-ary body after
`▒` inside `〖〗`, `√(n&x)` for a root's degree and `∛` for a cube root,
`■(1&2@3&4)` and `█(x&=1@y&=2)` for arrays, `𝑥┴2`, `𝑥┬2`, `□𝑥`, `▭𝑥`, `¯𝑥`,
and an accent after a no-break space.

`crates/onestore/tests/page_math.rs` builds every typed expression with
`Math::from_linear` and compares it with what the editor stored, run by run,
writes each tree back as the linear text OneNote showed, and rebuilds every
native equation from its linear form; `crates/canvas/src/editor/equation.rs`
types the same keys into the canvas editor, building up at each space.

`native-enter/` is OneNote 2010 pressing Enter around equations and a link,
driven by `tools/native_enter.py`: Enter at an equation's start moves the
equation to a paragraph of its own; inside the equation's own row it stores a
line break (`\r`, a plain run) in the paragraph, the equation's two sides
becoming two equations (two MathML blocks in the read); inside a fraction's
denominator it makes that argument an equation array (15, `█`) of the two
parts; at a link's start the whole link, field code and label, moves down. An
interactive session showed that Enter inside a link opens it instead.
`crates/canvas/tests/links_equations.rs` presses the same keys in the editor and
compares every paragraph's runs, and `page_math.rs` compares each equation's
MathML with the read.
