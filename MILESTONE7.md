# Milestone 7: concurrent editing

Acceptance gates passed for the tested macOS Rust, OneNote 2010 and Linux Samba
configuration. The edited notebook has fresh native references and an HTML
review; independent operation histories verify overlapping clients, retained
conflicts and the final notebook after native application closure.

## Gates

1. Document edits: publish text and dependent run boundaries atomically; check
   Unicode boundaries, preserve untouched objects and opaque properties, reject
   unsupported edits before writing. Validate native round trips before racing.
2. Random-edit CLI: select a page and seed, record the exact intended operation,
   source identity and commit outcome. Operate on disposable notebook copies.
3. Local concurrency: independent reader/writer processes, synchronized starts,
   stale snapshots, lock contention, randomized delays and interrupted commits.
   Verify every successful observation and explain every accepted edit.
4. Native concurrency: grow from three native clients plus multiple Rust
   processes to larger measured workloads. Mix disjoint and same-paragraph
   changes, ordinary reads, synchronization, disconnects and restarts. Prove
   operation overlap from recorded intervals; do not count open idle clients.
5. Adversarial replay: retain seeds, operation logs and snapshots; shrink failures
   and add regressions. Compare the independent operation model with current
   native content and explicitly reachable conflicts, then cold-reopen results.
6. Review and cleanup: inspect the edited report and native references, rerun
   prior gates, verify original source hashes, delete owned machines, and record
   observed client counts, operations, faults and the limits of the evidence.

Disjoint acknowledged changes must survive. Competing changes must be accounted
for by their observed ordering or accessible conflict content; historical bytes
alone do not prove recovery. Unknown commit outcomes require rereading before a
retry. Native COM page snapshots must not replay untouched stale containers.

Crash tests distinguish application cache state, server-persisted state and
storage durability. No finite campaign proves all schedules or hardware
power-loss behavior.

## Replay

Use fresh output directories and Linux VM names; the native harness deletes its
owned machines on exit. The Linux lab address allows one server run at a time.

```sh
python3 tools/native_collaboration.py evidence/m7/new-stress --linux new-stress --stress-clients 3 --stress-operations 150 --sync-every 0 --rust-writers 4 --rust-readers 3 --edit --seed 913
python3 tools/native_runner.py evidence/m7/new-stress/stress-closed/notebook evidence/m7/new-cold --expected-pages 1
python3 tools/verify-document.py evidence/m7/new-stress/stress-closed/notebook evidence/m7/new-cold/read
python3 tools/native_stress.py evidence/m7/new-stress evidence/m7/new-cold/read
python3 tools/native_collaboration.py evidence/m7/new-conflict --linux new-conflict --conflict-clients 3 --rust-writers 3 --rust-readers 2 --stress-operations 5 --seed 917
cargo +nightly fuzz run edit_text -- -max_total_time=300 -max_len=128 -rss_limit_mb=2048
```

## Evidence

- `evidence/m7/text-edit-native-02`: a length-changing Unicode edit reopened in
  a fresh OneNote VM; 297 explicit formatting comparisons passed on one page.
- `evidence/m7/local-concurrency-02`: ten Rust writers and six readers;
  600 acknowledged commits and 8,101 checked observations. The final notebook
  reopened in a fresh native cache (`local-concurrency-native-02`).
- `evidence/m7/edit-text-multi-fuzz-03.log`: 31,394 stateful executions with
  six independently stale writer snapshots and interrupted publication. This
  simulates interleavings; native concurrency is a separate gate.
- `evidence/m7/native-concurrency-01` and `native-concurrency-02`: three native
  clients, four Rust writers and three Rust readers exposed a macOS SMB lock
  failure. Native COM acknowledgments were not durable synchronization: the
  stalled server copy did not contain those cached native edits. These runs
  failed acceptance and all of their machines were deleted.
- `evidence/m7/lock-race-default`: a notebook-free, four-process reproducer
  detected overlapping exclusive holders using an independent local marker,
  plus stranded locks when switching read-only and read/write opens. Explicit
  unlock alone did not fix it. Uniform access modes still violated exclusion.
