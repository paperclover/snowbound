//! Controls built on the popup layer: menus and filterable lists, a colour grid, a gallery
//! and a command palette. Each builds while its popup is open and returns what was chosen the
//! frame it is, closing the popup.

use crate::{
    Anchor, Axis, Event, Flags, ICON, ICON_GAP, Id, List, Menu, Overflow, Popup, Row, Rows, Spec,
    TIP_DELAY, TIP_FADE, TIP_WARM, Tip, Ui, children, fill, fit, list::GUTTER, mix, px, text_field,
};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use std::{cell::RefCell, time::Duration};
use winit::keyboard::{Key, NamedKey};

pub(crate) const CHECK: &[&str] = &[include_str!("../assets/check.svg")];
/// Inset of a popup's contents, and its distance from what it drops down from.
pub(crate) const PAD: f32 = 4.0;
/// A palette's rows and filter field, and Snowbound's more compact menu rows.
const ROW: f32 = 26.0;
pub(crate) const MENU_ROW: f32 = 22.0;
/// Rows a list shows before it scrolls.
const ROWS: f32 = 12.0;
const NARROWEST: f32 = 140.0;
const PALETTE: f32 = 560.0;
/// Where a tooltip's description wraps.
const TIP_WIDTH: f32 = 280.0;
/// A colour grid's cell, around its swatch.
const CELL: f32 = 22.0;
/// Seconds a popup takes to ease to the height of its results.
const RESIZE: f32 = 0.15;
thread_local! {
    /// Scratch space for ranking, a few hundred kilobytes, reused across frames.
    static MATCHER: RefCell<Matcher> = RefCell::new(Matcher::new(Config::DEFAULT));
}
/// The keys a menu's list takes, and Enter to choose.
const KEYS: [NamedKey; 7] = [
    NamedKey::ArrowUp,
    NamedKey::ArrowDown,
    NamedKey::PageUp,
    NamedKey::PageDown,
    NamedKey::Home,
    NamedKey::End,
    NamedKey::Enter,
];

/// A row of a menu or palette.
#[derive(Clone, Copy, Debug, Default)]
pub struct Item<'a> {
    pub text: &'a str,
    /// The family the text previews, as in a font menu.
    pub font: Option<&'a str>,
    pub icon: Option<&'static [&'static str]>,
    /// The icon has colours of its own, so the text's colour does not tint it.
    pub colored: bool,
    /// Keys that run the item, shown dim at the trailing edge.
    pub shortcut: &'a str,
    /// Marked with a check in place of its icon.
    pub checked: bool,
    /// Starts highlighted when the menu opens, as a checked item does.
    pub current: bool,
    pub disabled: bool,
    /// Starts a group, ruled off from the one above while unfiltered.
    pub separated: bool,
    /// Names the group it starts: shown only while unfiltered, never chosen.
    pub heading: bool,
}

/// Builds popup `id` as a menu of `items` beside `anchor` while it is open, under a
/// filter field showing `filter` while empty when given. Returns the index of the item
/// chosen.
pub fn menu(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    items: &[Item],
    filter: Option<&str>,
) -> Option<usize> {
    if !ui.popup_open(id) {
        return None;
    }
    let style = ui.theme.menu();
    let mut measure = |text| {
        ui.texts
            .label(text, style.font_size, false, None, ui.frame)
            .size[0]
    };
    let [text, shortcut] = items.iter().fold([0.0_f32; 2], |[text, shortcut], item| {
        [
            text.max(measure(item.text)),
            shortcut.max(measure(item.shortcut)),
        ]
    });
    let icons = items.iter().any(|item| item.icon.is_some() || item.checked);
    let width =
        text + if shortcut > 0.0 {
            3.0 * ICON_GAP + shortcut
        } else {
            0.0
        } + if icons { ICON + ICON_GAP } else { 0.0 }
            + if items.len() as f32 > ROWS {
                GUTTER
            } else {
                0.0
            }
            + 2.0 * (style.pad + style.row_pad);
    let least = match anchor {
        Anchor::Below(rect) => rect[2] - rect[0],
        Anchor::Over(rect) => rect[2] - rect[0] + 2.0 * style.pad,
        _ => 0.0,
    };
    choose(
        ui,
        id,
        anchor,
        items,
        filter,
        width.max(least).max(NARROWEST),
        style.row,
    )
}

