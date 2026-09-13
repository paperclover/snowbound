use crate::{ExGuid, document::Format};
use std::{fmt, ops::Range};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Span {
    /// Exclusive UTF-8 boundary; the start is the preceding span's end.
    pub end: usize,
    pub format: Format,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
/// Editable text styles are coalesced independently of serialized run boundaries.
pub struct Paragraph {
    text: String,
    spans: Vec<Span>,
}

/// Which side of a hidden field a visible boundary maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Affinity {
    Upstream,
    Downstream,
}

#[derive(Clone)]
pub struct TextProjection {
    text: Paragraph,
    spans: Vec<ProjectedSpan>,
    source_len: u32,
}

#[derive(Clone)]
struct ProjectedSpan {
    visible: Range<u32>,
    source_start: u32,
}

impl TextProjection {
    pub fn text(&self) -> &Paragraph {
        &self.text
    }

    /// Maps visible UTF-8 boundaries to downstream source UTF-16 positions in one pass.
    pub fn source_boundaries(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        let mut spans = self.spans.iter().peekable();
        let mut visible = 0;
        self.text
            .text
            .char_indices()
            .map(move |(byte, character)| {
                while spans.peek().is_some_and(|span| span.visible.end <= visible) {
                    spans.next();
                }
                let span = spans.peek().unwrap();
                let source = span.source_start + (visible - span.visible.start);
                visible += character.len_utf16() as u32;
                (byte, source)
            })
            .chain(std::iter::once((self.text.text.len(), self.source_len)))
    }

    pub fn source_offset(&self, visible: u32, affinity: Affinity) -> Result<u32, EditError> {
        self.text.byte_offset(visible)?;
        Ok(match affinity {
            Affinity::Downstream => self
                .spans
                .iter()
                .find(|span| span.visible.end > visible)
                .map(|span| span.source_start + (visible - span.visible.start))
                .unwrap_or(self.source_len),
            Affinity::Upstream => self
                .spans
                .iter()
                .rev()
                .find(|span| span.visible.start < visible)
                .map(|span| span.source_start + (visible - span.visible.start))
                .unwrap_or(0),
        })
    }

    /// Hidden source positions collapse to their visible boundary.
    pub fn visible_offset(&self, source: u32) -> Result<u32, EditError> {
        if source > self.source_len {
            return Err(EditError::InvalidRange);
        }
        for span in &self.spans {
            if source < span.source_start {
                return Ok(span.visible.start);
            }
            let length = span.visible.end - span.visible.start;
            if source < span.source_start + length {
                let visible = span.visible.start + source - span.source_start;
                self.text.byte_offset(visible)?;
                return Ok(visible);
            }
        }
        self.text.utf16_offset(self.text.text.len())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub range: Range<u32>,
    pub replacement: Paragraph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    InvalidRange,
    TextTooLong,
    InvalidStructure,
    UnsupportedContent,
    /// The system random source failed while allocating a new identity.
    Identity,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Identity => "System random source failed",
            Self::UnsupportedContent => {
                "This paragraph contains content the text editor cannot edit"
            }
            Self::InvalidRange => "Text range is outside the paragraph or splits a surrogate pair",
            Self::TextTooLong => "Text exceeds the UTF-16 offset range",
            Self::InvalidStructure => {
                "This outline has duplicate objects or broken paragraph links"
            }
        })
    }
}

impl std::error::Error for EditError {}

impl From<EditError> for crate::Error {
    fn from(error: EditError) -> Self {
        Self {
            offset: 0,
            message: match error {
                EditError::InvalidRange => {
                    "Text range is outside the paragraph or splits a surrogate pair"
                }
                EditError::TextTooLong => "Text exceeds the UTF-16 offset range",
                EditError::InvalidStructure => {
                    "This outline has duplicate objects or broken paragraph links"
                }
                EditError::UnsupportedContent => {
                    "This paragraph contains content the text editor cannot edit"
                }
                EditError::Identity => "System random source failed",
            },
        }
    }
}

/// A fresh random identity for a new paragraph or text object.
/// A fresh identity with `n` 1: OneNote never stores an object as `{guid},0`, and OneNote
/// 2010 loses outline elements stored that way (corpus/math-edit/native-drop).
pub fn new_id() -> Result<ExGuid, EditError> {
    Ok(ExGuid {
        guid: crate::write::fresh_guid().map_err(|_| EditError::Identity)?,
        n: 1,
    })
}

