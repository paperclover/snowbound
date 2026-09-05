# Native hyperlink boundary controls

`notebook/fixture.json` is the independent authored input; `inputs/page-000.xml`
records the submitted COM page. `read/page-001.xml` and its PDF are the native
output. `read/environment.json` identifies the OneNote build and fresh clone;
`teardown.json` records deletion. The original capture location links here.

Ten labels isolate ASCII spaces, tabs, NBSP, EM space, literal HTML metacharacters,
and line breaks. Native import places leading ASCII spaces/tabs outside the link,
keeps trailing spaces/tabs inside it, and keeps NBSP linked at both ends. An
all-space label has no link. EM spaces are removed during import. Newlines become
stored CRs outside the hyperlink; the final CR is absent from exported HTML.

Authored red foreground and underline do not survive this hyperlink import:
native storage and exported XML use automatic foreground and no underline,
including the leading unlinked text. Bold, 14-point font, and yellow highlight
remain. These are native import observations, not transformations in the reader.

`native_hyperlink_boundaries_preserve_whitespace_and_literal_labels` checks the
stored visible text, linked text, exact destinations, and native formatting. The
seeded-history comparator must apply the evidenced leading ASCII-space/tab rule
to its independent expectation without stripping Unicode whitespace generally.