/// Shows `title`, with the `keys` that run it and a `description` under it, in a tooltip
/// below the box built last while the pointer rests on it or on a box inside it: after a
/// delay, or at once while another has just shown. A press or the wheel hides it until the
/// pointer leaves.
pub fn tooltip(ui: &mut Ui, title: &str, keys: &str, description: Option<&str>) {
    let Some(&index) = ui.nodes[*ui.stack.last().unwrap()].children.last() else {
        return;
    };
    // The boxes built since are the box's own.
    let hovered = ui
        .hover
        .is_some_and(|hover| ui.nodes[index..].iter().any(|node| node.id == hover));
    if !hovered || ui.active.is_some() {
        return;
    }
    let id = ui.nodes[index].id;
    let now = ui.now;
    let showing = ui
        .tip
        .is_some_and(|tip| tip.due.is_some_and(|due| due <= now));
    let warm = showing
        || ui
            .warm
            .is_some_and(|warm| now.saturating_duration_since(warm).as_secs_f32() < TIP_WARM);
    let due = match ui.tip {
        Some(tip) if tip.id == id => tip.due,
        // Shown at once, already faded in.
        _ if warm => Some(
            now.checked_sub(Duration::from_secs_f32(TIP_FADE))
                .unwrap_or(now),
        ),
        _ => Some(now + Duration::from_secs_f32(TIP_DELAY)),
    };
    ui.tip = Some(Tip {
        id,
        frame: ui.frame,
        due,
    });
    let Some(due) = due else {
        return;
    };
    if now < due {
        ui.wake = Some(ui.wake.map_or(due, |wake| wake.min(due)));
        return;
    }
    let title = if keys.is_empty() {
        title.to_owned()
    } else {
        format!("{title} ({keys})")
    };
    let [left, top, right, bottom] = ui.rect(id).unwrap_or_default();
    let theme = &ui.theme;
    let spec = Spec {
        axis: Axis::Y,
        fill: Some(theme.popup),
        border: Some(theme.chip),
        shadow: Some(theme.shadow),
        radius: 4.0,
        pad: [8.0, 5.0],
        gap: 3.0,
        anchor: Some(Anchor::Below([left, top, right, bottom + PAD])),
        ..Spec::default()
    };
    ui.open_as(id.child("tooltip"), spec);
    ui.leaf(
        "title",
        Spec {
            size: [fit(), fit()],
            text: Some(&title),
            bold: true,
            ..Spec::default()
        },
    );
    if let Some(description) = description {
        let width = ui.measure(description)[0].min(TIP_WIDTH);
        ui.leaf(
            "description",
            Spec {
                size: [px(width), fit()],
                text: Some(description),
                overflow: Overflow::Wrap,
                ..Spec::default()
            },
        );
    }
    ui.close();
}

/// Builds popup `id` as a command palette across the top of the window while it is open,
/// listing `commands` that match what is typed, most relevant first. Returns the index of
/// the command chosen.
pub fn palette(ui: &mut Ui, id: Id, commands: &[Item]) -> Option<usize> {
    if !ui.popup_open(id) {
        return None;
    }
    let window = ui.rect(Id::ROOT).unwrap_or_default();
    let width = PALETTE.min(window[2] - 8.0 * PAD).max(NARROWEST);
    let [left, top] = [(window[2] - width) / 2.0, window[3] / 8.0];
    choose(
        ui,
        id,
        Anchor::Below([left, top, left, top]),
        commands,
        Some("Search commands"),
        width,
        ROW,
    )
}

