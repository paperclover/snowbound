use crate::{
    Error, ExGuid, PropertySets, RevisionIndex, Store,
    create::{current_timestamps, properties, string},
    document::{Document, Kind},
    edit::editable_parents,
    write::{PropertyObject, fresh_guid, write_revision},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// An explicit character-format change; omitted attributes retain their current values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TextAttribute {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Strike(bool),
    /// Enabling superscript clears subscript.
    Superscript(bool),
    /// Enabling subscript clears superscript.
    Subscript(bool),
    Font(String),
    /// Points, from 6 through 130 in half-point increments.
    /// OneNote 2010 clamps larger sizes despite the specification allowing 144.
    FontSize(f32),
    /// RGB, or None for automatic text color.
    Color(Option<[u8; 3]>),
    /// RGB, or None to clear highlighting.
    Highlight(Option<[u8; 3]>),
}

impl TextAttribute {
    fn property(&self) -> Result<(u32, Vec<u8>), Error> {
        let boolean = |id, value: bool| (id | (u32::from(value) << 31), Vec::new());
        Ok(match self {
            Self::Bold(value) => boolean(0x08001c04, *value),
            Self::Italic(value) => boolean(0x08001c05, *value),
            Self::Underline(value) => boolean(0x08001c06, *value),
            Self::Strike(value) => boolean(0x08001c07, *value),
            Self::Superscript(value) => boolean(0x08001c08, *value),
            Self::Subscript(value) => boolean(0x08001c09, *value),
            Self::Font(font) => {
                if font.is_empty() || font.contains('\0') {
                    return Err(invalid("Font names must be nonempty and contain no NUL"));
                }
                (0x1c001c0a, string(font))
            }
            Self::FontSize(points) => {
                if !points.is_finite()
                    || !(6.0..=130.0).contains(points)
                    || (points * 2.0).fract() != 0.0
                {
                    return Err(invalid(
                        "Font size must be 6 to 130 points in half-point increments",
                    ));
                }
                (0x10001c0b, ((*points * 2.0) as u16).to_le_bytes().to_vec())
            }
            Self::Color(color) | Self::Highlight(color) => (
                if matches!(self, Self::Color(_)) {
                    0x14001c0c
                } else {
                    0x14001c0d
                },
                color
                    .map_or(0xff000000, |[r, g, b]| u32::from_le_bytes([r, g, b, 0]))
                    .to_le_bytes()
                    .to_vec(),
            ),
        })
    }
}

