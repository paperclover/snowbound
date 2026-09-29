use crate::{Anchor, Axis, Built, Flags, ICON, ICON_GAP, Id, Overflow, Size, State, text::Texts};
use std::collections::HashMap;

/// Sizes each box on both axes, then places it: standalone sizes, sizes taken from
/// ancestors (pre-order), sizes summed from children (post-order), overflow taken back
/// (by folding a row's groups for what space sized by ancestors can't give, then from that
/// space, then from the least strict boxes first), and positions along each parent's flow. Labels too wide for their
/// solved width wrap or shorten before heights are solved. Boxes are in build order, so
/// index order is pre-order.
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
    for axis in 0..2 {
        if axis == 1 {
            fit_labels(nodes, texts, frame);
        }
        for node in nodes.iter_mut() {
            node.computed[axis] = match node.size[axis].size {
                Size::Pixels(pixels) => match node.anchor {
                    Some(Anchor::Over(rect)) if axis == 0 => {
                        let from = rect[2] - rect[0];
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
        for index in 1..nodes.len() {
            if let Size::Fraction(fraction) = nodes[index].size[axis].size {
                let mut ancestor = nodes[index].parent;
                while ancestor != 0 && nodes[ancestor].size[axis].size == Size::Children {
                    ancestor = nodes[ancestor].parent;
                }
                let room = across(nodes, ancestor, index, axis) - 2.0 * nodes[ancestor].pad[axis];
                nodes[index].computed[axis] = room.max(0.0) * fraction;
            }
        }
        for index in (0..nodes.len()).rev() {
            if nodes[index].size[axis].size == Size::Children {
                let content = flow(nodes, index, axis);
                nodes[index].computed[axis] = content + 2.0 * nodes[index].pad[axis];
            }
        }
        for index in 0..nodes.len() {
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
                    let Some(group) = children
                        .iter()
                        .copied()
                        .filter(|child| {
                            nodes[*child].fold.is_some() && !nodes[nodes[*child].children[0]].hidden
                        })
                        .min_by_key(|child| nodes[*child].fold)
                    else {
                        break;
                    };
                    let [full, folded] = [nodes[group].children[0], nodes[group].children[1]];
                    nodes[full].hidden = true;
                    nodes[folded].hidden = false;
                    let width = nodes[folded].computed[0] + 2.0 * nodes[group].pad[0];
                    excess -= nodes[group].computed[0] - width;
                    nodes[group].computed[0] = width;
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
                    let over = nodes[child].computed[axis] - room;
                    if over > 0.0 {
                        nodes[child].computed[axis] -=
                            over * (1.0 - strictness(nodes, child, axis));
                    }
                }
            }
        }
        let window = nodes[0].computed[axis];
        for index in 0..nodes.len() {
            let mut cursor = nodes[index].pad[axis];
            // Where a popup widening over its anchor lays its children out, from where it is.
            let shift = match (nodes[index].anchor, nodes[index].size[axis].size) {
                (Some(anchor @ Anchor::Over(_)), Size::Pixels(full)) if axis == 0 => {
                    anchor.place(axis, full, full, window) - nodes[index].relative[axis]
                }
                _ => 0.0,
            };
            for child in nodes[index].children.clone() {
                nodes[child].relative[axis] = if let Some(anchor) = nodes[child].anchor {
                    let shown = nodes[child].computed[axis];
                    let size = match nodes[child].size[axis].size {
                        Size::Pixels(pixels) => pixels,
                        _ => shown,
                    };
                    anchor.place(axis, size, shown, window)
                } else if nodes[child].flags.contains(Flags::FLOAT) {
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
    nodes[0].rect = [
        0.0,
        0.0,
        snap(nodes[0].computed[0]),
        snap(nodes[0].computed[1]),
    ];
    for index in 1..nodes.len() {
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

/// The length `parent` lays `child` out across: a popup widening over its anchor lays its
/// contents out at its full width, but for boxes standing in for the anchor, which widen with it.
fn across(nodes: &[Built], parent: usize, child: usize, axis: usize) -> f32 {
    let node = &nodes[parent];
    match (node.anchor, node.size[axis].size) {
        (Some(Anchor::Over(_)), Size::Pixels(full))
            if axis == 0 && !nodes[child].flags.contains(Flags::STILL) =>
        {
            full
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

/// The children's extent on `axis`: summed with gaps along the flow, otherwise the largest.
fn flow(nodes: &[Built], index: usize, axis: usize) -> f32 {
    let sizes = in_flow(nodes, index).map(|child| nodes[child].computed[axis]);
    if along(&nodes[index], axis) {
        let (sum, count) = sizes.fold((0.0, 0), |(sum, count), size| (sum + size, count + 1));
        sum + nodes[index].gap * (count.max(1) - 1) as f32
    } else {
        sizes.fold(0.0, f32::max)
    }
}

fn fit_labels(nodes: &mut [Built], texts: &mut Texts, frame: u64) {
    for node in nodes.iter_mut() {
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
