//! Paragraph properties: list nodes, note tags, paragraph styles and paragraph formatting.

use super::Values;
use crate::{
    Error, ExGuid, PropertySets, Value,
    active::{ActivePage, Changes},
    document::{Kind, Tag},
    page::Definition,
    write::PropertyObject,
};
use std::collections::{BTreeMap, BTreeSet};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// The properties of a list node carrying `definition`.
pub(crate) fn list_values(definition: &Definition) -> Result<Values, Error> {
    let Kind::List {
        font,
        format,
        restart,
        bullet,
    } = &definition.kind
    else {
        return Err(invalid("A paragraph list must reference a list definition"));
    };
    if bullet.is_some() && (format.is_none() || font.is_none()) {
        return Err(invalid("A bullet definition names its glyph and font"));
    }
    let mut values: Values = Vec::new();
    if let Some(format) = format {
        let units: Vec<u16> = format.encode_utf16().collect();
        let count = u16::try_from(units.len())
            .map_err(|_| invalid("List format exceeds the document range"))?;
        let mut bytes = count.to_le_bytes().to_vec();
        bytes.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
        values.push((0x1c001c1a, bytes));
    }
    if let Some(font) = font {
        values.push((0x1c001c52, crate::create::string(font)));
    }
    if let Some(restart) = restart {
        values.push((0x14001cb7, restart.to_le_bytes().to_vec()));
    }
    if let Some(bullet) = bullet {
        values.push((0x10001d0e, bullet.to_le_bytes().to_vec()));
        // Every native bullet node carries this cleared flag alongside its index.
        values.push((0x0c001cc0, vec![0]));
    }
    let style = &definition.format;
    if let Some(font) = &style.font {
        values.push((0x1c001c0a, crate::create::string(font)));
    }
    if let Some(size) = style.font_size {
        let half = (size * 2.0).round();
        if !(0.0..=f32::from(u16::MAX)).contains(&half) {
            return Err(invalid("List font size is outside the document range"));
        }
        values.push((0x10001c0b, (half as u16).to_le_bytes().to_vec()));
    }
    if let Some(color) = style.color {
        values.push((0x14001c0c, color.to_le_bytes().to_vec()));
    }
    if let Some(language) = style.language {
        values.push((0x14001c3b, language.to_le_bytes().to_vec()));
    }
    for (flag, id) in [(style.bold, 0x08001c04), (style.italic, 0x08001c05)] {
        if let Some(flag) = flag {
            values.push((id | (u32::from(flag) << 31), Vec::new()));
        }
    }
    Ok(values)
}

/// Gives `paragraph` the list nodes `nodes` names, rewriting stored ones to their values
/// and creating the rest.
pub(crate) fn list_changes(
    active: &ActivePage<'_>,
    paragraph: ExGuid,
    nodes: &[(ExGuid, Values)],
) -> Result<Changes, Error> {
    let parents = active.editable_parents(paragraph)?;
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let raw = &active.live.revision;
    let mut changed = BTreeMap::new();
    let mut target = PropertyObject::from_object(&raw.objects[&paragraph])?;
    let mut references = Vec::new();
    for (list, values) in nodes {
        let mut node = match raw.objects.get(list) {
            Some(existing) => {
                if existing.jcid != 0x60012 {
                    return Err(invalid(
                        "A list definition identity belongs to another object",
                    ));
                }
                let mut node = PropertyObject::from_object(existing)?;
                node.remove(&[
                    0x1c001c1a, 0x1c001c52, 0x14001cb7, 0x10001d0e, 0x0c001cc0, 0x1c001c0a,
                    0x10001c0b, 0x14001c0c, 0x14001c3b, 0x08001c04, 0x08001c05,
                ])?;
                node
            }
            None => PropertyObject {
                jcid: 0x60012,
                bytes: crate::create::properties(&[])?,
                global_ids: std::sync::Arc::new(BTreeMap::from([(0, list.guid)])),
            },
        };
        node.set(
            &values
                .iter()
                .map(|(id, bytes)| (*id, bytes.as_slice()))
                .collect::<Vec<_>>(),
        )?;
        node.set(&[(0x14001d7a, &modified)])?;
        node.reference(*list)?;
        references.extend_from_slice(&target.reference(*list)?);
        changed.insert(*list, node);
    }
    if references.is_empty() {
        target.remove(&[0x24001c26])?;
    } else {
        target.set(&[(0x24001c26, &references)])?;
    }
    target.set(&[(0x14001d7a, &modified)])?;
    changed.insert(paragraph, target);
    crate::formatting::touch_ancestors(raw, parents, paragraph, &modified, &mut changed)?;
    Ok(changed)
}

