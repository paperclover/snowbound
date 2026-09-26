use crate::{
    Error, ExGuid, Object, ObjectData, PropertySets,
    active::{ActivePage, Changes},
    create::string,
    document::{Element, Kind},
    write::PropertyObject,
};
use std::{collections::BTreeMap, ops::Range, sync::Arc};

fn title_line(text: &str) -> &str {
    text.trim_start().split('\r').next().unwrap()
}

pub(crate) fn automatic_title(text: &str) -> &str {
    let line = title_line(text).trim_end();
    let mut units = 0;
    for (byte, character) in line.char_indices() {
        if units >= 255 {
            return line[..byte].trim_end();
        }
        units += character.len_utf16();
    }
    line
}

/// Replaces UTF-16 character positions across ordinary text runs.
/// Inserted text inherits the run at the start; surviving text retains its formatting.
/// Insertion at a run boundary uses the following run, except at the end of text.
/// The final run retains its insertion style even when emptied.
/// Replacing a range with identical text leaves its existing formatting unchanged.
/// Returns a complete file image without I/O; use `commit_file_text` to update an existing file.
pub fn replace_text(
    source: &[u8],
    space: ExGuid,
    object: ExGuid,
    range: Range<u32>,
    replacement: &str,
) -> Result<Vec<u8>, Error> {
    if range.start > range.end || replacement.contains(['\0', '\n', '\r', '\u{fffc}']) {
        return Err(Error {
            offset: 0,
            message: "Use a valid text range and ordinary paragraph text",
        });
    }
    crate::active::write(source, space, |active| {
        text_changes(active, object, range, replacement)
    })
}