impl Paragraph {
    pub fn project(&self) -> Result<TextProjection, EditError> {
        let mut runs = Vec::new();
        let mut spans: Vec<ProjectedSpan> = Vec::new();
        let mut byte = 0;
        let mut source = 0_u32;
        let mut visible = 0_u32;
        for span in &self.spans {
            let fragment = &self.text[byte..span.end];
            let length: u32 = fragment
                .encode_utf16()
                .count()
                .try_into()
                .map_err(|_| EditError::TextTooLong)?;
            let source_end = source.checked_add(length).ok_or(EditError::TextTooLong)?;
            if span.format.hidden != Some(true) {
                runs.push((fragment.replace('\u{000b}', "\n"), span.format.clone()));
                if length > 0 {
                    if let Some(last) = spans.last_mut()
                        && last.source_start + (last.visible.end - last.visible.start) == source
                    {
                        last.visible.end += length;
                    } else {
                        spans.push(ProjectedSpan {
                            visible: visible..visible + length,
                            source_start: source,
                        });
                    }
                    visible += length;
                }
            }
            byte = span.end;
            source = source_end;
        }
        if runs.is_empty() {
            let mut format = self.spans[0].format.clone();
            format.hidden = Some(false);
            runs.push((String::new(), format));
        }
        Ok(TextProjection {
            text: Self::from_runs(runs),
            spans,
            source_len: source,
        })
    }

    pub fn new(text: String, format: Format) -> Self {
        let end = text.len();
        Self {
            text,
            spans: vec![Span { end, format }],
        }
    }