/// The properties of a new tag definition object.
fn tag_definition_values(definition: &Definition) -> Result<Values, Error> {
    let Kind::TagDefinition {
        label,
        action_type,
        shape,
        color,
        highlight,
    } = &definition.kind
    else {
        return Err(invalid("A note tag must reference a tag definition"));
    };
    let mut values: Values = vec![
        (0x0c003473, vec![0]),
        (0x10003463, action_type.unwrap_or(0).to_le_bytes().to_vec()),
        (0x10003464, shape.unwrap_or(0).to_le_bytes().to_vec()),
        (0x14003467, 0u32.to_le_bytes().to_vec()),
    ];
    if let Some(label) = label {
        values.push((0x1c003468, crate::create::string(label)));
    }
    if let Some(color) = color {
        values.push((0x14003466, color.to_le_bytes().to_vec()));
    }
    if let Some(highlight) = highlight {
        values.push((0x14003465, highlight.to_le_bytes().to_vec()));
    }
    Ok(values)
}

/// Replaces the note tags of `object`. Each entry names the stored identity of its tag's
/// definition and, where the tag does not carry its action type or the page lacks the
/// definition, the definition itself.
pub(crate) fn tag_changes(
    active: &ActivePage<'_>,
    object: ExGuid,
    tags: &[(ExGuid, &Tag, Option<&Definition>)],
) -> Result<Changes, Error> {
    let raw = &active.live.revision;
    let mut definitions: Vec<(ExGuid, Option<Values>)> = Vec::new();
    let mut sets: Vec<(usize, Values)> = Vec::new();
    let mut action_types = BTreeSet::new();
    for (written, tag, definition) in tags {
        let action_type = if tag.status & 4 != 0 {
            tag.action_type
        } else {
            match definition.map(|d| &d.kind) {
                Some(Kind::TagDefinition { action_type, .. }) => *action_type,
                _ => return Err(invalid("A note tag must reference a tag definition")),
            }
        };
        if !action_types.insert(action_type.unwrap_or(0)) {
            return Err(invalid("An element holds one note tag per action type"));
        }
        let index = match definitions.iter().position(|(id, _)| id == written) {
            Some(index) => index,
            None => {
                let values = if raw.objects.contains_key(written) {
                    None
                } else {
                    let definition = definition
                        .ok_or_else(|| invalid("A note tag references a missing tag definition"))?;
                    Some(tag_definition_values(definition)?)
                };
                definitions.push((*written, values));
                definitions.len() - 1
            }
        };
        let mut fields: Values = Vec::new();
        if let Some(action_type) = tag.action_type {
            fields.push((0x10003463, action_type.to_le_bytes().to_vec()));
        }
        for (id, value) in [
            (0x1400346e, tag.created),
            (0x1400346f, tag.completed),
            (0x1400346a, tag.start),
            (0x1400346b, tag.due),
        ] {
            if let Some(value) = value {
                fields.push((id, value.to_le_bytes().to_vec()));
            }
        }
        fields.push((0x10003470, tag.status.to_le_bytes().to_vec()));
        if let Some(task) = tag.task_id {
            fields.push((0x1c003469, task.to_vec()));
        }
        sets.push((index, fields));
    }
    let parents = active.editable_parents(object)?;
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let mut changed = BTreeMap::new();
    for (id, values) in &definitions {
        let Some(values) = values else {
            continue;
        };
        let mut node = PropertyObject {
            jcid: 0x120043,
            bytes: crate::create::properties(values)?,
            global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
        };
        node.reference(*id)?;
        changed.insert(*id, node);
    }
    let mut target = PropertyObject::from_object(&raw.objects[&object])?;
    let mut encoded = Vec::new();
    for (index, fields) in &sets {
        let reference = target.reference(definitions[*index].0)?;
        let mut set = vec![(0x20003488, reference.to_vec())];
        set.extend(fields.iter().cloned());
        encoded.push(set);
    }
    target.set_sets(0x40003489, 0x44000811, &encoded)?;
    target.set(&[(0x14001d7a, &modified)])?;
    changed.insert(object, target);
    crate::formatting::touch_ancestors(raw, parents, object, &modified, &mut changed)?;
    Ok(changed)
}