/// `replace_text` on an active page.
pub(crate) fn text_changes(
    active: &ActivePage<'_>,
    object: ExGuid,
    range: Range<u32>,
    replacement: &str,
) -> Result<Changes, Error> {
    let invalid = |message| Error { offset: 0, message };
    let raw = &active.live.revision;
    if !active.live.is_reachable(object) {
        return Err(invalid("Object is not reachable in the active revision"));
    }
    let revision = &active.view;
    let parents = active.editable_parents(object)?;
    let node = revision
        .nodes
        .get(&object)
        .ok_or_else(|| invalid("The text object is unavailable"))?;
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
            "Generated title fields cannot be edited as ordinary text",
        ));
    }
    let selected = runs
        .iter()
        .rposition(|run| run.start <= range.start && range.start <= run.end)
        .ok_or_else(|| invalid("The edit range exceeds the text"))?;
    let resolved = revision.text_runs(object)?;
    for (i, run) in runs.iter().enumerate() {
        if i == selected || (run.start < range.end && range.start < run.end) {
            // Hyperlink field codes and their hidden runs are ordinary text with flags;
            // embedded objects and equations are not.
            let format = &resolved[i].format;
            if [format.math, format.embedded_object].contains(&Some(true))
                || resolved[i].text.contains('\u{fffc}')
                || run.extra_set.is_some_and(|set| !node.extra[set].is_empty())
            {
                return Err(invalid("This text run contains a field or embedded data"));
            }
        }
    }
    let ObjectData::Properties(blob) = raw.objects[&object].data else {
        unreachable!()
    };
    let properties = PropertySets::parse(blob)?;
    if properties.sets[0]
        .iter()
        .any(|p| p.id == 0x88001cde || p.id == 0x24003458)
    {
        return Err(invalid(
            "This text object is read-only or contains associated run data",
        ));
    }
    let mut units = 0;
    let mut start = None;
    let mut end = None;
    for (byte, character) in text.char_indices() {
        if units == range.start {
            start = Some(byte);
        }
        if units == range.end {
            end = Some(byte);
        }
        units += character.len_utf16() as u32;
    }
    if units == range.start {
        start = Some(text.len());
    }
    if units == range.end {
        end = Some(text.len());
    }
    let (Some(start), Some(end)) = (start, end) else {
        return Err(invalid(
            "The edit range splits a surrogate pair or exceeds the text",
        ));
    };
    if &text[start..end] == replacement {
        return Ok(Changes::new());
    }
    let added = u32::try_from(replacement.encode_utf16().count())
        .map_err(|_| invalid("Replacement text exceeds the UTF-16 offset range"))?;
    let removed = range.end - range.start;
    units
        .checked_sub(removed)
        .and_then(|n| n.checked_add(added))
        .ok_or_else(|| invalid("Edited text exceeds the UTF-16 offset range"))?;
    let mut segments = Vec::new();
    let mut position = 0;
    for (i, run) in runs.iter().enumerate() {
        let length = run.end.min(range.start).saturating_sub(run.start)
            + run.end.saturating_sub(run.start.max(range.end))
            + if i == selected { added } else { 0 };
        let untouched = i != selected && (run.end <= range.start || range.end <= run.start);
        if length > 0 || i == runs.len() - 1 || untouched {
            position += length;
            segments.push((i, position));
        }
    }
    if segments.len() != runs.len() && properties.sets[0].iter().any(|p| p.id == 0x40003499) {
        return Err(invalid("Text edits cannot remove preserved run data"));
    }
    if segments[..segments.len() - 1]
        .windows(2)
        .any(|pair| pair[0].1 >= pair[1].1)
    {
        return Err(invalid("Text-run boundaries must be strictly increasing"));
    }
    let boundaries: Vec<_> = segments[..segments.len() - 1]
        .iter()
        .flat_map(|(_, end)| end.to_le_bytes())
        .collect();
    let formats = properties.sets[0]
        .iter()
        .find(|p| p.id == 0x24001e13)
        .map(|p| {
            let crate::Value::References { compact_ids, .. } = p.value else {
                unreachable!()
            };
            if compact_ids.is_empty() {
                Vec::new()
            } else {
                segments
                    .iter()
                    .flat_map(|(i, _)| compact_ids[i * 4..i * 4 + 4].iter().copied())
                    .collect()
            }
        });
    let mut changed = String::with_capacity(text.len() - (end - start) + replacement.len());
    changed.push_str(&text[..start]);
    changed.push_str(replacement);
    changed.push_str(&text[end..]);
    let unicode = properties.sets[0].iter().any(|p| p.id == 0x1c001c22)
        || !properties.sets[0].iter().any(|p| p.id == 0x1c003498)
        || changed.chars().any(|c| c as u32 > 255);
    let encoded = if unicode {
        changed
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    } else {
        changed.chars().map(|c| u8::try_from(c).unwrap()).collect()
    };
    let text_property = if unicode { 0x1c001c22 } else { 0x1c003498 };
    let text_update = (text_property, encoded.as_slice());
    let mut updates = Vec::new();
    let insert = if properties.sets[0].iter().any(|p| p.id == text_property) {
        updates.push(text_update);
        None
    } else {
        Some(text_update)
    };
    if properties.sets[0].iter().any(|p| p.id == 0x1c001e12) {
        updates.push((0x1c001e12, &boundaries));
    }
    if let Some(formats) = &formats {
        updates.push((0x24001e13, formats));
    }
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    updates.push((0x14001d7a, &modified));
    let mut edits = vec![crate::write::ObjectEdit {
        object,
        updates: &updates,
        inserts: insert.as_slice(),
    }];
    // Native conflict merges can discard descendant edits when ancestor timestamps stay stale.
    let modified_update = [(0x14001d7a, modified.as_slice())];
    let mut ancestors = std::collections::BTreeSet::new();
    let mut pending = parents.get(&object).cloned().unwrap_or_default();
    while let Some(id) = pending.pop() {
        if !ancestors.insert(id) {
            continue;
        }
        if revision.nodes[&id].modified.is_some() {
            edits.push(crate::write::ObjectEdit {
                object: id,
                updates: &modified_update,
                inserts: &[],
            });
        }
        pending.extend(parents.get(&id).into_iter().flatten().copied());
    }
    let Some((page, automatic, title_text)) =
        active.title(&BTreeMap::new(), Some((object, &changed)))?
    else {
        return crate::write::patched(raw, &edits);
    };
    let Kind::Page {
        alternate_title, ..
    } = &revision.nodes[&page].kind
    else {
        unreachable!()
    };
    let metadata = revision
        .roots
        .get(&2)
        .ok_or_else(|| invalid("Page title metadata is unavailable"))?;
    let Kind::Metadata { title, .. } = &revision.nodes[metadata].kind else {
        return Err(invalid("Page title metadata is unavailable"));
    };
    let cached: Vec<_> = title_text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let metadata_update = [(0x1c001cf3, cached.as_slice())];
    edits.push(crate::write::ObjectEdit {
        object: *metadata,
        updates: if title.is_some() {
            &metadata_update
        } else {
            &[]
        },
        inserts: if title.is_none() {
            &metadata_update
        } else {
            &[]
        },
    });
    let mut alternate_update = vec![(
        0x1c001d3c,
        if automatic {
            cached.as_slice()
        } else {
            &[0u8, 0][..]
        },
    )];
    if revision.nodes[&page].modified.is_some() {
        alternate_update.push(modified_update[0]);
    }
    edits.retain(|edit| edit.object != page);
    edits.push(crate::write::ObjectEdit {
        object: page,
        updates: if alternate_title.is_some() {
            &alternate_update
        } else {
            &alternate_update[1..]
        },
        inserts: if alternate_title.is_none() {
            &alternate_update[..1]
        } else {
            &[]
        },
    });
    crate::write::patched(raw, &edits)
}

