use crate::{
    Anchor, Axis, Built, Flags, ICON, ICON_GAP, Id, Overflow, Size, State, fitting, popup::PAD,
    text::Texts,
};
use std::collections::HashMap;

/// Sizes each box on both axes, then places it: standalone sizes, sizes taken from
/// ancestors (pre-order), sizes summed from children (post-order), overflow taken back
/// (by folding a row's groups for what space sized by ancestors can't give, then from that
/// space, then from the least strict boxes first), and positions along each parent's flow. Labels too wide for their
/// solved width wrap or shorten before heights are solved. Boxes are in build order, so a
/// parent comes before its children. The interface is solved first, then each popup in
/// turn beside its anchor's box as laid out by then, or as last laid out where not yet.
pub(crate) fn solve(
    nodes: &mut [Built],
    states: &HashMap<Id, State>,
    scale: f32,
    texts: &mut Texts,
    frame: u64,
) {
    for index in 0..nodes.len() {
        if nodes[index].fold.is_some() {
            let folded = nodes[index].children[1];
            nodes[folded].hidden = true;
        }
    }
    // The popup each box lies in, by the popup's index; 0 for the interface beneath.
    let mut popup = vec![0; nodes.len()];
    for index in 1..nodes.len() {
        popup[index] = match nodes[index].anchor {
            Some(_) => index,
            None => popup[nodes[index].parent],
        };
    }
    let members = |group: usize| -> Vec<usize> {
        (0..popup.len())
            .filter(|index| popup[*index] == group)
            .collect()
    };
    solve_group(nodes, &members(0), states, scale, texts, frame);
    for group in 1..nodes.len() {
        if popup[group] == group {
            resolve(nodes, &popup, group, states);
            solve_group(nodes, &members(group), states, scale, texts, frame);
        }
    }
}

/// Finds where popup `index` opens from its anchor's box, laid out before it in `popup`'s
/// order, or as last laid out, and what it takes from the box: a popup below or over it is
/// at least as wide, and what holds still in a popup over it stands in for it as tall.
fn resolve(nodes: &mut [Built], popup: &[usize], index: usize, states: &HashMap<Id, State>) {
    let node = &nodes[index];
    let anchor = node.anchor.expect("a popup has an anchor");
    let laid_out = |id: Id| {
        let found = (1..nodes.len()).find(|at| nodes[*at].id == id && popup[*at] < index);
        found
            .map(|at| (nodes[at].rect, popup[at]))
            .or_else(|| Some((states.get(&id)?.rect?, 0)))
    };
    let [pad_x, pad_y] = node.pad;
    let around = match anchor {
        Anchor::Point([x, y]) => [x, y, x, y],
        Anchor::Dialog | Anchor::Top => [0.0; 4],
        Anchor::Below(id)
        | Anchor::Tip(id)
        | Anchor::Right(id)
        | Anchor::Over(id)
        | Anchor::Dock(id) => {
            let (rect, holder) = laid_out(id).unwrap_or_default();
            let [left, top, right, bottom] = rect;
            match anchor {
                Anchor::Right(_) => {
                    let [left, _, right, _] = if holder == 0 {
                        rect
                    } else {
                        nodes[holder].rect
                    };
                    [left, top - pad_y, right, bottom + pad_y]
                }
                Anchor::Over(_) => [left - pad_x, top - pad_y, right + pad_x, bottom + pad_y],
                Anchor::Dock(_) => rect,
                _ => [left, top - PAD, right, bottom + PAD],
            }
        }
    };
    let node = &mut nodes[index];
    node.around = around;
    if let (Some(Anchor::Below(_) | Anchor::Over(_)), Size::Pixels(width)) =
        (node.anchor, node.size[0].size)
    {
        node.size[0].size = Size::Pixels(width.max(around[2] - around[0]));
    }
    if let Anchor::Over(_) = anchor {
        let height = around[3] - around[1] - 2.0 * pad_y;
        for at in index..nodes.len() {
            if popup[at] == index && nodes[at].flags.contains(Flags::STILL) {
                nodes[at].size[1].size = Size::Pixels(height);
            }
        }
    }
}

