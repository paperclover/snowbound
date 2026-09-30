# Styles and themes

`onenote/` is OneNote 2010 applying its own Styles gallery in the lab (2026-09-30):
`Styles.one` holds a page with each of the eleven styles applied (Ctrl+Alt+1 to 6 and the
gallery), Enter after each, a style over explicit formatting, Clear Formatting, and a second
page whose styles were made through COM in Georgia, onto which OneNote added its own `h1`
and into which a pasted Georgia heading brought its own; `applied.xml` is the first page's
COM read. Applying a style stores a paragraph style object per style per page and clears
the paragraph's explicit character formatting; headings carry NextStyle `p`.

`candidate/` is the notebook `themed_pages_hold_their_theme_under_onenote_s_names` in
`crates/snowbound/src/themes.rs` writes: a section per built-in theme (OneNote, Manuscript,
Editorial, Modern), each a page with a paragraph in every gallery style applied through the
editor, then restyled as opening the page does, and `.snowbound/themes.json` giving each
section its theme. `cold/` is `tools/native_styles.py` in a fresh OneNote 2010 clone: every
page draws in its theme (`read/page-00N.png`) and lists the eleven styles under OneNote's
names with the theme's values, and no `.snowbound` section group appears.

`onenote-edit/` is OneNote 2010 editing the Manuscript page of `candidate/` in the lab:
Ctrl+Alt+2 on the Normal paragraph and Enter after it, and Enter after the theme's Heading
5. OneNote applies its own `h2` and `p` beside the theme's (`Manuscript.png`); reopened in
Snowbound they read under those names and restyling heals them
(`styles_onenote_adds_to_a_themed_page_are_healed`).

`tools/test_styles.py` checks this without a VM. Regenerate with `SNOWBOUND_STYLES_EXPORT`
set to a new absolute directory while running the test, then `tools/native_styles.py OUTPUT
COLD`.
