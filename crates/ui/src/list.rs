//! A virtualized list of equal rows keyed by their items: only rows in view are built, and
//! the view holds its place on the selection while items arrive and leave above it.

use crate::{Axis, Event, Flags, HALF_LIFE, Id, Size, Spec, Ui, fill, mix, px, scrollbar};
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

/// A row to build: its item's key, its index, and whether it is selected.
pub struct Row {
    pub key: u64,
    pub index: usize,
    pub selected: bool,
}

/// A list's view across frames, in logical pixels.
#[derive(Default)]
pub(crate) struct State {
    scroll: f64,
    target: f64,
    /// Keys of the rows built last frame and their tops from the top of the view.
    shown: Vec<(u64, f64)>,
    /// The selection last frame, so a new one scrolls into view.
    selected: Option<u64>,
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
    let held = state.selected.and_then(|selected| {
        state
            .shown
            .iter()
            .find(|(key, place)| *key == selected && (0.0..view).contains(place))
    });
    // At the top, the view stays there to show what arrives.
    let anchor = held
        .into_iter()
        .chain(
            state
                .shown
                .iter()
                .filter(|(_, place)| state.scroll > 0.0 && place + height > 0.0),
        )
        .find_map(|(key, place)| Some(top(rows.find(*key)?) - place));
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
    for (key, _) in &state.shown {
        let signal = ui.signal(id.child(*key));
        let Some(index) = rows.find(*key).filter(|index| rows.selectable(*index)) else {
            continue;
        };
        if hover_selects && signal.hovered && ui.moved {
            *selected = Some(*key);
            state.selected = *selected;
        }
        if signal.clicked {
            *selected = Some(*key);
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

    let rate = 1.0 - 0.5_f64.powf(f64::from(ui.dt / HALF_LIFE));
    if fresh || (state.target - state.scroll).abs() < 0.25 {
        state.scroll = state.target;
    } else {
        state.scroll += (state.target - state.scroll) * rate;
    }
    let scroll = state.scroll;
    ui.animating |= scroll != state.target;

    // Rows fill all but a gutter for the scrollbar, from their first frame: the padding
    // narrows what they fill, and they float from the corner.
    let gutter = if most > 0.0 { GUTTER / 2.0 } else { 0.0 };
    ui.open_as(
        id,
        Spec {
            flags: spec.flags | Flags::SCROLL | Flags::CLIP,
            pad: [spec.pad[0] + gutter, spec.pad[1]],
            ..spec
        },
    );
    let width = fill();
    let in_view = row_at(scroll)..(row_at(scroll + view) + 1).min(len);
    let mut shown = Vec::with_capacity(in_view.len());
    for index in in_view {
        let key = rows.key(index);
        let place = top(index) - scroll;
        let flags = if rows.selectable(index) {
            Flags::FLOAT | Flags::CLICKABLE
        } else {
            Flags::FLOAT
        };
        ui.open(
            key,
            Spec {
                flags,
                size: [width, px(row)],
                position: [0.0, place as f32],
                ..Spec::default()
            },
        );
        build(
            ui,
            Row {
                key,
                index,
                selected: *selected == Some(key),
            },
        );
        ui.close();
        shown.push((key, place));
    }
    let thumb = mix(ui.theme.text_dim, ui.theme.chip, 0.5);
    if let Some(offset) = scrollbar(
        ui,
        "bar",
        Axis::Y,
        scroll as f32,
        [0.0, most as f32],
        view as f32,
        thumb,
    ) {
        // Rows keep their places relative to the new offset, or next frame's hold on the
        // first row showing would scroll straight back.
        let step = f64::from(offset) - scroll;
        for (_, place) in &mut shown {
            *place -= step;
        }
        state.scroll = f64::from(offset);
        state.target = state.scroll;
    }
    ui.close();
    state.shown = shown;
    ui.lists.insert(id, state);
    clicked
}