fn choose(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    items: &[Item],
    filter: Option<&str>,
    width: f32,
    row: f32,
) -> Option<usize> {
    let field = id.child("filter");
    if filter.is_some() && ui.focus == Some(id) {
        ui.focus = Some(field);
    }
    let keys = navigation(ui, &[id, field], &KEYS);
    let popup = state(ui, id);
    let mut query = std::mem::take(&mut popup.query);
    let mut highlight = popup.highlight;
    let theme = ui.theme.clone();
    let style = theme.menu();
    // Over a combo box, the field takes the box's place at once, widening with the popup.
    let (flags, height) = match anchor {
        Anchor::Over(rect) => (Flags::STILL, rect[3] - rect[1]),
        _ => (Flags::default(), ROW),
    };
    surface(ui, id, anchor, width);
    if let Some(placeholder) = filter {
        let before = query.clone();
        text_field(
            ui,
            field,
            &mut query,
            placeholder,
            Spec {
                flags,
                size: [fill(), px(height)],
                fill: Some(theme.base),
                border: Some(theme.accent),
                radius: 4.0,
                pad: [8.0, 0.0],
                ..Spec::default()
            },
        );
        if query != before {
            highlight = None;
        }
    }
    let matches = Matches::new(items, &query, style.rule_band);
    // Unfiltered, the current or checked item starts highlighted; filtered, the best match.
    highlight = highlight.or_else(|| {
        let first = if query.is_empty() {
            matches
                .order
                .iter()
                .position(|index| items[*index].current || items[*index].checked)
        } else {
            (0..matches.count()).find(|row| matches.selectable(*row))
        };
        first.map(|row| matches.key(row))
    });
    let window = ui.rect(Id::ROOT).map_or(0.0, |window| window[3]);
    let field = if filter.is_some() {
        ROW + style.pad
    } else {
        0.0
    };
    let content = matches.count() as f32 * row + matches.space_before(matches.count());
    let view = content
        .max(row)
        .min((ROWS * row).min(window - 4.0 * PAD - field).max(row));
    // The popup eases to its new height as the results change; the rows do not move.
    let dt = ui.dt;
    let [from, to, elapsed] = state(ui, id).height.get_or_insert([view, view, RESIZE]);
    *elapsed = (*elapsed + dt).min(RESIZE);
    let height = *to - (*to - *from) * (1.0 - *elapsed / RESIZE).powi(3);
    if *to != view {
        [*from, *to, *elapsed] = [height, view, 0.0];
    }
    ui.animating |= height != view;
    ui.open(
        "results",
        Spec {
            flags: Flags::CLIP,
            axis: Axis::Y,
            size: [fill(), px(height)],
            ..Spec::default()
        },
    );
    let chosen = if matches.count() == 0 {
        ui.leaf(
            "empty",
            Spec {
                size: [fill(), px(row)],
                text: Some("No matches"),
                font_size: Some(style.font_size),
                color: Some(style.dim),
                pad: [8.0, 0.0],
                ..Spec::default()
            },
        );
        None
    } else {
        let list = List {
            rows: &matches,
            row,
            keys: &keys,
            hover_selects: true,
        };
        let clicked = crate::list(
            ui,
            id.child("rows"),
            Spec {
                size: [fill(), px(view)],
                fill: Some(style.fill),
                ..Spec::default()
            },
            list,
            &mut highlight,
            |ui, row| menu_row(ui, &style, &matches, row),
        );
        let entered = keys.contains(&NamedKey::Enter);
        clicked.or_else(|| {
            highlight
                .and_then(|key| matches.find(key))
                .filter(|row| entered && matches.selectable(*row))
        })
    };
    ui.close();
    ui.close();
    if let Some(row) = chosen {
        ui.close_popup(id);
        return Some(matches.order[row]);
    }
    let popup = state(ui, id);
    popup.query = query;
    popup.highlight = highlight;
    None
}

