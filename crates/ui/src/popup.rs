//! Controls built on the popup layer: menus and filterable lists, a colour grid, a gallery
//! and a command palette. Each builds while its popup is open and returns what was chosen the
//! frame it is, closing the popup.

use crate::{
    Anchor, Axis, Event, Flags, ICON, ICON_GAP, Id, List, Menu, Overflow, Popup, Row, Rows, Spec,
    TIP_DELAY, TIP_FADE, TIP_WARM, Tip, Ui, children, fill, fit, list::GUTTER, mix, px, text_field,
};
use accesskit::Role;
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
/// How long the pointer rests on a row before its submenu opens, as Windows waits by default.
const SUBMENU_DELAY: Duration = Duration::from_millis(200);
const NARROWEST: f32 = 140.0;
const PALETTE: f32 = 560.0;
/// Where a tooltip's description wraps.
const TIP_WIDTH: f32 = 280.0;
/// A colour grid's cell, around its swatch.
const CELL: f32 = 22.0;
/// Inset of an item's text from the highlight it previews.
const SAMPLE_PAD: f32 = 2.0;
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
    /// The colour and highlight the text previews, as a tag without a symbol shows in
    /// OneNote's Tags menu.
    pub ink: Option<[f32; 4]>,
    pub highlight: Option<[f32; 4]>,
    pub icon: Option<&'static [&'static str]>,
    /// What the icon's `currentColor` paints in place of the text's colour, as a section's
    /// icon takes its colour; white leaves an icon with colours of its own as drawn.
    pub tint: Option<[f32; 4]>,
    /// Keys that run the item, shown dim at the trailing edge.
    pub shortcut: &'a str,
    /// Whether a check marks it in place of its icon; `None` where it is not a toggle.
    pub checked: Option<bool>,
    /// Opens a further menu, as a trailing arrow shows.
    pub submenu: bool,
    /// The value a picker opens on, highlighted and scrolled into view; a command menu
    /// has none and opens at its top, however its items are checked.
    pub current: bool,
    pub disabled: bool,
    /// Starts a group, ruled off from the one above while unfiltered.
    pub separated: bool,
    /// Names the group it starts: shown only while unfiltered, never chosen.
    pub heading: bool,
    /// Repeats an item listed further on, so shows only while unfiltered.
    pub repeated: bool,
}

impl Item<'_> {
    /// What the row shows at its trailing edge: an arrow where it opens a menu, else its keys.
    fn trailing(&self) -> &str {
        if self.submenu { "›" } else { self.shortcut }
    }
}

/// Whether `items`' rows leave room for an icon or a check.
fn icons(items: &[Item]) -> bool {
    items
        .iter()
        .any(|item| item.icon.is_some() || item.checked == Some(true))
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
    let mut measure = |text| ui.texts.label(text, style.font_size, ui.frame).size[0];
    let [text, shortcut] = items.iter().fold([0.0_f32; 2], |[text, shortcut], item| {
        [
            text.max(
                measure(item.text)
                    + if item.highlight.is_some() {
                        2.0 * SAMPLE_PAD
                    } else {
                        0.0
                    },
            ),
            shortcut.max(measure(item.trailing())),
        ]
    });
    let width =
        text + if shortcut > 0.0 {
            3.0 * ICON_GAP + shortcut
        } else {
            0.0
        } + if icons(items) { ICON + ICON_GAP } else { 0.0 }
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
        Role::Menu,
        anchor,
        &[("", items)],
        filter,
        width.max(least).max(NARROWEST),
        style.row,
    )
    .map(|(_, index)| index)
}