- `evidence/m7/lock-race-atomic`: atomic open-and-lock passed all three access
  modes without overlap. The macOS file adapter now uses that operation.
- `evidence/m7/smb-rust-only-04`: the formerly stalled workload completed all
  120 commits, 940 verified reads and 203 overlapping writer calls after that
  change. Broader acceptance remains active; this does not establish a native
  multi-client pass.

- `evidence/m7/lock-race-atomic-16`: sixteen processes, 4,516 acquired locks
  across read-only, read/write and alternating access; no marker overlap.
- `evidence/m7/smb-rust-only-05`: ten writers and six readers completed 1,000
  commits and 14,913 verified observations. The saved result subsequently
  failed fresh-cache native acceptance (`smb-concurrency-native-05`): OneNote
  returned zero pages after five minutes. The dependency-checkpoint fix and subsequent native checks below explain
  and resolve this failure; Rust parsing alone was insufficient acceptance.
- `evidence/m7/native-concurrency-03`: all 90 native edits and 120 Rust edits
  converged, but the overlap gate correctly rejected the run. Native calls
  began 4.5–5.4 seconds after the first Rust commit; Rust finished at 2.7
  seconds. The shared-file start marker was delayed. The harness now waits
  for each native client's first-edit acknowledgment before releasing Rust.
- `evidence/m7/local-stateful-01`: eight writers and five readers, 800 random
  length-changing Unicode replacements, 9,820 verified observations. The
  independent oracle applies recorded UTF-16 edit intents to each committed
  state and checks all reader observations against the resulting history.
- `evidence/m7/edit-text-multi-fuzz-04.log`: 26,377 renewed stateful fuzz
  executions completed without a failure.
- `evidence/m7/native-concurrency-04`: all 90 native edits and 120 Rust edits
  converged, but overlap remained zero. Rust's first successful commit occurred
  10.21 seconds after its start signal, following thousands of busy reads while
  native clients synchronized after every edit. Atomic open requests deny-all
  sharing on this mount; native open handles can postpone Rust admission.
- `evidence/m7/native-threshold-02` and `native-threshold-03`: a fresh section
  containing the same final text opened normally. The long-history section
  opened after shortening revision dependencies while preserving its resolved
  graph; coalescing file-node fragments alone did not help. Tested histories
  through 676 transactions opened; 701 and longer failed. This bounds an
  observed compatibility failure, not a documented format limit.
- The writer now appends an independent current-object snapshot before dependency
  depth would exceed 512, retaining all older revisions. Section snapshots reuse
  immutable property and attachment declarations; TOC snapshots remap CompactIDs
  into a complete table. `checkpoint-boundaries-01.log` covers two boundaries in
  both native section and TOC fixtures, with every historical revision checked.
  `checkpoint-publication-01.log` checks interrupted text publication at the
  boundary against complete old/new text and formatting states.
- `evidence/m7/local-checkpoint-01`: ten writers and six readers completed
  1,500 randomized Unicode replacements, 22,995 checked observations and 11,966
  overlapping writer calls. `checkpoint-native-01` cold-opened its saved result;
  native text exactly matched the independently replayed edit history.
- `evidence/m7/edit-text-checkpoint-fuzz-05.log`: 4,558 stateful executions in
  302 seconds, including cached snapshots immediately before a full checkpoint,
  with interrupted publication and character/formatting oracles. No failure.
  `checkpoint-regression-02.log`, `checkpoint-clippy-01.log`, and
  `tool-regression-03.log` record passing Rust checks and 22 Python tests.
- `evidence/m7/checkpoint-attachment-native-01`: a native section retained its
  text, images, attachment and formatting through 2,052 scalar edits and repeated
  snapshots. Fresh-cache reading passed 483 explicit character-format checks;
  all native page XML matched the earlier reference except page modification
  time, recorded separately in `retention.json`.
