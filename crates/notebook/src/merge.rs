//! Three-way merge of page models: the remote page keeps everything it changed, and the
//! local changes (`base` → `ours`) are re-applied wherever the two sides touched
//! different objects, fields or text ranges. Any overlap is a conflict for review.

use onestore::{
    ExGuid,
    page::{Outline, Page, PageObject, PageParagraph, ParagraphContent, text::Edit},
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn merge(base: &Page, ours: &Page, theirs: &Page) -> Option<Page> {
    if ours == base {
        return Some(theirs.clone());
    }
    if theirs == base || ours == theirs {
        return Some(ours.clone());
    }
    if ours.created != base.created || ours.margin_origin != base.margin_origin {
        return None;
    }
    fn by_id(page: &Page) -> BTreeMap<ExGuid, &PageObject> {
        page.objects
            .iter()
            .map(|object| (object.id(), object))
            .collect()
    }
    let (b, o, t) = (by_id(base), by_id(ours), by_id(theirs));
    let mut result: Vec<PageObject> = Vec::new();
    for object in &theirs.objects {
        let id = object.id();
        match (b.get(&id), o.get(&id)) {
            (Some(before), Some(after)) => match (before, after, object) {
                (PageObject::Outline(x), PageObject::Outline(y), PageObject::Outline(z)) => {
                    result.push(PageObject::Outline(merge_outline(x, y, z)?));
                }
                _ => {
                    if after != before {
                        return None;
                    }
                    result.push(object.clone());
                }
            },
            (Some(before), None) => {
                // Removed locally; keep only if the remote left it untouched.
                if object != *before {
                    return None;
                }
            }
            (None, _) => result.push(object.clone()),
        }
    }
    for (index, object) in ours.objects.iter().enumerate() {
        let id = object.id();
        if b.contains_key(&id) {
            if !t.contains_key(&id) {
                // Removed remotely; a local edit to it cannot be placed.
                if b[&id] != object {
                    return None;
                }
            }
            continue;
        }
        if t.contains_key(&id) {
            return None;
        }
        let PageObject::Outline(_) = object else {
            return None;
        };
        let predecessor = ours.objects[..index]
            .iter()
            .rev()
            .map(PageObject::id)
            .find(|other| result.iter().any(|r| r.id() == *other));
        let at = match predecessor {
            Some(other) => result.iter().position(|r| r.id() == other).unwrap() + 1,
            None => 0,
        };
        let at = at.min(
            result
                .iter()
                .position(|r| matches!(r, PageObject::Title(_)))
                .unwrap_or(result.len()),
        );
        result.insert(at, object.clone());
    }
    let order = |objects: &[PageObject], keep: &dyn Fn(ExGuid) -> bool| -> Vec<ExGuid> {
        objects
            .iter()
            .filter(|object| matches!(object, PageObject::Outline(_)) && keep(object.id()))
            .map(PageObject::id)
            .collect()
    };
    let common = |id: ExGuid| b.contains_key(&id) && o.contains_key(&id) && t.contains_key(&id);
    let (base_order, our_order, their_order) = (
        order(&base.objects, &common),
        order(&ours.objects, &common),
        order(&theirs.objects, &common),
    );
    if our_order != base_order {
        if their_order != base_order && their_order != our_order {
            return None;
        }
        result = reorder(result, &our_order);
    }
    Some(Page {
        title: theirs.title.clone(),
        created: theirs.created,
        margin_origin: theirs.margin_origin,
        objects: result,
        definitions: theirs.definitions.clone(),
    })
}

/// Places the objects named in `order` into the slots those objects occupy in `objects`,
/// leaving every other object where it is.
fn reorder(objects: Vec<PageObject>, order: &[ExGuid]) -> Vec<PageObject> {
    let mut slots: Vec<Option<PageObject>> = objects.into_iter().map(Some).collect();
    let positions: Vec<usize> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| {
            slot.as_ref()
                .is_some_and(|object| order.contains(&object.id()))
        })
        .map(|(i, _)| i)
        .collect();
    let mut taken: BTreeMap<ExGuid, PageObject> = positions
        .iter()
        .map(|&i| {
            let object = slots[i].take().unwrap();
            (object.id(), object)
        })
        .collect();
    for (slot, id) in positions.into_iter().zip(order) {
        slots[slot] = taken.remove(id);
    }
    slots.into_iter().flatten().collect()
}

fn pick<T: PartialEq + Clone>(base: &T, ours: &T, theirs: &T) -> Option<T> {
    if ours == base {
        Some(theirs.clone())
    } else if theirs == base || theirs == ours {
        Some(ours.clone())
    } else {
        None
    }
}