/// Opens the submenus of open menu `id` of `items` beside their rows, as `submenu` names
/// them by item: a row's once the pointer rests on it for `SUBMENU_DELAY`, or at once on
/// Right or a click. Resting on another row closes it.
pub fn submenus(ui: &mut Ui, id: Id, items: &[Item], submenu: impl Fn(usize) -> Option<Id>) {
    let Some(highlight) = ui
        .popups
        .iter()
        .find(|popup| popup.id == id)
        .map(|popup| popup.highlight)
    else {
        return;
    };
    let rows = id.child("rows");
    let opens = |key: u64| {
        items
            .get(key as usize)
            .is_some_and(|item| item.submenu && !item.disabled)
    };
    let right =
        highlight.is_some_and(opens) && !navigation(ui, &[id], &[NamedKey::ArrowRight]).is_empty();
    let under = highlight.filter(|key| {
        ui.pointer
            .zip(ui.rect(rows.child(*key)))
            .is_some_and(|(point, rect)| crate::contains(rect, point))
    });
    let now = ui.now;
    let popup = state(ui, id);
    if right {
        popup.submenu = highlight.map(|key| (key, Some(now)));
    } else if let Some(key) = under
        && popup.submenu.is_none_or(|(rested, _)| rested != key)
    {
        popup.submenu = Some((key, Some(now + SUBMENU_DELAY)));
    }
    let Some((key, Some(due))) = popup.submenu else {
        return;
    };
    if now < due {
        ui.wake_by(due);
        return;
    }
    popup.submenu = Some((key, None));
    match submenu(key as usize).filter(|_| opens(key)) {
        Some(child) => ui.open_submenu(child, id, rows.child(key)),
        None => {
            if let Some(at) = ui.popups.iter().position(|popup| popup.id == id) {
                ui.close_from(at + 1);
            }
        }
    }
}

/// Shows `title`, with the `keys` that run it and a `description` under it, in a tooltip
/// below the box built last while the pointer rests on it or on a box inside it: after a
/// delay, or at once while another has just shown. A press or the wheel hides it until the
/// pointer leaves. It names the box to assistive technology, or where the box has no role,
/// the unnamed controls inside it.
pub fn tooltip(ui: &mut Ui, title: &str, keys: &str, description: Option<&str>) {
    tooltip_below(ui, None, title, keys, description);
}

/// A `tooltip` below `part`, a window rectangle within the box built last, as a custom
/// box names what it draws.
pub fn tooltip_over(ui: &mut Ui, part: [f32; 4], title: &str, description: Option<&str>) {
    tooltip_below(ui, Some(part), title, "", description);
}

fn tooltip_below(
    ui: &mut Ui,
    part: Option<[f32; 4]>,
    title: &str,
    keys: &str,
    description: Option<&str>,
) {
    let Some(&index) = ui.nodes[*ui.stack.last().unwrap()].children.last() else {
        return;
    };
    if part.is_none() {
        let own = ui.nodes[index].access.is_some();
        let controls = ui.nodes[index..].iter_mut().filter_map(|node| {
            node.access
                .as_mut()
                .filter(|access| own || !crate::access::holds(access.role()))
        });
        for (count, node) in controls.take(if own { 1 } else { usize::MAX }).enumerate() {
            if node.label().is_none() {
                node.set_label(title);
            }
            if count == 0 {
                if !keys.is_empty() {
                    node.set_keyboard_shortcut(keys);
                }
                if let Some(description) = description {
                    node.set_description(description);
                }
            }
        }
    }
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
        ui.wake_by(due);
        return;
    }
    let title = if keys.is_empty() {
        title.to_owned()
    } else {
        format!("{title} ({keys})")
    };
    let [left, top, right, bottom] = part.or(ui.rect(id)).unwrap_or_default();
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

/// A palette's item picked, by mode and index: to run, or for its actions, which the menu
/// `actions` names lists beside its row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Run(usize, usize),
    Actions(usize, usize),
}

/// The menu of the actions on palette `id`'s item, which Escape closes back to the palette.
pub fn actions(id: Id) -> Id {
    id.child("actions")
}

