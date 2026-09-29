//! The Page Color menu: the page's colour, template art to lie behind it, and its rule
//! lines.

use crate::{
    commands::Choice,
    templates::{TILE, Thumbnails},
};
use canvas::{
    gpu::{Paper, colorref},
    template::{PAGE_COLORS, RULE_LINES},
};
use onestore::page::{RuleLines, VerticalRule};
use ui::{Anchor, Flags, Id, Spec, Ui, fill, px};

const COLUMNS: usize = 9;
const SWATCH: [f32; 2] = [40.0, 32.0];
const RULES: f32 = 40.0;
/// Points of page per point of a rule-line thumbnail.
const SHRINK: f32 = 4.0;
/// The art offered after None.
const ART: [&str; 5] = ["Ivy", "Purple Clouds", "Notebook", "Bamboo", "Blue Clouds"];

/// Builds popup `id` beside `anchor` while it is open, for a page on `paper` (its own colour
/// aside) coloured `color` and ruled with `rule_lines`. Returns the choice made.
#[allow(clippy::too_many_arguments)]
pub fn menu(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    thumbnails: &mut Thumbnails,
    paper: Paper,
    color: Option<u32>,
    rule_lines: Option<RuleLines>,
) -> Option<Choice> {
    let cells: Vec<Choice> = std::iter::once(Choice::PageColor(None))
        .chain((0..PAGE_COLORS.len()).map(|index| Choice::PageColor(Some(index))))
        .chain(std::iter::once(Choice::Art(None)))
        .chain(ART.map(|name| Choice::Art(Some(name))))
        .chain(std::iter::once(Choice::RuleLines(None)))
        .chain((0..RULE_LINES.len()).map(|index| Choice::RuleLines(Some(index))))
        .collect();
    let groups = [
        ui::popup::Group {
            heading: "Page Color",
            cells: 1 + PAGE_COLORS.len(),
            columns: COLUMNS,
            size: SWATCH,
        },
        ui::popup::Group {
            heading: "Background",
            cells: 1 + ART.len(),
            columns: 3,
            size: [TILE[0] + 8.0, TILE[1] + 8.0],
        },
        ui::popup::Group {
            heading: "Rule Lines",
            cells: 1 + RULE_LINES.len(),
            columns: COLUMNS,
            size: [RULES; 2],
        },
    ];
    let shown = [
        Choice::PageColor(color.map(|color| {
            PAGE_COLORS
                .iter()
                .position(|(_, listed)| *listed == color)
                .unwrap_or(usize::MAX)
        })),
        Choice::RuleLines(rule_lines.map(|lines| {
            RULE_LINES
                .iter()
                .position(|(_, listed)| {
                    listed.spacing == lines.spacing
                        && matches!(listed.vertical, VerticalRule::Grid { .. })
                            == matches!(lines.vertical, VerticalRule::Grid { .. })
                })
                .unwrap_or(usize::MAX)
        })),
    ];
    let current: Vec<usize> = shown
        .iter()
        .filter_map(|choice| cells.iter().position(|cell| cell == choice))
        .collect();
    let theme = ui.theme.clone();
    let scale = ui.scale();
    let dark = paper.ink[0] > paper.color[0];
    let chosen = ui::popup::gallery(ui, id, anchor, &groups, &current, |ui, index| {
        match cells[index] {
            Choice::PageColor(None) | Choice::Art(None) | Choice::RuleLines(None) => {
                ui.leaf(
                    "none",
                    Spec {
                        size: [fill(), fill()],
                        text: Some("None"),
                        font_size: Some(theme.font_size - 2.0),
                        fill: Some(paper.color),
                        border: Some(theme.chip),
                        radius: 2.0,
                        center: true,
                        ..Spec::default()
                    },
                );
            }
            Choice::PageColor(Some(index)) => {
                let (name, color) = PAGE_COLORS[index];
                ui.leaf(
                    name,
                    Spec {
                        size: [fill(), fill()],
                        fill: Some(paper.colored(Some(color)).color),
                        border: Some(theme.chip),
                        radius: 2.0,
                        ..Spec::default()
                    },
                );
            }
            Choice::Art(Some(name)) => {
                ui.open(
                    name,
                    Spec {
                        flags: Flags::CLIP,
                        size: [px(TILE[0]), px(TILE[1])],
                        fill: Some(paper.color),
                        border: Some(theme.chip),
                        radius: 2.0,
                        ..Spec::default()
                    },
                );
                let art = canvas::template::find(name).map_or(&[][..], |template| {
                    thumbnails.art(template, paper, dark, scale)
                });
                for (index, (image, [x, y, width, height])) in art.iter().enumerate() {
                    ui.leaf(
                        index,
                        Spec {
                            flags: Flags::FLOAT,
                            size: [px(*width), px(*height)],
                            position: [*x, *y],
                            image: Some(image),
                            ..Spec::default()
                        },
                    );
                }
                ui.close();
            }
            Choice::RuleLines(Some(index)) => rule_thumbnail(ui, paper, RULE_LINES[index]),
            _ => unreachable!("The menu offers colours, art and rule lines"),
        }
    })?;
    Some(cells[chosen].clone())
}

/// A corner of a page ruled with `lines`, drawn a quarter size.
fn rule_thumbnail(ui: &mut Ui, paper: Paper, (name, lines): (&str, RuleLines)) {
    let side = RULES - 8.0;
    ui.open(
        name,
        Spec {
            flags: Flags::CLIP,
            size: [fill(), fill()],
            fill: Some(paper.color),
            border: Some(ui.theme.chip),
            radius: 2.0,
            ..Spec::default()
        },
    );
    let line = |ui: &mut Ui, index: usize, rect: [f32; 4], color: u32| {
        ui.leaf(
            ("line", index),
            Spec {
                flags: Flags::FLOAT,
                position: [rect[0], rect[1]],
                size: [px(rect[2]), px(rect[3])],
                fill: Some(paper.tint(colorref(color))),
                ..Spec::default()
            },
        );
    };
    let [x, y] = [36.0 / SHRINK, 14.4 / SHRINK];
    let step = |spacing: f32| spacing * 36.0 / SHRINK;
    let mut index = 0;
    let mut at = y;
    while at < side {
        line(ui, index, [0.0, at, side, 1.0], lines.color);
        index += 1;
        at += step(lines.spacing);
    }
    match lines.vertical {
        VerticalRule::Margin(color) => line(ui, index, [x - 1.0, 0.0, 1.0, side], color),
        VerticalRule::Grid { spacing, color } => {
            let mut at = x % step(spacing);
            while at < side {
                line(ui, index, [at, 0.0, 1.0, side], color);
                index += 1;
                at += step(spacing);
            }
        }
    }
    ui.close();
}
