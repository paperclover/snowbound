# Native equation controls

An isolated OneNote 14.0.4763.1000 clone authored a page through the equation UI
(Alt+=): `x^2+y^2=z^2`, `(a+b)/(c+d)`, and inline `\alpha+\beta`.
The UI showed superscripts and a stacked fraction; ordinary text follows the
first equation. `read/page-000.xml` exports three MathML expressions, including
their structure and display/inline distinction. The clone was deleted.

The source contains mathematical Unicode plus FDD0/FDEE/FDEF structural controls.
MathFormatting is set on the equation style. The two structured expressions have
11 and 3 TextRunData property sets respectively. Their private properties remain
opaque, with a checked one-to-one association to the UTF-16 runs in the model.
The report identifies these equations and links to the native reference; it does
not display structural controls as ordinary prose. The inline Greek expression
contains no structural controls and remains readable.

[A fresh-clone read](../native-math-cold-01/read/page-000.xml) exports identical
MathML. `tests/document.rs` verifies the stored text, run associations and math
flags; `tools/test_document_oracle.py` verifies native equation structure and its
preservation across caches. The public document fuzzer includes this source.
