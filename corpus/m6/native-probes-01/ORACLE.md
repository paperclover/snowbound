# Partial black highlighting is missing from native XML

OneNote 14.0.4763.1000 displays black highlighting on the word `middle` in
`Before middle after`. The application stores `Highlight = 0x00000000` on that
text run. `GetPageContent(piAll, xs2010)` omits the highlight from the returned
HTML. Whole-paragraph black highlighting is returned correctly in the same page.

The authored input is [inputs/format-probes.xml](inputs/format-probes.xml).
The stored file is [notebook/synthetic.one](notebook/synthetic.one); the native
XML and screenshot are [read/page-001.xml](read/page-001.xml) and
[read/page-001.png](read/page-001.png). The screenshot shows the black rectangle
between `Before` and `after`. Text, run boundaries, and other formatting agree.

This is an XML-oracle limitation. A reader must retain and render the black
highlight. It must not remove the highlight to match the XML. Four partial-black
runs in the copied personal notebook exhibit the same XML omission, including
on a fresh cache. Their visual checks belong to the personal report acceptance.

Reproduce with the current scripts:

```sh
python3 tools/native_runner.py corpus/writer/create-notebook-01/notebook NEW_OUTPUT \
  --author tools/native/features.ps1 --expected-pages 2 --screenshots
```

This capture's exact author/read/cache scripts are retained in `scripts/` and
hashed in `run.json`. The clone's successful deletion is in `teardown.json`.