- `evidence/m7/checkpoint-toc-native-01`: the native two-page notebook reopened
  after 1,026 TOC edits crossing two dependency checkpoints. The generator and
  input/output hashes are retained in `evidence/m7/checkpoint-fixtures`.
- `checkpoint-toc-native-02` repeated the TOC check with native PDFs: two pages,
  two tags and 3,562 explicit character-format comparisons passed. PDF geometry
  verified the existing black-highlight case that native XML omits.
- `native-concurrency-05` established 633 clock-bounded native/Rust call overlaps
  under background synchronization. Its original oracle incorrectly treated
  transaction numbers as permanent across native renumbering. Replaying intent
  content validated all 800 Rust commits and 6,370 reads through one counter
  decrease. The run stopped before explicit native convergence; after cleanup,
  one native paragraph lacked its final three cache-acknowledged edits. It is
  not an acceptance pass. The content-history oracle now rejects branches,
  missing acknowledgements, partial observations and reads inconsistent with
  recorded real-time bounds; its regressions are in `test_native_stress.py`.
- `native-concurrency-06`: three native writers, four Rust writers and three
  Rust readers passed the overlapping background-sync gate. All 600 native edits
  and 800 Rust commits converged in every native client and the shared file;
  7,252 Rust reads matched the intent history, with 609 conservatively
  clock-bounded native/Rust call overlaps. A separate cold-cache capture follows
  in `native-concurrency-cold-06`.
- `random-edit-smb-01.json` and `random-edit-smb-01.one`: the CLI created a
  separate edited file on Samba, with byte equality verified directly at the
  server. Examples share the commit adapter's standard-fsync fallback when
  macOS full-sync is unsupported. Rust commit/edit regressions and clippy passed
  (`flush-regression-01.log`, `flush-clippy-01.log`).
- `native-concurrency-cold-06`: a separate fresh OneNote cache retained all
  1,400 intended edits from the successful mixed run. Native comparison passed
  54,330 explicit character-format checks, and exact paragraph text matched
  the independently replayed operations. All associated machines were deleted.

- `native-concurrency-07`: five native writers, eight Rust writers and five Rust
  readers completed 1,000 native edits and 1,600 Rust commits, with 21,152 checked
  reads and 296 clock-bounded native/Rust call overlaps. All six paragraphs
  converged. `native-concurrency-cold-07` independently retained all 2,600 intended
  edits and passed 95,034 explicit format comparisons. Every owned VM was deleted.
- `edit-text-checkpoint-fuzz-06.log`: 10,596 stateful executions in 901 seconds,
  without failure. `edit-text-insertion-fuzz-07.log`: 4,639 executions in 301
  seconds including legacy and empty text properties, without failure.
- Random edits on the private corpus exposed unsupported legacy Unicode
  promotion and absent initial text properties. The writer now adds Unicode
  text while preserving the original legacy property, as MS-ONE 2.2.23 permits.
  `text-insertion-boundaries-02.log` checks native fixtures, unrelated objects,
  historical revisions, short writes and interrupted publication.
- A title edit previously left navigation caches stale. Text and cached titles
  now publish in one revision and transaction; `title-atomicity-04.log` and
  `title-regression-02.log` check rename/clear, history and interrupted short
  writes. `private-random-native-01/title-verification.log` demonstrates that
  the strengthened independent verifier rejects the earlier faulty copy.
  CachedTitleStringFromPage's mandatory empty value applies to nonempty Unicode
  titles; original legacy titles may retain it. The writer's modified-title
  checks enforce that condition without rejecting untouched legacy content.
- `edit-text-title-fuzz-08.log`: 7,383 executions in 301 seconds, adding a native
  title fixture and assertions relating title text, metadata and alternate title
  after every persisted observation. No failure. The later single-title-object
  admission guard was separately covered by `title-regression-02.log`.
- `title-empty-native-01`: a cleared title cold-opened successfully; two pages,
  two tags and 3,394 explicit format checks passed, with four native-PDF highlight
  checks. OneNote regenerated its empty page name from body text during opening;
  this check establishes text/format retention, not stored automatic-title parity.