/// Solves the boxes `members`, the interface or a popup and what it holds, in build order.
fn solve_group(
    nodes: &mut [Built],
    members: &[usize],
    states: &HashMap<Id, State>,
    scale: f32,
    texts: &mut Texts,
    frame: u64,
) {
    for axis in 0..2 {
        if axis == 1 {
            fit_labels(nodes, members, texts, frame);
        }
        for &index in members {
            let node = &mut nodes[index];
            node.computed[axis] = match node.size[axis].size {
                Size::Pixels(pixels) => match node.anchor {
                    Some(Anchor::Over(_)) if axis == 0 => {
                        let from = node.around[2] - node.around[0];
                        from + (pixels - from) * node.open
                    }
                    _ => pixels,
                },
                Size::Text => {
                    let content = if axis == 0 {
                        node.content_width()
                    } else {
                        let label = node.label.as_ref().map_or(0.0, |label| label.size[1]);
                        if node.icon.is_some() || node.image.is_some() {
                            label.max(crate::ICON)
                        } else {
                            label
                        }
                    };
                    content + 2.0 * node.pad[axis]
                }
                Size::Fraction(_) | Size::Children => 0.0,
            };
        }
        fit_popups(nodes, members, axis);
        for &index in members.iter().filter(|index| **index != 0) {
            if let Size::Fraction(fraction) = nodes[index].size[axis].size {
                if stretches(nodes, index, axis) {
                    nodes[index].computed[axis] = 0.0;
                    continue;
                }
                let mut ancestor = nodes[index].parent;
                while ancestor != 0 && nodes[ancestor].size[axis].size == Size::Children {
                    ancestor = nodes[ancestor].parent;
                }
                let room = across(nodes, ancestor, index, axis) - 2.0 * nodes[ancestor].pad[axis];
                nodes[index].computed[axis] = room.max(0.0) * fraction;
            }
        }
        for &index in members.iter().rev() {
            if nodes[index].size[axis].size == Size::Children {
                let content = flow(nodes, index, axis);
                nodes[index].computed[axis] = content + 2.0 * nodes[index].pad[axis];
            }
        }
        yield_popups(nodes, members, axis);
        fit_popups(nodes, members, axis);
        for &index in members {
            let node = &nodes[index];
            if node.children.is_empty() || (axis == 1 && node.flags.contains(Flags::SCROLL)) {
                continue;
            }
            let room = (node.computed[axis] - 2.0 * node.pad[axis]).max(0.0);
            let children: Vec<_> = in_flow(nodes, index).collect();
            if along(node, axis) {
                let space =
                    |child: &&usize| matches!(nodes[**child].size[axis].size, Size::Fraction(_));
                let (space, sized): (Vec<usize>, Vec<_>) = children.iter().partition(space);
                let mut excess = flow(nodes, index, axis) - room;
                // Groups fold only for what the space can't give, so the room a fold frees
                // goes back to the space.
                let spare: f32 = space.iter().map(|child| nodes[*child].computed[axis]).sum();
                while excess > spare && axis == 0 {
                    let mut groups = Vec::new();
                    unfolded(nodes, &children, &mut groups);
                    let Some(group) = groups.into_iter().min_by_key(|group| nodes[*group].fold)
                    else {
                        break;
                    };
                    let [full, folded] = [nodes[group].children[0], nodes[group].children[1]];
                    nodes[full].hidden = true;
                    nodes[folded].hidden = false;
                    let width = nodes[folded].computed[0] + 2.0 * nodes[group].pad[0];
                    let freed = nodes[group].computed[0] - width;
                    excess -= freed;
                    // A group inside boxes sized by their children narrows them too.
                    let mut inside = group;
                    while inside != index {
                        nodes[inside].computed[0] -= freed;
                        inside = nodes[inside].parent;
                    }
                }
                excess = give_back(nodes, &space, axis, excess);
                let mut tiers: Vec<f32> = sized
                    .iter()
                    .map(|child| strictness(nodes, *child, axis))
                    .filter(|strictness| *strictness < 1.0)
                    .collect();
                tiers.sort_by(f32::total_cmp);
                tiers.dedup();
                for tier in tiers {
                    let members: Vec<_> = sized
                        .iter()
                        .copied()
                        .filter(|child| strictness(nodes, *child, axis) == tier)
                        .collect();
                    excess = give_back(nodes, &members, axis, excess);
                }
            } else {
                for child in children {
                    let room =
                        (across(nodes, index, child, axis) - 2.0 * nodes[index].pad[axis]).max(0.0);
                    if let Size::Fraction(fraction) = nodes[child].size[axis].size
                        && stretches(nodes, child, axis)
                    {
                        nodes[child].computed[axis] = room * fraction;
                        continue;
                    }
                    let over = nodes[child].computed[axis] - room;
                    if over > 0.0 {
                        nodes[child].computed[axis] -=
                            over * (1.0 - strictness(nodes, child, axis));
                    }
                }
            }
        }
        let window = nodes[0].computed[axis];
        for &index in members {
            let node = &nodes[index];
            if let Some(anchor) = node.anchor {
                let shown = node.computed[axis];
                let size = match node.size[axis].size {
                    Size::Pixels(pixels) => pixels,
                    _ => shown,
                };
                nodes[index].relative[axis] = anchor.place(node.around, axis, size, shown, window);
            }
            let mut cursor = nodes[index].pad[axis];
            // Where a popup widening over its anchor lays its children out, from where it is.
            let shift = match (nodes[index].anchor, nodes[index].size[axis].size) {
                (Some(anchor @ Anchor::Over(_)), Size::Pixels(full)) if axis == 0 => {
                    let around = nodes[index].around;
                    anchor.place(around, axis, full, full, window) - nodes[index].relative[axis]
                }
                _ => 0.0,
            };
            for child in nodes[index].children.clone() {
                if nodes[child].anchor.is_some() {
                    continue;
                }
                nodes[child].relative[axis] = if nodes[child].flags.contains(Flags::FLOAT) {
                    nodes[child].position[axis]
                } else if nodes[child].hidden {
                    nodes[index].pad[axis]
                } else if along(&nodes[index], axis) {
                    let at = cursor;
                    cursor += nodes[child].computed[axis] + nodes[index].gap;
                    at
                } else {
                    nodes[index].pad[axis]
                };
                if !nodes[child].flags.contains(Flags::STILL) {
                    nodes[child].relative[axis] += shift;
                }
            }
            if axis == 1 {
                nodes[index].content = flow(nodes, index, axis) + 2.0 * nodes[index].pad[axis];
            }
        }
    }
    let snap = |value: f32| (value * scale).round() / scale;
    for &index in members {
        if index == 0 {
            let [width, height] = nodes[0].computed;
            nodes[0].rect = [0.0, 0.0, snap(width), snap(height)];
            continue;
        }
        let parent = nodes[index].parent;
        if nodes[index].hidden || nodes[parent].hidden {
            nodes[index].hidden = true;
            nodes[index].rect = nodes[parent].rect;
            continue;
        }
        let parent = &nodes[parent];
        let scroll =
            if parent.flags.contains(Flags::SCROLL) && !nodes[index].flags.contains(Flags::FLOAT) {
                states.get(&parent.id).map_or(0.0, |state| state.scroll)
            } else {
                0.0
            };
        let [dx, dy] = nodes[index].offset;
        let x = parent.rect[0] + nodes[index].relative[0] + dx;
        let y = parent.rect[1] + nodes[index].relative[1] - scroll + dy;
        nodes[index].rect = [
            snap(x),
            snap(y),
            snap(x + nodes[index].computed[0]),
            snap(y + nodes[index].computed[1]),
        ];
    }
}

