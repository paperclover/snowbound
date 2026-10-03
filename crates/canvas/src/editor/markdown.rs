//! Markdown shortcuts: Markdown typed at a paragraph's start or around text becomes the
//! OneNote formatting it stands for, as one undo step after the typing, which Undo takes back
//! to the characters typed, as Word's AutoFormat As You Type does.

use super::format::{ListStyle, NoteTag, list_definition, styled, time32};
use super::*;

/// What a marker typed at a paragraph's start makes of it.
#[derive(Debug, PartialEq)]
enum Block {
    /// A style of the Styles gallery, by stored name.
    Style(String),
    /// A list, its count restarting at the number given.
    List(ListStyle, Option<u32>),
    /// A To Do check box, checked or not.
    ToDo(bool),
}

impl Block {
    /// The lists are those OneNote 2010's AutoFormat makes of the same markers, but for `>`,
    /// which it makes an arrow bullet.
    fn of(marker: &str) -> Option<Self> {
        Some(match marker {
            "*" => Self::List(ListStyle::BULLET, None),
            "-" => Self::List(ListStyle::Bullet(25), None),
            "a." => Self::List(ListStyle::Number(2), None),
            ">" => Self::Style("blockquote".into()),
            "[ ]" => Self::ToDo(false),
            "[x]" | "[X]" => Self::ToDo(true),
            _ if marker.len() <= 6 && !marker.is_empty() && marker.bytes().all(|b| b == b'#') => {
                Self::Style(format!("h{}", marker.len()))
            }
            _ => {
                let (digits, style) = match marker.as_bytes().last()? {
                    b'.' => (&marker[..marker.len() - 1], ListStyle::NUMBER),
                    b')' => (&marker[..marker.len() - 1], ListStyle::Number(6)),
                    _ => return None,
                };
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let number = digits.parse::<u32>().ok()?;
                Self::List(style, (number != 1).then_some(number))
            }
        })
    }
}

#[derive(Clone, Copy)]
enum Inline {
    Bold,
    Italic,
    Strike,
    Code,
}

/// Markers typed around text, a pair before its half.
const INLINE: [(&str, Inline); 5] = [
    ("**", Inline::Bold),
    ("~~", Inline::Strike),
    ("*", Inline::Italic),
    ("_", Inline::Italic),
    ("`", Inline::Code),
];

/// Whether no character of bytes `range` belongs to a link, field or equation, or is set in
/// the Code style's font.
fn plain(text: &Paragraph, range: Range<usize>, code_font: Option<&str>) -> bool {
    let mut start = 0;
    text.spans().iter().all(|span| {
        let overlaps = start < range.end && span.end > range.start;
        start = span.end;
        let format = &span.format;
        !overlaps
            || ![
                format.hidden,
                format.hyperlink,
                format.math,
                format.embedded_object,
            ]
            .contains(&Some(true))
                && (code_font.is_none() || format.font.as_deref() != code_font)
    })
}

/// `text` without bytes `markers`, what lies between them changed by `change`; a paragraph
/// left empty keeps the format of the first marker.
fn unmarked(
    text: &Paragraph,
    markers: [Range<usize>; 2],
    change: impl Fn(&mut Format),
) -> Paragraph {
    let [open, close] = markers;
    let kept = [
        (0..open.start, false),
        (open.end..close.start, true),
        (close.end..text.text().len(), false),
    ];
    let mut runs = Vec::new();
    let mut start = 0;
    for span in text.spans() {
        for (range, inside) in &kept {
            let [from, to] = [range.start, range.end].map(|at| at.clamp(start, span.end));
            if from < to {
                let mut format = span.format.clone();
                if *inside {
                    change(&mut format);
                }
                runs.push((text.text()[from..to].to_owned(), format));
            }
        }
        start = span.end;
    }
    if runs.is_empty() {
        let first = text.spans().partition_point(|span| span.end <= open.start);
        let format = text.spans()[first.min(text.spans().len() - 1)]
            .format
            .clone();
        return Paragraph::new(String::new(), format);
    }
    Paragraph::from_runs(runs)
}

impl CanvasEditor {
    /// The Styles gallery's style `name`: the page's theme's, else OneNote 2010's.
    fn gallery(&self, name: &str) -> Option<Definition> {
        self.styles
            .get(name)
            .or_else(|| self.markdown.as_ref()?.get(name))
            .cloned()
    }