- `private-random-campaign-04`: five seeded edits on each of 26 current pages,
  130 total. An independent UTF-16 splice model checks all resolved styles,
  unrelated nodes, metadata, contexts, historical revisions and payload hashes
  after every edit. All ten frozen source files remained byte-identical.
  `private-random-native-02/verification-02.log` records a fresh-cache pass on
  all 26 pages, 109 tags and 186,325 explicit format comparisons, with five native
  PDF highlight checks. The capture VM was deleted.
- `title-all-targets-01.log` records passing Rust all-target tests;
  `title-clippy-01.log` has no warnings. `random-native-oracles-01.log` records
  30 passing Python tests, including content-chain replacement histories and
  deliberate title-cache damage.
- `private-random-report-04`: the edited copy is rendered with all 45 stored
  pages and 26 current native references. Browser inspection covered the edited
  Video page; all 2,357 local links resolve (`link-check.json`). The original
  source remains untouched.
- `native-random-08`: randomized replacements and bold/italic changes exposed
  a test-driver error. On its second edit, Win7's framework left OneNote's
  `&#129408;` entity undecoded, so the script treated entity spelling as text.
  The independent intent oracle rejected that history. This run is not a pass;
  all four machines were deleted. `tools/native/text.ps1` now decodes supplementary
  numeric entities before the framework decoder, preserving escaped literals.
  `native-text-test-01/failure-artifacts/text-result.json` records six passing
  Windows decoder regressions. Its unrelated native-capture phase was correctly
  stopped by the profile-isolation guard because this text-only author script
  had not initialized a test profile; the VM was deleted.
- `automatic-title-native-01`: 18 application-authored automatic-title cases
  captured trimming, empty paragraphs, formatting, entities and long text.
  Input and stored metadata are retained with the native page names. Additional
  Unicode-boundary and outline-order cases are being measured before extending
  automatic-title updates.
- `automatic-title-native-02` captured ten more application-authored cases.
  Automatic names select an outline by position (top before bottom, then left),
  use the first text line, and trim whitespace. The 255-UTF-16-unit boundary
  includes a complete supplementary character when it starts at unit 254,
  yielding 256 units. Both fixture sets pass native comparison: 19/11 pages and
  11,680/5,304 explicit format checks. The first set also establishes OneNote's
  XML omission of an otherwise empty outline containing only ASCII spaces;
  the oracle permits that case while still rejecting omitted text, lists or tags.
  These are measured compatibility rules, not a completed automatic-title writer.
- `title-checkpoint-atomicity-02.log` passes interrupted multi-object title
  publication both on the native fixture and at a full dependency checkpoint,
  using 17/257-byte short writes respectively.
- `native-random-09` replayed seed 912 after the native decoder fix: three
  original OneNote writers, four Rust writers and three Rust readers completed
  450 native replacements with bold/italic changes and 600 Rust Unicode
  replacements. All four paragraphs converged; 8,564 reads matched the
  independently chained replacement intents, with 260 clock-bounded native/Rust
  call overlaps. The final native formatting matched each actor's last recorded
  intent. All three Windows VMs and the Linux server were deleted.
- `native-random-cold-09`: a separate fresh cache retained the exact final
  text produced by all 1,050 replacement intents. Native comparison passed
  1,974 explicit format checks; an independent replay also checked 456 character
  bold/italic values against the recorded native editing intent. The cold VM
  was deleted.
- `random-noop-01` records seed 301 replacing ` café ` with itself and producing
  identical output. The CLI now turns an identical replacement into an insertion.
  `test_random_edit_campaign.py` verifies both new-file and in-place operation;
  `tool-regression-06.log` records 31 passing Python tests and
  `final-clippy-04.log` is clean.

- `automatic-title-native-03`: twelve additional native cases distinguish
  explicit titles (full length, leading whitespace removed, trailing whitespace
  retained, first line) from automatic body summaries (trimmed and bounded).
  Empty lines/paragraphs/outlines fall through to later text; table cells supply
  automatic titles. All 13 captured pages pass 9,744 native format comparisons.