/// Builds popup `id` as a command palette across the top of the window while it is open,
/// under a filter field showing `placeholder` while empty. What is typed picks the first of
/// `modes` whose prefix it starts with, and the rest of it narrows that mode's items, most
/// relevant first. Enter or a click runs the item; ⌘K (Ctrl+K elsewhere) on the highlighted
/// one or a right-click on any opens its `actions` menu.
pub fn palette(ui: &mut Ui, id: Id, modes: &[(&str, &[Item])], placeholder: &str) -> Option<Pick> {
    if !ui.popup_open(id) {
        return None;
    }
    let rows = id.child("rows");
    let clicked = (ui.lists.get(&rows).into_iter())
        .flat_map(crate::list::State::shown)
        .find(|key| {
            (ui.signals.get(&rows.child(*key))).is_some_and(|signal| signal.context.is_some())
        });
    let chord = chord(ui, id.child("filter"), "k");
    let window = ui.rect(Id::ROOT).unwrap_or_default();
    let width = PALETTE.min(window[2] - 8.0 * PAD).max(NARROWEST);
    if let Some((mode, index)) = choose(
        ui,
        id,
        Role::Dialog,
        Anchor::Top,
        modes,
        Some(placeholder),
        width,
        ROW,
    ) {
        return Some(Pick::Run(mode, index));
    }
    if !ui.popup_open(id) {
        return None;
    }
    let popup = state(ui, id);
    popup.highlight = clicked.or(popup.highlight);
    let key = clicked.or(popup.highlight.filter(|_| chord))?;
    let mode = modes
        .iter()
        .position(|(prefix, _)| popup.query.starts_with(prefix))?;
    if modes[mode].1.get(key as usize)?.heading {
        return None;
    }
    ui.open_submenu(actions(id), id, rows.child(key));
    Some(Pick::Actions(mode, key as usize))
}

/// Takes a press of `character` with the shortcut modifier, ⌘ or Ctrl, routed to `owner`.
fn chord(ui: &mut Ui, owner: Id, character: &str) -> bool {
    if !crate::edit_modifiers(ui.modifiers).command {
        return false;
    }
    let Some(signal) = ui.signals.get_mut(&owner) else {
        return false;
    };
    let before = signal.events.len();
    signal.events.retain(|event| {
        !matches!(event, Event::Key { key: Key::Character(typed), .. }
            if typed.eq_ignore_ascii_case(character))
    });
    signal.events.len() != before
}

/// Opens popup `id` with `query` typed in its filter field and the caret after it, or
/// retypes it where the popup is open.
pub fn open_with(ui: &mut Ui, id: Id, query: &str) {
    ui.open_popup(id);
    let popup = state(ui, id);
    popup.query = query.to_owned();
    popup.highlight = None;
    ui.states.entry(id.child("filter")).or_default().select = Some([usize::MAX; 2]);
}

/// Builds popup `id` as a `role` under a filter field showing `filter` while empty, listing
/// the items of the first of `modes` whose prefix the query starts with: a menu of items, or
/// a dialog of a list box of options.
#[allow(clippy::too_many_arguments)]
fn choose(
    ui: &mut Ui,
    id: Id,
    role: Role,
    anchor: Anchor,
    modes: &[(&str, &[Item])],
    filter: Option<&str>,
    width: f32,
    row: f32,
) -> Option<(usize, usize)> {
    let field = id.child("filter");
    if filter.is_some() && ui.focus == Some(id) {
        ui.focus = Some(field);
    }
    let keys = navigation(ui, &[id, field], &KEYS);
    let rows = id.child("rows");
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
    surface(ui, id, role, anchor, width);
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
                role: Some(Role::SearchInput),
                ..Spec::default()
            },
        );
        // New results show from their top at once.
        if query != before {
            highlight = None;
            ui.lists.remove(&rows);
        }
    }
    let (mode, (prefix, items)) = modes
        .iter()
        .enumerate()
        .find(|(_, (prefix, _))| query.starts_with(prefix))
        .expect("a mode takes any query");
    if role == Role::Dialog
        && let Some(node) = ui.access(id)
    {
        node.set_label(filter.unwrap_or_default());
    }
    let typed = &query[prefix.len()..];
    let matches = Matches::new(items, typed, style.rule_band);
    // Unfiltered, the current item starts highlighted; filtered, the best match.
    highlight = highlight.or_else(|| {
        let first = if typed.is_empty() {
            matches.order.iter().position(|index| items[*index].current)
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
            rows,
            Spec {
                size: [fill(), px(view)],
                fill: Some(style.fill),
                role: (role == Role::Dialog).then_some(Role::ListBox),
                ..Spec::default()
            },
            list,
            &mut highlight,
            |ui, row| menu_row(ui, &style, &matches, role, row),
        );
        // The keys choose from the list while the focus stays where they are typed.
        let active = highlight.filter(|key| matches.find(*key).is_some());
        for owner in [id, id.child("filter")] {
            highlighted(ui, owner, active.map(|key| rows.child(key)));
        }
        let entered = keys.contains(&NamedKey::Enter);
        clicked.or_else(|| {
            highlight
                .and_then(|key| matches.find(key))
                .filter(|row| entered && matches.selectable(*row))
        })
    };
    ui.close();
    if let Some(row) = chosen {
        let index = matches.order[row];
        // A row opening a submenu opens it beside itself, keeping the menu, as `submenus` builds.
        if !items[index].submenu {
            ui.close_popup(id);
            return Some((mode, index));
        }
        state(ui, id).submenu = Some((matches.key(row), Some(ui.now)));
    }
    let popup = state(ui, id);
    popup.query = query;
    popup.highlight = highlight;
    None
}