/// Builds a menu's row: the item's icon or check, its text and its shortcut, on the
/// highlight when selected, under a rule when it starts a group.
fn menu_row(ui: &mut Ui, style: &Menu, matches: &Matches, row: Row) {
    let item = &matches.items[row.key as usize];
    if row.index > 0 && matches.ruled && item.separated {
        ui.leaf(
            "rule",
            Spec {
                flags: Flags::FLOAT,
                size: [fill(), px(1.0)],
                position: [0.0, -(style.rule_band + 1.0) / 2.0],
                inset: [style.rule_inset, 0.0, style.rule_inset, 0.0],
                fill: Some(style.rule),
                ..Spec::default()
            },
        );
    }
    let color = if item.heading {
        style.dim
    } else if item.disabled {
        style.disabled
    } else {
        style.text
    };
    ui.open(
        "item",
        Spec {
            size: [fill(), fill()],
            fill: row.selected.then_some(style.highlight),
            border: style.highlight_border.filter(|_| row.selected),
            radius: style.row_radius,
            pad: [style.row_pad, 0.0],
            gap: ICON_GAP,
            ..Spec::default()
        },
    );
    if matches.icons {
        let tint = if item.colored && !item.checked {
            [1.0, 1.0, 1.0, color[3]]
        } else {
            color
        };
        ui.leaf(
            "icon",
            Spec {
                size: [px(ICON), fill()],
                icon: if item.checked { Some(CHECK) } else { item.icon },
                color: Some(if item.disabled {
                    mix(tint, style.fill, 0.5)
                } else {
                    tint
                }),
                ..Spec::default()
            },
        );
    }
    ui.leaf(
        "text",
        Spec {
            size: [fill(), fill()],
            text: Some(item.text),
            font: item.font,
            font_size: Some(style.font_size - if item.heading { 2.0 } else { 0.0 }),
            bold: item.heading,
            color: Some(color),
            ..Spec::default()
        },
    );
    if !item.shortcut.is_empty() {
        ui.leaf(
            "shortcut",
            Spec {
                size: [fit(), fill()],
                text: Some(item.shortcut),
                font_size: Some(style.font_size),
                color: Some(style.dim),
                ..Spec::default()
            },
        );
    }
    ui.close();
}

/// Builds popup `id` as a grid of `swatches` in rows of `columns` beside `anchor` while it
/// is open, under a button labelled `none` for no colour of its own. Returns the swatch
/// chosen, or None for the button.
pub fn colors(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    none: &str,
    swatches: &[[f32; 4]],
    columns: usize,
) -> Option<Option<[f32; 4]>> {
    if !ui.popup_open(id) {
        return None;
    }
    // The button is cell 0 and the swatches follow it.
    let cell = |index: usize| id.child(("cell", index));
    let count = swatches.len() + 1;
    let keys = navigation(
        ui,
        &[id],
        &[
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::ArrowUp,
            NamedKey::ArrowDown,
            NamedKey::Enter,
        ],
    );
    let mut highlight = state(ui, id).highlight.map(|cell| cell as usize);
    let mut chosen = None;
    for index in 0..count {
        let signal = ui.signal(cell(index));
        if signal.hovered && ui.moved {
            highlight = Some(index);
        }
        if signal.clicked {
            chosen = Some(index);
        }
    }
    for key in keys {
        highlight = Some(match (key, highlight) {
            (NamedKey::Enter, _) => {
                chosen = chosen.or(highlight);
                continue;
            }
            (_, None) => 0,
            (NamedKey::ArrowLeft, Some(at)) => at.saturating_sub(1),
            (NamedKey::ArrowRight, Some(at)) => (at + 1).min(count - 1),
            (NamedKey::ArrowUp, Some(at)) => at.saturating_sub(columns),
            (_, Some(0)) => 1,
            (_, Some(at)) if at + columns < count => at + columns,
            (_, Some(at)) => at,
        });
    }
    if let Some(index) = chosen {
        ui.close_popup(id);
        return Some((index > 0).then(|| swatches[index - 1]));
    }

    let theme = ui.theme.clone();
    let lit = |index: usize| (highlight == Some(index)).then(|| theme.hover());
    surface(
        ui,
        id,
        anchor,
        columns as f32 * CELL + 2.0 * ui.theme.menu().pad,
    );
    ui.open_as(
        cell(0),
        Spec {
            flags: Flags::CLICKABLE,
            size: [fill(), px(ROW)],
            text: Some(none),
            fill: lit(0),
            radius: 4.0,
            pad: [8.0, 0.0],
            ..Spec::default()
        },
    );
    ui.close();
    for (row, colors) in swatches.chunks(columns).enumerate() {
        ui.open(
            ("row", row),
            Spec {
                size: [fill(), px(CELL)],
                ..Spec::default()
            },
        );
        for (column, color) in colors.iter().enumerate() {
            let index = 1 + row * columns + column;
            ui.open_as(
                cell(index),
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(CELL), px(CELL)],
                    fill: lit(index),
                    radius: 4.0,
                    pad: [3.0, 3.0],
                    ..Spec::default()
                },
            );
            ui.leaf(
                "swatch",
                Spec {
                    size: [fill(), fill()],
                    fill: Some(*color),
                    border: Some(mix(*color, theme.text, 0.25)),
                    radius: 2.0,
                    ..Spec::default()
                },
            );
            ui.close();
        }
        ui.close();
    }
    ui.close();
    state(ui, id).highlight = highlight.map(|cell| cell as u64);
    None
}