- The writer now publishes automatic navigation metadata when body text changes
  and when an explicit title is cleared, choosing body outlines by position.
  New sections use the same bounded automatic-title encoding. No public API was
  added. `automatic-title-regression-03.log` passes edit/writer regressions;
  `automatic-title-edit-tests-01.log` covers native line/UTF-16 boundaries and
  multi-object interrupted publication. Historical objects and all non-title
  metadata fields remain checked independently.
- `automatic-title-edits-01` applies 26 independently checked edits to the 13-page
  native fixture. `automatic-title-edits-native-01` then passes fresh-cache text,
  cached navigation title and 6,992 character-format comparisons. Its VM was
  deleted. `title-empty-native-02` independently confirms the corrected cleared
  title's navigation label, two pages, two tags and 3,394 format checks, with four
  PDF black-highlight checks; its VM was deleted.
- `edit-text-automatic-title-fuzz-09.log`: 6,550 stateful executions in 301 seconds
  after automatic-title changes, with no failure. `automatic-title-all-targets-01.log`
  passes all Rust targets; `automatic-title-clippy-01.log` has no warnings.
- `private-random-campaign-05` passes another 130 edits against the whole-document
  and payload-preservation oracle. All ten frozen source files remained unchanged.
- `private-random-native-03` cold-opens that edited copy: 26 pages, 109 tags,
  186,325 format checks, five PDF highlight checks, and cached navigation titles
  pass. Its VM was deleted. `private-random-report-05` contains the edited pages
  and fresh native references; all 2,357 local links in 46 HTML files resolve.
- `automatic-title-native-04/05` establish RTL outline ordering by descending
  x anchor (width does not affect it), reversed RTL table-cell order, skipped
  attachments, and whitespace trimming after UTF-16 truncation. The two public
  fixtures in `corpus/m7/automatic-titles` retain native provenance. Ten direct
  body-edit regressions cover the RTL/attachment selection rules.
- `automatic-title-edits-02/03` apply 21/18 independently checked edits to these
  fixtures. Fresh native captures pass seven/six pages, cached navigation labels,
  and 2,064/6,288 format checks. Both VMs were deleted. All Rust targets and
  clippy pass in `automatic-title-all-targets-02.log` and
  `automatic-title-clippy-03.log`.
- `native-random-10` repeats the ten-client replacement workload with seed 913:
  450 native edits, 600 Rust commits, 5,595 reads, and 466 clock-bounded native/Rust
  call overlaps. All four paragraphs converge. `native-random-cold-10` passes
  1,740 format comparisons; intent replay independently checks all 1,050 edits
  and 392 native bold/italic character values. All associated VMs were deleted.
- `tools/native_stress.py RUN CAPTURE/read` now replays saved editing intents
  directly against fresh native XML, including exact paragraph multiplicity and
  native formatting intent. `tool-regression-08.log` records 33 passing tests,
  including rejection of missing/extra content, incorrect formatting, lost edits
  and split surrogate pairs.
- `recovery-01` repeats all seven shared-notebook scenarios after the writer
  changes: disjoint edits, reachable competing edits, server restart, transport
  reconnect, lock contention, application restart and VM restart. Both Windows
  clients and the Linux server were deleted. `recovery-cold-01` opens the final
  saved notebook in a new cache and passes text, navigation and 641 character
  format checks; its VM was deleted.
- The conflict oracle follows current page-manifest references only.
  `conflict-oracle-regressions-01.log` checks the native offline-edit fixture and
  rejects treating unreferenced history or detached object spaces as recovery.
  `tool-regression-09.log` records 34 passing Python tests.
- `edit-text-rtl-fuzz-10.log` adds the native RTL title and non-title body
  candidates to the stateful interrupted-edit workload: 4,531 executions in
  301 seconds, no failure.
