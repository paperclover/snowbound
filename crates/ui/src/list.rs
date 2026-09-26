//! A virtualized list of equal rows keyed by their items: only rows in view are built,
//! rows move to where their items go without passing over one another, and the view holds
//! its place on the selection while items arrive and leave above it.

use crate::{Axis, Event, Flags, HALF_LIFE, Id, Size, Spec, Ui, fill, mix, px, scrollbar};
use std::collections::{HashMap, HashSet};
use winit::keyboard::NamedKey;

/// Room beside a scrolling list's rows for its scrollbar.
pub(crate) const GUTTER: f32 = 12.0;
/// Seconds a row takes to reach its new place: half leaving where it was, half arriving.
const DURATION: f32 = 0.15;

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
    /// How far the furthest sliding row was from its place last frame.
    slack: f64,
}

#[derive(Clone, Copy)]
struct Shown {
    key: u64,
    /// Where its row lies, from the top of the view.
    place: f64,
    index: Option<usize>,
    motion: Option<Motion>,
}

impl Shown {
    /// Where the row is drawn from the top of the view, and its opacity.
    fn drawn(&self, reach: f64, slack: f64) -> (f64, f32) {
        self.motion.map_or((self.place, 1.0), |motion| {
            let (offset, alpha) = motion.at(reach, slack);
            (self.place + offset, alpha)
        })
    }
}

/// A row's way to its place after the items change, between offsets from the place.
#[derive(Clone, Copy)]
struct Motion {
    from: f64,
    to: f64,
    /// Its opacity as it set out.
    alpha: f32,
    style: Style,
    elapsed: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    /// A move keeping its order with the rows around: waits out those leaving, then slides.
    Slide,
    /// Fades out setting off towards the midpoint and in arriving from it, unseen where
    /// it passes other rows.
    Cross,
    /// Fades in arriving from the middle of the rows entering with it.
    Enter,
    /// Fades out setting off towards the middle of the rows leaving with it.
    Leave,
}

impl Motion {
    /// Its offset and opacity; beyond `reach` of both ends it cannot be seen, so its text
    /// never meets a neighbour's, nor arriving while a row sliding is `slack` from its place.
    fn at(self, reach: f64, slack: f64) -> (f64, f32) {
        let t = (self.elapsed / DURATION).min(1.0);
        let [first, second] = [(2.0 * t).min(1.0), (2.0 * t - 1.0).max(0.0)];
        let out = |x: f32| 1.0 - (1.0 - x).powi(3);
        let inward = |x: f32| x.powi(3);
        let leave = self.alpha * (1.0 - out(first));
        let (travel, fade) = match self.style {
            Style::Slide => (out(second), 1.0),
            Style::Cross => (
                0.5 * inward(first) + 0.5 * out(second),
                if t < 0.5 { leave } else { inward(second) },
            ),
            Style::Enter => (out(second), inward(second)),
            Style::Leave => (inward(first), leave),
        };
        let offset = self.from + (self.to - self.from) * f64::from(travel);
        let near = (offset - self.from).abs().min((offset - self.to).abs());
        let seen = |distance: f64| (1.0 - distance / reach).clamp(0.0, 1.0) as f32;
        let alpha = match self.style {
            Style::Slide => 1.0,
            Style::Enter => fade.min(seen(near)) * seen(slack),
            Style::Cross if t >= 0.5 => fade.min(seen(near)) * seen(slack),
            _ => fade.min(seen(near)),
        };
        (offset, alpha)
    }
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

    let rate = 1.0 - 0.5_f64.powf(f64::from(ui.dt / HALF_LIFE));
    if fresh || (state.target - state.scroll).abs() < 0.25 {
        state.scroll = state.target;
    } else {
        state.scroll += (state.target - state.scroll) * rate;
    }
    let scroll = state.scroll;
    let step = scroll - anchored;