/// Builds popup `id` as a grid of `size` columns and rows beside `anchor` while it is open,
/// lit from its corner to the cell pointed at, as Office's table picker is. Returns the
/// columns and rows chosen.
pub fn table_picker(ui: &mut Ui, id: Id, anchor: Anchor, size: [usize; 2]) -> Option<[usize; 2]> {
    if !ui.popup_open(id) {
        return None;
    }
    let cell = |index: usize| id.child(("cell", index));
    let keys = navigation(
        ui,
        &[id],
        &[
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::ArrowUp,
            NamedKey::ArrowDown,
            NamedKey::Enter,
        ],
    );
    let [columns, rows] = size;
    let mut highlight = state(ui, id).highlight.map(|cell| cell as usize);
    let mut chosen = None;
    for index in 0..columns * rows {
        let signal = ui.signal(cell(index));
        if signal.hovered && ui.moved {
            highlight = Some(index);
        }
        if signal.clicked {
            chosen = Some(index);
        }
    }
    for key in keys {
        let at = highlight.unwrap_or(0);
        let [column, row] = [at % columns, at / columns];
        highlight = Some(match key {
            NamedKey::Enter => {
                chosen = chosen.or(highlight);
                continue;
            }
            _ if highlight.is_none() => 0,
            NamedKey::ArrowLeft => at - usize::from(column > 0),
            NamedKey::ArrowRight => at + usize::from(column + 1 < columns),
            NamedKey::ArrowUp => at - if row > 0 { columns } else { 0 },
            _ => at + if row + 1 < rows { columns } else { 0 },
        });
    }
    let extent = |index: usize| [index % columns + 1, index / columns + 1];
    if let Some(index) = chosen {
        ui.close_popup(id);
        return Some(extent(index));
    }
    let theme = ui.theme.clone();
    let reach = highlight.map_or([0, 0], extent);
    surface(
        ui,
        id,
        anchor,
        columns as f32 * CELL + 2.0 * ui.theme.menu().pad,
    );
    let heading = match highlight {
        Some(_) => format!("{}x{} Table", reach[0], reach[1]),
        None => "Insert Table".to_owned(),
    };
    ui.leaf(
        "heading",
        Spec {
            size: [fill(), px(ROW)],
            text: Some(&heading),
            bold: true,
            pad: [8.0, 0.0],
            ..Spec::default()
        },
    );
    ui.open(
        "grid",
        Spec {
            axis: Axis::Y,
            size: [fill(), children()],
            ..Spec::default()
        },
    );
    for row in 0..rows {
        ui.open(
            ("row", row),
            Spec {
                size: [fill(), px(CELL)],
                ..Spec::default()
            },
        );
        for column in 0..columns {
            let lit = column < reach[0] && row < reach[1];
            ui.open_as(
                cell(row * columns + column),
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(CELL), px(CELL)],
                    pad: [3.0, 3.0],
                    ..Spec::default()
                },
            );
            ui.leaf(
                "square",
                Spec {
                    size: [fill(), fill()],
                    fill: Some(if lit { theme.hover() } else { theme.base }),
                    border: Some(if lit {
                        theme.accent
                    } else {
                        mix(theme.text, theme.popup, 0.7)
                    }),
                    radius: 2.0,
                    ..Spec::default()
                },
            );
            ui.close();
        }
        ui.close();
    }
    ui.close();
    ui.close();
    state(ui, id).highlight = highlight.map(|cell| cell as u64);
    None
}