- `mixed-conflict-01` FAILS acceptance. Three native clients edited the same
  paragraph offline while three Rust writers committed 15 appends and two Rust
  readers observed them. After reconnection, all three native alternatives were
  reachable, but the final Rust text was absent from the current page and its
  conflict pages. The pre-reconnect notebook and every convergence snapshot are
  retained. All three Windows VMs and the Linux server were deleted. Rust edits
  changed the text object's modification time while ancestor paragraph/outline/
  page times stayed unchanged; native edits changed those ancestors too. That
  difference is a hypothesis for investigation, not an established cause.
- `mixed-conflict-02` reduces the workload to one native client, two Rust writers,
  one reader and two Rust commits. Both competing results remain reachable; this
  reduction does not reproduce the failure. Its Windows and Linux VMs were deleted.
- `mixed-conflict-cold-01` independently opens the failed final notebook in a
  fresh cache. The native conflict UI shows only the three native alternatives;
  clipboard captures recover the exact Unicode text of both conflict pages.
  The Rust result is also absent from all exported stored revision objects in
  the last convergence snapshot (`mixed-conflict-01/lost-edit.json`). Native
  comparison passes 200 format checks on the surviving main page; that reader
  agreement does not validate retention of the missing edits. The VM was deleted.
- `mixed-conflict-03` tests ancestor modification timestamps with the original
  three-native/three-Rust-writer workload, using the separate
  `evidence/m7/ancestor-timestamp-probe.py`. The first reconnect capture referenced
  three conflict spaces without default revisions, so the strict reader stopped
  the run. After client teardown, two references remained unresolved. All VMs
  were deleted. `mixed-conflict-cold-03` nevertheless opens the saved file in a
  fresh OneNote cache with the full Rust text; its VM was deleted. This is an
  inconclusive experiment, not acceptance of a timestamp fix.
- The native checkpoint observer now preserves each distinct incomplete snapshot
  and retries missing document contexts for at most two minutes. Other parse
  errors still fail immediately, and retention still requires every competing
  result to be reachable. This allows the next experiment to distinguish an
  intermediate cross-space save from a persistent missing conflict.
- `mixed-conflict-04` repeats the timestamp experiment with that observer. One
  incomplete snapshot resolves, then all three native alternatives and the full
  15-commit Rust result are reachable. All Windows and Linux VMs were deleted.
- Text edits now publish existing ancestor modification timestamps in the same
  transaction as text, run boundaries and navigation caches. Preservation tests
  independently follow raw object references and compare every unrelated field;
  interrupted title/checkpoint publication also checks all modification times.
  `ancestor-edit-regressions-04.log`, `ancestor-all-targets-01.log`,
  `ancestor-clippy-01.log` and `ancestor-tool-regressions-01.log` pass.
  Native acceptance of the atomic implementation is recorded below, separately
  from the timestamp experiment.
- `mixed-conflict-05` stopped before any Rust commit: disconnecting native NICs
  stranded an existing SMB handle and correctly excluded Rust. The setup now
  holds the file's exclusive lock while disconnecting clients. Rust readers
  back off during contention. All owned VMs were deleted; the final stopped
  clone's removal is recorded in `cleanup-confirmed.json`.
- `private-random-campaign-06` applies 130 edits with seed 918 and verifies all
  ten frozen source files unchanged. Fresh native capture
  `private-random-native-04/verification-02.log` passes 26 pages, 109 tags,
  188,956 character-format comparisons and five black-highlight paragraphs.
  Its PDF maps a rendered combining-accent glyph to a space. The PDF oracle
  checks the uniquely located paragraph and the unhighlighted glyph's inline
  region; missing ordinary text, ambiguous matches and black rectangles in that
  region fail. `tool-regression-14.log` passes all 35 Python tests.
  `private-random-report-06` associates all 26 current pages with the fresh
  native references; all 2,357 local links resolve. Its VM was deleted.
- `edit-text-ancestor-fuzz-11.log` exercises stateful edits and interrupted
  publication after ancestor timestamp propagation: 4,236 runs in 301 seconds,
  no failure.