/// Builds a menu's row: the item's icon or check, its text and its shortcut, on the
/// highlight when selected, under a rule when it starts a group. In a dialog's list it is
/// an option.
fn menu_row(ui: &mut Ui, style: &Menu, matches: &Matches, owner: Role, row: Row) {
    let item = &matches.items[row.key as usize];
    let current = ui.current();
    if let Some(node) = ui.access(current) {
        node.set_role(match owner {
            _ if item.heading => Role::Heading,
            Role::Dialog => Role::ListBoxOption,
            _ if item.checked.is_some() => Role::MenuItemCheckBox,
            _ => Role::MenuItem,
        });
        node.set_label(item.text);
        if let Some(checked) = item.checked {
            node.set_toggled(checked.into());
        }
        if item.submenu {
            node.set_has_popup(accesskit::HasPopup::Menu);
            node.set_expanded(false);
        }
        if row.selected {
            node.set_selected(true);
        }
        if item.disabled {
            node.set_disabled();
        }
        match item.shortcut {
            "" => {}
            // A palette's trailing text may say where an option lies rather than its keys.
            trailing if owner == Role::Dialog => node.set_description(trailing),
            keys => node.set_keyboard_shortcut(keys),
        }
    }
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
        let checked = item.checked == Some(true);
        let tint = match item.tint {
            Some([red, green, blue, _]) if !checked => [red, green, blue, color[3]],
            _ => color,
        };
        ui.leaf(
            "icon",
            Spec {
                size: [px(ICON), fill()],
                icon: if checked { Some(CHECK) } else { item.icon },
                color: Some(if item.disabled {
                    mix(tint, style.fill, 0.5)
                } else {
                    tint
                }),
                ..Spec::default()
            },
        );
    }
    let text = Spec {
        size: [fill(), fill()],
        text: Some(item.text),
        font: item.font,
        font_size: Some(style.font_size - if item.heading { 2.0 } else { 0.0 }),
        bold: item.heading,
        color: Some(match (item.ink, item.highlight) {
            (None, None) => color,
            // Automatic text on a highlight is black, as on OneNote's page.
            (ink, highlight) => {
                let ink = ink.unwrap_or([0.0, 0.0, 0.0, 1.0]);
                if item.disabled {
                    mix(ink, highlight.unwrap_or(style.fill), 0.5)
                } else {
                    ink
                }
            }
        }),
        ..Spec::default()
    };
    match item.highlight {
        Some(highlight) => {
            ui.leaf(
                "text",
                Spec {
                    size: [fit(), px(style.row - 4.0)],
                    fill: Some(highlight),
                    pad: [SAMPLE_PAD, 0.0],
                    ..text
                },
            );
            ui.leaf(
                "rest",
                Spec {
                    size: [fill(), fill()],
                    ..Spec::default()
                },
            );
        }
        None => {
            ui.leaf("text", text);
        }
    }
    if !item.trailing().is_empty() {
        ui.leaf(
            "shortcut",
            Spec {
                size: [fit(), fill()],
                text: Some(item.trailing()),
                font_size: Some(style.font_size),
                color: Some(style.dim),
                ..Spec::default()
            },
        );
    }
    ui.close();
}