    pub fn from_runs(runs: impl IntoIterator<Item = (String, Format)>) -> Self {
        let mut text = String::new();
        let mut spans: Vec<Span> = Vec::new();
        for (fragment, format) in runs {
            text.push_str(&fragment);
            if let Some(last) = spans.last_mut() {
                if last.format == format {
                    last.end = text.len();
                    continue;
                }
                if fragment.is_empty() && !text.is_empty() {
                    continue;
                }
                if last.end == 0 {
                    spans.clear();
                }
            }
            spans.push(Span {
                end: text.len(),
                format,
            });
        }
        if spans.is_empty() {
            spans.push(Span {
                end: 0,
                format: Format::default(),
            });
        }
        Self { text, spans }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn byte_offset(&self, utf16: u32) -> Result<usize, EditError> {
        let mut units = 0_u64;
        for (byte, character) in self.text.char_indices() {
            if units == u64::from(utf16) {
                return Ok(byte);
            }
            units += character.len_utf16() as u64;
            if units > u64::from(utf16) {
                return Err(EditError::InvalidRange);
            }
        }
        if units == u64::from(utf16) {
            Ok(self.text.len())
        } else {
            Err(EditError::InvalidRange)
        }
    }

    pub fn utf16_offset(&self, byte: usize) -> Result<u32, EditError> {
        self.text
            .get(..byte)
            .ok_or(EditError::InvalidRange)?
            .encode_utf16()
            .count()
            .try_into()
            .map_err(|_| EditError::TextTooLong)
    }

    pub fn format_at(&self, utf16: u32) -> Result<&Format, EditError> {
        let byte = self.byte_offset(utf16)?;
        let index = self.spans.partition_point(|span| span.end < byte);
        Ok(&self.spans[index].format)
    }

    pub fn slice(&self, range: Range<u32>) -> Result<Self, EditError> {
        if range.start > range.end {
            return Err(EditError::InvalidRange);
        }
        let start = self.byte_offset(range.start)?;
        let end = self.byte_offset(range.end)?;
        if start == end {
            return Ok(Self::new(
                String::new(),
                self.format_at(range.start)?.clone(),
            ));
        }
        let mut previous = 0;
        let mut spans = Vec::new();
        for span in &self.spans {
            if previous < end && span.end > start {
                spans.push(Span {
                    end: span.end.min(end) - start,
                    format: span.format.clone(),
                });
            }
            previous = span.end;
        }
        Ok(Self {
            text: self.text[start..end].to_owned(),
            spans,
        })
    }

    /// Returns the inverse edit; applying that inverse returns a redo operation.
    pub fn apply(&mut self, edit: Edit) -> Result<Edit, EditError> {
        let end = self.utf16_offset(self.text.len())?;
        let removed = self.slice(edit.range.clone())?;
        let inserted = edit.replacement.utf16_offset(edit.replacement.text.len())?;
        let new_end = edit
            .range
            .start
            .checked_add(inserted)
            .ok_or(EditError::TextTooLong)?;
        let resulting_len = end
            .checked_sub(edit.range.end - edit.range.start)
            .and_then(|n| n.checked_add(inserted))
            .ok_or(EditError::TextTooLong)?;
        if resulting_len == 0 {
            *self = edit.replacement;
            return Ok(Edit {
                range: edit.range.start..new_end,
                replacement: removed,
            });
        }
        let prefix = self.slice(0..edit.range.start)?;
        let suffix = self.slice(edit.range.end..end)?;
        let mut runs = Vec::new();
        for part in [prefix, edit.replacement, suffix] {
            let mut start = 0;
            for span in part.spans {
                if span.end > start {
                    runs.push((part.text[start..span.end].to_owned(), span.format));
                }
                start = span.end;
            }
        }
        *self = Self::from_runs(runs);
        Ok(Edit {
            range: edit.range.start..new_end,
            replacement: removed,
        })
    }

    pub fn append(&mut self, other: Self) -> Result<Edit, EditError> {
        let end = self.utf16_offset(self.text.len())?;
        self.apply(Edit {
            range: end..end,
            replacement: other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regular() -> Format {
        Format {
            font: Some("Arial".into()),
            font_size: Some(11.0),
            ..Format::default()
        }
    }

    fn bold() -> Format {
        Format {
            bold: Some(true),
            ..regular()
        }
    }
    #[test]
    fn soft_breaks_keep_source_offsets_and_formatting() {
        let source = Paragraph::from_runs([("A\u{000b}".into(), regular()), ("🌳".into(), bold())]);
        let projection = source.project().unwrap();
        assert_eq!(source.text(), "A\u{000b}🌳");
        assert_eq!(projection.text().text(), "A\n🌳");
        assert_eq!(projection.text().spans(), source.spans());
        assert_eq!(
            projection.source_boundaries().collect::<Vec<_>>(),
            [(0, 0), (1, 1), (2, 2), (6, 4)]
        );
        for offset in [0, 1, 2, 4] {
            assert_eq!(projection.visible_offset(offset), Ok(offset));
            assert_eq!(
                projection.source_offset(offset, Affinity::Downstream),
                Ok(offset)
            );
        }
        assert!(projection.visible_offset(3).is_err());
    }

    #[test]
    fn hidden_fields_keep_source_offsets_at_both_sides_of_a_gap() {
        let hidden = Format {
            hidden: Some(true),
            ..regular()
        };
        let source = Paragraph::from_runs([
            ("A".into(), regular()),
            ("🌳".into(), hidden.clone()),
            ("e\u{301}".into(), bold()),
            ("X".into(), hidden),
        ]);
        let original = source.clone();
        let projection = source.project().unwrap();
        assert_eq!(projection.text().text(), "Ae\u{301}");
        assert_eq!(
            projection.source_boundaries().collect::<Vec<_>>(),
            [(0, 0), (1, 3), (2, 4), (4, 6)]
        );
        for (visible, upstream, downstream) in [(0, 0, 0), (1, 1, 3), (2, 4, 4), (3, 5, 6)] {
            assert_eq!(
                projection.source_offset(visible, Affinity::Upstream),
                Ok(upstream)
            );
            assert_eq!(
                projection.source_offset(visible, Affinity::Downstream),
                Ok(downstream)
            );
        }
        for (source, visible) in [(0, 0), (1, 1), (2, 1), (3, 1), (4, 2), (5, 3), (6, 3)] {
            assert_eq!(projection.visible_offset(source), Ok(visible));
        }
        assert_eq!(source, original);
    }

    #[test]
    fn projection_validates_visible_surrogates_and_preserves_empty_field_boundaries() {
        let visible = Paragraph::new("a🌳z".into(), regular()).project().unwrap();
        assert_eq!(
            visible.source_offset(2, Affinity::Downstream),
            Err(EditError::InvalidRange)
        );
        assert_eq!(visible.visible_offset(2), Err(EditError::InvalidRange));
        let hidden = Paragraph::new(
            "🌳".into(),
            Format {
                hidden: Some(true),
                ..regular()
            },
        )
        .project()
        .unwrap();
        assert!(hidden.text().text().is_empty());
        assert_eq!(hidden.source_boundaries().collect::<Vec<_>>(), [(0, 2)]);
        assert_eq!(
            visible.source_boundaries().collect::<Vec<_>>(),
            [(0, 0), (1, 1), (5, 3), (6, 4)]
        );
        assert_eq!(hidden.source_offset(0, Affinity::Upstream), Ok(0));
        assert_eq!(hidden.source_offset(0, Affinity::Downstream), Ok(2));
        assert_eq!(hidden.visible_offset(1), Ok(0));
        assert_eq!(hidden.visible_offset(3), Err(EditError::InvalidRange));
    }

    #[test]
    fn offsets_distinguish_bytes_utf16_and_scalars() {
        let text = Paragraph::new("a🌳e\u{301}".into(), regular());
        for (utf16, byte) in [(0, 0), (1, 1), (3, 5), (4, 6), (5, 8)] {
            assert_eq!(text.byte_offset(utf16), Ok(byte));
            assert_eq!(text.utf16_offset(byte), Ok(utf16));
        }
        assert_eq!(text.byte_offset(2), Err(EditError::InvalidRange));
        assert_eq!(text.utf16_offset(2), Err(EditError::InvalidRange));
        assert_eq!(text.byte_offset(6), Err(EditError::InvalidRange));
        assert_eq!(text.utf16_offset(9), Err(EditError::InvalidRange));
    }

    #[test]
    fn replacement_preserves_styles_outside_selection() {
        let mut text = Paragraph::from_runs([
            ("plain ".into(), regular()),
            ("bold".into(), bold()),
            (" end".into(), regular()),
        ]);
        let original = text.clone();
        let edit = Edit {
            range: 4..8,
            replacement: Paragraph::new("🌳".into(), bold()),
        };
        let undo = text.apply(edit).unwrap();
        assert_eq!(text.text(), "plai🌳ld end");
        assert_eq!(
            text.spans()
                .iter()
                .map(|s| (s.end, s.format.bold))
                .collect::<Vec<_>>(),
            [(4, None), (10, Some(true)), (14, None)]
        );
        let after = text.clone();
        let redo = text.apply(undo).unwrap();
        assert_eq!(text, original);
        text.apply(redo).unwrap();
        assert_eq!(text, after);
    }

    #[test]
    fn invalid_edits_leave_text_and_styles_unchanged() {
        let original = Paragraph::new("a🌳b".into(), regular());
        for (start, end) in [(2, 3), (0, 2), (3, 2), (0, 5)] {
            let mut text = original.clone();
            assert_eq!(
                text.apply(Edit {
                    range: start..end,
                    replacement: Paragraph::new("x".into(), bold())
                }),
                Err(EditError::InvalidRange)
            );
            assert_eq!(text, original);
        }
    }

    #[test]
    fn undo_restores_empty_paragraph_format() {
        let mut text = Paragraph::new(String::new(), regular());
        let original = text.clone();
        let undo = text
            .apply(Edit {
                range: 0..0,
                replacement: Paragraph::new("bold".into(), bold()),
            })
            .unwrap();
        text.apply(undo).unwrap();
        assert_eq!(text, original);
    }

    #[test]
    fn every_scalar_range_round_trips_styled_unicode_edits() {
        let original = Paragraph::from_runs([
            ("a🌳".into(), regular()),
            ("e\u{301}שלום".into(), bold()),
            ("Z".into(), regular()),
        ]);
        let boundaries: Vec<_> = original
            .text()
            .char_indices()
            .map(|(n, _)| n)
            .chain([original.text().len()])
            .collect();
        for &start in &boundaries {
            for &end in boundaries.iter().filter(|&&end| end >= start) {
                let range =
                    original.utf16_offset(start).unwrap()..original.utf16_offset(end).unwrap();
                for replacement in ["", "👩‍👩‍👧‍👦", "xyz", "\u{301}"] {
                    let mut edited = original.clone();
                    let undo = edited
                        .apply(Edit {
                            range: range.clone(),
                            replacement: Paragraph::new(replacement.into(), bold()),
                        })
                        .unwrap();
                    assert_eq!(
                        edited.text(),
                        format!(
                            "{}{replacement}{}",
                            &original.text()[..start],
                            &original.text()[end..]
                        )
                    );
                    let after = edited.clone();
                    let redo = edited.apply(undo).unwrap();
                    assert_eq!(edited, original);
                    edited.apply(redo).unwrap();
                    assert_eq!(edited, after);
                }
            }
            let at = original.utf16_offset(start).unwrap();
            let mut left = original.slice(0..at).unwrap();
            let right = original
                .slice(at..original.utf16_offset(original.text.len()).unwrap())
                .unwrap();
            left.append(right).unwrap();
            assert_eq!(left, original);
        }
    }
}
