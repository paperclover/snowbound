# Native page-origin control

The input starts from `corpus/native-ink/cold-ui-ink/notebook`. Rust scalar edits
change only PageMarginOriginX/Y to 3 and -1 half-inches (108 and -36 points).
`input/operations.json` records the input hash, object identities, and old/new
property bytes. A fresh OneNote 2010 clone opened that input; `read` contains its
independent XML/PDF view and `teardown.json` records cleanup.

For non-ink page children, native XML positions equal stored positions plus
(36, 14.4) minus the stored origin. `origin-observations.json` records all observed
positions. Ink XML exports stroke bounds, which remain opaque in the model.
The fixture also contains a 1 by 1 PNG whose native XML omits Size; its stored
intrinsic size is 0.75 by 0.75 points and it has no explicit layout dimensions.

Run `tools/verify-document.py input/notebook read` from this fixture directory
using the repository tool path to check every supported object and coordinate.
