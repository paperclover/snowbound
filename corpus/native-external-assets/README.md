# External payload fixture

OneNote 2010 authored an empty attachment and a 1 KiB attachment with
`tools/native/external-assets.ps1`. The retained native section is in `native/`.
The fixture generator converts all file-data declarations into the documented
external representation while retaining their identities and exact payloads.
The `read/` capture is an independent fresh-cache OneNote reopen; `payloads.json`
contains the native attachment hashes. `provenance.json` records source hashes.

```sh
cargo run -p onestore-notebook --example externalize_fixture -- corpus/native-external-assets/native/synthetic.one /tmp/external-notebook
python3 tools/native_runner.py /tmp/external-notebook /tmp/external-native --expected-pages 3 --collect-notebook
```

The generator creates a new destination and requires reserved fragment space.
It is a fixture converter, not a concurrent publication API.
