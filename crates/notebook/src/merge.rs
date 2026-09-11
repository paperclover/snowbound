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
                _ => result.push(pick(before, after, &object)?.clone()),
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
    let title = pick(&base.title, &ours.title, &theirs.title)?;
    let min_width = pick(&base.min_width, &ours.min_width, &theirs.min_width)?;
    let indents = pick(&base.indents, &ours.indents, &theirs.indents)?;
    let unsupported = pick(&base.unsupported, &ours.unsupported, &theirs.unsupported)?;
    let mut layout = theirs.layout.clone();
    let (x, y) = pick(
        &(base.layout.x, base.layout.y),
        &(ours.layout.x, ours.layout.y),
        &(theirs.layout.x, theirs.layout.y),
    )?;
    // A user-set width clears the wrap reservation, so the three travel together.
    let width = |layout: &onestore::document::Layout| {
        (
            layout.max_width,
            layout.width_set_by_user,
            layout.reserved_width,
        )
    };
    let (max_width, user_set, reserved) = pick(
        &width(&base.layout),
        &width(&ours.layout),
        &width(&theirs.layout),
    )?;
    layout.x = x;
    layout.y = y;
    layout.max_width = max_width;
    layout.width_set_by_user = user_set;
    layout.reserved_width = reserved;
    Some(Outline {
        id: theirs.id,
        title,
        min_width,
        layout,
        indents,
        paragraphs: merge_paragraphs(&base.paragraphs, &ours.paragraphs, &theirs.paragraphs)?,
        unsupported,
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
    // Child order per container: the remote order with our repositioned paragraphs re-placed.
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
        let (our_list, their_list) = (live(oc.get(&container)), live(tc.get(&container)));
        let of_base = |ids: Option<&Vec<ExGuid>>| -> Vec<ExGuid> {
            ids.map(|ids| {
                ids.iter()
                    .copied()
                    .filter(|id| b.contains_key(id))
                    .collect()
            })
            .unwrap_or_default()
        };
        let (base_ids, our_ids, their_ids) = (
            of_base(bc.get(&container)),
            of_base(oc.get(&container)),
            of_base(tc.get(&container)),
        );
        let ours_moved = if our_ids == base_ids || our_ids == their_ids {
            BTreeSet::new()
        } else {
            moved(&base_ids, &our_ids)
        };
        // A paragraph we repositioned that the remote also moved, reparented or removed
        // has two destinations; neither side's placement can be assumed.
        let mut theirs_touched = moved(&base_ids, &their_ids);
        theirs_touched.extend(base_ids.iter().filter(|id| !their_ids.contains(id)));
        if ours_moved.iter().any(|id| theirs_touched.contains(id)) {
            return None;
        }
        let mut list: Vec<ExGuid> = their_list.clone();
        list.retain(|id| merged[id].parent == container);
        // Paragraphs we repositioned, added or brought here follow their predecessor in our list.
        for (at, id) in our_list.iter().enumerate() {
            if merged[id].parent != container || (list.contains(id) && !ours_moved.contains(id)) {
                continue;
            }
            list.retain(|other| other != id);
            let predecessor = our_list[..at]
                .iter()
                .rev()
                .find(|other| list.contains(other));
            let position = predecessor.map_or(0, |p| list.iter().position(|x| x == p).unwrap() + 1);
            list.insert(position, *id);
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

/// The members of `list` outside a longest common subsequence with `base`: the paragraphs
/// a reorder of the shared members had to move.
fn moved(base: &[ExGuid], list: &[ExGuid]) -> BTreeSet<ExGuid> {
    let a: Vec<ExGuid> = base
        .iter()
        .copied()
        .filter(|id| list.contains(id))
        .collect();
    let b: Vec<ExGuid> = list
        .iter()
        .copied()
        .filter(|id| base.contains(id))
        .collect();
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut kept = BTreeSet::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            kept.insert(a[i]);
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    b.into_iter().filter(|id| !kept.contains(id)).collect()
}

fn merge_paragraph(
    base: &PageParagraph,
    ours: &PageParagraph,
    theirs: &PageParagraph,
) -> Option<PageParagraph> {
    let lists = pick(&base.lists, &ours.lists, &theirs.lists)?;
    let tags = pick(&base.tags, &ours.tags, &theirs.tags)?;
    let style = pick(&base.style, &ours.style, &theirs.style)?;
    let format = pick(&base.format, &ours.format, &theirs.format)?;
    let (parent, level) = pick(
        &(base.parent, base.level),
        &(ours.parent, ours.level),
        &(theirs.parent, theirs.level),
    )?;
    let collapsed = pick(&base.collapsed, &ours.collapsed, &theirs.collapsed)?;
    let content = match pick(&base.content, &ours.content, &theirs.content) {
        Some(picked) => picked,
        None => match (&base.content, &ours.content, &theirs.content) {
            (ParagraphContent::Text(x), ParagraphContent::Text(y), ParagraphContent::Text(z))
                if x.id == y.id && x.id == z.id =>
            {
                let mut text = z.clone();
                text.date_field = pick(&x.date_field, &y.date_field, &z.date_field)?;
                text.tags = pick(&x.tags, &y.tags, &z.tags)?;
                text.text = merge_text(&x.text, &y.text, &z.text)?;
                ParagraphContent::Text(text)
            }
            _ => return None,
        },
    };
    Some(PageParagraph {
        id: theirs.id,
        parent,
        level,
        style,
        format,
        content,
        lists,
        tags,
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
    if base.text() == ours.text() {
        // A local formatting-only change cannot be attributed to a range of the remote text.
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
    let our_end = ours.utf16_offset(o.len()).ok()?;
    let base_end = base.utf16_offset(b.len()).ok()?;
    if (start > 0 && ours.slice(0..start).ok()? != base.slice(0..start).ok()?)
        || (end < base_end
            && ours.slice(inserted..our_end).ok()? != base.slice(end..base_end).ok()?)
    {
        // Formatting changed outside the replaced range; the remote text would hide it.
        return None;
    }
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
        let bold = onestore::document::Format {
            bold: Some(true),
            ..Default::default()
        };
        let remote = Paragraph::new("one two three".into(), bold);
        let merged = merge_text(&base, &text("one two three four"), &remote).unwrap();
        assert_eq!(merged.text(), "one two three four");
        assert_eq!(merged.format_at(0).unwrap(), remote.format_at(0).unwrap());
        assert_eq!(
            merge_text(&base, &remote, &text("one two three four")),
            None
        );
        assert_eq!(
            merge_text(&base, &text("one two"), &text("X one two three"))
                .unwrap()
                .text(),
            "X one two"
        );
        assert_eq!(
            merge_text(&base, &text("two three"), &text("one two three!"))
                .unwrap()
                .text(),
            "two three!"
        );
    }
}
