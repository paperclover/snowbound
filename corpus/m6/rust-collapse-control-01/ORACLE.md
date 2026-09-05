# Saved paragraph collapse default

The Rust scalar writer changed only property `0x0c001c11` on the paragraph
containing `Collapsed parent` from byte `01` to `00`, starting from
`../native-features-01/notebook`. The mutated input is `input/notebook`.
A fresh OneNote 14.0.4763.1000 clone opened it without further editing.

The original native fixture reports `collapsed="1"` and hides both descendants
in its PDF. This control omits `collapsed` from XML and shows both descendants
in its PDF. Both versions retain the same child/grandchild structure.
`tests/document.rs` and `tools/test_document_oracle.py` assert these facts.
The model exposes the byte as `collapse_state`: 0 means expanded and 1 collapsed;
other values remain uninterpreted. The report folds state 1 while retaining
expandable descendant content.

The separate `../native-expanded-control-ui-01` demonstrates that a client can
expand its transient view without changing the notebook or its PDF output.