/// Builds popup `id` as a grid of `swatches`, each a colour and its name or `""` to be
/// named by its hex, in rows of `columns` beside `anchor` while it is open, under a button
/// labelled `none` for no colour of its own. Returns the swatch chosen, or None for the
/// button.
pub fn colors(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    none: &str,
    swatches: &[([f32; 4], &str)],
    columns: usize,
) -> Option<Option<[f32; 4]>> {
    if !ui.popup_open(id) {
        return None;
    }
    // The button is cell 0 and the swatches follow it.
    let cell = |index: usize| id.child(("cell", index));
    let count = swatches.len() + 1;
    let (highlight, chosen) = pick_cell(ui, id, count, cell, |key, highlight| {
        match (key, highlight) {
            (_, None) => 0,
            (NamedKey::ArrowLeft, Some(at)) => at.saturating_sub(1),
            (NamedKey::ArrowRight, Some(at)) => (at + 1).min(count - 1),
            (NamedKey::ArrowUp, Some(at)) => at.saturating_sub(columns),
            (_, Some(0)) => 1,
            (_, Some(at)) if at + columns < count => at + columns,
            (_, Some(at)) => at,
        }
    });
    if let Some(index) = chosen {
        ui.close_popup(id);
        return Some((index > 0).then(|| swatches[index - 1].0));
    }

    let theme = ui.theme.clone();
    let lit = |index: usize| (highlight == Some(index)).then(|| theme.hover());
    surface(
        ui,
        id,
        Role::Menu,
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
            role: Some(Role::MenuItem),
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
        for (column, (color, name)) in colors.iter().enumerate() {
            let index = 1 + row * columns + column;
            ui.open_as(
                cell(index),
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(CELL), px(CELL)],
                    fill: lit(index),
                    radius: 4.0,
                    pad: [3.0, 3.0],
                    role: Some(Role::MenuItem),
                    ..Spec::default()
                },
            );
            if let Some(node) = ui.access(cell(index)) {
                if name.is_empty() {
                    let [red, green, blue] = draw::srgb_bytes(*color);
                    node.set_label(format!("#{red:02X}{green:02X}{blue:02X}"));
                } else {
                    node.set_label(*name);
                }
            }
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
    highlighted(ui, id, highlight.map(cell));
    ui.close();
    None
}

/// Tells assistive technology the keys of `id` highlight `cell`, as a focus within it.
fn highlighted(ui: &mut Ui, id: Id, cell: Option<Id>) {
    if let (Some(cell), Some(node)) = (cell, ui.access(id)) {
        node.set_active_descendant(cell.node());
    }
}

/// Routes the pointer and keys over open popup `id`'s `count` cells, keeping its highlight,
/// which `step` moves for each arrow. Returns the highlight and the cell a click or Enter chose.
fn pick_cell(
    ui: &mut Ui,
    id: Id,
    count: usize,
    cell: impl Fn(usize) -> Id,
    step: impl Fn(NamedKey, Option<usize>) -> usize,
) -> (Option<usize>, Option<usize>) {
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
        match key {
            NamedKey::Enter => chosen = chosen.or(highlight),
            _ => highlight = Some(step(key, highlight)),
        }
    }
    state(ui, id).highlight = highlight.map(|cell| cell as u64);
    (highlight, chosen)
}

/// Builds popup `id` as a grid of `size` columns and rows beside `anchor` while it is open,
/// lit from its corner to the cell pointed at, as Office's table picker is. Returns the
/// columns and rows chosen.
pub fn table_picker(ui: &mut Ui, id: Id, anchor: Anchor, size: [usize; 2]) -> Option<[usize; 2]> {
    if !ui.popup_open(id) {
        return None;
    }
    let cell = |index: usize| id.child(("cell", index));
    let [columns, rows] = size;
    let (highlight, chosen) = pick_cell(ui, id, columns * rows, cell, |key, highlight| {
        let Some(at) = highlight else {
            return 0;
        };
        let [column, row] = [at % columns, at / columns];
        match key {
            NamedKey::ArrowLeft => at - usize::from(column > 0),
            NamedKey::ArrowRight => at + usize::from(column + 1 < columns),
            NamedKey::ArrowUp => at - if row > 0 { columns } else { 0 },
            _ => at + if row + 1 < rows { columns } else { 0 },
        }
    });
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
        Role::Menu,
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
                    role: Some(Role::MenuItem),
                    ..Spec::default()
                },
            );
            if let Some(node) = ui.access(cell(row * columns + column)) {
                node.set_label(format!("{}x{} Table", column + 1, row + 1));
            }
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
    highlighted(ui, id, highlight.map(cell));
    ui.close();
    None
}