    /// Applies the Markdown shortcut that `typed`, just typed at the caret, completes, as its
    /// own undo step; a marker at the paragraph's start can then be taken back by Backspace.
    pub(super) fn markdown(
        &mut self,
        engine: &mut TextEngine,
        typed: &str,
    ) -> Result<(), EditorError> {
        let outline = self.active_outline();
        let [anchor, focus] = outline.selection.positions;
        if self.markdown.is_none()
            || !matches!(typed, " " | "*" | "_" | "~" | "`")
            || outline.title
            || anchor != focus
            || self.page_selected()
        {
            return Ok(());
        }
        let (container, index, node) = outline
            .document
            .leaf(focus.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let id = outline.id;
        let mut node = node.clone();
        if node
            .style
            .and_then(|style| self.definitions.get(&style))
            .is_some_and(|style| matches!(&style.kind, Kind::Style { name: Some(name), .. } if name == "code"))
        {
            return Ok(());
        }
        let text = node.text().ok_or(EditError::InvalidRange)?.text.clone();
        let caret = text.byte_offset(focus.offset)?;
        let code_font = self.gallery("code").and_then(|code| code.format.font);
        let code_font = code_font.as_deref();
        let (replacement, at, block) = if typed == " " {
            let Some(block) = text.text()[..caret].strip_suffix(' ').and_then(Block::of) else {
                return Ok(());
            };
            if !plain(&text, 0..caret, code_font) {
                return Ok(());
            }
            let rest = unmarked(&text, [0..caret, caret..caret], |_| {});
            (rest, 0, Some(block))
        } else {
            let Some((rest, at)) = inline(&text, caret, code_font) else {
                return Ok(());
            };
            (rest, at, None)
        };
        let typing = text.format_at(focus.offset)?.clone();
        let text = &mut node.text_mut().unwrap().text;
        *text = replacement;
        let offset = text.utf16_offset(at)?;
        let line = block.is_some();
        match block {
            Some(Block::Style(name)) => {
                let Some(definition) = self.gallery(&name) else {
                    return Ok(());
                };
                let old = self.style_format(node.style)?;
                node.style = Some(self.define_style(&definition)?);
                let text = &mut node.text_mut().unwrap().text;
                *text = styled(text, &old, &definition);
            }
            Some(Block::List(style, restart)) => {
                let format = &node.text().unwrap().text.spans()[0].format;
                let mut definition = list_definition(style, format);
                if let Kind::List { restart: count, .. } = &mut definition.kind {
                    *count = restart;
                }
                let list = onestore::page::text::new_id()?;
                self.definitions.insert(list, definition);
                node.lists = vec![list];
            }
            // A check box takes a list's place, as To Do List does.
            Some(Block::ToDo(checked)) => {
                node.lists.clear();
                let tag = &NoteTag::defaults()[0];
                let definition = tag.definition(0);
                let tag_id = self.define_tag(&definition)?;
                let ParagraphContent::Text(text) = &mut node.content else {
                    unreachable!()
                };
                self.retag(
                    [&mut node.tags, &mut text.tags],
                    &definition.kind,
                    Some((tag_id, tag.shape, time32())),
                );
                if checked {
                    self.toggle_checks(node.tags.iter_mut().chain(&mut text.tags).collect());
                }
            }
            None => {}
        }
        let caret = TextPosition {
            paragraph: focus.paragraph,
            offset,
        };
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index..index + 1,
                replacement: vec![node],
            },
            Selection {
                positions: [caret; 2],
                affinities: [Affinity::Upstream; 2],
            },
        )?;
        self.typing = None;
        if line {
            self.formatted = Some((id, caret, self.undo.len()));
        } else {
            // Typing goes on in the format the closing marker was typed in.
            self.pending = Some((id, caret, typing));
        }
        Ok(())
    }
}

