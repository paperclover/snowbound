use super::*;
use onestore::TextAttribute;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Span {
    end: u32,
    values: Vec<Value>,
}

/// A formatting intent and the text/attribute values observed before local publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatEdit {
    pub object: ExGuid,
    pub before: String,
    pub range: Range<u32>,
    pub attributes: Vec<TextAttribute>,
    observed: Vec<Span>,
}

fn fields(attributes: &[TextAttribute]) -> BTreeMap<&'static str, Value> {
    let mut fields = BTreeMap::new();
    for attribute in attributes {
        let (name, value) = match attribute {
            TextAttribute::Bold(v) => ("bold", json!(v)),
            TextAttribute::Italic(v) => ("italic", json!(v)),
            TextAttribute::Underline(v) => ("underline", json!(v)),
            TextAttribute::Strike(v) => ("strike", json!(v)),
            TextAttribute::Superscript(v) => {
                if *v {
                    fields.insert("subscript", json!(false));
                }
                ("superscript", json!(v))
            }
            TextAttribute::Subscript(v) => {
                if *v {
                    fields.insert("superscript", json!(false));
                }
                ("subscript", json!(v))
            }
            TextAttribute::Font(v) => ("font", json!(v)),
            TextAttribute::FontSize(v) => ("font_size", json!(v)),
            TextAttribute::Color(v) | TextAttribute::Highlight(v) => (
                if matches!(attribute, TextAttribute::Color(_)) {
                    "color"
                } else {
                    "highlight"
                },
                json!(v.map_or(0xff000000, |[r, g, b]| u32::from_le_bytes([r, g, b, 0]))),
            ),
        };
        fields.insert(name, value);
    }
    fields
}

fn observe(
    source: &[u8],
    space: ExGuid,
    object: ExGuid,
    range: Range<u32>,
    fields: &BTreeMap<&str, Value>,
) -> Result<Vec<Span>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let space = &document.spaces[&space];
    let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
    let mut spans: Vec<Span> = Vec::new();
    let mut start = 0;
    for run in revision.text_runs(object)? {
        let end =
            start + u32::try_from(run.text.encode_utf16().count()).map_err(io::Error::other)?;
        if (start < range.end && range.start < end) || (start == 0 && end == 0 && range == (0..0)) {
            let format = serde_json::to_value(run.format).map_err(io::Error::other)?;
            let values = fields
                .iter()
                .map(|(name, desired)| {
                    let value = &format[*name];
                    if value.is_null() && desired.is_boolean() {
                        json!(false)
                    } else if value.is_null() && matches!(*name, "color" | "highlight") {
                        json!(0xff000000_u32)
                    } else {
                        value.clone()
                    }
                })
                .collect::<Vec<_>>();
            let end = end.min(range.end) - range.start;
            if let Some(last) = spans.last_mut().filter(|s| s.values == values) {
                last.end = end;
            } else {
                spans.push(Span { end, values });
            }
        }
        start = end;
    }
    Ok(spans)
}

impl Replica {
    /// Durably records a visual-formatting change and its observed attribute values.
    /// Independent remote attributes can merge; competing values preserve a conflict.
    pub fn format(
        &self,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
        range: Range<u32>,
        attributes: &[TextAttribute],
    ) -> Result<Option<u64>> {
        let (edit, prepared) = FormatEdit::capture(source, space, object, range, attributes)?;
        self.record(source, space, Operation::Format(edit), &prepared)
    }
}

impl FormatEdit {
    pub(crate) fn capture<'a>(
        source: &'a [u8],
        space: ExGuid,
        object: ExGuid,
        range: Range<u32>,
        attributes: &[TextAttribute],
    ) -> Result<(Self, PreparedEdit<'a>)> {
        let prepared = PreparedEdit::format(source, space, object, range.clone(), attributes)?;
        let before = paragraph(source, space, object)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Prepared formatting has no text target",
            )
        })?;
        let observed = observe(source, space, object, range.clone(), &fields(attributes))?;
        let edit = Self {
            object,
            before,
            range,
            attributes: attributes.to_vec(),
            observed,
        };
        Ok((edit, prepared))
    }

    pub(crate) fn prepare<'a>(
        &self,
        snapshot: &'a [u8],
        space: ExGuid,
    ) -> Result<std::result::Result<PreparedEdit<'a>, ConflictKind>> {
        let Some(text) = paragraph(snapshot, space, self.object)? else {
            return Ok(Err(ConflictKind::TargetUnavailable));
        };
        let Some(range) = rebase::rebase(&self.before, &text, self.range.clone()) else {
            return Ok(Err(ConflictKind::TextChanged));
        };
        let prepared = match PreparedEdit::format(
            snapshot,
            space,
            self.object,
            range.clone(),
            &self.attributes,
        ) {
            Ok(prepared) => prepared,
            Err(_) => return Ok(Err(ConflictKind::UnsupportedEdit)),
        };
        let desired = fields(&self.attributes);
        let observed = observe(snapshot, space, self.object, range.clone(), &desired)?;
        let wanted: Vec<_> = desired.values().collect();
        for spans in [&self.observed, &observed] {
            if spans.is_empty()
                || (!range.is_empty() && spans[0].end == 0)
                || spans.last().unwrap().end != range.end - range.start
                || spans.windows(2).any(|s| s[0].end >= s[1].end)
                || spans.iter().any(|s| s.values.len() != wanted.len())
            {
                return Ok(Err(ConflictKind::UnsupportedEdit));
            }
        }
        let (mut before, mut after) = (0, 0);
        while before < self.observed.len() && after < observed.len() {
            let old = &self.observed[before];
            let current = &observed[after];
            if old
                .values
                .iter()
                .zip(&current.values)
                .zip(&wanted)
                .any(|((old, new), wanted)| new != old && new != *wanted)
            {
                return Ok(Err(ConflictKind::FormattingChanged));
            }
            let end = old.end.min(current.end);
            if old.end == end {
                before += 1;
            }
            if current.end == end {
                after += 1;
            }
        }
        Ok(Ok(prepared))
    }
}
