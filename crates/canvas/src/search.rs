//! Search as OneNote 2010 searches: each query word matches the start of a word, ignoring
//! case and diacritics; a page matches when all its words appear in its title or text, and
//! pages whose titles hold every word come first, then the most recently modified.

use crate::document::TextPosition;
use crate::editor::{CanvasEditor, Selection};
use icu_normalizer::DecomposingNormalizerBorrowed;
use onestore::ExGuid;
use onestore::document::Kind;
use onestore::page::{Page, PageObject, PageParagraph, ParagraphContent, text::Paragraph};
use std::ops::Range;

/// Characters around a hit a snippet shows before it, and in all.
const BEFORE: usize = 24;
const SNIPPET: usize = 90;

/// A query's words, lowered and stripped of diacritics; a quoted phrase is one word.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    terms: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Self {
        let mut terms = Vec::new();
        for (index, part) in text.split('"').enumerate() {
            if index % 2 == 1 {
                terms.push(fold(part).split_whitespace().collect::<Vec<_>>().join(" "));
            } else {
                terms.extend(fold(part).split_whitespace().map(str::to_owned));
            }
        }
        terms.retain(|term| !term.is_empty());
        Self { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Where the words start words of `folded`, as `fold` leaves text, in order and merged
    /// where they overlap.
    fn hits(&self, folded: &str) -> Vec<Range<usize>> {
        let mut hits: Vec<Range<usize>> = self
            .terms
            .iter()
            .flat_map(|term| {
                folded
                    .match_indices(term.as_str())
                    .filter(|(at, _)| word_start(folded, *at))
                    .map(|(at, term)| at..at + term.len())
            })
            .collect();
        hits.sort_by_key(|hit| (hit.start, std::cmp::Reverse(hit.end)));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(hits.len());
        for hit in hits {
            match merged.last_mut() {
                Some(last) if hit.start <= last.end => last.end = last.end.max(hit.end),
                _ => merged.push(hit),
            }
        }
        merged
    }

    fn all_in(&self, folded: &str) -> bool {
        self.terms.iter().all(|term| starts_word(term, folded))
    }

    /// Where the query's words start words of `text`, as byte ranges of it.
    pub fn find(&self, text: &str) -> Vec<Range<usize>> {
        let (folded, source) = fold_mapped(text);
        self.hits(&folded)
            .into_iter()
            .map(|hit| source_range(text, &source, hit))
            .collect()
    }
}

/// Ideographs and kana are words of their own, as their text has no spaces between words.
fn ideographic(character: char) -> bool {
    matches!(u32::from(character),
        0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x3ffff)
}

fn starts_word(term: &str, folded: &str) -> bool {
    folded
        .match_indices(term)
        .any(|(at, _)| word_start(folded, at))
}

fn word_start(text: &str, at: usize) -> bool {
    match text[..at].chars().next_back() {
        None => true,
        Some(before) => {
            !before.is_alphanumeric()
                || ideographic(before)
                || text[at..].chars().next().is_some_and(ideographic)
        }
    }
}

fn diacritic(character: char) -> bool {
    matches!(u32::from(character),
        0x300..=0x36f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f)
}

/// Appends `character` lowered and without diacritics; line breaks stay and other spaces
/// become one space each.
fn fold_char(character: char, out: &mut String) {
    const DECOMPOSE: DecomposingNormalizerBorrowed = DecomposingNormalizerBorrowed::new_nfd();
    if character.is_ascii() {
        out.push(match character {
            '\n' => '\n',
            space if space.is_whitespace() => ' ',
            other => other.to_ascii_lowercase(),
        });
        return;
    }
    match character {
        '\u{2018}' | '\u{2019}' | '\u{02bc}' => out.push('\''),
        '\u{201c}' | '\u{201d}' => out.push('"'),
        'ß' => out.push_str("ss"),
        'æ' | 'Æ' => out.push_str("ae"),
        'œ' | 'Œ' => out.push_str("oe"),
        'ø' | 'Ø' => out.push('o'),
        'ł' | 'Ł' => out.push('l'),
        'đ' | 'Đ' => out.push('d'),
        'ı' => out.push('i'),
        space if space.is_whitespace() => out.push(' '),
        other => out.extend(
            DECOMPOSE
                .normalize_iter(other.to_lowercase())
                .filter(|part| !diacritic(*part)),
        ),
    }
}

