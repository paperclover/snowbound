# Search

OneNote 2010's search, observed on the Win7 VM (2026-09-27) with `notebook` open, a copy of
`docs/sample-notebook/Personal`. `crates/canvas/tests/search.rs` checks the shared engine
against it.

- The box beside the section tabs reads "Search All Notebooks (Ctrl+E)". Its arrow opens
  "Search In:" with "Find on This Page (Ctrl+F)", "This Section", "This Section Group",
  "This Notebook", "All Notebooks" (checked) and "Set This Scope as Default"; a scope not set
  as default returns to it for the next search.
- Typing `tom` with This Notebook lists, under "Finished: This Notebook (change)" and "Find
  on page: Ctrl+F", "Title contains: tom (1)" (Tomatoes) and "Body contains: tom (4)"
  (Weeknight dal, Spring planting plan, Harvest log 2025, Seed inventory), each with its
  notebook and section, "(Personal/Garden)". Words match word starts: `tom` finds
  "Tomatoes", `sungold` finds "‘Sungold’".
- `sungold tom` lists only "Body contains: sungold tom (4)": Tomatoes, Spring planting plan,
  Harvest log 2025, Seed inventory. A page matches when every word is in its title or text;
  "Title contains" needs every word in the title.
- Down arrow selects the next result and shows its page with every match highlighted in
  yellow. Esc closes the list and empties the box; the page keeps its first match selected.
- `zzq` shows "No matches:  All Notebooks".
- Ctrl+F on Spring planting plan turns the box into "Find on page"; `bed` shows "Match 1 of
  6" with previous and next arrows ("Previous Match (Shift+F3)"), the current match
  selected and the others yellow. Enter goes to the next. `zzq` shows "No matches. Try
  Ctrl+E".
- The Search Results pane (Alt+O) lists each page under its date with the paragraph around
  its first match, "…" where the paragraph is cut.
