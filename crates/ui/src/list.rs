//! A virtualized list of equal rows keyed by their items: only rows in view are built,
//! rows slide to where their items go, rows leaving fold into the seam their gap closes on
//! and rows arriving unfold out of it, and the view holds its place on the selection while
//! items arrive and leave above it.

use crate::{Axis, Event, Flags, HALF_LIFE, Id, Size, Spec, Ui, fill, mix, px, scrollbar};
use std::collections::{HashMap, HashSet};
use winit::keyboard::NamedKey;

/// Room beside a scrolling list's rows for its scrollbar.
pub(crate) const GUTTER: f32 = 12.0;
/// Seconds rows take to reach their new places.
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

/// A row to build: its item's key, its index while listed or None as it folds away after
/// its item left or moved, and whether it is selected.
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
    /// None for a row folding away after its item left or moved.
    index: Option<usize>,
    motion: Option<Motion>,
    /// Where it was drawn from the top of the view, and how strongly.
    drawn: f64,
    alpha: f32,
}

#[derive(Clone, Copy)]
struct Motion {
    elapsed: f32,
    way: Way,
}

#[derive(Clone, Copy)]
enum Way {
    /// Slides to its place from this offset.
    Slide(f64),
    Fold(Fold),
}

/// A row folding into the gap its run leaves between two rows, or unfolding out of the
/// gap its run opens, squeezed among the run and blending into the background as the gap
/// narrows.
#[derive(Clone, Copy)]
struct Fold {
    /// The gap's edges as it starts and ends, as offsets from the row's place.
    start: [f64; 2],
    end: [f64; 2],
    /// The row's position in its run, and the run's length.
    at: usize,
    of: usize,
    /// How strongly it was drawn as it set out.
    alpha: f32,
}