fn merge_outline(base: &Outline, ours: &Outline, theirs: &Outline) -> Option<Outline> {
    if ours.title != base.title
        || ours.min_width != base.min_width
        || ours.indents != base.indents
        || ours.unsupported != base.unsupported
    {
        return None;
    }
    let mut layout = theirs.layout.clone();
    let (x, y) = pick(
        &(base.layout.x, base.layout.y),
        &(ours.layout.x, ours.layout.y),
        &(theirs.layout.x, theirs.layout.y),
    )?;
    let (max_width, user_set) = pick(
        &(base.layout.max_width, base.layout.width_set_by_user),
        &(ours.layout.max_width, ours.layout.width_set_by_user),
        &(theirs.layout.max_width, theirs.layout.width_set_by_user),
    )?;
    layout.x = x;
    layout.y = y;
    layout.max_width = max_width;
    layout.width_set_by_user = user_set;
    Some(Outline {
        id: theirs.id,
        title: theirs.title,
        min_width: theirs.min_width,
        layout,
        indents: theirs.indents.clone(),
        paragraphs: merge_paragraphs(&base.paragraphs, &ours.paragraphs, &theirs.paragraphs)?,
        unsupported: theirs.unsupported.clone(),
    })
}

type Children = BTreeMap<Option<ExGuid>, Vec<ExGuid>>;

fn children(list: &[PageParagraph]) -> Children {
    let mut children: Children = BTreeMap::new();
    for paragraph in list {
        children
            .entry(paragraph.parent)
            .or_default()
            .push(paragraph.id);
    }
    children
}

fn merge_paragraphs(
    base: &[PageParagraph],
    ours: &[PageParagraph],
    theirs: &[PageParagraph],
) -> Option<Vec<PageParagraph>> {
    fn index(list: &[PageParagraph]) -> BTreeMap<ExGuid, &PageParagraph> {
        list.iter().map(|p| (p.id, p)).collect()
    }
    let (b, o, t) = (index(base), index(ours), index(theirs));
    let mut merged: BTreeMap<ExGuid, PageParagraph> = BTreeMap::new();
    let mut removed = BTreeSet::new();
    for id in b.keys().chain(o.keys()).chain(t.keys()) {
        if merged.contains_key(id) || removed.contains(id) {
            continue;
        }
        let paragraph = match (b.get(id), o.get(id), t.get(id)) {
            (Some(x), Some(y), Some(z)) => merge_paragraph(x, y, z)?,
            (Some(x), None, Some(z)) => {
                if z != x {
                    return None;
                }
                removed.insert(*id);
                continue;
            }
            (Some(x), Some(y), None) => {
                if y != x {
                    return None;
                }
                removed.insert(*id);
                continue;
            }
            (Some(_), None, None) => {
                removed.insert(*id);
                continue;
            }
            (None, Some(_), Some(_)) => return None,
            (None, Some(y), None) => (*y).clone(),
            (None, None, Some(z)) => (*z).clone(),
            (None, None, None) => unreachable!(),
        };
        merged.insert(*id, paragraph);
    }
    // Child order per container: the remote order unless only we reordered it.
    let (bc, oc, tc) = (children(base), children(ours), children(theirs));
    let mut order: BTreeMap<Option<ExGuid>, Vec<ExGuid>> = BTreeMap::new();
    let containers: BTreeSet<Option<ExGuid>> = bc
        .keys()
        .chain(oc.keys())
        .chain(tc.keys())
        .copied()
        .collect();
    for container in containers {
        let live = |ids: Option<&Vec<ExGuid>>| -> Vec<ExGuid> {
            ids.map(|ids| {
                ids.iter()
                    .copied()
                    .filter(|id| merged.contains_key(id))
                    .collect()
            })
            .unwrap_or_default()
        };
        let (base_list, our_list, their_list) = (
            live(bc.get(&container)),
            live(oc.get(&container)),
            live(tc.get(&container)),
        );
        let common = |list: &[ExGuid]| -> Vec<ExGuid> {
            list.iter()
                .copied()
                .filter(|id| {
                    base_list.contains(id) && our_list.contains(id) && their_list.contains(id)
                })
                .collect()
        };
        let (base_common, our_common, their_common) =
            (common(&base_list), common(&our_list), common(&their_list));
        let mut list: Vec<ExGuid> = their_list.clone();
        if our_common != base_common {
            if their_common != base_common && their_common != our_common {
                return None;
            }
            let slots: Vec<usize> = list
                .iter()
                .enumerate()
                .filter(|(_, id)| our_common.contains(id))
                .map(|(i, _)| i)
                .collect();
            for (slot, id) in slots.into_iter().zip(&our_common) {
                list[slot] = *id;
            }
        }
        // Paragraphs we added go after the predecessor they follow in our list.
        for (at, id) in our_list.iter().enumerate() {
            if list.contains(id) {
                continue;
            }
            if !b.contains_key(id) && merged[id].parent == container {
                let predecessor = our_list[..at]
                    .iter()
                    .rev()
                    .find(|other| list.contains(other));
                let position =
                    predecessor.map_or(0, |p| list.iter().position(|x| x == p).unwrap() + 1);
                list.insert(position, *id);
            }
        }
        for id in &list {
            if merged[id].parent != container {
                return None;
            }
        }
        order.insert(container, list);
    }
    let mut out = Vec::new();
    let mut pending: Vec<ExGuid> = order.get(&None).cloned().unwrap_or_default();
    pending.reverse();
    while let Some(id) = pending.pop() {
        let paragraph = merged.remove(&id)?;
        out.push(paragraph);
        if let Some(children) = order.get(&Some(id)) {
            pending.extend(children.iter().rev().copied());
        }
    }
    if !merged.is_empty() {
        return None;
    }
    Some(out)
}