/// Builds popup `id` as a gallery beside `anchor` while it is open: `groups` of cells, each
/// a heading and a count, in rows of `columns` cells `size` large, with cell `current`
/// outlined. `cell` builds a cell's contents from its index, counted through the groups.
/// Returns the index of the cell chosen.
#[allow(clippy::too_many_arguments)]
pub fn gallery(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    groups: &[(&str, usize)],
    columns: usize,
    size: [f32; 2],
    current: Option<usize>,
    mut cell: impl FnMut(&mut Ui, usize),
) -> Option<usize> {
    if !ui.popup_open(id) {
        return None;
    }
    let cell_id = |index: usize| id.child(("cell", index));
    let count: usize = groups.iter().map(|(_, count)| count).sum();
    let keys = navigation(
        ui,
        &[id],
        &[
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::ArrowUp,
            NamedKey::ArrowDown,
            NamedKey::Enter,
        ],
    );
    let mut highlight = state(ui, id).highlight.map(|cell| cell as usize);
    let mut chosen = None;
    for index in 0..count {
        let signal = ui.signal(cell_id(index));
        if signal.hovered && ui.moved {
            highlight = Some(index);
        }
        if signal.clicked {
            chosen = Some(index);
        }
    }
    for key in keys {
        let at = highlight.or(current).unwrap_or(0);
        highlight = Some(match key {
            NamedKey::Enter => {
                chosen = chosen.or(highlight);
                continue;
            }
            NamedKey::ArrowLeft => at.saturating_sub(1),
            NamedKey::ArrowRight => (at + 1).min(count - 1),
            NamedKey::ArrowUp => at.saturating_sub(columns),
            _ => (at + columns).min(count - 1),
        });
    }
    if let Some(index) = chosen {
        ui.close_popup(id);
        return Some(index);
    }
    let theme = ui.theme.clone();
    surface(
        ui,
        id,
        anchor,
        columns as f32 * size[0] + 2.0 * ui.theme.menu().pad,
    );
    let mut index = 0;
    for (group, (heading, cells)) in groups.iter().enumerate() {
        ui.leaf(
            ("heading", group),
            Spec {
                size: [fill(), px(MENU_ROW)],
                text: Some(heading),
                font_size: Some(theme.font_size - 2.0),
                bold: true,
                color: Some(theme.text_dim),
                pad: [PAD, 0.0],
                ..Spec::default()
            },
        );
        for row in 0..cells.div_ceil(columns) {
            ui.open(
                ("row", group, row),
                Spec {
                    size: [fill(), px(size[1])],
                    ..Spec::default()
                },
            );
            for _ in 0..columns.min(cells - row * columns) {
                ui.open_as(
                    cell_id(index),
                    Spec {
                        flags: Flags::CLICKABLE,
                        axis: Axis::Y,
                        size: [px(size[0]), px(size[1])],
                        fill: (highlight == Some(index)).then(|| theme.hover()),
                        border: (current == Some(index)).then_some(theme.accent),
                        radius: 4.0,
                        pad: [PAD, PAD],
                        ..Spec::default()
                    },
                );
                cell(ui, index);
                ui.close();
                index += 1;
            }
            ui.close();
        }
    }
    ui.close();
    state(ui, id).highlight = highlight.map(|cell| cell as u64);
    None
}

