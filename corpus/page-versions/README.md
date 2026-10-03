# Page versions

What OneNote 2010 stores for a page's earlier versions (Share, Page Versions), what its
Restore Version, Delete Version, Copy Page To and Delete All Versions write, and that it
reads and deletes the versions Rust writes. `tools/test_page_versions.py` checks the rows
without a VM; `crates/onestore/tests/versions.rs` compares Rust's writes with OneNote's.

## Storage

A version is an earlier revision of the page's own object space, kept current under a
context of its own (MS-ONE 2.1.17, 2.1.18); nothing is copied. The page's *version history*
is another revision of the same space, current under context
`{7111497F-1B6B-4209-9491-C98B04CF4C5A},1`: root `jcidVersionHistoryContent` (`0x6003c`)
whose `ElementChildNodesOfVersionHistory` (`0x24001c20`) lists a `jcidVersionProxy`
(`0x6003d`) per version, newest first. A proxy holds `VersionContextNodes` (`0x3400347b`,
the version's context), `LastModifiedTimeStamp` and `AuthorMostRecent` of the version (from
its revision metadata, root role 4) and a `CreationTimeStamp` of when the version was made.
The page manifest names the history context in `0x3400347b` from the page's creation, and
the page's metadata and the section's copy of it carry `HasVersionPages` (`0x88003462`)
while any version is listed. A version's context is its revision identity XOR
`9D52FE7C-8FBC-4495-9300-129B5797A9EA`, its proxy's identity the context XOR
`88440BCA-118C-0E81-32BF-83DB8CB8B6B5`.

- **Making one.** When a different author edits the page, OneNote labels the page's last
  revision as a version (a `0x5d` RevisionRoleAndContextDeclaration in the space's list)
  and appends a history revision listing its proxy. One author's edits make none.
- **Restore Version** (`step-03` to `step-04`): the page's next revision depends on the
  version's revision (the page's chain forks there), its revision metadata naming who
  restored it; the page as it stood is labelled as the newest version and the history lists
  it first. The version restored stays listed.
- **Delete Version** (`step-04` to `step-05`): only the history changes, dropping the proxy.
  The version's label and revisions stay in the file until Optimize drops them. Deleting
  the last one (`step-07` to `step-08`) also clears `HasVersionPages`.
- **Delete All Versions in Section** (`step-05` to `step-06`) asks first ("Do you want to
  delete all page versions in the section "History"?") and writes the same as deleting each.
- **Copy Page To** (`step-02` to `step-03`) copies the version as a new page without
  versions; the copy's manifest still names the history context, which no revision
  carries. Readers must accept that.
- **Optimize** (`step-10`, two years on) rewrites each kept version and the history as
  checkpoints; versions are not expired by age here.

## Interface

A version is listed under its page as "M/D/YYYY author" (`shots/06-shown-again.png`) once
the page's Show Page Versions (page menu, `shots/22-page-menu.png`) or Share, Page
Versions is chosen. An open version is read-only under a yellow bar (RGB 255, 238, 194):
"This is an earlier version of the page. It will be deleted over time. Click here to restore
or delete this version." What the version changed since the one before it is banded green
(RGB 214, 255, 214). The bar's menu (`shots/02-version-menu.png`): Restore Version, Delete
Version, Copy Page To; Delete All Versions in Section, Section Group, Notebook; Disable
History for This Notebook; Hide Page Versions. Share also has Hide Authors, Recent Edits and
Find by Author (`shots/14`–`16`).

## Rows

- `native`: one OneNote client driven by hand, authors switched through the Office user
  name. `step-NN/notebook` is the section after each step above, `shots/` the screens.
- `native-scripted`: the same steps, one to nine, by `tools/native_versions.py observe`.
- `candidate-restore`, `candidate-delete-all`: `restoring_a_version_stores_what_onenote_stores`
  (Restore Version of `step-03`'s version) and `deleting_versions_stores_what_onenote_stores`
  (Delete Version of `step-05`'s last one), `ONESTORE_VERSIONS_EXPORT`.
- `cold-restore`: OneNote's fresh read of `candidate-restore` (`native_versions.py cold
  --delete`): both versions listed, the newest one Rust labelled opened read-only under the
  bar and deleted through its menu (`notebook` is what OneNote left). `cold-delete-all`: its
  fresh read of `candidate-delete-all`, the page without versions.
