//! A virtualized list of equal rows keyed by their items: only rows in view are built,
//! rows ease to where their items move, and the view holds its place on the selection
//! while items arrive and leave above it.

use crate::{Axis, Event, Flags, Id, SLIDE, Size, Spec, Ui, fill, mix, px, scrollbar};
use std::collections::HashMap;
use winit::keyboard::NamedKey;

/// Room beside a scrolling list's rows for its scrollbar.
pub(crate) const GUTTER: f32 = 12.0;

/// Items a list shows, in order.
pub trait Rows {
    /// How many items are listed.
    fn count(&self) -> usize;
    /// The key naming the item at `index`, the same in every frame the item is listed.
    fn key(&self, index: usize) -> u64;
    /// Where the item named `key` is listed now.
    fn find(&self, key: u64) -> Option<usize>;
    /// Whether the selection may rest on row `index`.
    fn selectable(&self, _index: usize) -> bool {
        true
    }
    /// Extra room above row `index`, summed over it and the rows before; `index` may be
    /// the length.
    fn space_before(&self, _index: usize) -> f32 {
        0.0
    }
}

/// How a list shows and moves through its rows.
pub struct List<'a, R> {
    pub rows: &'a R,
    /// Every row's height.
    pub row: f32,
    /// Arrow, page, Home and End presses meant for the list this frame; others are ignored.
    pub keys: &'a [NamedKey],
    /// The selection follows the pointer, as in a menu.
    pub hover_selects: bool,
}

/// A row to build: its item's key, its index while listed or None as it fades out after
/// leaving, and whether it is selected.
pub struct Row {
    pub key: u64,
    pub index: Option<usize>,
    pub selected: bool,
}

/// A list's view across frames, in logical pixels.
#[derive(Default)]
pub(crate) struct State {
    scroll: f64,
    target: f64,
    /// Rows built last frame, from the top of the view down.
    shown: Vec<Shown>,
    /// The selection last frame, so a new one scrolls into view.
    selected: Option<u64>,
}

#[derive(Clone, Copy)]
struct Shown {
    key: u64,
    /// Where its row lies, from the top of the view.
    place: f64,
    /// How far from its place the row is drawn, easing to nothing.
    offset: f64,
    alpha: f32,
    index: Option<usize>,
}

