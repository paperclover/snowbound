//! Text editing shared by the page and interface text: keys, macOS editing chords, click
//! counting and caret movement within one parley layout.

use parley::{
    Affinity, Brush, Layout,
    editing::{Cursor, Selection},
};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Named(NamedKey),
    Character(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NamedKey {
    Escape,
    Tab,
    Space,
    Enter,
    Backspace,
    Delete,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    /// Shift, Control, Option or Command pressed alone.
    Modifier,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    pub command: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    ParagraphStart,
    ParagraphEnd,
    DocumentStart,
    DocumentEnd,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionUnit {
    #[default]
    Grapheme,
    Word,
    Paragraph,
}

/// What a key does to text under macOS conventions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Moves the focus, extending the selection with Shift.
    Move(Movement),
    /// Deletes the selection, or the grapheme before or after the caret.
    Delete { backward: bool },
    /// Deletes the selection, or from the caret to where the movement would take it.
    DeleteTo(Movement),
    /// Control-K: deletes to the line's end, or the break after it when already there.
    Kill,
}

impl Command {
    pub fn from_key(key: &Key, modifiers: Modifiers) -> Option<Self> {
        let Modifiers {
            shift,
            control,
            option,
            command,
        } = modifiers;
        let pick = |plain, with_option, with_command| {
            Some(Self::Move(if command {
                with_command
            } else if option {
                with_option
            } else {
                plain
            }))
        };
        match key {
            Key::Named(NamedKey::ArrowLeft) => {
                pick(Movement::Left, Movement::WordLeft, Movement::LineStart)
            }
            Key::Named(NamedKey::ArrowRight) => {
                pick(Movement::Right, Movement::WordRight, Movement::LineEnd)
            }
            Key::Named(NamedKey::ArrowUp) => pick(
                Movement::Up,
                Movement::ParagraphStart,
                Movement::DocumentStart,
            ),
            Key::Named(NamedKey::ArrowDown) => pick(
                Movement::Down,
                Movement::ParagraphEnd,
                Movement::DocumentEnd,
            ),
            Key::Named(NamedKey::Home) if shift => Some(Self::Move(Movement::DocumentStart)),
            Key::Named(NamedKey::End) if shift => Some(Self::Move(Movement::DocumentEnd)),
            Key::Named(NamedKey::Backspace) if command || option => {
                Some(Self::DeleteTo(if command {
                    Movement::LineStart
                } else {
                    Movement::WordLeft
                }))
            }
            Key::Named(NamedKey::Delete) if command || option => Some(Self::DeleteTo(if command {
                Movement::LineEnd
            } else {
                Movement::WordRight
            })),
            Key::Named(NamedKey::Backspace) => Some(Self::Delete { backward: true }),
            Key::Named(NamedKey::Delete) => Some(Self::Delete { backward: false }),
            Key::Character(key) if control && !option => match key.as_str() {
                "a" => Some(Self::Move(Movement::LineStart)),
                "e" => Some(Self::Move(Movement::LineEnd)),
                "b" => Some(Self::Move(Movement::Left)),
                "f" => Some(Self::Move(Movement::Right)),
                "p" => Some(Self::Move(Movement::Up)),
                "n" => Some(Self::Move(Movement::Down)),
                "h" => Some(Self::Delete { backward: true }),
                "d" => Some(Self::Delete { backward: false }),
                "k" => Some(Self::Kill),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Counts presses into single, double and triple clicks.
#[derive(Debug)]
pub struct Clicks {
    interval: Duration,
    last: Option<(Instant, [f32; 2], SelectionUnit)>,
}

impl Clicks {
    /// `interval` is the platform's double-click interval.
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// Records a press at `point` and returns the unit it selects by. A press within the
    /// interval and `slop` of the previous one on each axis continues its count, cycling
    /// after three.
    pub fn press(&mut self, at: Instant, point: [f32; 2], slop: f32) -> SelectionUnit {
        let unit = self
            .last
            .filter(|(time, last, _)| {
                at.saturating_duration_since(*time) <= self.interval
                    && (0..2).all(|axis| (last[axis] - point[axis]).abs() <= slop)
            })
            .map_or(SelectionUnit::Grapheme, |(_, _, unit)| match unit {
                SelectionUnit::Grapheme => SelectionUnit::Word,
                SelectionUnit::Word => SelectionUnit::Paragraph,
                SelectionUnit::Paragraph => SelectionUnit::Grapheme,
            });
        self.last = Some((at, point, unit));
        unit
    }
}

/// The selection a drag makes from the selection its press made to the one under the
/// pointer, both by `unit`, ordered by `position`. Words and paragraphs stay whole, so
/// the press's selection stays covered when the drag reverses past it.
pub fn drag<T: Copy, P: PartialOrd>(
    anchor: [T; 2],
    target: [T; 2],
    unit: SelectionUnit,
    position: impl Fn(T) -> P,
) -> [T; 2] {
    if unit == SelectionUnit::Grapheme {
        return [anchor[0], target[1]];
    }
    let [anchor_start, anchor_end] = anchor.map(&position);
    let [target_start, target_end] = target.map(&position);
    let backwards = target_start < anchor_start && target_start < anchor_end;
    let start = usize::from((anchor_start > anchor_end) != backwards);
    let end = usize::from((target_start > target_end) == backwards);
    [anchor[start], target[end]]
}

/// The selection a press makes by `unit`, where `hit` is the cursor at the press's `x`.
pub fn selection_at<B: Brush>(
    layout: &Layout<B>,
    hit: Cursor,
    x: f32,
    unit: SelectionUnit,
) -> Selection {
    match unit {
        SelectionUnit::Grapheme => hit.into(),
        SelectionUnit::Word => {
            let rect = hit.geometry(layout, 1.0);
            Selection::word_from_point(layout, x, ((rect.y0 + rect.y1) * 0.5) as f32)
        }
        SelectionUnit::Paragraph => Selection::new(
            Cursor::from_byte_index(layout, 0, Affinity::Downstream),
            Cursor::from_byte_index(layout, usize::MAX, Affinity::Upstream),
        ),
    }
}

/// Where `movement` takes `selection` within `layout`, keeping its anchor when `extend`.
/// Up, Down and the paragraph and document movements stop at the layout's ends.
pub fn step<B: Brush>(
    layout: &Layout<B>,
    selection: Selection,
    movement: Movement,
    extend: bool,
) -> Selection {
    let to = |focus: Cursor| {
        if extend {
            selection.extend(focus)
        } else {
            focus.into()
        }
    };
    match movement {
        Movement::Left => selection.previous_visual(layout, extend),
        Movement::Right => selection.next_visual(layout, extend),
        Movement::WordLeft => to(word_cursor(layout, selection.focus(), true)),
        Movement::WordRight => to(word_cursor(layout, selection.focus(), false)),
        Movement::LineStart => selection.line_start(layout, extend),
        Movement::LineEnd => selection.line_end(layout, extend),
        Movement::Up | Movement::ParagraphStart | Movement::DocumentStart => {
            to(Cursor::from_byte_index(layout, 0, Affinity::Downstream))
        }
        Movement::Down | Movement::ParagraphEnd | Movement::DocumentEnd => to(
            Cursor::from_byte_index(layout, usize::MAX, Affinity::Upstream),
        ),
    }
}

/// The next word boundary in visual order, stopping before a word going left and after
/// one going right, as macOS does.
pub fn word_cursor<B: Brush>(layout: &Layout<B>, cursor: Cursor, backward: bool) -> Cursor {
    let mut current = cursor;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert((current.index(), current.affinity() == Affinity::Upstream)) {
            // Parley visual cursors can cycle at soft-wrapped bidi boundaries.
            let [left, right] = cursor.visual_clusters(layout);
            let rtl = if backward {
                left.or(right)
            } else {
                right.or(left)
            }
            .is_some_and(|cluster| cluster.is_rtl());
            return if backward != rtl {
                cursor.previous_logical_word(layout)
            } else {
                cursor.next_logical_word(layout)
            };
        }
        let next = if backward {
            current.previous_visual(layout)
        } else {
            current.next_visual(layout)
        };
        if next == current {
            return current;
        }
        current = next;
        let [Some(left), Some(right)] = current.visual_clusters(layout) else {
            return current;
        };
        let boundary = if left.is_rtl() {
            left.is_word_boundary()
                && if backward {
                    left.is_space_or_nbsp()
                        || (right.is_word_boundary() && !right.is_space_or_nbsp())
                } else {
                    !left.is_space_or_nbsp()
                }
        } else {
            right.is_word_boundary()
                && if backward {
                    !right.is_space_or_nbsp()
                } else {
                    !left.is_space_or_nbsp()
                }
        };
        if boundary {
            return current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parley::{FontContext, LayoutContext};

    fn layout(text: &str) -> Layout<()> {
        let mut fonts = FontContext::new();
        let mut context = LayoutContext::new();
        let mut layout = context
            .ranged_builder(&mut fonts, text, 1.0, false)
            .build(text);
        layout.break_all_lines(None);
        layout
    }

    #[test]
    fn keys_follow_macos_editing_chords() {
        let with = |modifiers: fn(&mut Modifiers)| {
            let mut value = Modifiers::default();
            modifiers(&mut value);
            value
        };
        let named = |named| Key::Named(named);
        let cases = [
            (
                named(NamedKey::ArrowLeft),
                with(|m| m.option = true),
                Some(Command::Move(Movement::WordLeft)),
            ),
            (
                named(NamedKey::ArrowRight),
                with(|m| (m.command, m.option) = (true, true)),
                Some(Command::Move(Movement::LineEnd)),
            ),
            (
                named(NamedKey::ArrowUp),
                with(|m| m.option = true),
                Some(Command::Move(Movement::ParagraphStart)),
            ),
            (named(NamedKey::Home), Modifiers::default(), None),
            (
                named(NamedKey::End),
                with(|m| m.shift = true),
                Some(Command::Move(Movement::DocumentEnd)),
            ),
            (
                named(NamedKey::Backspace),
                with(|m| m.option = true),
                Some(Command::DeleteTo(Movement::WordLeft)),
            ),
            (
                named(NamedKey::Delete),
                with(|m| m.command = true),
                Some(Command::DeleteTo(Movement::LineEnd)),
            ),
            (
                named(NamedKey::Backspace),
                with(|m| m.control = true),
                Some(Command::Delete { backward: true }),
            ),
            (
                Key::Character("e".into()),
                with(|m| m.control = true),
                Some(Command::Move(Movement::LineEnd)),
            ),
            (
                Key::Character("k".into()),
                with(|m| m.control = true),
                Some(Command::Kill),
            ),
            (
                Key::Character("k".into()),
                with(|m| (m.control, m.option) = (true, true)),
                None,
            ),
            (Key::Character("a".into()), Modifiers::default(), None),
        ];
        for (key, modifiers, command) in cases {
            assert_eq!(
                Command::from_key(&key, modifiers),
                command,
                "{key:?} {modifiers:?}"
            );
        }
    }

    #[test]
    fn presses_count_within_the_interval_and_slop_and_cycle() {
        let mut clicks = Clicks::new(Duration::from_millis(500));
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let units: Vec<_> = [(0, 0.0), (100, 3.0), (200, 3.0), (300, 0.0), (1000, 0.0)]
            .map(|(millis, x)| clicks.press(at(millis), [x, 0.0], 4.0))
            .into();
        use SelectionUnit::*;
        assert_eq!(units, [Grapheme, Word, Paragraph, Grapheme, Grapheme]);
        assert_eq!(clicks.press(at(1100), [10.0, 0.0], 4.0), Grapheme);
    }

    #[test]
    fn grouped_drag_keeps_the_initial_word_when_reversing_direction() {
        let drag = |anchor, target, unit| drag(anchor, target, unit, |offset: usize| offset);
        let word = [6, 10];
        assert_eq!(drag(word, [11, 16], SelectionUnit::Word), [6, 16]);
        assert_eq!(drag(word, [0, 5], SelectionUnit::Word), [10, 0]);
        assert_eq!(drag(word, word, SelectionUnit::Word), word);
        assert_eq!(drag([10, 3], [7, 7], SelectionUnit::Grapheme), [10, 7]);
    }

    #[test]
    fn words_stop_at_their_edges_in_both_directions() {
        let layout = layout("one two  three");
        let at = |index| Cursor::from_byte_index(&layout, index, Affinity::Downstream);
        let words = |start, backward| {
            std::iter::successors(Some(at(start)), |cursor| {
                let next = word_cursor(&layout, *cursor, backward);
                (next != *cursor).then_some(next)
            })
            .skip(1)
            .map(|cursor| cursor.index())
            .collect::<Vec<_>>()
        };
        assert_eq!(words(0, false), [3, 7, 14]);
        assert_eq!(words(14, true), [9, 4, 0]);
    }

    #[test]
    fn steps_collapse_extend_and_reach_the_ends() {
        let layout = layout("one two");
        let all = selection_at(&layout, Cursor::default(), 0.0, SelectionUnit::Paragraph);
        assert_eq!(all.text_range(), 0..7);
        let left = step(&layout, all, Movement::Left, false);
        assert_eq!(left.text_range(), 0..0);
        let word = step(&layout, left, Movement::WordRight, true);
        assert_eq!(word.text_range(), 0..3);
        assert_eq!(
            step(&layout, word, Movement::Down, false).focus().index(),
            7
        );
        assert_eq!(
            step(&layout, word, Movement::Up, true).text_range(),
            0..0,
            "extending keeps the anchor"
        );
    }
}