/// Whether `index`, sized from an ancestor, is across a parent sized by its children: it takes
/// no room while the parent sums its other children, then that share of the parent's room.
fn stretches(nodes: &[Built], index: usize, axis: usize) -> bool {
    let node = &nodes[index];
    let parent = &nodes[node.parent];
    index != 0
        && parent.size[axis].size == Size::Children
        && !along(parent, axis)
        && !(axis == 1 && parent.flags.contains(Flags::SCROLL))
        && !node.flags.contains(Flags::FLOAT)
        && node.anchor.is_none()
}

/// Lets each popup sized loosely give way to the window, as far as its strictness lets it:
/// a dialog keeps below it the margin it opens under.
fn yield_popups(nodes: &mut [Built], members: &[usize], axis: usize) {
    let window = nodes[0].computed[axis];
    for &index in members {
        let node = &mut nodes[index];
        let room = match node.anchor {
            None => continue,
            Some(Anchor::Dialog | Anchor::Top) if axis == 1 => window * 3.0 / 4.0,
            Some(_) => fitting(window),
        };
        let over = node.computed[axis] - room;
        if over > 0.0 {
            node.computed[axis] -= over * (1.0 - node.size[axis].strictness.clamp(0.0, 1.0));
        }
    }
}

/// Shrinks each popup longer than the window lets it be on `axis`; one cut short vertically
/// scrolls what it holds.
fn fit_popups(nodes: &mut [Built], members: &[usize], axis: usize) {
    let most = fitting(nodes[0].computed[axis]);
    for &index in members {
        let node = &mut nodes[index];
        if node.anchor.is_some() && node.computed[axis] > most {
            node.computed[axis] = most;
            if axis == 1 {
                node.flags = node.flags | Flags::SCROLL | Flags::CLIP;
            }
        }
    }
}