/// A gallery's run of `cells` under `heading`, in rows of `columns` cells `size` large.
#[derive(Clone, Copy, Debug)]
pub struct Group<'a> {
    pub heading: &'a str,
    pub cells: usize,
    pub columns: usize,
    pub size: [f32; 2],
}

/// Builds popup `id` as a gallery beside `anchor` while it is open: `groups` of cells, with
/// the cells in `current` outlined. `cell` builds a cell's contents from its index, counted
/// through the groups. Returns the index of the cell chosen.
pub fn gallery(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    groups: &[Group],
    current: &[usize],
    mut cell: impl FnMut(&mut Ui, usize),
) -> Option<usize> {
    if !ui.popup_open(id) {
        return None;
    }
    let cell_id = |index: usize| id.child(("cell", index));
    let count: usize = groups.iter().map(|group| group.cells).sum();
    // The columns of the group holding cell `at`.
    let columns = |at: usize| {
        let mut start = 0;
        groups
            .iter()
            .find(|group| {
                start += group.cells;
                at < start
            })
            .map_or(1, |group| group.columns)
    };
    let (highlight, chosen) = pick_cell(ui, id, count, cell_id, |key, highlight| {
        let at = highlight.or(current.first().copied()).unwrap_or(0);
        match key {
            NamedKey::ArrowLeft => at.saturating_sub(1),
            NamedKey::ArrowRight => (at + 1).min(count - 1),
            NamedKey::ArrowUp => at.saturating_sub(columns(at)),
            _ => (at + columns(at)).min(count - 1),
        }
    });
    if let Some(index) = chosen {
        ui.close_popup(id);
        return Some(index);
    }
    let theme = ui.theme.clone();
    let width = groups
        .iter()
        .map(|group| group.columns as f32 * group.size[0])
        .fold(0.0, f32::max);
    surface(
        ui,
        id,
        Role::Menu,
        anchor,
        width + 2.0 * ui.theme.menu().pad,
    );
    let mut index = 0;
    for (number, group) in groups.iter().enumerate() {
        ui.leaf(
            ("heading", number),
            Spec {
                size: [fill(), px(MENU_ROW)],
                text: Some(group.heading),
                font_size: Some(theme.font_size - 2.0),
                bold: true,
                color: Some(theme.text_dim),
                pad: [PAD, 0.0],
                ..Spec::default()
            },
        );
        for row in 0..group.cells.div_ceil(group.columns) {
            ui.open(
                ("row", number, row),
                Spec {
                    size: [fill(), px(group.size[1])],
                    ..Spec::default()
                },
            );
            for _ in 0..group.columns.min(group.cells - row * group.columns) {
                ui.open_as(
                    cell_id(index),
                    Spec {
                        flags: Flags::CLICKABLE,
                        axis: Axis::Y,
                        size: [px(group.size[0]), px(group.size[1])],
                        fill: (highlight == Some(index)).then(|| theme.hover()),
                        border: current.contains(&index).then_some(theme.accent),
                        radius: 4.0,
                        pad: [PAD, PAD],
                        role: Some(Role::MenuItem),
                        ..Spec::default()
                    },
                );
                if current.contains(&index)
                    && let Some(node) = ui.access(cell_id(index))
                {
                    node.set_selected(true);
                }
                cell(ui, index);
                ui.close();
                index += 1;
            }
            ui.close();
        }
    }
    highlighted(ui, id, highlight.map(cell_id));
    ui.close();
    None
}

