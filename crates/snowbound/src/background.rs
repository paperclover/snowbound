//! The Page Color menu: the page's colour, template art to lie behind it, and its rule
//! lines.

use crate::{
    art,
    commands::Choice,
    templates::{TILE, Thumbnails, View},
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
/// The art offered between None and Show All.
const ART: [&str; 7] = [
    "Ivy",
    "Purple Clouds",
    "Notebook",
    "Blue Clouds",
    "Tulips",
    "Bamboo",
    "Sparks",
];

#[derive(Clone, Copy, PartialEq)]
enum Cell {
    /// A COLORREF, or none.
    Color(Option<u32>),
    Custom,
    Art(Option<&'static str>),
    ShowAll,
    /// An index into `RULE_LINES`, or none.
    Rules(Option<usize>),
}

/// Builds popup `id` beside `anchor` while it is open, for a page on `paper` (its own colour
/// aside) coloured `color` and ruled with `rule_lines`, and the custom colour picker it
/// opens. Show All opens every template's art over the page through `view`. Returns the
/// choice made.
#[allow(clippy::too_many_arguments)]
pub fn menu(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    thumbnails: &mut Thumbnails,
    view: &mut View,
    paper: Paper,
    color: Option<u32>,
    rule_lines: Option<RuleLines>,
) -> Option<Choice> {
    let picker = id.child("custom");
    // A pale blue to start from on a page without a colour.
    let [red, green, blue, _] = color.unwrap_or(0x00f8eedd).to_le_bytes();
    if let Some([red, green, blue]) = ui::popup::color_picker(
        ui,
        picker,
        anchor,
        "Custom Color",
        [red, green, blue],
        |[red, green, blue]| {
            paper
                .colored(Some(u32::from_le_bytes([red, green, blue, 0])))
                .color
        },
    ) {
        return Some(Choice::PageColor(Some(u32::from_le_bytes([
            red, green, blue, 0,
        ]))));
    }
    let custom = color.filter(|color| !PAGE_COLORS.iter().any(|(_, listed)| listed == color));
    let cells: Vec<Cell> = std::iter::once(Cell::Color(None))
        .chain(
            PAGE_COLORS
                .iter()
                .map(|(_, color)| Cell::Color(Some(*color))),
        )
        .chain([Cell::Custom, Cell::Art(None)])
        .chain(ART.map(|name| Cell::Art(Some(name))))
        .chain([Cell::ShowAll, Cell::Rules(None)])
        .chain((0..RULE_LINES.len()).map(|index| Cell::Rules(Some(index))))
        .collect();
    let groups = [
        ui::popup::Group {
            heading: "Page Color",
            ruled: false,
            cells: 2 + PAGE_COLORS.len(),
            columns: COLUMNS,
            size: SWATCH,
        },
        ui::popup::Group {
            heading: "Background",
            ruled: false,
            cells: 2 + ART.len(),
            columns: 3,
            size: [TILE[0] + 8.0, TILE[1] + 8.0],
        },
        ui::popup::Group {
            heading: "Rule Lines",
            ruled: false,
            cells: 1 + RULE_LINES.len(),
            columns: COLUMNS,
            size: [RULES; 2],
        },
    ];
    let shown = [
        if custom.is_some() {
            Cell::Custom
        } else {
            Cell::Color(color)
        },
        Cell::Rules(rule_lines.map(|lines| {
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
        .filter_map(|shown| cells.iter().position(|cell| cell == shown))
        .collect();
    let theme = ui.theme.clone();
    let scale = ui.scale();
    let dark = paper.ink[0] > paper.color[0];
    let label = |ui: &mut Ui, text: &str| {
        ui.leaf(
            "label",
            Spec {
                size: [fill(), fill()],
                text: Some(text),
                font_size: Some(theme.font_size - 2.0),
                fill: Some(paper.color),
                border: Some(theme.chip),
                radius: 2.0,
                center: true,
                ..Spec::default()
            },
        );
    };
    let chosen = ui::popup::gallery(ui, id, anchor, &groups, &current, |ui, index| {
        let name = match cells[index] {
            Cell::Color(Some(color)) => PAGE_COLORS
                .iter()
                .find(|(_, listed)| *listed == color)
                .map(|(name, _)| *name),
            Cell::Custom => Some("Custom Color"),
            Cell::Art(Some(name)) => Some(name),
            Cell::Rules(Some(index)) => Some(RULE_LINES[index].0),
            _ => None,
        };
        if let Some(name) = name {
            crate::name(ui, ui.current(), name);
        }
        match cells[index] {
            Cell::Color(None) | Cell::Art(None) | Cell::Rules(None) => label(ui, "None"),
            Cell::ShowAll => label(ui, "Show All"),
            Cell::Color(Some(color)) => {
                ui.leaf(
                    "swatch",
                    Spec {
                        size: [fill(), fill()],
                        fill: Some(paper.colored(Some(color)).color),
                        border: Some(theme.chip),
                        radius: 2.0,
                        ..Spec::default()
                    },
                );
            }
            // The rainbow, or the page's custom colour with the rainbow in its corner.
            Cell::Custom => {
                ui.open(
                    "custom",
                    Spec {
                        size: [fill(), fill()],
                        fill: custom.map(|color| paper.colored(Some(color)).color),
                        border: custom.map(|_| theme.chip),
                        radius: 2.0,
                        icon: custom.is_none().then_some(art::CUSTOM_COLOR),
                        center: true,
                        ..Spec::default()
                    },
                );
                if custom.is_some() {
                    ui.leaf(
                        "rainbow",
                        Spec {
                            flags: Flags::FLOAT,
                            size: [px(12.0), px(12.0)],
                            position: [SWATCH[0] - 22.0, SWATCH[1] - 22.0],
                            icon: Some(art::CUSTOM_COLOR),
                            ..Spec::default()
                        },
                    );
                }
                ui.close();
            }
            Cell::Art(Some(name)) => {
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
            Cell::Rules(Some(index)) => rule_thumbnail(ui, paper, RULE_LINES[index]),
        }
    })?;
    match cells[chosen] {
        Cell::Color(color) => Some(Choice::PageColor(color)),
        Cell::Custom => {
            ui.open_popup(picker);
            None
        }
        Cell::Art(name) => Some(Choice::Art(name)),
        Cell::ShowAll => {
            *view = View::Art;
            None
        }
        Cell::Rules(index) => Some(Choice::RuleLines(index)),
    }
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
