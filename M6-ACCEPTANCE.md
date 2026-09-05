# Milestone 6 verification

The deliverable is a read-only document model and reading report. Its acceptance
boundary is the copied personal notebook: automated native comparisons plus a
page-by-page review with no known easily spottable content failures. Native PDF
references preserve access to the original canvas arrangement.

## Personal notebook

[The current report](evidence/m6/private-current-report-05/index.html) includes the
user-approved newer `Video.one`. [Its source manifest](corpus/private/current-source-manifest.json)
identifies that snapshot; [the original frozen manifest](corpus/private/source-manifest.json)
remains separate. The original files are never edited by the library tests.

| Check | Evidence |
| --- | --- |
| 26 current/recycle pages, 109 tags, 188,560 explicit character-format comparisons | [Fresh-cache comparison](evidence/m6/private-current-compare-01.log) |
| 18 historical pages, including all three versions of the updated Video page | [Album](evidence/m6/private-history-final-01.log), [Video](evidence/m6/private-video-current-history-cold-compare-01.log), [deleted page](evidence/m6/private-deleted-history-final-01.log) |
| All 45 report pages reviewed: ordinary, historical, recycle-bin and default template | [Review record](evidence/m6/private-current-report-05/qa/review.json) |
| Frozen copy, approved current copy and live sources match their respective manifests | [Integrity check](evidence/m6/source-integrity-current-01.json) |

The updated current page was inspected through eight browser viewports and two
native PDF pages. Unchanged page bodies inherit their earlier review only after
an exact rendered-content comparison. All exported image payloads decode.
History labels use America/Los_Angeles. Seventeen historical dates match native
UI evidence; copying the sole deleted-page version changes its native displayed
date, so the report retains the source revision timestamp and records that
normalization separately.

## Independent and adversarial checks

| Campaign | Result and evidence |
| --- | --- |
| Native edit histories | [100 histories / 2,000 independently specified operations](evidence/m6/native-histories-independent-03.log); [101 resulting pages compared](evidence/m6/native-histories-final-02.log) |
| Shrinking | [Seed 21](evidence/m6/history-shrink-21/minimal.json) reduced from 20 operations to 3 in 37 fresh-clone replays; [empty-link regression](corpus/m6/native-empty-link-01) |
| Rust scale generation | [128 sections / 2,097,152 Unicode scalars](evidence/m6/rust-scale-final-01.log), including reverse section ordering after native import |
| Document fuzzing | [65,736 runs / 121 seconds](evidence/m6/document-public-fuzz-08.log) after the collapse-field change; prior [100,398-run campaign](evidence/m6/document-public-fuzz-07.log) and [11,423 private-seed runs](evidence/m6/document-private-fuzz-03.log) |
| Rust checks | [All targets](evidence/m6/rust-regression-06.log), [clippy](evidence/m6/clippy-final-04.log) |
| Native/report tooling | [18 tests](evidence/m6/tool-regression-15.log), including deliberate oracle failures and TIFF/native pixel parity |
| Existing writer and fault cases | [Writer](evidence/m6/writer-regression-01.log), [collaboration/FLUSH regression](evidence/m6/collaboration-regression-02.log) |
| Expanded live matrix | [Seven passing cases](corpus/m6/live-collaboration-15/ORACLE.md), [fresh-cache comparison](evidence/m6/live-collaboration-cold-compare-15.log), [native conflict view](corpus/m6/live-collaboration-cold-15/conflict.png) |

The document fuzzer mutates property streams inside native files and repairs
object checksums, reaching document interpretation beyond header rejection.
It traverses referenced historical revisions and resolves text runs. Native
histories have a separate expected-operation model; matching two views derived
from the Rust decoder is not counted as independent verification.

The live matrix covers disjoint edits, competing edits, Samba restart, per-client
transport loss, whole-file lock contention, OneNote process termination and abrupt
VM termination. The process/VM cases recover already server-persisted edits.
They do not establish unsynchronized-cache durability or physical power-loss
safety. Earlier stage-5 cases separately cover lost successful SMB FLUSH replies.

## Interpretation and oracle boundaries

- Native XML omits partial black highlights. Native PDF rectangles supply a
  separate check; the reader retains the stored highlight.
- RTL native XML column order differs from physical left-to-right storage order.
  Independent PDF coordinates establish the conversion. Page-origin translation,
  locked/unlocked column widths and printout borders have isolated controls.
- A saved paragraph collapse default differs from a client's transient expanded
  view. [The native UI control](corpus/m6/native-expanded-control-ui-01/ORACLE.md)
  and [Rust-authored control](corpus/m6/rust-collapse-control-01/ORACLE.md) distinguish them.
- Ink, structured equations, encrypted content and unknown properties retain
  payloads or source identities. Preservation is not decryption or interpretation.
  Structured equation runs expose a native-reference placeholder; ordinary inline
  math text remains readable.
- TIFF browser previews match OneNote's exported pixels while original TIFF bytes
  remain available. Native image conversion is not a license to replace source data.
- [Cell shading](corpus/m6/cell-shading-control-01/ORACLE.md) follows the documented
  stored COLORREF value. A fresh OneNote 2010 cache opens the control but omits
  shading from XML and renders the cell white; native rendering parity for that
  property is not claimed.

Verification uses OneNote 2010 build 14.0.4763.1000 in disposable Windows 7 clones.
The earlier storage/collaboration corpus used build 14.0.7015.1000 on the physical
machine. Each run records its environment and cleanup. These finite campaigns
support this milestone's review boundary, not a claim of universal compatibility
or absence of bugs.

## Reproduce

Use a new output directory for each native capture or report:

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --example document
PYTHONPATH=tools python3 -m unittest discover -s tools -p 'test_*.py'
python3 tools/native_runner.py COPIED_NOTEBOOK NEW_CAPTURE --pdf
python3 tools/verify-document.py COPIED_NOTEBOOK NEW_CAPTURE/read
python3 tools/notebook_report.py COPIED_NOTEBOOK NEW_REPORT \
  --native NEW_CAPTURE/read --timezone America/Los_Angeles
cargo +nightly fuzz run document -- -max_total_time=120 -max_len=512 -rss_limit_mb=2048
python3 tools/native_collaboration.py NEW_CAPTURE --linux OWNED_LAB_NAME
```

Python native comparison/report tests require Pillow and pdfplumber. Historical
report references use repeated `--versions CAPTURE_DIRECTORY` arguments after
`verify-document.py --versions CAPTURE_DIRECTORY/version-ui.json` has checked
source hashes and associated the native copies with stored revisions.
