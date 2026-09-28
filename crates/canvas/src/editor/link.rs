//! Hyperlinks as OneNote 2010 edits them (`corpus/link-edit/native-typed`): the Link dialog
//! stores the address as typed in a hidden `HYPERLINK` field code before the label, a URL
//! typed and ended by a space or Enter becomes a link of its own text, typing after a label
//! is plain text, and Remove Link leaves the label as plain text.

use super::format::{leaves_mut, restyle, selected};
use super::*;

/// A link in a paragraph, in UTF-16 offsets of its stored text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub paragraph: usize,
    /// The hidden field code naming the address, before the label; none where the label is
    /// its own address.
    pub code: Option<Range<u32>>,
    pub label: Range<u32>,
    pub target: String,
}

impl Link {
    fn range(&self) -> Range<u32> {
        self.code
            .as_ref()
            .map_or(self.label.start, |code| code.start)..self.label.end
    }
}

/// The links in `text`, the text of paragraph `paragraph`.
pub(crate) fn links(text: &Paragraph, paragraph: usize) -> Vec<Link> {
    let mut found: Vec<Link> = Vec::new();
    let mut code: Option<(Range<u32>, String)> = None;
    let (mut byte, mut unit) = (0, 0);
    for span in text.spans() {
        let fragment = &text.text()[byte..span.end];
        let range = unit..unit + fragment.encode_utf16().count() as u32;
        (byte, unit) = (span.end, range.end);
        let format = &span.format;
        if format.hyperlink != Some(true) {
            code = None;
            continue;
        }
        if format.hidden == Some(true) {
            code = fragment
                .strip_prefix("\u{fddf}HYPERLINK \"")
                .and_then(|target| target.strip_suffix('"'))
                .map(|target| (range, target.to_owned()));
            continue;
        }
        let labelled = format.hyperlink_label == Some(true);
        match (code.take(), found.last_mut()) {
            (Some((code, target)), _) if code.end == range.start => found.push(Link {
                paragraph,
                code: Some(code),
                label: range,
                target,
            }),
            // A label runs on across format changes; a bare link is its own text.
            (_, Some(last)) if last.label.end == range.start && last.code.is_some() == labelled => {
                last.label.end = range.end;
            }
            _ => found.push(Link {
                paragraph,
                code: None,
                label: range,
                target: String::new(),
            }),
        }
    }
    for link in &mut found {
        if link.code.is_none() {
            let start = text.byte_offset(link.label.start).unwrap_or(0);
            let end = text.byte_offset(link.label.end).unwrap_or(start);
            link.target = text.text()[start..end].to_owned();
        }
    }
    found
}

/// Where a typed URL lies in `word`, a run of text ended by a space or Enter, as OneNote
/// links it: from a scheme it knows (or `www.`, or a share's `\\`) preceded by punctuation
/// only, to before any trailing punctuation or unmatched closing bracket.
pub(crate) fn typed_url(word: &str) -> Option<Range<usize>> {
    const SCHEMES: [&str; 9] = [
        "http://", "https://", "ftp://", "file://", "mailto:", "news:", "onenote:", "www.", "\\\\",
    ];
    let lower = word.to_ascii_lowercase();
    let (start, scheme) = SCHEMES
        .iter()
        .filter_map(|scheme| Some((lower.find(scheme)?, scheme.len())))
        .min()?;
    if word[..start].chars().any(char::is_alphanumeric) {
        return None;
    }
    let mut end = word.len();
    while let Some(last) = word[start..end].chars().last() {
        let count = |c: char| word[start..end].matches(c).count();
        let trailing = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => count('(') < count(')'),
            ']' => count('[') < count(']'),
            '}' => count('{') < count('}'),
            _ => false,
        };
        if !trailing {
            break;
        }
        end -= last.len_utf8();
    }
    (end > start + scheme).then_some(start..end)
}

