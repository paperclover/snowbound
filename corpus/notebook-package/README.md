# Notebook packages

OneNote 2010's Save As offers a page and a section as a OneNote 2010 section (`.one`), 2007
section, PDF, XPS or single-file web page, and a notebook as a OneNote Package (`.onepkg`), PDF
or XPS. Observed in a lab clone on 2026-10-01:

- `native/Lab.onepkg` is OneNote's package of a notebook of two sections and a recycle bin: an
  LZX (2^18-byte window) cabinet of the TOC and sections at their paths, the bin left out though
  the TOC keeps its entry; each file written anew, with no `guidAncestor` or `crcName`.
- A section or page saved as a section is a file of its own identity, outside any notebook,
  which OneNote then opens among Open Sections.
- Unpack Notebook (opening a package) asks for the name (the package's), colour (the package's)
  and folder, writes each file with a new identity and version, places it under its folder's
  TOC, and lists it anew beside its stale entry.
- Show Unread Changes, Mark as Read and what is unread live in OneNote's local cache, not in the
  notebook's files or the registry.

`candidate/Packed.onepkg` and `candidate/Unpacked` are what
`a_notebook_packs_and_unpacks_as_onenote_packs_one` in `crates/notebook/tests/package.rs`
exports with `NOTEBOOK_PACKAGE_EXPORT`: Snowbound's MSZIP package of a notebook with a section
group, and the notebook Snowbound unpacked from it. `onenote-unpacked` is OneNote 2010 unpacking
Snowbound's package through its Unpack Notebook dialog; `cold` is a fresh OneNote 2010's read of
Snowbound's unpacked notebook, every file left as written. `save-as/candidate` holds Save As's
copy of a section and section of a page as `save_as_writes_copies_outside_the_notebook` exports
them with `NOTEBOOK_SAVE_AS_EXPORT`; `save-as/cold` is OneNote 2010 opening their folder, which
adopts them (new identities, placed) and reads each page. `tools/test_notebook_package.py`
checks them all without a VM.
