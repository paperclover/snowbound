# Edits reaching into equation objects

`lab/` holds COM reads of a disposable OneNote 2010 clone editing a copy of
`corpus/math-edit/native-editor-4` by hand. `before.xml` is the page as it opened and
`after.xml` as it ended; the rest are reads between steps.

- A mouse selection from inside `√x+1`'s root into `∛x`'s shows both objects selected
  whole, and Backspace removes both paragraphs (`roots-deleted.xml`).
- From after `a₁` in `a₁+b₂` into the body of a sum ending its paragraph: the sum is taken
  whole with the paragraph's end, so Backspace joins `a₁` to the integral below.
- From a fraction's numerator in `One a/b+c two` into the root of `Three √x+y four`:
  both objects whole, leaving `One +y four`.
- Enter between `→` and `∞` in `lim_(n→∞) a_n` makes the limit an equation array
  (`limit-enter.xml`).
- A click in "Type equation here." selects the placeholder whole; Enter leaves its
  paragraph empty above a new one (`placeholder-enter.xml`), and typing `x` replaces it.
- A drag past a line's end also takes the paragraph's end: "Alpha beta", "Gamma delta"
  selected from `Alp` to past `delta` delete to `Alpepsilon`.

`candidate/` is the output of `selections_into_equation_objects_take_them_whole` in
`crates/canvas/tests/links_equations.rs`: the same edits through the editor on the same
page, one revision each. `cold/` is a fresh OneNote 2010 read of it, whose MathML matches
the lab's equation for equation. `tools/test_equation_select.py` checks this without a VM.
Regenerate with `CANVAS_EQUATION_SELECT_EXPORT` set to a new absolute directory while
running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook --screenshots`.