fn merge_paragraph(
    base: &PageParagraph,
    ours: &PageParagraph,
    theirs: &PageParagraph,
) -> Option<PageParagraph> {
    if ours.lists != base.lists
        || ours.tags != base.tags
        || ours.style != base.style
        || ours.format != base.format
    {
        return None;
    }
    let (parent, level) = pick(
        &(base.parent, base.level),
        &(ours.parent, ours.level),
        &(theirs.parent, theirs.level),
    )?;
    let collapsed = pick(&base.collapsed, &ours.collapsed, &theirs.collapsed)?;
    let content = match (&base.content, &ours.content, &theirs.content) {
        (ParagraphContent::Text(x), ParagraphContent::Text(y), ParagraphContent::Text(z)) => {
            if x.id != y.id || x.id != z.id || y.date_field != x.date_field || y.tags != x.tags {
                return None;
            }
            let mut text = z.clone();
            text.text = merge_text(&x.text, &y.text, &z.text)?;
            ParagraphContent::Text(text)
        }
        (x, y, z) => {
            if y != x {
                return None;
            }
            z.clone()
        }
    };
    Some(PageParagraph {
        id: theirs.id,
        parent,
        level,
        style: theirs.style,
        format: theirs.format.clone(),
        content,
        lists: theirs.lists.clone(),
        tags: theirs.tags.clone(),
        collapsed,
    })
}

/// Re-applies our replacement inside the remote text when its range maps unambiguously.
fn merge_text(
    base: &onestore::page::Paragraph,
    ours: &onestore::page::Paragraph,
    theirs: &onestore::page::Paragraph,
) -> Option<onestore::page::Paragraph> {
    if let Some(picked) = pick(base, ours, theirs) {
        return Some(picked);
    }
    if base.text() == ours.text() || base.text() == theirs.text() {
        // Formatting-only changes on both sides cannot be attributed to ranges.
        return None;
    }
    let (b, o) = (base.text(), ours.text());
    let prefix = b
        .char_indices()
        .zip(o.chars())
        .take_while(|((_, x), y)| x == y)
        .map(|((i, x), _)| i + x.len_utf8())
        .last()
        .unwrap_or(0);
    let suffix = b[prefix..]
        .chars()
        .rev()
        .zip(o[prefix..].chars().rev())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x.len_utf8())
        .sum::<usize>();
    let start = base.utf16_offset(prefix).ok()?;
    let end = base.utf16_offset(b.len() - suffix).ok()?;
    let inserted = ours.utf16_offset(o.len() - suffix).ok()?;
    let replacement = ours.slice(start..inserted).ok()?;
    let mapped = crate::rebase::rebase(b, theirs.text(), start..end)?;
    let mut merged = theirs.clone();
    merged
        .apply(Edit {
            range: mapped,
            replacement,
        })
        .ok()?;
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::page::Paragraph;

    fn text(s: &str) -> Paragraph {
        Paragraph::new(s.into(), Default::default())
    }

    #[test]
    fn same_paragraph_edits_merge_when_their_ranges_do_not_overlap() {
        let base = text("one two three");
        let ours = text("one two three four");
        assert_eq!(
            merge_text(&base, &ours, &text("zero one two three"))
                .unwrap()
                .text(),
            "zero one two three four"
        );
        assert_eq!(
            merge_text(&base, &ours, &text("one 2 three"))
                .unwrap()
                .text(),
            "one 2 three four"
        );
        assert_eq!(
            merge_text(&base, &text("one two THREE"), &text("one two 3")),
            None
        );
        assert_eq!(
            merge_text(&base, &text("one two three"), &text("x"))
                .unwrap()
                .text(),
            "x"
        );
        assert_eq!(
            merge_text(&base, &text("y"), &text("one two three"))
                .unwrap()
                .text(),
            "y"
        );
    }
}