/// The URLs written as text in `text`, as byte ranges: OneNote links them when it opens a
/// page, so they show and open as links without being stored as ones.
pub fn shown_urls(text: &Paragraph) -> Vec<Range<usize>> {
    let plain =
        |format: &Format| ![format.hyperlink, format.hidden, format.math].contains(&Some(true));
    let mut urls = Vec::new();
    let mut start = 0;
    for span in text.spans() {
        if !plain(&span.format) {
            start = span.end;
            continue;
        }
        let run = &text.text()[start..span.end];
        let mut at = 0;
        for word in run.split(char::is_whitespace) {
            // Most words hold none of what a URL starts with.
            let candidate = word.contains([':', '\\'])
                || word
                    .as_bytes()
                    .windows(4)
                    .any(|w| w.eq_ignore_ascii_case(b"www."));
            if candidate && let Some(url) = typed_url(word) {
                urls.push(start + at + url.start..start + at + url.end);
            }
            at += word.len()
                + run[at + word.len()..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8);
        }
        start = span.end;
    }
    urls
}

/// The format of the character at UTF-16 `offset`.
pub(crate) fn format_after(text: &Paragraph, offset: u32) -> Option<&Format> {
    let byte = text.byte_offset(offset).ok()?;
    let length = text.text()[byte..].chars().next()?.len_utf16() as u32;
    text.format_at(offset + length).ok()
}

/// Text that follows a link rather than extending it.
pub(crate) fn unlinked(format: &mut Format) {
    format.hidden = format.hidden.map(|_| false);
    format.hyperlink = format.hyperlink.map(|_| false);
    format.hyperlink_label = None;
}

/// A field code naming `address`, which cannot hold its quotes or line breaks.
fn field_code(address: &str) -> String {
    let address: String = address
        .trim()
        .chars()
        .filter(|c| !c.is_control() && *c != '\u{fddf}')
        .map(|c| {
            if c == '"' {
                "%22".to_owned()
            } else {
                c.to_string()
            }
        })
        .collect();
    format!("\u{fddf}HYPERLINK \"{address}\"")
}

impl CanvasEditor {
    /// The link in the active outline at `position`, its ends included.
    pub fn link_at(&self, position: TextPosition) -> Option<Link> {
        let text = self
            .active_outline()
            .document
            .paragraph(position.paragraph)?;
        links(text, position.paragraph).into_iter().find(|link| {
            link.range().contains(&position.offset) || link.label.end == position.offset
        })
    }

    /// The address of the link drawn under document point `point` of outline `id`, which a
    /// click opens as OneNote's does.
    pub fn link_under(&self, id: ExGuid, [x, y]: [f32; 2]) -> Option<String> {
        let outline = self.visible_outlines().find(|outline| outline.id == id)?;
        let index = outline.paragraph_at(x, y).ok()?;
        let paragraph = &outline.shaped.paragraphs[index];
        let point = [x - paragraph.origin[0], y - paragraph.origin[1]];
        let cursor = paragraph.text.hit_test(point[0], point[1]);
        let edge = paragraph.text.caret(cursor, 0.0).x0 as f32;
        let visible = paragraph.projection.text();
        let byte = cursor.index();
        // The character under the point lies on the side of the nearest boundary it is on.
        let at = if point[0] < edge {
            visible.text()[..byte].char_indices().next_back()?.0
        } else {
            visible.text()[byte..].chars().next()?;
            byte
        };
        let (line, _) = paragraph
            .text
            .lines()
            .find(|(_, bounds)| bounds.top <= point[1] && point[1] < bounds.top + bounds.height)?;
        if point[0] > line.metrics().advance {
            return None;
        }
        let source = paragraph
            .projection
            .source_offset(
                visible.utf16_offset(at).ok()?,
                onestore::page::text::Affinity::Downstream,
            )
            .ok()?;
        let paragraph = outline.source_index(index);
        links(outline.document.paragraph(paragraph)?, paragraph)
            .into_iter()
            .find(|link| link.label.contains(&source))
            .map(|link| link.target)
            .or_else(|| {
                shown_urls(visible)
                    .into_iter()
                    .find(|url| url.contains(&at))
                    .map(|url| visible.text()[url].to_owned())
            })
    }