/// A colour picker's hue and lightness field, and its saturation bar beneath.
const FIELD: [f32; 2] = [256.0, 128.0];
const BAR: f32 = 14.0;
/// Columns the field is drawn in, each a gradient from white to the hue and to black.
const HUES: usize = 64;

/// Builds popup `id` beside `anchor` while it is open as a colour picker titled `title`,
/// starting from `initial`, sRGB: hue across a field drawn fully saturated, lightness down
/// it, saturation along a bar, and a preview of the colour as `preview` shows it, linear
/// RGBA. Returns the colour applied.
pub fn color_picker(
    ui: &mut Ui,
    id: Id,
    anchor: Anchor,
    title: &str,
    initial: [u8; 3],
    preview: impl Fn([u8; 3]) -> [f32; 4],
) -> Option<[u8; 3]> {
    if !ui.popup_open(id) {
        return None;
    }
    let [field, bar] = ["field", "saturation"].map(|part| id.child(part));
    let apply = id.child("footer").child("apply");
    let mut picked = state(ui, id).picked.unwrap_or_else(|| to_hsl(initial));
    let pointer = ui.pointer();
    let along = |ui: &Ui, part: Id| {
        let [left, top, right, bottom] = ui.rect(part)?;
        let [x, y] = pointer?;
        Some([
            ((x - left) / (right - left)).clamp(0.0, 1.0),
            ((y - top) / (bottom - top)).clamp(0.0, 1.0),
        ])
    };
    let held = |ui: &mut Ui, part: Id| {
        let signal = ui.signal(part);
        signal.pressed || signal.dragging
    };
    if held(ui, field)
        && let Some([x, y]) = along(ui, field)
    {
        picked[0] = x * 360.0;
        picked[2] = 1.0 - y;
    }
    if held(ui, bar)
        && let Some([x, _]) = along(ui, bar)
    {
        picked[1] = x;
    }
    let keys = navigation(ui, &[id], &[NamedKey::Enter]);
    state(ui, id).picked = Some(picked);
    let rgb = from_hsl(picked);
    if ui.signal(apply).clicked || !keys.is_empty() {
        ui.close_popup(id);
        return Some(rgb);
    }

    let theme = ui.theme.clone();
    surface(
        ui,
        id,
        Role::Dialog,
        anchor,
        FIELD[0] + 2.0 * ui.theme.menu().pad,
    );
    if let Some(node) = ui.access(id) {
        node.set_label(title);
    }
    let color = |hsl| {
        let [red, green, blue] = from_hsl(hsl);
        draw::srgb(red, green, blue)
    };
    let [hue, saturation, lightness] = picked;
    ui.leaf(
        "heading",
        Spec {
            size: [fill(), px(MENU_ROW)],
            text: Some(title),
            font_size: Some(theme.font_size - 2.0),
            bold: true,
            color: Some(theme.text_dim),
            pad: [PAD, 0.0],
            ..Spec::default()
        },
    );
    ui.open_as(
        field,
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(FIELD[0]), px(FIELD[1])],
            ..Spec::default()
        },
    );
    let column = FIELD[0] / HUES as f32;
    for index in 0..HUES {
        let shade = (index as f32 + 0.5) / HUES as f32 * 360.0;
        for (half, [top, bottom]) in [[1.0, 0.5], [0.5, 0.0]].into_iter().enumerate() {
            ui.leaf(
                (index, half),
                Spec {
                    flags: Flags::FLOAT,
                    position: [index as f32 * column, half as f32 * FIELD[1] / 2.0],
                    size: [px(column + 0.5), px(FIELD[1] / 2.0)],
                    fill: Some(color([shade, 1.0, top])),
                    gradient: Some(color([shade, 1.0, bottom])),
                    ..Spec::default()
                },
            );
        }
    }
    // A white ring inside a dark one, seen on any colour.
    let ring = |ui: &mut Ui, part: &str, at: [f32; 2]| {
        for (index, (radius, color)) in [(6.0, [0.0, 0.0, 0.0, 0.6]), (5.0, [1.0; 4])]
            .into_iter()
            .enumerate()
        {
            ui.leaf(
                (part, index),
                Spec {
                    flags: Flags::FLOAT,
                    position: [at[0] - radius, at[1] - radius],
                    size: [px(2.0 * radius), px(2.0 * radius)],
                    border: Some(color),
                    radius,
                    ..Spec::default()
                },
            );
        }
    };
    ring(
        ui,
        "ring",
        [hue / 360.0 * FIELD[0], (1.0 - lightness) * FIELD[1]],
    );
    ui.close();
    ui.open_as(
        bar,
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(FIELD[0]), px(BAR)],
            radius: 2.0,
            ..Spec::default()
        },
    );
    let steps = 32;
    let step = FIELD[0] / steps as f32;
    for index in 0..steps {
        ui.leaf(
            index,
            Spec {
                flags: Flags::FLOAT,
                position: [index as f32 * step, 0.0],
                size: [px(step + 0.5), px(BAR)],
                fill: Some(color([hue, index as f32 / (steps - 1) as f32, lightness])),
                ..Spec::default()
            },
        );
    }
    ring(ui, "ring", [saturation * FIELD[0], BAR / 2.0]);
    ui.close();
    ui.open(
        "footer",
        Spec {
            size: [fill(), px(ROW)],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "preview",
        Spec {
            size: [px(2.0 * ROW), px(ROW)],
            fill: Some(preview(rgb)),
            border: Some(theme.chip),
            radius: 4.0,
            ..Spec::default()
        },
    );
    let [red, green, blue] = rgb;
    let hex = format!("#{red:02X}{green:02X}{blue:02X}");
    ui.leaf(
        "hex",
        Spec {
            size: [fill(), px(ROW)],
            text: Some(&hex),
            color: Some(theme.text_dim),
            ..Spec::default()
        },
    );
    crate::button(ui, "apply", "Apply");
    ui.close();
    ui.close();
    None
}