/// Builds `list` into a box of `spec`, calling `build` inside each row in view, and moves
/// `selected` with the list's keys. Returns the index of the row clicked.
pub fn list<R: Rows>(
    ui: &mut Ui,
    id: Id,
    spec: Spec<'_>,
    list: List<'_, R>,
    selected: &mut Option<u64>,
    mut build: impl FnMut(&mut Ui, Row),
) -> Option<usize> {
    let List {
        rows,
        row,
        keys,
        hover_selects,
    } = list;
    let height = f64::from(row);
    let len = rows.count();
    let top = |index: usize| index as f64 * height + f64::from(rows.space_before(index));
    // The first row whose bottom lies below `y`.
    let row_at = |y: f64| {
        let [mut low, mut high] = [0, len];
        while low < high {
            let middle = (low + high) / 2;
            if top(middle) + height <= y {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    };
    let rect = ui.rect(id);
    let view = f64::from(match spec.size[1].size {
        Size::Pixels(pixels) => pixels,
        _ => rect.map_or(0.0, |rect| rect[3] - rect[1]),
    });
    let most = (top(len) - view).max(0.0);
    let fresh = !ui.lists.contains_key(&id);
    let mut state = ui.lists.remove(&id).unwrap_or_default();

    // Hold the view on the selection, or the first row showing, wherever it now lies.
    let listed = || state.shown.iter().filter(|shown| shown.index.is_some());
    let held = state.selected.and_then(|key| {
        listed().find(|shown| shown.key == key && shown.place >= 0.0 && shown.place < view)
    });
    // At the top, the view stays there to show what arrives.
    let anchor = held
        .into_iter()
        .chain(listed().filter(|shown| state.scroll > 0.0 && shown.place + height > 0.0))
        .find_map(|shown| Some(top(rows.find(shown.key)?) - shown.place));
    if let Some(scroll) = anchor {
        state.target += scroll - state.scroll;
        state.scroll = scroll;
    }
    state.scroll = state.scroll.clamp(0.0, most);
    state.target = state.target.clamp(0.0, most);
    let anchored = state.scroll;

    for event in ui.signal(id).events {
        if let Event::Wheel(delta) = event {
            state.target = (state.target - f64::from(delta[1])).clamp(0.0, most);
        }
    }
    let mut clicked = None;
    for shown in &state.shown {
        let signal = ui.signal(id.child(shown.key));
        let Some(index) = rows.find(shown.key).filter(|index| rows.selectable(*index)) else {
            continue;
        };
        if hover_selects && signal.hovered && ui.moved {
            *selected = Some(shown.key);
            state.selected = *selected;
        }
        if signal.clicked {
            *selected = Some(shown.key);
            clicked = Some(index);
        }
    }
    let forward = |from: usize| (from..len).find(|index| rows.selectable(*index));
    let backward = |from: usize| {
        (0..=from.min(len.saturating_sub(1)))
            .rev()
            .find(|index| rows.selectable(*index))
    };
    let page = ((view / height) as usize).max(1);
    let mut current = selected.and_then(|key| rows.find(key));
    for key in keys.iter().filter(|_| len > 0) {
        let next = match (key, current) {
            (NamedKey::ArrowDown, None) => forward(row_at(anchored)),
            (NamedKey::ArrowDown, Some(at)) => forward(at + 1),
            (NamedKey::ArrowUp, None) => backward(row_at(anchored + view).saturating_sub(1)),
            (NamedKey::ArrowUp, Some(at)) => at.checked_sub(1).and_then(backward),
            (NamedKey::PageDown, at) => {
                let to = at.map_or(0, |at| (at + page).min(len - 1));
                forward(to).or_else(|| backward(to))
            }
            (NamedKey::PageUp, at) => {
                let to = at.map_or(0, |at| at.saturating_sub(page));
                backward(to).or_else(|| forward(to))
            }
            (NamedKey::Home, _) => forward(0),
            (NamedKey::End, _) => backward(len - 1),
            _ => None,
        };
        if next.is_some() {
            current = next;
            *selected = current.map(|index| rows.key(index));
        }
    }
    if *selected != state.selected {
        if let Some(at) = selected.and_then(|key| rows.find(key)) {
            let lowest = (top(at) + height - view).min(top(at));
            state.target = state.target.clamp(lowest, top(at)).clamp(0.0, most);
        }
        state.selected = *selected;
    }

    let rate = f64::from(ui.rate);
    let settle = |value: f64, target: f64, close: f64| {
        if fresh || (target - value).abs() < close {
            target
        } else {
            value + (target - value) * rate
        }
    };
    state.scroll = settle(state.scroll, state.target, 0.25);
    let scroll = state.scroll;
    let step = scroll - anchored;

    // Where each row built last frame was drawn, in the view it was drawn in.
    let before: HashMap<u64, Shown> = state
        .shown
        .iter()
        .map(|shown| (shown.key, *shown))
        .collect();
    let place = |index: usize| {
        let key = rows.key(index);
        let was = top(index) - anchored;
        let (offset, alpha) = match before.get(&key) {
            Some(old) => (old.place + old.offset - was, old.alpha),
            None if !fresh && was + height > 0.0 && was < view => (-SLIDE as f64, 0.0),
            None => (0.0, 1.0),
        };
        Shown {
            key,
            place: top(index) - scroll,
            offset: settle(offset, 0.0, 0.5),
            alpha: settle(f64::from(alpha), 1.0, 0.01) as f32,
            index: Some(index),
        }
    };
    let in_view = row_at(scroll)..row_at(scroll + view + height).min(len);
    let mut shown: Vec<Shown> = in_view.clone().map(place).collect();
    let mut leaving = Vec::new();
    for old in before.values() {
        match rows.find(old.key) {
            Some(index) if !in_view.contains(&index) => {
                let moving = place(index);
                let drawn = moving.place + moving.offset;
                if drawn + height > 0.0 && drawn < view {
                    shown.push(moving);
                }
            }
            Some(_) => {}
            None => {
                let alpha = settle(f64::from(old.alpha), 0.0, 0.02) as f32;
                if alpha > 0.0 {
                    leaving.push(Shown {
                        place: old.place + old.offset - step,
                        offset: 0.0,
                        alpha,
                        index: None,
                        ..*old
                    });
                }
            }
        }
    }
    shown.sort_by(|a, b| a.place.total_cmp(&b.place));
    ui.animating |= state.scroll != state.target
        || !leaving.is_empty()
        || shown
            .iter()
            .any(|shown| shown.offset != 0.0 || shown.alpha < 1.0);

    let theme = ui.theme.clone();
    ui.open_as(
        id,
        Spec {
            flags: spec.flags | Flags::SCROLL | Flags::CLIP,
            ..spec
        },
    );
    // Rows leave the scrollbar a gutter.
    let width = match rect {
        Some(rect) if most > 0.0 => px(rect[2] - rect[0] - GUTTER),
        _ => fill(),
    };
    for shown in leaving.iter().chain(&shown) {
        let flags = match shown.index {
            Some(index) if rows.selectable(index) => Flags::FLOAT | Flags::CLICKABLE,
            _ => Flags::FLOAT,
        };
        ui.open(
            shown.key,
            Spec {
                flags,
                size: [width, px(row)],
                position: [0.0, (shown.place + shown.offset) as f32],
                fade: 1.0 - shown.alpha,
                ..Spec::default()
            },
        );
        build(
            ui,
            Row {
                key: shown.key,
                index: shown.index,
                selected: shown.index.is_some() && *selected == Some(shown.key),
            },
        );
        ui.close();
    }
    let thumb = mix(theme.text_dim, theme.chip, 0.5);
    if let Some(offset) = scrollbar(
        ui,
        "bar",
        Axis::Y,
        scroll as f32,
        [0.0, most as f32],
        view as f32,
        thumb,
    ) {
        state.scroll = f64::from(offset);
        state.target = state.scroll;
    }
    ui.close();
    shown.extend(leaving);
    state.shown = shown;
    ui.lists.insert(id, state);
    clicked
}