/// The length `parent` lays `child` out across: a popup widening over its anchor lays its
/// contents out at its full width, but for boxes standing in for the anchor, which widen with it.
fn across(nodes: &[Built], parent: usize, child: usize, axis: usize) -> f32 {
    let node = &nodes[parent];
    match (node.anchor, node.size[axis].size) {
        (Some(Anchor::Over(_)), Size::Pixels(full))
            if axis == 0 && !nodes[child].flags.contains(Flags::STILL) =>
        {
            full.min(fitting(nodes[0].computed[axis]))
        }
        _ => node.computed[axis],
    }
}

/// Takes up to `excess` back from `boxes`, each giving in proportion to what its strictness
/// lets it, and returns what is left.
fn give_back(nodes: &mut [Built], boxes: &[usize], axis: usize, excess: f32) -> f32 {
    let give = |nodes: &[Built], child: usize| {
        nodes[child].computed[axis] * (1.0 - strictness(nodes, child, axis))
    };
    let total: f32 = boxes.iter().map(|child| give(nodes, *child)).sum();
    if excess <= 0.0 || total <= 0.0 {
        return excess;
    }
    let taken = excess.min(total);
    for child in boxes {
        nodes[*child].computed[axis] -= taken * give(nodes, *child) / total;
    }
    excess - taken
}

fn along(node: &Built, axis: usize) -> bool {
    (node.axis == Axis::Y) == (axis == 1)
}

/// The share of its size a box keeps when its siblings overflow. A box sized by the children
/// along its flow keeps what they keep.
fn strictness(nodes: &[Built], index: usize, axis: usize) -> f32 {
    let node = &nodes[index];
    let declared = node.size[axis].strictness.clamp(0.0, 1.0);
    if node.size[axis].size != Size::Children || !along(node, axis) || node.computed[axis] <= 0.0 {
        return declared;
    }
    let give: f32 = in_flow(nodes, index)
        .map(|child| nodes[child].computed[axis] * (1.0 - strictness(nodes, child, axis)))
        .sum();
    declared.min(1.0 - give / node.computed[axis])
}

