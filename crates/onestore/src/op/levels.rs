//! Outline levels: OneNote stores a container's children at its child level and nests each
//! run of deeper paragraphs in an outline group adding the rest
//! (`evidence/structural-edits/xml/c6-first-tab.xml`, MS-ONE 2.2.22).

use crate::{
    Error, ExGuid,
    active::{ActivePage, Changes},
    document::{Kind, Revision},
    write::PropertyObject,
};
use std::collections::BTreeMap;

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// The children of `container` with its outline groups lifted into it, in order.
pub(crate) fn flat_children(view: &Revision<'_>, container: ExGuid) -> Vec<ExGuid> {
    fn flat(view: &Revision<'_>, id: ExGuid, out: &mut Vec<ExGuid>) {
        for child in &view.nodes[&id].children {
            match view.nodes[child].kind {
                Kind::OutlineGroup => flat(view, *child, out),
                _ => out.push(*child),
            }
        }
    }
    let mut children = Vec::new();
    flat(view, container, &mut children);
    children
}

/// How much deeper than `container` each of its children lies, groups lifted.
pub(crate) fn stored_depths(view: &Revision<'_>, container: ExGuid) -> Vec<(ExGuid, u32)> {
    fn walk(view: &Revision<'_>, id: ExGuid, base: u32, out: &mut Vec<(ExGuid, u32)>) {
        let node = &view.nodes[&id];
        let depth = base + u32::from(node.child_level.unwrap_or(0));
        for child in &node.children {
            match view.nodes[child].kind {
                Kind::OutlineGroup => walk(view, *child, depth, out),
                _ => out.push((*child, depth)),
            }
        }
    }
    let mut out = Vec::new();
    walk(view, container, 0, &mut out);
    out
}

/// Lifts the children of `container`'s outline groups into it.
pub(crate) fn ungroup_changes(active: &ActivePage<'_>, container: ExGuid) -> Result<Changes, Error> {
    let children = flat_children(&active.view, container);
    let parents = active.editable_parents(container)?;
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let raw = &active.live.revision;
    let mut holder = PropertyObject::from_object(&raw.objects[&container])?;
    let mut references = Vec::new();
    for child in children {
        references.extend(holder.reference(child)?);
    }
    holder.set(&[(0x24001c20, &references), (0x14001d7a, &modified)])?;
    let mut changed = BTreeMap::from([(container, holder)]);
    crate::formatting::touch_ancestors(raw, parents, container, &modified, &mut changed)?;
    Ok(changed)
}

/// Children in runs: one child at the container's child level, or a run sharing a deeper
/// depth.
pub(crate) type Runs = Vec<(u32, Vec<ExGuid>)>;

/// The child level of a container whose children lie `depths` deeper than it, and its
/// children in runs.
pub(crate) fn runs(children: &[ExGuid], depths: &[u32], is_cell: bool) -> Result<(u32, Runs), Error> {
    let level = *depths
        .iter()
        .min()
        .ok_or_else(|| invalid("A container needs a child"))?;
    if !(1..=31).contains(&level) || depths.iter().any(|depth| *depth > level + 31) {
        return Err(invalid(
            "Outline levels lie 1 to 31 deeper than their parent",
        ));
    }
    if is_cell && depths.iter().any(|depth| *depth != level) {
        return Err(invalid("Table cells hold no outline groups"));
    }
    let mut runs: Runs = Vec::new();
    for (id, depth) in children.iter().zip(depths) {
        match runs.last_mut() {
            Some((last, ids)) if *last == *depth && *depth != level => ids.push(*id),
            _ => runs.push((*depth, vec![*id])),
        }
    }
    Ok((level, runs))
}

/// Stores `container`'s children as `runs` below child level `level`, each deeper run in a
/// new outline group.
pub(crate) fn regroup_changes(
    active: &ActivePage<'_>,
    container: ExGuid,
    level: u32,
    runs: &[(u32, Vec<ExGuid>)],
) -> Result<Changes, Error> {
    let parents = active.editable_parents(container)?;
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let raw = &active.live.revision;
    let mut holder = PropertyObject::from_object(&raw.objects[&container])?;
    let mut changed = BTreeMap::new();
    let mut references = Vec::new();
    for (depth, ids) in runs {
        if *depth == level {
            references.extend(holder.reference(ids[0])?);
            continue;
        }
        let id = ExGuid {
            guid: crate::write::fresh_guid()?,
            n: 1,
        };
        let mut group = PropertyObject {
            jcid: 0x60019,
            bytes: crate::create::properties(&[
                (0x14001d7a, modified.to_vec()),
                (0x0c001c03, vec![(depth - level) as u8]),
            ])?,
            global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
        };
        let mut members = Vec::new();
        for member in ids {
            members.extend(group.reference(*member)?);
        }
        group.set(&[(0x24001c20, &members)])?;
        group.reference(id)?;
        changed.insert(id, group);
        references.extend(holder.reference(id)?);
    }
    holder.set(&[
        (0x24001c20, &references),
        (0x0c001c03, &[level as u8]),
        (0x14001d7a, &modified),
    ])?;
    changed.insert(container, holder);
    crate::formatting::touch_ancestors(raw, parents, container, &modified, &mut changed)?;
    Ok(changed)
}