- `mixed-conflict-06` passes with the atomic implementation: three native
  alternatives and the full 15-commit Rust result remain reachable after all
  three native clients close. A transient incomplete save resolves before
  acceptance. All Windows and Linux VMs were deleted. `mixed-conflict-cold-06`
  opens the closed result in a fresh cache; native conflict UI clipboard
  captures independently match all four intended results exactly. That VM was
  deleted too. The ordinary page comparison passes separately; its zero
  explicit format checks do not substitute for the four intent comparisons.
- The native clone cleanup now handles the VM helper's `SystemExit` on shutdown
  timeout, terminates the owned VM and deletes its overlay. The regression
  checks deletion and the teardown record; `tool-regression-17.log` passes all
  36 Python tests, and the private native comparison still passes.
- `native-random-11` never reached notebook editing: YAML parsed its all-digit
  WAN MAC address as a sexagesimal integer, so cloud-init could not configure
  the interface. The failed boot log is preserved and its VM was deleted.
  Quoting both MAC addresses fixes the seed. `linux-lab-regression-01.log`
  passes the two lifecycle tests; `native-random-12` reuses the same VM name
  and deterministic MAC after deletion and reaches ready Windows clients.
- `native-random-12` passes seed 918 with the ancestor fix: three native
  writers, four Rust writers and three Rust readers; 450 native edits, 600
  Rust commits, 2,471 checked reads and 428 clock-bounded overlapping native/Rust
  calls. All four intended paragraphs converge and native formatting matches
  the recorded operations. All task-owned Windows and Linux VMs were deleted.
- `native-random-cold-12` independently reopens that result and compares every
  recorded native/Rust edit with the native XML. Its VM was deleted.
- `same-second-build` is an isolated controlled-clock build with one recorded
  source substitution: a text edit uses its baseline element timestamp.
  `same-second-equivalence/result.json` confirms that its resolved revision
  exactly matches a production-library edit completed in the same actual
  second. The production clock and library are unchanged. `mixed-conflict-07`
  uses that build to test baseline-equal timestamps with three native writers,
  three Rust writers and two Rust readers.
- `mixed-conflict-07` passes: the four page-content element timestamps remain
  exactly equal to the cached baseline across all 15 Rust commits, and all
  four competing results survive reconnect and application closure. The three
  Windows VMs and Linux server were deleted. This distinguishes valid equal
  timestamps from the inconsistent descendant/ancestor times in the original
  failing writer; it does not require inventing future timestamps.
- `mixed-conflict-cold-07` independently opens the same-second result and
  captures exact Unicode clipboard text from the main page and all three
  native conflict pages. All four results match the recorded intent, and the
  VM was deleted. Conflict retention now counts duplicate text instead of
  collapsing it into a set: `exact-retention.json` rechecks both successful
  closed notebooks against the exact four-result multiset.
- `native-random-13` passes seed 919 with twelve clients: four native writers,
  five Rust writers and three Rust readers; 600 native edits, 750 Rust commits,
  1,903 checked reads and 456 clock-bounded overlapping native/Rust calls.
  All five intended paragraphs converge. The harness now captures
  `stress-closed` after every native client closes; fresh-cache acceptance uses
  this snapshot instead of the earlier live checkpoint.
- `native-random-cold-13` opens that closed snapshot in a fresh native cache:
  all 1,350 recorded edits match exactly across five paragraphs, with 1,672
  explicit character-format comparisons and 390 checks against the native
  writers' formatting intents. Its VM and all campaign VMs were deleted.
- Final cleanup records are `final-teardown-check.json` and
  `final-source-check.json`: 21 owned Windows clone identities and five Linux
  VM identities are absent, and all ten frozen private source files retain
  their hashes. `tool-regression-19.log` passes 36 Python tests. The HTML review
  is `private-random-report-06/index.html`; its 26 current pages link to the
  verified native references. Physical power-loss and unsynchronized native
  cache durability remain outside these guarantees.