/// `text` as search compares it: lowered and without diacritics.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        fold_char(character, &mut out);
    }
    out
}

/// `fold`, with the byte of `text` each folded byte came from.
fn fold_mapped(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut source = Vec::with_capacity(text.len());
    for (at, character) in text.char_indices() {
        fold_char(character, &mut out);
        source.resize(out.len(), at);
    }
    (out, source)
}

/// The bytes of `text` a range of its folding came from, whole characters.
fn source_range(text: &str, source: &[usize], hit: Range<usize>) -> Range<usize> {
    let start = source[hit.start];
    let last = source[hit.end - 1];
    let end = last + text[last..].chars().next().map_or(0, char::len_utf8);
    start..end
}

/// The text a paragraph shows, without hidden field codes.
pub fn shown(paragraph: &Paragraph) -> String {
    let mut start = 0;
    paragraph
        .spans()
        .iter()
        .filter_map(|span| {
            let run = &paragraph.text()[start..span.end];
            start = span.end;
            (span.format.hidden != Some(true)).then_some(run)
        })
        .collect()
}

/// Calls `visit` with each text paragraph of the page's outlines, in page order, tables
/// cell by cell.
fn text_paragraphs<'a>(page: &'a Page, mut visit: impl FnMut(&'a PageParagraph, &'a Paragraph)) {
    fn walk<'a>(
        paragraphs: &'a [PageParagraph],
        visit: &mut impl FnMut(&'a PageParagraph, &'a Paragraph),
    ) {
        for paragraph in paragraphs {
            match &paragraph.content {
                ParagraphContent::Text(text) => visit(paragraph, &text.text),
                ParagraphContent::Table(table) => {
                    for cell in table.rows.iter().flat_map(|row| &row.cells) {
                        walk(&cell.paragraphs, visit);
                    }
                }
                _ => {}
            }
        }
    }
    for object in &page.objects {
        if let PageObject::Outline(outline) = object {
            walk(&outline.paragraphs, &mut visit);
        }
    }
}

/// A page's text outside its title, a paragraph to a line.
pub fn page_text(page: &Page) -> String {
    let mut out = Vec::new();
    text_paragraphs(page, |_, text| out.push(shown(text)));
    out.retain(|line| !line.trim().is_empty());
    out.join("\n")
}

/// A tagged paragraph, as OneNote's Tags Summary lists it: once for each of its tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Tagged {
    pub section: String,
    pub space: ExGuid,
    /// The page's title.
    pub title: String,
    pub paragraph: ExGuid,
    /// The tag's name, as its definition stores it.
    pub name: String,
    /// The tag's symbol, as its definition stores it.
    pub shape: u16,
    /// A check box tag is checked.
    pub checked: bool,
    pub text: String,
    /// When the tag was applied, in Time32, or the page's modification time where unknown.
    pub created: u64,
}

/// The page's tagged paragraphs, oldest tag first on each, as OneNote lists them; a tag
/// without a definition, as an Outlook task's, is left out.
fn tagged(section: &str, space: ExGuid, page: &Page, modified: u64) -> Vec<Tagged> {
    let mut out = Vec::new();
    text_paragraphs(page, |paragraph, text| {
        let ParagraphContent::Text(object) = &paragraph.content else {
            return;
        };
        // Stored newest first.
        for tag in paragraph.tags.iter().chain(&object.tags).rev() {
            let Some(Kind::TagDefinition {
                label: Some(name),
                shape,
                ..
            }) = tag
                .definition
                .and_then(|id| page.definitions.get(&id))
                .map(|definition| &definition.kind)
            else {
                continue;
            };
            out.push(Tagged {
                section: section.to_owned(),
                space,
                title: page.title.clone(),
                paragraph: paragraph.id,
                name: name.to_string(),
                shape: shape.unwrap_or(0),
                checked: tag.status & 1 != 0,
                text: shown(text),
                created: tag.created.map_or(modified, u64::from),
            });
        }
    });
    out
}