    /// What the Link dialog links, as OneNote 2010 picks it: the selection within one
    /// paragraph, else the link or word at the caret, else nothing at the caret. With the
    /// text it shows and the address it holds.
    fn link_range(&self) -> Result<(usize, Range<u32>, String, String), EditError> {
        let [anchor, focus] = self.active_outline().selection.positions;
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        if start.paragraph != end.paragraph {
            return Err(EditError::UnsupportedContent);
        }
        let text = self
            .active_outline()
            .document
            .paragraph(start.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let shown = |range: Range<u32>| -> Result<String, EditError> {
            Ok(text.slice(range)?.project()?.text().text().to_owned())
        };
        if start != end {
            let mut range = start.offset..end.offset;
            // Links the selection touches are relinked whole.
            for link in links(text, start.paragraph) {
                let whole = link.range();
                if whole.start < range.end && range.start < whole.end {
                    range = range.start.min(whole.start)..range.end.max(whole.end);
                }
            }
            let address = self
                .link_at(start)
                .filter(|link| link.range().start <= range.start && range.end <= link.label.end)
                .map(|link| link.target)
                .unwrap_or_default();
            return Ok((start.paragraph, range.clone(), shown(range)?, address));
        }
        if let Some(link) = self.link_at(start) {
            let label = shown(link.label.clone())?;
            return Ok((start.paragraph, link.range(), label, link.target));
        }
        // The word at the caret, within plain visible text.
        let byte = text.byte_offset(start.offset)?;
        let plain = |at: usize, c: char| {
            !c.is_whitespace()
                && text
                    .utf16_offset(at)
                    .ok()
                    .and_then(|at| format_after(text, at))
                    .is_some_and(|format| {
                        ![format.hidden, format.hyperlink, format.math].contains(&Some(true))
                    })
        };
        let from = text.text()[..byte]
            .char_indices()
            .rev()
            .take_while(|&(at, c)| plain(at, c))
            .last()
            .map_or(byte, |(at, _)| at);
        let to = text.text()[byte..]
            .char_indices()
            .take_while(|&(at, c)| plain(byte + at, c))
            .last()
            .map_or(byte, |(at, c)| byte + at + c.len_utf8());
        let range = text.utf16_offset(from)?..text.utf16_offset(to)?;
        Ok((start.paragraph, range.clone(), shown(range)?, String::new()))
    }

    /// The text and address the Link dialog opens with.
    pub fn link_prefill(&self) -> (String, String) {
        self.link_range()
            .map(|(_, _, text, address)| (text, address))
            .unwrap_or_default()
    }

    /// OK in the Link dialog: what [`Self::link_prefill`] picked becomes `text`, or the
    /// address where `text` is empty, linked to `address`, and the caret follows it.
    pub fn set_link(
        &mut self,
        engine: &mut TextEngine,
        text: &str,
        address: &str,
    ) -> Result<(), EditorError> {
        if address.trim().is_empty() {
            return Ok(());
        }
        let (paragraph, range, _, _) = self.link_range()?;
        let current = self
            .active_outline()
            .document
            .paragraph(paragraph)
            .ok_or(EditError::InvalidRange)?;
        let label: String = if text.is_empty() {
            address.trim()
        } else {
            text
        }
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
        let mut base = match format_after(current, range.start).filter(|_| !range.is_empty()) {
            Some(format) => format.clone(),
            None => self.typing_format(TextPosition {
                paragraph,
                offset: range.start,
            })?,
        };
        unlinked(&mut base);
        base.hidden = Some(false);
        let mut code = base.clone();
        code.hidden = Some(true);
        code.hyperlink = Some(true);
        code.hyperlink_label = Some(true);
        let mut shown = base;
        shown.hyperlink = Some(true);
        shown.hyperlink_label = Some(true);
        let replacement = Paragraph::from_runs([(field_code(address), code), (label, shown)]);
        let length = replacement.utf16_offset(replacement.text().len())?;
        let mut updated = current.clone();
        updated.apply(onestore::page::text::Edit {
            range: range.clone(),
            replacement,
        })?;
        let caret = TextPosition {
            paragraph,
            offset: range.start + length,
        };
        self.rewrite(engine, paragraph, updated, [caret; 2].into())
    }

    /// Remove Link: the link at the caret keeps its label as plain text.
    pub fn remove_link(&mut self, engine: &mut TextEngine) -> Result<bool, EditorError> {
        let [anchor, focus] = self.active_outline().selection.positions;
        let Some(link) = self.link_at(anchor.min(focus)) else {
            return Ok(false);
        };
        let text = self
            .active_outline()
            .document
            .paragraph(link.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let start = text.byte_offset(link.label.start)?;
        let end = text.byte_offset(link.label.end)?;
        let mut updated = restyle(text, start..end, unlinked);
        let mut label = link.label.clone();
        if let Some(code) = &link.code {
            updated.apply(onestore::page::text::Edit {
                range: code.clone(),
                replacement: Paragraph::new(String::new(), Format::default()),
            })?;
            label = label.start - code.len() as u32..label.end - code.len() as u32;
        }
        let at = |offset| TextPosition {
            paragraph: link.paragraph,
            offset,
        };
        self.rewrite(
            engine,
            link.paragraph,
            updated,
            [at(label.start), at(label.end)].into(),
        )?;
        Ok(true)
    }

    /// Select Link: the label of the link at the caret.
    pub fn select_link(&mut self) -> Result<bool, EditError> {
        let [anchor, focus] = self.active_outline().selection.positions;
        let Some(link) = self.link_at(anchor.min(focus)) else {
            return Ok(false);
        };
        let at = |offset| TextPosition {
            paragraph: link.paragraph,
            offset,
        };
        self.select([at(link.label.start), at(link.label.end)].into())?;
        Ok(true)
    }

    /// Links the URL typed before `end` once a space or Enter ends it, as its own undo step.
    pub(super) fn link_typed_url(
        &mut self,
        engine: &mut TextEngine,
        end: TextPosition,
    ) -> Result<(), EditorError> {
        let Some(text) = self.active_outline().document.paragraph(end.paragraph) else {
            return Ok(());
        };
        let byte = text.byte_offset(end.offset)?;
        let from = text.text()[..byte]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| !c.is_whitespace())
            .last()
            .map_or(byte, |(at, _)| at);
        let Some(url) = typed_url(&text.text()[from..byte]) else {
            return Ok(());
        };
        let url = from + url.start..from + url.end;
        // Only plain text links; a link, field code or equation there stays as it is.
        let mut offset = text.utf16_offset(url.start)?;
        for c in text.text()[url.clone()].chars() {
            let format = format_after(text, offset).ok_or(EditError::InvalidRange)?;
            if [format.hyperlink, format.hidden, format.math].contains(&Some(true)) {
                return Ok(());
            }
            offset += c.len_utf16() as u32;
        }
        let updated = restyle(text, url, |format| format.hyperlink = Some(true));
        let selection = self.active_outline().selection;
        self.rewrite(engine, end.paragraph, updated, selection)
    }

    /// Moves a caret between a field code and its label to before the code, so what is
    /// typed there stays outside the link.
    pub(super) fn leave_link_code(&mut self) -> Result<(), EditError> {
        let selection = self.active_outline().selection;
        let [anchor, focus] = selection.positions;
        if anchor != focus {
            return Ok(());
        }
        let Some(link) = self.link_at(focus) else {
            return Ok(());
        };
        if let Some(code) = link.code.filter(|code| code.end == focus.offset) {
            let at = TextPosition {
                paragraph: focus.paragraph,
                offset: code.start,
            };
            self.active_outline_mut().selection = [at; 2].into();
        }
        Ok(())
    }

    /// Replaces the text of paragraph `paragraph` of the active outline as one undo step
    /// ending at `selection`.
    pub(super) fn rewrite(
        &mut self,
        engine: &mut TextEngine,
        paragraph: usize,
        text: Paragraph,
        selection: Selection,
    ) -> Result<(), EditorError> {
        let document = &self.active_outline().document;
        let at = TextPosition {
            paragraph,
            offset: 0,
        };
        let (container, range, [(id, _), _]) = selected(document, [at; 2].into())?;
        let mut replacement = document.container(container)?[range.clone()].to_vec();
        leaves_mut(&mut replacement, &mut |node| {
            if node.id == id {
                node.text_mut().unwrap().text = text.clone();
            }
        });
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range,
                replacement,
            },
            selection,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::page::PageObject;

    /// A paragraph's text as runs of (text, hyperlink, label, hidden), runs with the same
    /// flags merged.
    fn flags(text: &Paragraph) -> Vec<(String, bool, bool, bool)> {
        let mut runs: Vec<(String, bool, bool, bool)> = Vec::new();
        let mut start = 0;
        for span in text.spans() {
            let format = &span.format;
            let run = (
                text.text()[start..span.end].to_owned(),
                format.hyperlink == Some(true),
                format.hyperlink_label == Some(true),
                format.hidden == Some(true),
            );
            start = span.end;
            match runs.last_mut() {
                Some(last) if (last.1, last.2, last.3) == (run.1, run.2, run.3) => {
                    last.0.push_str(&run.0);
                }
                _ => runs.push(run),
            }
        }
        runs
    }

    fn native_paragraphs() -> Vec<Paragraph> {
        let bytes = include_bytes!("../../../../corpus/link-edit/native-typed/notebook/links.one");
        let store = onestore::Store::parse(bytes).unwrap();
        let index = onestore::RevisionIndex::parse(&store).unwrap();
        let document = onestore::document::Document::parse(&index).unwrap();
        let (space, _) = document.pages().unwrap()[0];
        let page = Page::from_space(&document, space).unwrap();
        page.objects
            .iter()
            .filter_map(|object| match object {
                PageObject::Outline(outline) if !outline.title => Some(outline),
                _ => None,
            })
            .flat_map(|outline| &outline.paragraphs)
            .filter_map(|paragraph| paragraph.text().map(|text| text.text.clone()))
            .skip(1)
            .collect()
    }

    /// Types `text`, each space on its own as a key does.
    fn typed(editor: &mut CanvasEditor, engine: &mut TextEngine, text: &str) {
        for (index, word) in text.split(' ').enumerate() {
            if index > 0 {
                editor.insert(engine, " ").unwrap();
            }
            if !word.is_empty() {
                editor.insert(engine, word).unwrap();
            }
        }
    }

    fn at(paragraph: usize, offset: u32) -> Selection {
        [TextPosition { paragraph, offset }; 2].into()
    }

    /// The editor replays what `tools/native_links.py` typed into OneNote 2010 and stores
    /// the same links: typed URLs, the Link dialog on a word, on a selection and on nothing,
    /// typing after a link, and Remove Link.
    #[test]
    fn link_editing_stores_what_onenote_stored() {
        let native = native_paragraphs();
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]).unwrap(),
            400.0,
        )
        .unwrap();
        let engine = &mut engine;
        for url in [
            "www.example.com",
            "http://example.org/a?b=1",
            "https://a.example/p.",
            "(http://b.example/q)",
            "ftp://c.example/f",
            "mailto:me@example.com",
            "me@example.com",
            "example.com",
            "onenote:#Page&section-id={x}",
            "file://c:/windows",
            "\\\\server\\share\\f",
            "news:comp.lang",
            "www.d.example,",
            "HTTP://E.EXAMPLE/UP",
            "https://f.example/a_b-c~d%20e#frag",
        ] {
            // OneNote capitalized the first letter of each sentence as it was typed.
            let after = if url.ends_with('.') { "Z" } else { "z" };
            typed(&mut editor, engine, &format!("T {url} {after}"));
            editor.enter(engine, false).unwrap();
        }
        typed(&mut editor, engine, "See http://g.example/end");
        editor.enter(engine, false).unwrap();
        typed(&mut editor, engine, "Zplain words here");
        let paragraph = editor.selection().positions[0].paragraph;
        editor.select(at(paragraph, 13)).unwrap();
        assert_eq!(editor.link_prefill(), ("here".into(), String::new()));
        editor.set_link(engine, "here", "example.net/x").unwrap();
        typed(&mut editor, engine, "Q");
        editor.enter(engine, false).unwrap();
        typed(&mut editor, engine, "Select me please");
        let paragraph = editor.selection().positions[0].paragraph;
        editor
            .select(
                [
                    TextPosition {
                        paragraph,
                        offset: 7,
                    },
                    TextPosition {
                        paragraph,
                        offset: 16,
                    },
                ]
                .into(),
            )
            .unwrap();
        assert_eq!(editor.link_prefill(), ("me please".into(), String::new()));
        editor
            .set_link(engine, "me please", "https://example.com/selected")
            .unwrap();
        editor.enter(engine, false).unwrap();
        assert_eq!(editor.link_prefill(), (String::new(), String::new()));
        editor
            .set_link(engine, "", "https://example.com/only")
            .unwrap();
        typed(&mut editor, engine, " tail");
        editor.enter(engine, false).unwrap();
        typed(&mut editor, engine, "Gone ftp://h.example/f z");
        let paragraph = editor.selection().positions[0].paragraph;
        editor.select(at(paragraph, 8)).unwrap();
        assert!(editor.remove_link(engine).unwrap());
        let written: Vec<_> = editor
            .active_outline()
            .document
            .text_nodes()
            .map(|node| flags(&node.text().unwrap().text))
            .collect();
        let expected: Vec<_> = native.iter().map(flags).collect();
        assert_eq!(written, expected);
    }

    /// Every URL OneNote 2010 linked or left as typed in `corpus/link-edit/native-typed`.
    #[test]
    fn typed_urls_link_as_onenote_links_them() {
        for (word, linked) in [
            ("www.example.com", Some("www.example.com")),
            ("http://example.org/a?b=1", Some("http://example.org/a?b=1")),
            ("https://a.example/p.", Some("https://a.example/p")),
            ("(http://b.example/q)", Some("http://b.example/q")),
            ("ftp://c.example/f", Some("ftp://c.example/f")),
            ("mailto:me@example.com", Some("mailto:me@example.com")),
            ("me@example.com", None),
            ("example.com", None),
            (
                "onenote:#Page&section-id={x}",
                Some("onenote:#Page&section-id={x}"),
            ),
            ("file://c:/windows", Some("file://c:/windows")),
            ("\\\\server\\share\\f", Some("\\\\server\\share\\f")),
            ("news:comp.lang", Some("news:comp.lang")),
            ("www.d.example,", Some("www.d.example")),
            ("HTTP://E.EXAMPLE/UP", Some("HTTP://E.EXAMPLE/UP")),
            (
                "https://f.example/a_b-c~d%20e#frag",
                Some("https://f.example/a_b-c~d%20e#frag"),
            ),
            ("wwWw.example.com", None),
            ("http://", None),
        ] {
            assert_eq!(typed_url(word).map(|range| &word[range]), linked, "{word}");
        }
    }
}