pub(crate) fn format_text(
    source: &[u8],
    space: ExGuid,
    object: ExGuid,
    range: Range<u32>,
    attributes: &[TextAttribute],
) -> Result<Vec<u8>, Error> {
    if attributes.is_empty() || range.start > range.end {
        return Err(invalid(
            "Select a text range and at least one formatting attribute",
        ));
    }
    let values = attribute_values(attributes)?;
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let view = document.active(space)?;
    let pages = document.pages_in(space)?;
    let parents = editable_parents(view, &pages, object)?;
    let node = &view.nodes[&object];
    let Kind::RichText {
        text,
        runs,
        boilerplate,
        ..
    } = &node.kind
    else {
        return Err(invalid("Select a rich-text object"));
    };
    if *boilerplate {
        return Err(invalid(
            "Generated title fields cannot be formatted as ordinary text",
        ));
    }
    let (mut valid_start, mut valid_end) = (range.start == 0, range.end == 0);
    let mut offset = 0;
    for character in text.chars() {
        offset += character.len_utf16() as u32;
        valid_start |= offset == range.start;
        valid_end |= offset == range.end;
    }
    if !valid_start || !valid_end {
        return Err(invalid(
            "The format range splits a surrogate pair or exceeds the text",
        ));
    }
    if range.is_empty() && !text.is_empty() {
        return Err(invalid(
            "Select characters, or an empty paragraph's insertion style",
        ));
    }
    let resolved = view.text_runs(object)?;
    let mut segments = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        let selected = if text.is_empty() {
            true
        } else {
            run.start < range.end && range.start < run.end
        };
        if selected {
            let format = &resolved[i].format;
            if [
                format.hidden,
                format.hyperlink,
                format.math,
                format.embedded_object,
            ]
            .contains(&Some(true))
                || resolved[i].text.contains(['\u{fffc}', '\u{fddf}'])
            {
                return Err(invalid(
                    "This format range contains a field or embedded data",
                ));
            }
            if run.start < range.start {
                segments.push((i, range.start, false));
            }
            segments.push((i, run.end.min(range.end), true));
            if range.end < run.end {
                segments.push((i, run.end, false));
            }
        } else {
            segments.push((i, run.end, false));
        }
    }
    let modified = current_timestamps()?.0.to_le_bytes();
    write_revision(source, space, |raw| {
        let mut target = PropertyObject::from_object(&raw.objects[&object])?;
        let fields = PropertySets::parse(&target.bytes)?;
        if fields.sets[0].iter().any(|p| p.id == 0x24003458) {
            return Err(invalid("This text object contains associated run objects"));
        }
        if segments.len() != runs.len() && fields.sets[0].iter().any(|p| p.id == 0x40003499) {
            return Err(invalid(
                "Formatting boundaries cannot split preserved run data",
            ));
        }
        let mut styles = BTreeMap::new();
        let mut changed = BTreeMap::new();
        let mut references = Vec::new();
        let mut ends = Vec::new();
        let mut updated = false;
        let changes: Vec<_> = values
            .iter()
            .map(|(id, value)| (*id, value.as_slice()))
            .collect();
        for &(i, end, selected) in &segments {
            let previous = runs[i].format;
            let key = (previous, selected);
            let id = if let Some(id) = styles.get(&key) {
                *id
            } else if let Some(id) = previous.filter(|_| !selected) {
                id
            } else {
                let mut style = match previous {
                    Some(id) => PropertyObject::from_object(&raw.objects[&id])?,
                    None => PropertyObject {
                        jcid: 0x12004d,
                        bytes: properties(&[])?,
                        global_ids: Arc::new(BTreeMap::new()),
                    },
                };
                if selected {
                    style.set(&changes)?;
                }
                if let Some(id) = previous.filter(|id| {
                    raw.objects[id].data == crate::ObjectData::Properties(&style.bytes)
                }) {
                    styles.insert(key, id);
                    id
                } else {
                    if !PropertySets::parse(&style.bytes)?.sets[0]
                        .iter()
                        .any(|p| p.id == 0x14001c3b)
                    {
                        style.set(&[(
                            0x14001c3b,
                            &resolved[i].format.language.unwrap_or(0x409).to_le_bytes(),
                        )])?;
                    }
                    let id = ExGuid {
                        guid: fresh_guid()?,
                        n: 1,
                    };
                    style.reference(id)?;
                    changed.insert(id, style);
                    styles.insert(key, id);
                    id
                }
            };
            updated |= selected && previous != Some(id);
            references.extend_from_slice(&target.reference(id)?);
            ends.extend_from_slice(&end.to_le_bytes());
        }
        if !updated {
            return Ok(BTreeMap::new());
        }
        ends.truncate(ends.len() - 4);
        target.set(&[
            (0x24001e13, &references),
            (0x1c001e12, &ends),
            (0x14001d7a, &modified),
        ])?;
        changed.insert(object, target);
        let mut pending = parents.get(&object).cloned().unwrap_or_default();
        let mut ancestors = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !ancestors.insert(id) {
                continue;
            }
            let mut ancestor = PropertyObject::from_object(&raw.objects[&id])?;
            ancestor.set(&[(0x14001d7a, &modified)])?;
            changed.insert(id, ancestor);
            pending.extend(parents.get(&id).into_iter().flatten().copied());
        }
        Ok(changed)
    })
}

pub(crate) fn attribute_values(attributes: &[TextAttribute]) -> Result<Vec<(u32, Vec<u8>)>, Error> {
    if attributes.is_empty() {
        return Err(invalid("Choose at least one formatting attribute"));
    }
    let mut values = Vec::new();
    let mut seen = BTreeSet::new();
    for attribute in attributes {
        let (id, value) = attribute.property()?;
        if !seen.insert(id & 0x7fffffff) {
            return Err(invalid("Specify each formatting attribute once"));
        }
        values.push((id, value));
    }
    if attributes.contains(&TextAttribute::Superscript(true))
        && attributes.contains(&TextAttribute::Subscript(true))
    {
        return Err(invalid("Text cannot be both superscript and subscript"));
    }
    // Setting either script position clears its mutually exclusive counterpart.
    for (set, opposite) in [(0x88001c08, 0x08001c09), (0x88001c09, 0x08001c08)] {
        if values.iter().any(|(id, _)| *id == set) && !seen.contains(&opposite) {
            values.push((opposite, Vec::new()));
        }
    }
    Ok(values)
}