impl Fold {
    /// Where the row is drawn and how strongly, `progress` of the way through, never over
    /// the `solid` rows drawn at these tops, in order. Its text keeps inside the gap, gone
    /// before the gap is too narrow to hold it clear of the rows either side.
    fn at(&self, place: f64, progress: f64, height: f64, solid: &[f64]) -> (f64, f32) {
        let [top, bottom] = [0, 1]
            .map(|side| place + self.start[side] + (self.end[side] - self.start[side]) * progress);
        let below = solid.partition_point(|y| y + height / 2.0 <= (top + bottom) / 2.0);
        let top = below
            .checked_sub(1)
            .map_or(top, |above| top.max(solid[above] + height));
        let bottom = solid.get(below).map_or(bottom, |y| bottom.min(*y));
        let share = (bottom - top).max(0.0) / self.of as f64;
        let y = top + (self.at as f64 + 0.5) * share - height / 2.0;
        let room = (2.0 * share / height - 1.0).clamp(0.0, 1.0) as f32;
        (y, room * self.alpha)
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
    for shown in state.shown.iter().filter(|shown| shown.index.is_some()) {
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

    let dt = ui.dt;
    let tick = |motion: Motion| {
        let elapsed = motion.elapsed + dt;
        (elapsed < DURATION).then_some(Motion { elapsed, ..motion })
    };
    let progress = |motion: &Motion| 1.0 - (1.0 - f64::from(motion.elapsed / DURATION)).powi(3);
    let listed: Vec<Shown> = state
        .shown
        .iter()
        .filter(|shown| shown.index.is_some())
        .copied()
        .collect();
    let before: HashMap<u64, Shown> = listed.iter().map(|shown| (shown.key, *shown)).collect();
    // The most rows that stay in their order slide; the rest would pass over them, so fold
    // away and unfold where they go, as do rows moving while partly folded.
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
        .filter(|key| before[key].alpha >= 1.0)
        .collect();

    let settled = |index: usize| {
        let key = rows.key(index);
        let was = top(index) - anchored;
        let motion = before.get(&key).and_then(|old| {
            if (old.place - was).abs() < 0.5 {
                old.motion.and_then(tick)
            } else {
                tick(Motion {
                    elapsed: 0.0,
                    way: Way::Slide(old.drawn - was),
                })
            }
        });
        Shown {
            key,
            place: top(index) - scroll,
            index: Some(index),
            motion,
            drawn: 0.0,
            alpha: 1.0,
        }
    };
    // Rows arriving where the view already was, rather than scrolled into it, unfold.
    let arriving = |index: usize| {
        let key = rows.key(index);
        let was = top(index) - anchored;
        match before.get(&key) {
            Some(old) => (old.place - was).abs() >= 0.5 && !kept.contains(&key),
            None => !fresh && was + height > 0.0 && was < view,
        }
    };
    let in_view = row_at(scroll)..(row_at(scroll + view) + 1).min(len);
    let mut shown = Vec::new();
    let mut index = in_view.start;
    while index < in_view.end {
        if !arriving(index) {
            shown.push(settled(index));
            index += 1;
            continue;
        }
        let end = (index..in_view.end)
            .find(|index| !arriving(*index))
            .unwrap_or(in_view.end);
        let [above, below] = [index.checked_sub(1), (end < len).then_some(end)]
            .map(|side| side.map(|side| rows.key(side)));
        let [first, last] = [top(index), top(end - 1) + height].map(|y| y - anchored);
        // The seam opens where the rows either side were drawn, or mid-run with neither.
        let seam = above
            .and_then(|key| Some(before.get(&key)?.drawn + height))
            .or_else(|| below.and_then(|key| Some(before.get(&key)?.drawn)))
            .unwrap_or((first + last) / 2.0);
        shown.extend((index..end).map(|at| {
            let was = top(at) - anchored;
            Shown {
                motion: tick(Motion {
                    elapsed: 0.0,
                    way: Way::Fold(Fold {
                        start: [seam - was; 2],
                        end: [first - was, last - was],
                        at: at - index,
                        of: end - index,
                        alpha: 1.0,
                    }),
                }),
                ..settled(at)
            }
        }));
        index = end;
    }
    for old in &listed {
        if let Some(index) = rows
            .find(old.key)
            .filter(|index| !in_view.contains(index) && kept.contains(&old.key))
        {
            shown.push(settled(index));
        }
    }
    // Rows already folding away carry on; runs of rows gone or moving out of order start.
    let mut folding: Vec<Shown> = state
        .shown
        .iter()
        .filter(|shown| shown.index.is_none())
        .filter_map(|shown| {
            Some(Shown {
                place: shown.place - step,
                motion: Some(tick(shown.motion?)?),
                ..*shown
            })
        })
        .collect();
    let staying = |shown: &Shown| {
        kept.contains(&shown.key)
            || rows
                .find(shown.key)
                .is_some_and(|index| (top(index) - anchored - shown.place).abs() < 0.5)
    };
    let mut at = 0;
    for run in listed.chunk_by(|a, b| staying(a) == staying(b)) {
        let start = at;
        at += run.len();
        if staying(&run[0]) {
            continue;
        }
        let above = start.checked_sub(1).map(|side| listed[side].key);
        let below = listed.get(at).map(|side| side.key);
        let [first, last] = [run[0].drawn, run[run.len() - 1].drawn + height];
        // The seam the gap closes on: where the row below or above it now lies.
        let seam = below
            .and_then(|key| Some(top(rows.find(key)?) - anchored))
            .or_else(|| above.and_then(|key| Some(top(rows.find(key)?) + height - anchored)))
            .unwrap_or(first);
        folding.extend(run.iter().enumerate().map(|(at, old)| Shown {
            key: old.key,
            place: old.drawn - step,
            index: None,
            motion: tick(Motion {
                elapsed: 0.0,
                way: Way::Fold(Fold {
                    start: [first - old.drawn, last - old.drawn],
                    end: [seam - old.drawn; 2],
                    at,
                    of: run.len(),
                    alpha: old.alpha,
                }),
            }),
            drawn: 0.0,
            alpha: 0.0,
        }));
    }

    // Rows sliding or still are drawn first, as folds are bounded by them.
    for shown in &mut shown {
        if let Some(Motion {
            way: Way::Slide(from),
            ..
        }) = shown.motion
        {
            shown.drawn = shown.place + from * (1.0 - progress(&shown.motion.unwrap()));
        } else {
            shown.drawn = shown.place;
        }
    }
    let mut solid: Vec<f64> = shown
        .iter()
        .filter(|shown| {
            !matches!(
                shown.motion,
                Some(Motion {
                    way: Way::Fold(_),
                    ..
                })
            )
        })
        .map(|shown| shown.drawn)
        .collect();
    solid.sort_by(f64::total_cmp);
    for shown in shown.iter_mut().chain(&mut folding) {
        if let Some(
            motion @ Motion {
                way: Way::Fold(fold),
                ..
            },
        ) = shown.motion
        {
            (shown.drawn, shown.alpha) = fold.at(shown.place, progress(&motion), height, &solid);
        }
    }
    shown.retain(|shown| {
        shown.drawn + height > 0.0 && shown.drawn < view
            || shown.index.is_some_and(|index| in_view.contains(&index))
    });
    shown.sort_by(|a, b| a.place.total_cmp(&b.place));
    ui.animating |= state.scroll != state.target
        || !folding.is_empty()
        || shown.iter().any(|shown| shown.motion.is_some());

    let theme = ui.theme.clone();
    let background = spec.fill.unwrap_or(theme.base);
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
    // Rows folding away paint beneath the rest, and blend into the list rather than fade,
    // so those squeezed together never add up.
    for shown in folding.iter().chain(&shown) {
        let flags = match shown.index {
            Some(index) if rows.selectable(index) => Flags::FLOAT | Flags::CLICKABLE,
            _ => Flags::FLOAT,
        };
        let row_spec = Spec {
            flags,
            size: [width, px(row)],
            position: [0.0, shown.drawn as f32],
            fade: 1.0 - shown.alpha,
            fade_into: Some(background),
            ..Spec::default()
        };
        match shown.index {
            Some(_) => ui.open(shown.key, row_spec),
            None => ui.open(("folding", shown.key), row_spec),
        };
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
    shown.extend(folding);
    shown.sort_by(|a, b| a.place.total_cmp(&b.place));
    state.shown = shown;
    ui.lists.insert(id, state);
    clicked
}