pub(crate) fn editable_parents(
    revision: &crate::document::Revision<'_>,
    pages: &[ExGuid],
    object: ExGuid,
) -> Result<std::collections::BTreeMap<ExGuid, Vec<ExGuid>>, Error> {
    let parents = revision.parents(pages)?;
    check_editable(revision, &parents, pages, object)?;
    Ok(parents)
}

/// Requires `object` on an active page with neither it nor an ancestor read-only.
pub(crate) fn check_editable(
    revision: &crate::document::Revision<'_>,
    parents: &BTreeMap<ExGuid, Vec<ExGuid>>,
    pages: &[ExGuid],
    object: ExGuid,
) -> Result<(), Error> {
    let invalid = |message| Error { offset: 0, message };
    if !parents.contains_key(&object) && !pages.contains(&object) {
        return Err(invalid("Select content on an active editable page"));
    }
    let mut pending = vec![object];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        if revision.nodes[&id].extra[0]
            .iter()
            .any(|field| field.id == 0x88001cde)
        {
            return Err(invalid("This page or its content is read-only"));
        }
        pending.extend(parents.get(&id).into_iter().flatten().copied());
    }
    Ok(())
}

pub(crate) fn page_title(
    revision: &crate::document::Revision<'_>,
    pages: &[ExGuid],
    text_update: Option<(ExGuid, &str)>,
) -> Result<Option<(ExGuid, bool, String)>, Error> {
    title_of(
        |id| revision.nodes.get(&id),
        &revision.parents(pages)?,
        revision.nodes.keys().copied(),
        pages,
        text_update,
    )
}