/// A page as search knows it.
pub struct Entry {
    /// The section's key, which the host chooses.
    pub section: String,
    pub space: ExGuid,
    pub title: String,
    /// When the page last changed, in any unit that orders.
    pub modified: u64,
    text: String,
    folded_title: String,
    folded_text: String,
    tagged: Vec<Tagged>,
}

impl Entry {
    /// Page `space` of `section` as `page` shows it.
    pub fn new(section: &str, space: ExGuid, page: &Page, modified: u64) -> Self {
        let text = page_text(page);
        Self {
            section: section.to_owned(),
            space,
            folded_title: fold(&page.title),
            folded_text: fold(&text),
            title: page.title.clone(),
            modified,
            text,
            tagged: tagged(section, space, page, modified),
        }
    }
}

/// A page matching a query.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub section: String,
    pub space: ExGuid,
    pub title: String,
    pub modified: u64,
    /// The page's place in the index, which keeps each section's pages in order.
    pub order: usize,
    /// Every word is in the title, as OneNote's "Title contains" lists it.
    pub in_title: bool,
    /// Byte ranges of the title the words match.
    pub title_hits: Vec<Range<usize>>,
    /// The line around the first match in the page's text, or its first line.
    pub snippet: String,
    pub snippet_hits: Vec<Range<usize>>,
}

/// Pages of any number of sections, searchable together.
#[derive(Default)]
pub struct Index {
    entries: Vec<Entry>,
}

impl Index {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every page, each section's in order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Adds `entry`, replacing the page it names.
    pub fn set(&mut self, entry: Entry) {
        match self
            .entries
            .iter_mut()
            .find(|old| old.space == entry.space && old.section == entry.section)
        {
            Some(old) => *old = entry,
            None => self.entries.push(entry),
        }
    }

    /// Keeps the pages whose section `keep` accepts.
    pub fn retain(&mut self, mut keep: impl FnMut(&Entry) -> bool) {
        self.entries.retain(|entry| keep(entry));
    }

    /// The page `space` of `section`, when indexed.
    pub fn get(&self, section: &str, space: ExGuid) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.space == space && entry.section == section)
    }

    /// Pages of the sections `scope` accepts that hold every word of `query`, title hits
    /// first, then the most recently modified.
    pub fn search(&self, query: &Query, scope: impl Fn(&str) -> bool) -> Vec<Found> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(usize, &Entry, bool)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| scope(&entry.section))
            .filter_map(|(order, entry)| {
                let in_title = query.all_in(&entry.folded_title);
                let matches = in_title
                    || query.terms.iter().all(|term| {
                        starts_word(term, &entry.folded_title)
                            || starts_word(term, &entry.folded_text)
                    });
                matches.then_some((order, entry, in_title))
            })
            .collect();
        found.sort_by_key(|(_, entry, in_title)| (!in_title, std::cmp::Reverse(entry.modified)));
        found
            .into_iter()
            .map(|(order, entry, in_title)| {
                let (snippet, snippet_hits) = snippet(entry, query);
                Found {
                    section: entry.section.clone(),
                    space: entry.space,
                    title: entry.title.clone(),
                    modified: entry.modified,
                    order,
                    in_title,
                    title_hits: query.find(&entry.title),
                    snippet,
                    snippet_hits,
                }
            })
            .collect()
    }

    /// The tagged paragraphs of the pages whose section and page `scope` accepts, in page
    /// order.
    pub fn tagged(&self, scope: impl Fn(&Entry) -> bool) -> Vec<Tagged> {
        self.entries
            .iter()
            .filter(|entry| scope(entry))
            .flat_map(|entry| entry.tagged.iter().cloned())
            .collect()
    }
}