/// Opens popup `id`'s panel `width` wide beside `anchor`; the caller closes it.
fn surface(ui: &mut Ui, id: Id, anchor: Anchor, width: f32) {
    let style = ui.theme.menu();
    let pad = style.pad;
    let anchor = match anchor {
        Anchor::Below([left, top, right, bottom]) => {
            Anchor::Below([left, top - PAD, right, bottom + PAD])
        }
        // Level with the row it opens from, or with its contents over the box.
        Anchor::Right([left, top, right, bottom]) => {
            Anchor::Right([left, top - pad, right, bottom + pad])
        }
        Anchor::Over([left, top, right, bottom]) => {
            Anchor::Over([left - pad, top - pad, right + pad, bottom + pad])
        }
        point => point,
    };
    let spec = Spec {
        axis: Axis::Y,
        size: [px(width), children()],
        fill: Some(style.fill),
        border: Some(style.border),
        shadow: Some(ui.theme.shadow),
        radius: style.radius,
        pad: [pad; 2],
        gap: pad,
        anchor: Some(anchor),
        ..Spec::default()
    };
    ui.open_as(id, spec);
}

/// What is typed in open popup `id`'s filter field.
pub fn query(ui: &Ui, id: Id) -> Option<&str> {
    ui.popups
        .iter()
        .find(|popup| popup.id == id)
        .map(|popup| popup.query.as_str())
}

fn state(ui: &mut Ui, id: Id) -> &mut Popup {
    ui.popups
        .iter_mut()
        .find(|popup| popup.id == id)
        .expect("the popup is open")
}

/// Takes the presses of `keys` routed this frame to the focus when it is one of `owners`.
pub fn navigation(ui: &mut Ui, owners: &[Id], keys: &[NamedKey]) -> Vec<NamedKey> {
    let Some(signal) = ui
        .focus
        .filter(|focus| owners.contains(focus))
        .and_then(|focus| ui.signals.get_mut(&focus))
    else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    signal.events.retain(|event| match event {
        Event::Key {
            key: Key::Named(key),
            ..
        } if keys.contains(key) => {
            taken.push(*key);
            false
        }
        _ => true,
    });
    taken
}

/// The items fuzzily matching a query as fzf does, best first, keyed by their index in
/// `items`.
pub(crate) struct Matches<'a> {
    items: &'a [Item<'a>],
    pub(crate) order: Vec<usize>,
    /// Each item's row, or None when it does not match.
    rows: Vec<Option<usize>>,
    /// Rules above each row and the end, counted from the top; only while unfiltered.
    rules: Vec<u32>,
    ruled: bool,
    /// Height of the band a rule takes.
    rule: f32,
    icons: bool,
}

impl<'a> Matches<'a> {
    pub(crate) fn new(items: &'a [Item<'a>], query: &str, rule: f32) -> Self {
        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let order: Vec<usize> = if pattern.atoms.is_empty() {
            (0..items.len()).collect()
        } else {
            let mut chars = Vec::new();
            let mut ranked: Vec<_> = MATCHER.with_borrow_mut(|matcher| {
                items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| !item.heading)
                    .filter_map(|(index, item)| {
                        let text = Utf32Str::new(item.text, &mut chars);
                        Some((pattern.score(text, matcher)?, index))
                    })
                    .collect()
            });
            // Stable, so equal matches keep the items' order.
            ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            ranked.into_iter().map(|(_, index)| index).collect()
        };
        let mut rows = vec![None; items.len()];
        for (row, index) in order.iter().enumerate() {
            rows[*index] = Some(row);
        }
        let ruled = query.is_empty();
        let mut rules = vec![0];
        for (row, index) in order.iter().enumerate().skip(1) {
            let rule = u32::from(ruled && items[*index].separated);
            rules.push(rules[row - 1] + rule);
        }
        rules.push(*rules.last().unwrap());
        Self {
            items,
            order,
            rows,
            rules,
            ruled,
            rule,
            icons: items.iter().any(|item| item.icon.is_some() || item.checked),
        }
    }
}

impl Rows for Matches<'_> {
    fn count(&self) -> usize {
        self.order.len()
    }

    fn key(&self, index: usize) -> u64 {
        self.order[index] as u64
    }

    fn find(&self, key: u64) -> Option<usize> {
        *self.rows.get(key as usize)?
    }

    fn selectable(&self, index: usize) -> bool {
        let item = &self.items[self.order[index]];
        !item.disabled && !item.heading
    }

    fn space_before(&self, index: usize) -> f32 {
        self.rules[index] as f32 * self.rule
    }
}