/// `text` with the span whose closing marker ends at byte `caret` formatted and its markers
/// gone, and where the caret goes then; none where no span closes there, or it would
/// close inside a word, a URL, a link, an equation or code.
fn inline(text: &Paragraph, caret: usize, code_font: Option<&str>) -> Option<(Paragraph, usize)> {
    let source = text.text();
    let before = &source[..caret];
    let (marker, inline) = INLINE
        .into_iter()
        .find(|(marker, _)| before.ends_with(marker))?;
    let edge = marker.chars().next()?;
    let close = caret - marker.len();
    let open = before[..close].rfind(marker)?;
    let content = &before[open + marker.len()..close];
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == edge);
    if content.is_empty()
            || content.starts_with(char::is_whitespace)
            || content.ends_with(char::is_whitespace)
            || content.starts_with(edge)
            || content.ends_with(edge)
            || word(before[..open].chars().next_back())
            || source[caret..].chars().next().is_some_and(char::is_alphanumeric)
            || !plain(text, open..caret, code_font)
            // An odd backtick before the span opens code around it.
            || !matches!(inline, Inline::Code) && before[..open].matches('`').count() % 2 == 1
    {
        return None;
    }
    let token = before[..open]
        .rfind(char::is_whitespace)
        .map_or(0, |space| space + 1);
    let url = &before[token..];
    if url.contains("://") || url.starts_with("www.") {
        return None;
    }
    let font = code_font.unwrap_or("Consolas").to_owned();
    let rest = unmarked(
        text,
        [open..open + marker.len(), close..caret],
        |format| match inline {
            Inline::Bold => format.bold = Some(true),
            Inline::Italic => format.italic = Some(true),
            Inline::Strike => format.strike = Some(true),
            Inline::Code => format.font = Some(font.clone()),
        },
    );
    Some((rest, caret - 2 * marker.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_name_their_blocks() {
        assert_eq!(Block::of("###"), Some(Block::Style("h3".into())));
        assert_eq!(Block::of("#######"), None);
        assert_eq!(
            Block::of("2."),
            Some(Block::List(ListStyle::NUMBER, Some(2)))
        );
        assert_eq!(
            Block::of("1)"),
            Some(Block::List(ListStyle::Number(6), None))
        );
        assert_eq!(Block::of("."), None);
        assert_eq!(Block::of("1a."), None);
        assert_eq!(Block::of("[x]"), Some(Block::ToDo(true)));
    }

    fn calibri() -> Format {
        Format {
            font: Some("Calibri".into()),
            font_size: Some(11.0),
            ..Format::default()
        }
    }

    fn style(name: &str, font: &str) -> Definition {
        Definition {
            kind: Kind::Style {
                name: Some(name.into()),
                next: name.starts_with('h').then(|| "p".into()),
            },
            format: Format {
                bold: Some(name.starts_with('h')),
                font: Some(font.into()),
                font_size: Some(14.0),
                ..Format::default()
            },
        }
    }

    /// An editor with Markdown shortcuts on, its one paragraph holding `text`, the caret
    /// at its end.
    fn opened(engine: &mut TextEngine, text: &str) -> CanvasEditor {
        let paragraph = Paragraph::new(text.into(), calibri());
        let mut editor =
            CanvasEditor::new(engine, TextDocument::new(vec![paragraph]).unwrap(), 300.0).unwrap();
        let gallery = ["h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "p"]
            .map(|name| (name.to_owned(), style(name, "Calibri")));
        editor.markdown = Some(
            gallery
                .into_iter()
                .chain([("code".to_owned(), style("code", "Consolas"))])
                .collect(),
        );
        let end = text.encode_utf16().count() as u32;
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: end,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor
    }

    fn typed(engine: &mut TextEngine, editor: &mut CanvasEditor, text: &str) {
        for character in text.chars() {
            editor
                .insert(engine, character.encode_utf8(&mut [0; 4]))
                .unwrap();
        }
    }

    fn text(editor: &CanvasEditor) -> String {
        editor
            .active_outline()
            .document
            .paragraphs()
            .next()
            .unwrap()
            .text()
            .to_owned()
    }

    /// The paragraph's runs, with whether each is bold, italic, struck and in Consolas.
    fn runs(editor: &CanvasEditor) -> Vec<(String, [bool; 4])> {
        let paragraph = editor
            .active_outline()
            .document
            .paragraphs()
            .next()
            .unwrap();
        let mut start = 0;
        paragraph
            .spans()
            .iter()
            .map(|span| {
                let text = paragraph.text()[start..span.end].to_owned();
                start = span.end;
                let format = &span.format;
                let on = |value: Option<bool>| value == Some(true);
                let code = format.font.as_deref() == Some("Consolas");
                (
                    text,
                    [on(format.bold), on(format.italic), on(format.strike), code],
                )
            })
            .collect()
    }

    #[test]
    fn hashes_make_headings_that_undo_and_backspace_take_back_to_the_typed_text() {
        let mut engine = TextEngine::default();
        for level in 1..=6 {
            let mut editor = opened(&mut engine, "");
            typed(&mut engine, &mut editor, &format!("{} ", "#".repeat(level)));
            assert_eq!(text(&editor), "");
            let state = editor.format_state().unwrap();
            assert_eq!(state.style, Some(format!("h{level}")));
            typed(&mut engine, &mut editor, "Title");
            assert_eq!(text(&editor), "Title");
            assert_eq!(
                runs(&editor),
                [("Title".into(), [true, false, false, false])]
            );
            // Undo takes the typing, then the heading back to the marker.
            editor.undo(&mut engine).unwrap();
            assert_eq!(text(&editor), "");
            editor.undo(&mut engine).unwrap();
            assert_eq!(text(&editor), format!("{} ", "#".repeat(level)));
            assert_eq!(editor.format_state().unwrap().style, None);
        }
        let mut editor = opened(&mut engine, "");
        typed(&mut engine, &mut editor, "## ");
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(text(&editor), "## ");
        assert_eq!(editor.format_state().unwrap().style, None);
        // A second Backspace deletes as ever.
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(text(&editor), "##");
    }

    #[test]
    fn markers_start_lists_to_dos_and_quotes_as_onenote_s_autoformat_does() {
        let mut engine = TextEngine::default();
        for (marker, list) in [
            ("* ", ListStyle::BULLET),
            ("- ", ListStyle::Bullet(25)),
            ("1. ", ListStyle::NUMBER),
            ("1) ", ListStyle::Number(6)),
            ("a. ", ListStyle::Number(2)),
        ] {
            let mut editor = opened(&mut engine, "");
            typed(&mut engine, &mut editor, marker);
            assert_eq!(text(&editor), "", "{marker}");
            assert_eq!(editor.format_state().unwrap().list, Some(list), "{marker}");
            editor.delete(&mut engine, true).unwrap();
            assert_eq!(text(&editor), marker);
            assert_eq!(editor.format_state().unwrap().list, None);
        }
        let mut editor = opened(&mut engine, "");
        typed(&mut engine, &mut editor, "3. ");
        let node = &editor.active_outline().document.nodes()[0];
        assert!(matches!(
            editor.definitions[&node.lists[0]].kind,
            Kind::List {
                restart: Some(3),
                ..
            }
        ));
        for (marker, checked) in [("[ ] ", false), ("[x] ", true)] {
            let mut editor = opened(&mut engine, "");
            typed(&mut engine, &mut editor, marker);
            assert_eq!(text(&editor), "");
            let state = editor.format_state().unwrap();
            assert_eq!(state.tags[0].0.label, "To Do");
            let tag = &editor.active_outline().document.nodes()[0]
                .text()
                .unwrap()
                .tags[0];
            assert_eq!(tag.status & 1 != 0, checked);
        }
        let mut editor = opened(&mut engine, "");
        typed(&mut engine, &mut editor, "> ");
        assert_eq!(
            editor.format_state().unwrap().style.as_deref(),
            Some("blockquote")
        );
        // A marker before text already there formats it too.
        let mut editor = opened(&mut engine, "words");
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        typed(&mut engine, &mut editor, "# ");
        assert_eq!(text(&editor), "words");
        assert_eq!(editor.format_state().unwrap().style.as_deref(), Some("h1"));
    }

    #[test]
    fn closing_markers_format_the_text_between_and_undo_returns_the_markers() {
        let mut engine = TextEngine::default();
        for (input, format) in [
            ("**bold**", [true, false, false, false]),
            ("*it*", [false, true, false, false]),
            ("_it_", [false, true, false, false]),
            ("~~gone~~", [false, false, true, false]),
            ("`a*b*c`", [false, false, false, true]),
        ] {
            let mut editor = opened(&mut engine, "say ");
            typed(&mut engine, &mut editor, input);
            let inner = input.trim_matches(['*', '_', '~', '`']);
            assert_eq!(
                runs(&editor),
                [("say ".into(), [false; 4]), (inner.into(), format),],
                "{input}"
            );
            // What is typed next is plain.
            typed(&mut engine, &mut editor, " on");
            assert_eq!(runs(&editor).last().unwrap(), &(" on".into(), [false; 4]));
            editor.undo(&mut engine).unwrap();
            editor.undo(&mut engine).unwrap();
            assert_eq!(text(&editor), format!("say {input}"), "{input}");
        }
    }

    #[test]
    fn markers_inside_words_urls_code_and_titles_or_with_shortcuts_off_stay_typed() {
        let mut engine = TextEngine::default();
        for input in [
            "snake_case_name",
            "2*3*4",
            "a * b *",
            "http://example.com/_a_",
            "www.example.com/*a*",
            "`a *b*",
            "**",
            "#tag ",
            "x # ",
            "####### ",
        ] {
            let mut editor = opened(&mut engine, "");
            typed(&mut engine, &mut editor, input);
            assert_eq!(text(&editor), input);
            assert_eq!(runs(&editor), [(input.into(), [false; 4])], "{input}");
        }
        // A closing marker typed before a word's letters leaves them unformatted.
        let mut editor = opened(&mut engine, "*a b");
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 3,
                }; 2]
                    .into(),
            )
            .unwrap();
        typed(&mut engine, &mut editor, "*");
        assert_eq!(text(&editor), "*a *b");
        let mut editor = opened(&mut engine, "");
        editor.markdown = None;
        typed(&mut engine, &mut editor, "# **a** ");
        assert_eq!(text(&editor), "# **a** ");
        // Code keeps what it holds.
        let mut editor = opened(&mut engine, "");
        let code = editor.gallery("code").unwrap();
        editor.format(&mut engine, Formatting::Style(code)).unwrap();
        typed(&mut engine, &mut editor, "# *a* ");
        assert_eq!(text(&editor), "# *a* ");
    }

    /// Every shortcut stores ordinary OneNote formatting that reads back as itself.
    /// `SNOWBOUND_MARKDOWN_EXPORT` names a new directory receiving the notebook, for a cold
    /// open in OneNote.
    #[test]
    fn shortcuts_write_as_onenote_formatting() {
        use onestore::{RevisionIndex, Store, document::Document};
        const NOTEBOOK: &str = "../../corpus/paragraph-edit/before/notebook";
        let section = std::fs::read(format!("{NOTEBOOK}/synthetic.one")).unwrap();
        let page = |bytes: &[u8]| {
            let store = Store::parse(bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            document
                .pages()
                .unwrap()
                .into_iter()
                .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
                .find(|(_, page)| page.title == "Split middle")
                .unwrap()
        };
        let (space, source) = page(&section);
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::from_page(source, &mut engine).unwrap();
        editor.markdown = opened(&mut engine, "").markdown;
        let body = editor
            .outlines()
            .iter()
            .find(|outline| !outline.title)
            .unwrap()
            .id;
        editor.focus_outline(body).unwrap();
        editor
            .move_selection(&mut engine, Movement::DocumentEnd, false)
            .unwrap();
        for line in [
            "Plain **bold** *italic* _also_ ~~struck~~ `code` end",
            "# Heading one",
            "## Heading two",
            "### Heading three",
            "#### Heading four",
            "##### Heading five",
            "###### Heading six",
            "* Star",
            "- Dash",
            "1. One",
            "2) Two",
            "a. Alpha",
            "3. Three",
            "[ ] To do",
            "[x] Done",
            "[X] Done too",
            "> Quoted",
        ] {
            editor.enter(&mut engine, false).unwrap();
            typed(&mut engine, &mut editor, line);
        }
        let edited = editor.page().unwrap();
        let written = super::super::ops::saved(&section, space, &mut editor);
        let (_, reread) = page(&written);
        let settled = |mut page: Page| {
            for object in &mut page.objects {
                if let PageObject::Outline(outline) = object {
                    for text in outline.paragraphs.iter_mut().filter_map(|p| p.text_mut()) {
                        for tag in &mut text.tags {
                            tag.extra_set = 0;
                        }
                    }
                }
            }
            page
        };
        assert_eq!(settled(reread).objects, settled(edited).objects);
        if let Some(directory) = std::env::var_os("SNOWBOUND_MARKDOWN_EXPORT") {
            let directory = std::path::Path::new(&directory);
            std::fs::create_dir(directory).unwrap();
            std::fs::write(directory.join("synthetic.one"), &written).unwrap();
            std::fs::copy(
                format!("{NOTEBOOK}/Open Notebook.onetoc2"),
                directory.join("Open Notebook.onetoc2"),
            )
            .unwrap();
        }
    }
}