    let reach = height / 6.0;
    let dt = ui.dt;
    let tick = |motion: Motion| {
        let elapsed = motion.elapsed + dt;
        (elapsed < DURATION).then_some(Motion { elapsed, ..motion })
    };
    // Rows built last frame, drawn in the view as anchored.
    let before: HashMap<u64, Shown> = state
        .shown
        .iter()
        .map(|shown| (shown.key, *shown))
        .collect();
    let listed: Vec<&Shown> = state
        .shown
        .iter()
        .filter(|shown| shown.index.is_some())
        .collect();
    // The most rows staying listed that keep their order slide; the rest would pass over
    // them, so cross unseen.
    let order: Vec<(u64, usize)> = listed
        .iter()
        .filter_map(|shown| Some((shown.key, rows.find(shown.key)?)))
        .collect();
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; order.len()];
    for at in 0..order.len() {
        let length = tails.partition_point(|tail| order[*tail].1 < order[at].1);
        previous[at] = length.checked_sub(1).map(|length| tails[length]);
        if length == tails.len() {
            tails.push(at);
        } else {
            tails[length] = at;
        }
    }
    let kept: HashSet<u64> = std::iter::successors(tails.last().copied(), |at| previous[*at])
        .map(|at| order[at].0)
        .collect();
    let arrive = |index: usize| {
        let key = rows.key(index);
        let was = top(index) - anchored;
        let motion = before.get(&key).and_then(|old| {
            if old.index.is_some() && (old.place - was).abs() < 0.5 {
                return old.motion.and_then(tick);
            }
            let (drawn, alpha) = old.drawn(reach, state.slack);
            let from = drawn - was;
            let style = if alpha < 1.0 || !kept.contains(&key) {
                Style::Cross
            } else {
                Style::Slide
            };
            tick(Motion {
                from,
                to: 0.0,
                alpha,
                style,
                elapsed: 0.0,
            })
        });
        Shown {
            key,
            place: top(index) - scroll,
            index: Some(index),
            motion,
        }
    };
    // Rows newly where the view already was, rather than scrolled into it, enter.
    let entering = |index: usize| {
        let was = top(index) - anchored;
        !fresh && !before.contains_key(&rows.key(index)) && was + height > 0.0 && was < view
    };
    let in_view = row_at(scroll)..(row_at(scroll + view) + 1).min(len);
    let mut shown = Vec::new();
    let mut index = in_view.start;
    while index < in_view.end {
        if !entering(index) {
            shown.push(arrive(index));
            index += 1;
            continue;
        }
        let end = (index..in_view.end)
            .find(|index| !entering(*index))
            .unwrap_or(in_view.end);
        let middle = (top(index) + top(end - 1)) / 2.0;
        shown.extend((index..end).map(|index| Shown {
            key: rows.key(index),
            place: top(index) - scroll,
            index: Some(index),
            motion: tick(Motion {
                from: middle - top(index),
                to: 0.0,
                alpha: 0.0,
                style: Style::Enter,
                elapsed: 0.0,
            }),
        }));
        index = end;
    }
    for old in &listed {
        if let Some(index) = rows.find(old.key).filter(|index| !in_view.contains(index)) {
            let moving = arrive(index);
            let drawn = moving.drawn(reach, state.slack).0;
            if drawn + height > 0.0 && drawn < view {
                shown.push(moving);
            }
        }
    }
    let mut leaving: Vec<Shown> = state
        .shown
        .iter()
        .filter(|shown| shown.index.is_none() && rows.find(shown.key).is_none())
        .filter_map(|shown| {
            Some(Shown {
                place: shown.place - step,
                motion: Some(tick(shown.motion?)?),
                ..*shown
            })
        })
        .collect();
    let gone = |shown: &&Shown| rows.find(shown.key).is_none();
    for run in listed.chunk_by(|a, b| gone(a) == gone(b)) {
        if !gone(&run[0]) {
            continue;
        }
        let drawn: Vec<_> = run
            .iter()
            .map(|shown| shown.drawn(reach, state.slack))
            .collect();
        let middle = (drawn[0].0 + drawn[drawn.len() - 1].0) / 2.0;
        leaving.extend(run.iter().zip(drawn).map(|(shown, (y, alpha))| Shown {
            key: shown.key,
            place: y - step,
            index: None,
            motion: tick(Motion {
                from: 0.0,
                to: middle - y,
                alpha,
                style: Style::Leave,
                elapsed: 0.0,
            }),
        }));
    }
    shown.sort_by(|a, b| a.place.total_cmp(&b.place));
    let slack = shown
        .iter()
        .filter_map(|shown| shown.motion.filter(|motion| motion.style == Style::Slide))
        .map(|motion| motion.at(reach, 0.0).0.abs())
        .fold(0.0, f64::max);
    ui.animating |= state.scroll != state.target
        || !leaving.is_empty()
        || shown.iter().any(|shown| shown.motion.is_some());

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
        let (y, alpha) = shown.drawn(reach, slack);
        ui.open(
            shown.key,
            Spec {
                flags,
                size: [width, px(row)],
                position: [0.0, y as f32],
                fade: 1.0 - alpha,
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
    shown.sort_by(|a, b| a.place.total_cmp(&b.place));
    state.shown = shown;
    state.slack = slack;
    ui.lists.insert(id, state);
    clicked
}
