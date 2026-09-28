//! Controls built on the popup layer: menus and filterable lists, a colour grid, a gallery
//! and a command palette. Each builds while its popup is open and returns what was chosen the
//! frame it is, closing the popup.

use crate::{
    Anchor, Axis, Event, Flags, ICON, ICON_GAP, Id, List, Popup, Row, Rows, Spec, Ui, children,
    fill, fit, list::GUTTER, mix, px, text_field,
};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use std::cell::RefCell;
use winit::keyboard::{Key, NamedKey};

pub(crate) const CHECK: &[&str] = &[include_str!("../assets/check.svg")];
/// Inset of a popup's contents, and its distance from what it drops down from.
const PAD: f32 = 4.0;
/// A palette's rows and filter field, and a menu's more compact rows.
const ROW: f32 = 26.0;
pub(crate) const MENU_ROW: f32 = 22.0;
/// Inset of a menu row's icon and text.
const ROW_PAD: f32 = 8.0;
/// How far a menu's icons sit inside its leading edge.
pub(crate) const ICON_INSET: f32 = PAD + ROW_PAD;
/// Rows a list shows before it scrolls.
const ROWS: f32 = 12.0;
/// Height of the rule between groups.
const RULE: f32 = 9.0;
const NARROWEST: f32 = 140.0;
const PALETTE: f32 = 560.0;
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
    let [text, shortcut] = items.iter().fold([0.0_f32; 2], |[text, shortcut], item| {
        [
            text.max(ui.measure(item.text)[0]),
            shortcut.max(ui.measure(item.shortcut)[0]),
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
            + 2.0 * (PAD + 8.0);
    let least = match anchor {
        Anchor::Below(rect) => rect[2] - rect[0],
        Anchor::Over(rect) => rect[2] - rect[0] + 2.0 * PAD,
        _ => 0.0,
    };
    choose(
        ui,
        id,
        anchor,
        items,
        filter,
        width.max(least).max(NARROWEST),
        MENU_ROW,
    )
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
    // Over a combo box, the field takes the box's place at once and widens as the popup
    // comes in around it.
    let (flags, size) = match anchor {
        Anchor::Over(rect) => {
            let open = ui.opening(id, anchor).unwrap_or(1.0);
            let from = rect[2] - rect[0];
            let to = width - 2.0 * PAD;
            (
                Flags::STILL,
                [px(from + (to - from) * open), px(rect[3] - rect[1])],
            )
        }
        _ => (Flags::default(), [fill(), px(ROW)]),
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
                size,
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
    let matches = Matches::new(items, &query);
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
    let field = if filter.is_some() { ROW + PAD } else { 0.0 };
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
                color: Some(theme.text_dim),
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
                fill: Some(theme.popup),
                ..Spec::default()
            },
            list,
            &mut highlight,
            |ui, row| menu_row(ui, &theme, &matches, row),
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
fn menu_row(ui: &mut Ui, theme: &crate::Theme, matches: &Matches, row: Row) {
    let item = &matches.items[row.key as usize];
    if row.index > 0 && matches.ruled && item.separated {
        ui.leaf(
            "rule",
            Spec {
                flags: Flags::FLOAT,
                size: [fill(), px(1.0)],
                position: [0.0, -(RULE + 1.0) / 2.0],
                fill: Some(theme.chip),
                ..Spec::default()
            },
        );
    }
    let color = if item.disabled || item.heading {
        theme.text_dim
    } else {
        theme.text
    };
    ui.open(
        "item",
        Spec {
            size: [fill(), fill()],
            fill: row.selected.then(|| theme.hover()),
            radius: 4.0,
            pad: [ROW_PAD, 0.0],
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
                    mix(tint, theme.popup, 0.5)
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
            font_size: item.heading.then_some(theme.font_size - 2.0),
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
                color: Some(theme.text_dim),
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
    surface(ui, id, anchor, columns as f32 * CELL + 2.0 * PAD);
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
    surface(ui, id, anchor, columns as f32 * size[0] + 2.0 * PAD);
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
    let anchor = match anchor {
        Anchor::Below([left, top, right, bottom]) => {
            Anchor::Below([left, top - PAD, right, bottom + PAD])
        }
        // Level with the row it opens from, or with its contents over the box.
        Anchor::Right([left, top, right, bottom]) => {
            Anchor::Right([left, top - PAD, right, bottom + PAD])
        }
        Anchor::Over([left, top, right, bottom]) => {
            Anchor::Over([left - PAD, top - PAD, right + PAD, bottom + PAD])
        }
        point => point,
    };
    let theme = &ui.theme;
    let spec = Spec {
        axis: Axis::Y,
        size: [px(width), children()],
        fill: Some(theme.popup),
        border: Some(theme.chip),
        shadow: Some(theme.shadow),
        radius: 6.0,
        pad: [PAD; 2],
        gap: PAD,
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
    icons: bool,
}

impl<'a> Matches<'a> {
    pub(crate) fn new(items: &'a [Item<'a>], query: &str) -> Self {
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
        self.rules[index] as f32 * RULE
    }
}
