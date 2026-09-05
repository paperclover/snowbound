# Native paragraph expansion control

The input is `../native-features-01/notebook`. A fresh OneNote 14.0.4763.1000
clone opened it, and the paragraph handle was double-clicked as recorded in
`ui-operation.json`. `expanded.png` shows both descendants visible.

The selected paragraph reports `collapsed="1"` in `before-read` XML and omits
that attribute in `read` XML. Both captures retain the child and grandchild.
The copied `Features.one` is byte-identical to the input. Native PDF publication
still hides the descendants in both captures, despite the expanded desktop view.
Thus transient native view state and the PDF rendering are distinct oracles.
`tools/test_document_oracle.py` locks in these observations.

A preceding COM experiment accepted `collapsed="false"` in UpdatePageContent
without changing the returned collapse state. Its failure evidence remains in
`evidence/m6/native-expanded-control-03`; it is not an expansion control.