/// `page_title` over the elements `node` finds, whose parents from `pages` are `parents`;
/// `candidates` lists, in order, every element that may be title text.
pub(crate) fn title_of<'n>(
    node: impl Fn(ExGuid) -> Option<&'n Element<'n>>,
    parents: &BTreeMap<ExGuid, Vec<ExGuid>>,
    candidates: impl Iterator<Item = ExGuid>,
    pages: &[ExGuid],
    text_update: Option<(ExGuid, &str)>,
) -> Result<Option<(ExGuid, bool, String)>, Error> {
    let invalid = |message| Error { offset: 0, message };
    let titles: Vec<_> = candidates
        .filter_map(|id| {
            if !parents.contains_key(&id) && !pages.contains(&id) {
                return None;
            }
            let node = node(id)?;
            if !node.extra[0].iter().any(|field| field.id == 0x88001cb4) {
                return None;
            }
            match &node.kind {
                Kind::RichText {
                    text,
                    boilerplate: false,
                    ..
                } => Some((id, text.as_str())),
                _ => None,
            }
        })
        .collect();
    let title_text = match titles.as_slice() {
        [] => "",
        [(id, text)] => {
            if let Some((_, changed)) = text_update.filter(|(object, _)| object == id) {
                changed
            } else {
                text
            }
        }
        _ => return Err(invalid("Title editing requires a single title text object")),
    };
    let automatic = title_line(title_text).is_empty();
    if !automatic && text_update.is_none_or(|(id, _)| titles[0].0 != id) {
        return Ok(None);
    }
    let [page] = pages else {
        return Err(invalid("Title editing requires a single active page"));
    };
    let Kind::Page { rtl, .. } = &node(*page).unwrap().kind else {
        unreachable!()
    };
    let mut title_text = title_line(title_text);
    if automatic {
        let mut roots = node(*page).unwrap().children.clone();
        roots.sort_by(|a, b| {
            let a = &node(*a).unwrap().layout;
            let b = &node(*b).unwrap().layout;
            a.y.unwrap_or(0.0)
                .total_cmp(&b.y.unwrap_or(0.0))
                .then_with(|| {
                    if *rtl == Some(true) {
                        b.x.unwrap_or(0.0).total_cmp(&a.x.unwrap_or(0.0))
                    } else {
                        a.x.unwrap_or(0.0).total_cmp(&b.x.unwrap_or(0.0))
                    }
                })
        });
        let mut pending: Vec<_> = roots.into_iter().rev().collect();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let element = node(id).unwrap();
            if let Kind::RichText {
                text,
                boilerplate: false,
                ..
            } = &element.kind
            {
                title_text = automatic_title(
                    if let Some((_, changed)) = text_update.filter(|(object, _)| *object == id) {
                        changed
                    } else {
                        text
                    },
                );
                if !title_text.is_empty() {
                    break;
                }
            }
            if *rtl == Some(true) && matches!(element.kind, Kind::Row) {
                pending.extend(element.children.iter().copied());
            } else {
                pending.extend(element.children.iter().rev().copied());
            }
            pending.extend(element.content.iter().rev().copied());
            pending.extend(element.structure.iter().rev().copied());
        }
    }
    Ok(Some((*page, automatic, title_text.to_owned())))
}

/// Updates the page title for `changed` objects made on `view`, a copy of the page's view.
pub(crate) fn update_title(
    active: &ActivePage<'_>,
    view: crate::document::Revision<'_>,
    changed: &mut BTreeMap<ExGuid, PropertyObject>,
) -> Result<(), Error> {
    let invalid = |message| Error { offset: 0, message };
    let (raw, pages) = (&active.live.revision, &active.pages);
    // Shorten the moved view's lifetime to the changed property buffers.
    let mut view = view;
    for (id, object) in changed.iter() {
        view.nodes.insert(
            *id,
            active.element(&Object {
                jcid: object.jcid,
                reference_count: 0,
                data: ObjectData::Properties(&object.bytes),
                global_ids: Arc::clone(&object.global_ids),
            })?,
        );
    }
    let title = page_title(&view, pages, None)?;
    drop(view);
    if let Some((_, automatic, title)) = title {
        let metadata = raw
            .roots
            .get(&2)
            .ok_or_else(|| invalid("Page title metadata is unavailable"))?;
        if raw.objects[metadata].jcid != 0x20030 {
            return Err(invalid("Page title metadata is unavailable"));
        }
        let title = string(&title);
        let mut object = PropertyObject::from_object(&raw.objects[metadata])?;
        object.set(&[(0x1c001cf3, &title)])?;
        changed.insert(*metadata, object);
        changed
            .get_mut(&pages[0])
            .unwrap()
            .set(&[(0x1c001d3c, if automatic { &title } else { &[0, 0] })])?;
    }
    Ok(())
}