/// Hue in degrees, saturation and lightness of an sRGB colour.
fn to_hsl(rgb: [u8; 3]) -> [f32; 3] {
    let [red, green, blue] = rgb.map(|channel| f32::from(channel) / 255.0);
    let (most, least) = (red.max(green).max(blue), red.min(green).min(blue));
    let lightness = (most + least) / 2.0;
    let chroma = most - least;
    if chroma == 0.0 {
        return [0.0, 0.0, lightness];
    }
    let saturation = chroma / (1.0 - (2.0 * lightness - 1.0).abs());
    let sector = if most == red {
        ((green - blue) / chroma).rem_euclid(6.0)
    } else if most == green {
        (blue - red) / chroma + 2.0
    } else {
        (red - green) / chroma + 4.0
    };
    [sector * 60.0, saturation, lightness]
}

fn from_hsl([hue, saturation, lightness]: [f32; 3]) -> [u8; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = (hue / 60.0).rem_euclid(6.0);
    let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let [red, green, blue] = match sector as u32 {
        0 => [chroma, second, 0.0],
        1 => [second, chroma, 0.0],
        2 => [0.0, chroma, second],
        3 => [0.0, second, chroma],
        4 => [second, 0.0, chroma],
        _ => [chroma, 0.0, second],
    };
    let base = lightness - chroma / 2.0;
    [red, green, blue].map(|channel| ((channel + base) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Opens popup `id`'s panel, a `role` `width` wide beside `anchor`; the caller closes it.
fn surface(ui: &mut Ui, id: Id, role: Role, anchor: Anchor, width: f32) {
    let style = ui.theme.menu();
    let pad = style.pad;
    // Level with its row, past the menu's edges.
    let beside = ui
        .popups
        .iter()
        .position(|popup| popup.id == id)
        .and_then(|at| {
            let [_, top, _, bottom] = ui.rect(ui.popups[at].beside?)?;
            let [left, _, right, _] = ui.rect(ui.popups[at.checked_sub(1)?].id)?;
            Some([left, top, right, bottom])
        });
    let anchor = match beside.map_or(anchor, Anchor::Right) {
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
        role: Some(role),
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
                    .filter(|(_, item)| !item.heading && !item.repeated)
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
            icons: icons(items),
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
