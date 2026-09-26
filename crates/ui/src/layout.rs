use crate::{Axis, Built, Flags, Id, Size, State};
use std::collections::HashMap;

/// Sizes each box on both axes, then places it: standalone sizes, sizes taken from
/// ancestors (pre-order), sizes summed from children (post-order), overflow shared out
/// by strictness, and positions along each parent's flow. Boxes are in build order, so
/// index order is pre-order.
pub(crate) fn solve(nodes: &mut [Built], states: &HashMap<Id, State>, scale: f32) {
    for axis in 0..2 {
        for node in nodes.iter_mut() {
            node.computed[axis] = match node.size[axis].size {
                Size::Pixels(pixels) => pixels,
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
                let room = nodes[ancestor].computed[axis] - 2.0 * nodes[ancestor].pad[axis];
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
            let room = node.computed[axis] - 2.0 * node.pad[axis];
            let children: Vec<_> = in_flow(nodes, index).collect();
            if along(node, axis) {
                let excess = flow(nodes, index, axis) - room;
                let give: f32 = children
                    .iter()
                    .map(|child| {
                        nodes[*child].computed[axis] * (1.0 - strictness(nodes, *child, axis))
                    })
                    .sum();
                if excess > 0.0 && give > 0.0 {
                    for child in children {
                        let share =
                            nodes[child].computed[axis] * (1.0 - strictness(nodes, child, axis));
                        nodes[child].computed[axis] -= excess.min(give) * share / give;
                    }
                }
            } else {
                for child in children {
                    let over = nodes[child].computed[axis] - room;
                    if over > 0.0 {
                        nodes[child].computed[axis] -=
                            over * (1.0 - strictness(nodes, child, axis));
                    }
                }
            }
        }
        for index in 0..nodes.len() {
            let mut cursor = nodes[index].pad[axis];
            for child in nodes[index].children.clone() {
                nodes[child].relative[axis] = if nodes[child].flags.contains(Flags::FLOAT) {
                    nodes[child].position[axis]
                } else if along(&nodes[index], axis) {
                    let at = cursor;
                    cursor += nodes[child].computed[axis] + nodes[index].gap;
                    at
                } else {
                    nodes[index].pad[axis]
                };
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
        let parent = &nodes[nodes[index].parent];
        let scroll =
            if parent.flags.contains(Flags::SCROLL) && !nodes[index].flags.contains(Flags::FLOAT) {
                states.get(&parent.id).map_or(0.0, |state| state.scroll)
            } else {
                0.0
            };
        let x = parent.rect[0] + nodes[index].relative[0];
        let y = parent.rect[1] + nodes[index].relative[1] - scroll;
        nodes[index].rect = [
            snap(x),
            snap(y),
            snap(x + nodes[index].computed[0]),
            snap(y + nodes[index].computed[1]),
        ];
    }
}

fn along(node: &Built, axis: usize) -> bool {
    (node.axis == Axis::Y) == (axis == 1)
}

fn strictness(nodes: &[Built], index: usize, axis: usize) -> f32 {
    nodes[index].size[axis].strictness.clamp(0.0, 1.0)
}

fn in_flow(nodes: &[Built], index: usize) -> impl Iterator<Item = usize> + '_ {
    nodes[index]
        .children
        .iter()
        .copied()
        .filter(|child| !nodes[*child].flags.contains(Flags::FLOAT))
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