/// Part of the line holding the first match in the entry's text, the matches within it;
/// the text's first line when nothing there matches.
fn snippet(entry: &Entry, query: &Query) -> (String, Vec<Range<usize>>) {
    let first = query.hits(&entry.folded_text).first().map(|hit| hit.start);
    let line = first.map_or(0, |at| {
        entry.folded_text[..at]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
    });
    let text = entry.text.split('\n').nth(line).unwrap_or_default();
    let hits = query.find(text);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = hits.first().map_or(0, |hit| {
        chars.partition_point(|(byte, _)| *byte < hit.start)
    });
    let mut start = at.saturating_sub(BEFORE);
    let mut end = (start + SNIPPET).min(chars.len());
    // Cut ends fall back to the nearest space, so no word shows in part.
    if start > 0
        && let Some(space) = chars[start..at]
            .iter()
            .position(|(_, char)| char.is_whitespace())
    {
        start += space + 1;
    }
    if end < chars.len()
        && let Some(space) = chars[at..end]
            .iter()
            .rposition(|(_, char)| char.is_whitespace())
    {
        end = at + space;
    }
    let byte = |index: usize| chars.get(index).map_or(text.len(), |(byte, _)| *byte);
    let [from, to] = [byte(start), byte(end)];
    let lead = if start > 0 { "…" } else { "" };
    let shown = format!(
        "{lead}{}{}",
        text[from..to].trim_end(),
        if end < chars.len() { "…" } else { "" }
    );
    let hits = hits
        .into_iter()
        .filter(|hit| hit.start >= from && hit.end <= to)
        .map(|hit| hit.start - from + lead.len()..hit.end - from + lead.len())
        .filter(|hit| hit.end <= shown.len())
        .collect();
    (shown, hits)
}

/// A match on a page: the outline it is in and its range there.
pub type PageMatch = (ExGuid, Selection);

/// Where `query` matches the page `editor` shows, from the page's top: outlines by where
/// they stand, then their shown paragraphs in order.
pub fn page_matches(editor: &CanvasEditor, query: &Query) -> Vec<PageMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut outlines: Vec<_> = editor.outlines().iter().collect();
    outlines.sort_by(|a, b| {
        let [ax, ay] = a.origin();
        let [bx, by] = b.origin();
        ay.total_cmp(&by).then(ax.total_cmp(&bx))
    });
    let mut matches = Vec::new();
    for outline in outlines {
        for (index, _) in outline.layouts() {
            let Some(paragraph) = outline.document().paragraph(index) else {
                continue;
            };
            // Hidden field codes are left out, and each shown byte keeps its source byte.
            let mut shown = String::new();
            let mut source = Vec::new();
            let mut start = 0;
            for span in paragraph.spans() {
                if span.format.hidden != Some(true) {
                    shown.push_str(&paragraph.text()[start..span.end]);
                    source.extend(start..span.end);
                }
                start = span.end;
            }
            for hit in query.find(&shown) {
                let end = source[hit.end - 1]
                    + paragraph.text()[source[hit.end - 1]..]
                        .chars()
                        .next()
                        .map_or(0, char::len_utf8);
                let (Ok(from), Ok(to)) = (
                    paragraph.utf16_offset(source[hit.start]),
                    paragraph.utf16_offset(end),
                ) else {
                    continue;
                };
                let at = |offset| TextPosition {
                    paragraph: index,
                    offset,
                };
                matches.push((outline.id, [at(from), at(to)].into()));
            }
        }
    }
    matches
}

/// The whole of paragraph `id` on the page `editor` shows, as a tag summary selects it.
pub fn paragraph_match(editor: &CanvasEditor, id: ExGuid) -> Option<PageMatch> {
    editor.outlines().iter().find_map(|outline| {
        let document = outline.document();
        let index = document.text_nodes().position(|node| node.id == id)?;
        let paragraph = document.paragraph(index)?;
        let end = paragraph.utf16_offset(paragraph.text().len()).ok()?;
        let at = |offset| TextPosition {
            paragraph: index,
            offset,
        };
        Some((outline.id, [at(0), at(end)].into()))
    })
}