fn in_flow(nodes: &[Built], index: usize) -> impl Iterator<Item = usize> + '_ {
    nodes[index].children.iter().copied().filter(|child| {
        !nodes[*child].flags.contains(Flags::FLOAT)
            && nodes[*child].anchor.is_none()
            && !nodes[*child].hidden
    })
}

/// The groups among `boxes` still in their full form, and those inside any of them that is a
/// row sized by its children, whose width follows theirs.
fn unfolded(nodes: &[Built], boxes: &[usize], groups: &mut Vec<usize>) {
    for &child in boxes {
        let node = &nodes[child];
        if node.fold.is_some() {
            if !nodes[node.children[0]].hidden {
                groups.push(child);
            }
        } else if node.size[0].size == Size::Children && node.axis == Axis::X {
            unfolded(nodes, &in_flow(nodes, child).collect::<Vec<_>>(), groups);
        }
    }
}

/// The children's extent on `axis`: summed with gaps along the flow, otherwise the largest.
fn flow(nodes: &[Built], index: usize, axis: usize) -> f32 {
    let sizes = in_flow(nodes, index).map(|child| nodes[child].computed[axis]);
    span(&nodes[index], axis, sizes)
}

/// The extent of children `sizes` long on `axis` of `node`, as `flow` measures it.
fn span(node: &Built, axis: usize, sizes: impl Iterator<Item = f32>) -> f32 {
    if along(node, axis) {
        let (sum, count) = sizes.fold((0.0, 0), |(sum, count), size| (sum + size, count + 1));
        sum + node.gap * (count.max(1) - 1) as f32
    } else {
        sizes.fold(0.0, f32::max)
    }
}

/// The least width `index`'s children fit in, with its padding: each row's groups folded, and
/// each box sized on its own yielding all its strictness lets it.
pub(crate) fn narrowest(nodes: &[Built], index: usize) -> f32 {
    let node = &nodes[index];
    let sizes = node
        .children
        .iter()
        .filter(|child| {
            !nodes[**child].flags.contains(Flags::FLOAT) && nodes[**child].anchor.is_none()
        })
        .map(|child| least(nodes, *child));
    span(node, 0, sizes) + 2.0 * node.pad[0]
}

/// The least width `index` takes without clipping its children.
fn least(nodes: &[Built], index: usize) -> f32 {
    let node = &nodes[index];
    if node.fold.is_some() {
        return least(nodes, node.children[1]) + 2.0 * node.pad[0];
    }
    let own = match node.size[0].size {
        Size::Pixels(pixels) => pixels,
        Size::Text => node.content_width() + 2.0 * node.pad[0],
        Size::Fraction(_) | Size::Children => return narrowest(nodes, index),
    };
    own * node.size[0].strictness.clamp(0.0, 1.0)
}

fn fit_labels(nodes: &mut [Built], members: &[usize], texts: &mut Texts, frame: u64) {
    for &index in members {
        let node = &mut nodes[index];
        let Some(label) = node.label.clone() else {
            continue;
        };
        let icon = if node.icon.is_some() || node.image.is_some() {
            ICON + ICON_GAP
        } else {
            0.0
        };
        let [left, _, right, _] = node.inset;
        let width = (node.computed[0] - left - right - 2.0 * node.pad[0] - icon).max(0.0);
        if label.size[0] <= width {
            continue;
        }
        node.label = match node.overflow {
            Overflow::Clip => continue,
            Overflow::Wrap => Some(label.wrapped(width, node.center)),
            Overflow::Ellipsis => Some(texts.ellipsis(&label, width, frame)),
        };
    }
}