/// Makes `text` reference paragraph style `style`, created from `definition` where the
/// page does not store it.
pub(crate) fn style_changes(
    active: &ActivePage<'_>,
    text: ExGuid,
    style: ExGuid,
    definition: Option<&Definition>,
) -> Result<Changes, Error> {
    let raw = &active.live.revision;
    let mut changed = BTreeMap::new();
    if !raw.objects.contains_key(&style) {
        let definition =
            definition.ok_or_else(|| invalid("A paragraph references a missing style definition"))?;
        let Kind::Style { name } = &definition.kind else {
            return Err(invalid("A paragraph style must be a style definition"));
        };
        let format = &definition.format;
        let mut values = super::content::style_values(format);
        if let Some(name) = name {
            values.push((0x1c00345a, crate::create::string(name)));
        }
        if let Some(alignment) = format.alignment {
            values.push((0x0c003477, vec![alignment]));
        }
        for (property, value) in [
            (0x1400342e, format.space_before),
            (0x1400342f, format.space_after),
            (0x14003430, format.line_spacing),
        ] {
            if let Some(points) = value {
                values.push((property, (points / 36.0).to_le_bytes().to_vec()));
            }
        }
        let mut node = PropertyObject {
            jcid: 0x12004d,
            bytes: crate::create::properties(&values)?,
            global_ids: std::sync::Arc::new(BTreeMap::from([(0, style.guid)])),
        };
        node.reference(style)?;
        changed.insert(style, node);
    }
    let mut target = PropertyObject::from_object(&raw.objects[&text])?;
    let reference = target.reference(style)?;
    target.set(&[(0x2000342c, &reference)])?;
    changed.insert(text, target);
    Ok(changed)
}

/// The text-object properties of paragraph formatting, in the order they are written.
pub(crate) fn paragraph_values(
    alignment: Option<u8>,
    rtl: Option<bool>,
    space_before: Option<f32>,
    space_after: Option<f32>,
    line_spacing: Option<f32>,
    language: Option<u32>,
) -> Result<Values, Error> {
    let mut values = Values::new();
    if let Some(alignment) = alignment {
        if alignment > 2 {
            return Err(invalid("Paragraph alignment must be left, center or right"));
        }
        values.push((0x0c003477, vec![alignment]));
    }
    if let Some(rtl) = rtl {
        values.push((0x08003476 | (u32::from(rtl) << 31), Vec::new()));
    }
    for (property, value) in [
        (0x1400342e, space_before),
        (0x1400342f, space_after),
        (0x14003430, line_spacing),
    ] {
        if let Some(points) = value {
            let stored = points / 36.0;
            if !stored.is_finite() || !(0.0..=27777.777).contains(&stored) {
                return Err(invalid("Paragraph spacing is outside the document range"));
            }
            values.push((property, stored.to_le_bytes().to_vec()));
        }
    }
    if let Some(language) = language {
        values.push((0x14001c3b, language.to_le_bytes().to_vec()));
    }
    Ok(values)
}

/// Writes paragraph formatting `values` on text object `object`; alignment also sets the
/// layout alignment fields OneNote keeps beside it.
pub(crate) fn paragraph_format_changes(
    active: &ActivePage<'_>,
    object: ExGuid,
    values: &Values,
) -> Result<Changes, Error> {
    let parents = active.editable_parents(object)?;
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let raw = &active.live.revision;
    let mut target = PropertyObject::from_object(&raw.objects[&object])?;
    target.set(
        &values
            .iter()
            .map(|(id, bytes)| (*id, bytes.as_slice()))
            .collect::<Vec<_>>(),
    )?;
    if let Some((_, alignment)) = values.iter().find(|(id, _)| *id == 0x0c003477) {
        for property in [0x14001c3e, 0x14001c84] {
            let fields = PropertySets::parse(&target.bytes)?;
            let previous = fields.sets[0]
                .iter()
                .find(|field| field.id == property)
                .map(|field| match field.value {
                    Value::Bytes(bytes) => bytes
                        .try_into()
                        .map(u32::from_le_bytes)
                        .map_err(|_| invalid("Invalid paragraph layout alignment")),
                    _ => Err(invalid("Invalid paragraph layout alignment")),
                })
                .transpose()?
                .unwrap_or(0);
            let value = (previous & !7) | (u32::from(alignment[0]) + 1);
            target.set(&[(property, &value.to_le_bytes())])?;
        }
    }
    target.set(&[(0x14001d7a, &modified)])?;
    let mut changed = BTreeMap::from([(object, target)]);
    crate::formatting::touch_ancestors(raw, parents, object, &modified, &mut changed)?;
    Ok(changed)
}
