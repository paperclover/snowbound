//! Text editing shared by the page and interface text: keys, each platform's editing chords
//! and caret, click counting and caret movement within one parley layout.

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
    PageUp,
    PageDown,
    /// Shift, Control, Option or Command pressed alone.
    Modifier,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    /// Option, or Alt.
    pub option: bool,
    /// The shortcut modifier: Command on macOS, Control elsewhere, where it comes with
    /// `control`.
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

/// What a key does to text under the platform's conventions.
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
    /// Scrolls the view a page, leaving the caret, as Page Up and Page Down do on macOS.
    ScrollPage { up: bool },
    /// Moves the caret a page and the view with it, extending the selection with Shift.
    MovePage { up: bool },
    /// Moves the selected paragraphs past their sibling above or below: OneNote's Alt+Shift+Up
    /// and Down, Option-Command on macOS as OneNote for Mac binds them.
    MoveParagraphs { up: bool },
}

impl Command {
    pub fn from_key(key: &Key, modifiers: Modifiers) -> Option<Self> {
        Platform::CURRENT.command(key, modifiers)
    }
}

/// A desktop's text editing conventions: its chords, and how its caret looks and blinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    /// GTK, as GNOME and most Linux desktops edit text.
    Gtk,
    Windows,
}

/// A caret's blink, in microseconds: solid for `delay` after it moves, then repeating
/// `phases` of fading out, off, fading in and on, each fade in `steps` equal steps, until
/// `timeout` after the move leaves it solid.
struct Blink {
    delay: u64,
    phases: [u64; 4],
    steps: u64,
    timeout: Option<u64>,
}

impl Platform {
    pub const CURRENT: Platform = if cfg!(target_vendor = "apple") {
        Platform::MacOs
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Gtk
    };

    pub fn command(self, key: &Key, modifiers: Modifiers) -> Option<Command> {
        match self {
            Platform::MacOs => mac_command(key, modifiers),
            Platform::Gtk | Platform::Windows => pc_command(key, modifiers),
        }
    }

    /// The caret's width in logical pixels: AppKit's insertion indicator, GTK's stem at
    /// its 0.04 aspect ratio for body text, and Windows' default caret.
    pub const fn caret_width(self) -> f32 {
        match self {
            Platform::MacOs => 2.0,
            Platform::Gtk | Platform::Windows => 1.0,
        }
    }

    fn blink(self) -> Blink {
        match self {
            // AppKit's insertion indicator: 350 ms off and 650 ms on, each opening with a
            // fade in four 37.5 ms steps.
            Platform::MacOs => Blink {
                delay: 650_000,
                phases: [150_000, 200_000, 150_000, 500_000],
                steps: 4,
                timeout: None,
            },
            // GtkText at the default 1200 ms blink time: half a cycle's pause after a
            // move, then quarters of on, fading out, off and fading in, stopping after the
            // default 10 s; its per-frame fade is taken in 60 Hz steps.
            Platform::Gtk => Blink {
                delay: 900_000,
                phases: [300_000; 4],
                steps: 18,
                timeout: Some(10_000_000),
            },
            // GetCaretBlinkTime's default 530 ms, without fades, until CaretTimeout's 5 s.
            Platform::Windows => Blink {
                delay: 530_000,
                phases: [0, 530_000, 0, 530_000],
                steps: 1,
                timeout: Some(5_000_000),
            },
        }
    }

    /// The caret's opacity `since` it last moved, and how long that opacity holds; None
    /// once it has stopped blinking.
    pub fn caret_blink(self, since: Duration) -> (f32, Option<Duration>) {
        let Blink {
            delay,
            phases,
            steps,
            timeout,
        } = self.blink();
        let period: u64 = phases.iter().sum();
        // The opacity at `time` and when it next may change.
        let at = |time: u64| -> (f32, u64) {
            if time < delay {
                return (1.0, delay);
            }
            let mut within = (time - delay) % period;
            let mut start = time - within;
            for (phase, length) in phases.into_iter().enumerate() {
                if within < length {
                    return match phase {
                        1 => (0.0, start + length),
                        3 => (1.0, start + length),
                        _ => {
                            let step = within * steps / length;
                            let shown = step as f32 / steps as f32;
                            (
                                if phase == 0 { 1.0 - shown } else { shown },
                                start + ((step + 1) * length).div_ceil(steps),
                            )
                        }
                    };
                }
                within -= length;
                start += length;
            }
            unreachable!("a time within the period falls in a phase")
        };
        let now = u64::try_from(since.as_micros()).unwrap_or(u64::MAX);
        if timeout.is_some_and(|timeout| now >= timeout) {
            return (1.0, None);
        }
        let (opacity, mut until) = at(now);
        // A settled phase holds into the next one's first step, which shows the same.
        for _ in 0..2 * (steps + 2) {
            let (next, end) = at(until);
            if next != opacity {
                break;
            }
            until = end;
        }
        match timeout {
            Some(timeout) if until >= timeout && opacity == 1.0 => (opacity, None),
            Some(timeout) if until >= timeout => {
                (opacity, Some(Duration::from_micros(timeout - now)))
            }
            _ => (opacity, Some(Duration::from_micros(until - now))),
        }
    }

    /// The caret's colour and selected text's fill with and without keyboard focus as sRGB
    /// and alpha, for a light or `dark` appearance: AppKit's system colours, libadwaita's
    /// text colour and translucent accent, and Windows' Highlight made translucent, as
    /// selected text keeps its colour here.
    pub fn text_colors(self, dark: bool) -> [([u8; 3], f32); 3] {
        match (self, dark) {
            (Platform::MacOs, false) => [
                ([0x00, 0x7a, 0xff], 1.0),
                ([0xb3, 0xd7, 0xff], 1.0),
                ([0xdc, 0xdc, 0xdc], 1.0),
            ],
            (Platform::MacOs, true) => [
                ([0x00, 0x7a, 0xff], 1.0),
                ([0x3f, 0x63, 0x8b], 1.0),
                ([0x46, 0x46, 0x46], 1.0),
            ],
            (Platform::Gtk, false) => [
                ([0x00, 0x00, 0x06], 0.8),
                ([0x35, 0x84, 0xe4], 0.3),
                ([0x00, 0x00, 0x06], 0.08),
            ],
            (Platform::Gtk, true) => [
                ([0xff, 0xff, 0xff], 1.0),
                ([0x35, 0x84, 0xe4], 0.3),
                ([0xff, 0xff, 0xff], 0.1),
            ],
            (Platform::Windows, false) => [
                ([0x00, 0x00, 0x00], 1.0),
                ([0x00, 0x78, 0xd7], 0.4),
                ([0x00, 0x00, 0x00], 0.1),
            ],
            (Platform::Windows, true) => [
                ([0xff, 0xff, 0xff], 1.0),
                ([0x00, 0x78, 0xd7], 0.5),
                ([0xff, 0xff, 0xff], 0.15),
            ],
        }
    }
}

/// AppKit's key bindings, including its Emacs chords on Control.
fn mac_command(key: &Key, modifiers: Modifiers) -> Option<Command> {
    let Modifiers {
        shift,
        control,
        option,
        command,
    } = modifiers;
    let pick = |plain, with_option, with_command| {
        Some(Command::Move(if command {
            with_command
        } else if option {
            with_option
        } else {
            plain
        }))
    };
    match key {
        Key::Named(arrow @ (NamedKey::ArrowUp | NamedKey::ArrowDown))
            if command && option && !shift && !control =>
        {
            Some(Command::MoveParagraphs {
                up: *arrow == NamedKey::ArrowUp,
            })
        }
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
        Key::Named(NamedKey::Home) if shift => Some(Command::Move(Movement::DocumentStart)),
        Key::Named(NamedKey::End) if shift => Some(Command::Move(Movement::DocumentEnd)),
        Key::Named(NamedKey::PageUp) if option => Some(Command::MovePage { up: true }),
        Key::Named(NamedKey::PageDown) if option => Some(Command::MovePage { up: false }),
        Key::Named(NamedKey::PageUp) => Some(Command::ScrollPage { up: true }),
        Key::Named(NamedKey::PageDown) => Some(Command::ScrollPage { up: false }),
        Key::Named(NamedKey::Backspace) if command || option => {
            Some(Command::DeleteTo(if command {
                Movement::LineStart
            } else {
                Movement::WordLeft
            }))
        }
        Key::Named(NamedKey::Delete) if command || option => Some(Command::DeleteTo(if command {
            Movement::LineEnd
        } else {
            Movement::WordRight
        })),
        Key::Named(NamedKey::Backspace) => Some(Command::Delete { backward: true }),
        Key::Named(NamedKey::Delete) => Some(Command::Delete { backward: false }),
        Key::Character(key) if control && !option => match key.as_str() {
            "a" => Some(Command::Move(Movement::LineStart)),
            "e" => Some(Command::Move(Movement::LineEnd)),
            "b" => Some(Command::Move(Movement::Left)),
            "f" => Some(Command::Move(Movement::Right)),
            "p" => Some(Command::Move(Movement::Up)),
            "n" => Some(Command::Move(Movement::Down)),
            "h" => Some(Command::Delete { backward: true }),
            "d" => Some(Command::Delete { backward: false }),
            "k" => Some(Command::Kill),
            _ => None,
        },
        _ => None,
    }
}

/// GTK's and Windows' shared bindings: Control moves and deletes by words, and with Home
/// and End reaches the document's ends; Control with Up and Down moves by paragraphs.
fn pc_command(key: &Key, modifiers: Modifiers) -> Option<Command> {
    let Modifiers {
        shift,
        control,
        option,
        ..
    } = modifiers;
    if option {
        return match key {
            _ if !shift || control => None,
            Key::Named(NamedKey::ArrowUp) => Some(Command::MoveParagraphs { up: true }),
            Key::Named(NamedKey::ArrowDown) => Some(Command::MoveParagraphs { up: false }),
            _ => None,
        };
    }
    let pick =
        |plain, with_control| Some(Command::Move(if control { with_control } else { plain }));
    match key {
        Key::Named(NamedKey::ArrowLeft) => pick(Movement::Left, Movement::WordLeft),
        Key::Named(NamedKey::ArrowRight) => pick(Movement::Right, Movement::WordRight),
        Key::Named(NamedKey::ArrowUp) => pick(Movement::Up, Movement::ParagraphStart),
        Key::Named(NamedKey::ArrowDown) => pick(Movement::Down, Movement::ParagraphEnd),
        Key::Named(NamedKey::Home) => pick(Movement::LineStart, Movement::DocumentStart),
        Key::Named(NamedKey::End) => pick(Movement::LineEnd, Movement::DocumentEnd),
        Key::Named(NamedKey::PageUp) if !control => Some(Command::MovePage { up: true }),
        Key::Named(NamedKey::PageDown) if !control => Some(Command::MovePage { up: false }),
        Key::Named(NamedKey::Backspace) if control => Some(Command::DeleteTo(Movement::WordLeft)),
        Key::Named(NamedKey::Delete) if control => Some(Command::DeleteTo(Movement::WordRight)),
        Key::Named(NamedKey::Backspace) => Some(Command::Delete { backward: true }),
        Key::Named(NamedKey::Delete) => Some(Command::Delete { backward: false }),
        _ => None,
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

/// The caret's width in logical pixels on this platform.
pub const CARET_WIDTH: f32 = Platform::CURRENT.caret_width();

/// The caret's opacity `since` it last moved on this platform; see [`Platform::caret_blink`].
pub fn caret_blink(since: Duration) -> (f32, Option<Duration>) {
    Platform::CURRENT.caret_blink(since)
}

/// `caret`, linear RGBA, with the alpha that shows it at `opacity` over `backdrop` as bright
/// as AppKit's layers do, which blend in sRGB rather than linear light.
pub fn caret_color(caret: [f32; 4], backdrop: [f32; 4], opacity: f32) -> [f32; 4] {
    let encode = |value: f32| {
        if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        }
    };
    let decode = |value: f32| {
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = |[red, green, blue]: [f32; 3]| 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    let [under, over] = [backdrop, caret].map(|color| [color[0], color[1], color[2]]);
    let shown: [f32; 3] = std::array::from_fn(|channel| {
        let [under, over] = [under[channel], over[channel]].map(encode);
        decode(under + opacity * (over - under))
    });
    let span = luminance(over) - luminance(under);
    let alpha = if span.abs() < 1e-4 {
        opacity
    } else {
        ((luminance(shown) - luminance(under)) / span).clamp(0.0, 1.0)
    };
    [caret[0], caret[1], caret[2], caret[3] * alpha]
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
            (
                named(NamedKey::ArrowDown),
                with(|m| (m.command, m.option) = (true, true)),
                Some(Command::MoveParagraphs { up: false }),
            ),
            (
                named(NamedKey::ArrowUp),
                with(|m| (m.shift, m.option) = (true, true)),
                Some(Command::Move(Movement::ParagraphStart)),
            ),
        ];
        for (key, modifiers, command) in cases {
            assert_eq!(
                Platform::MacOs.command(&key, modifiers),
                command,
                "{key:?} {modifiers:?}"
            );
        }
    }

    #[test]
    fn keys_follow_gtk_and_windows_editing_chords() {
        // Control arrives as the shortcut modifier too.
        let control = Modifiers {
            control: true,
            command: true,
            ..Modifiers::default()
        };
        let plain = Modifiers::default();
        let alt = Modifiers {
            option: true,
            ..Modifiers::default()
        };
        let alt_shift = Modifiers { shift: true, ..alt };
        let named = |named| Key::Named(named);
        let cases = [
            (
                named(NamedKey::ArrowUp),
                alt_shift,
                Some(Command::MoveParagraphs { up: true }),
            ),
            (named(NamedKey::ArrowUp), alt, None),
            (
                named(NamedKey::ArrowLeft),
                control,
                Some(Command::Move(Movement::WordLeft)),
            ),
            (
                named(NamedKey::ArrowRight),
                control,
                Some(Command::Move(Movement::WordRight)),
            ),
            (
                named(NamedKey::ArrowUp),
                control,
                Some(Command::Move(Movement::ParagraphStart)),
            ),
            (
                named(NamedKey::ArrowDown),
                plain,
                Some(Command::Move(Movement::Down)),
            ),
            (
                named(NamedKey::Home),
                plain,
                Some(Command::Move(Movement::LineStart)),
            ),
            (
                named(NamedKey::End),
                plain,
                Some(Command::Move(Movement::LineEnd)),
            ),
            (
                named(NamedKey::Home),
                control,
                Some(Command::Move(Movement::DocumentStart)),
            ),
            (
                named(NamedKey::End),
                control,
                Some(Command::Move(Movement::DocumentEnd)),
            ),
            (
                named(NamedKey::PageUp),
                plain,
                Some(Command::MovePage { up: true }),
            ),
            (
                named(NamedKey::PageDown),
                plain,
                Some(Command::MovePage { up: false }),
            ),
            (
                named(NamedKey::Backspace),
                control,
                Some(Command::DeleteTo(Movement::WordLeft)),
            ),
            (
                named(NamedKey::Delete),
                control,
                Some(Command::DeleteTo(Movement::WordRight)),
            ),
            (
                named(NamedKey::Backspace),
                plain,
                Some(Command::Delete { backward: true }),
            ),
            (named(NamedKey::ArrowLeft), alt, None),
            // No Emacs chords: Control-A and Control-E are shortcuts.
            (Key::Character("a".into()), control, None),
            (Key::Character("e".into()), control, None),
        ];
        for platform in [Platform::Gtk, Platform::Windows] {
            for (key, modifiers, command) in &cases {
                assert_eq!(
                    platform.command(key, *modifiers),
                    *command,
                    "{platform:?} {key:?} {modifiers:?}"
                );
            }
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

    #[test]
    fn caret_fades_as_bright_as_appkit_s_srgb_blend() {
        let linear = |value: f32| ((value / 255.0 + 0.055) / 1.055).powf(2.4);
        let pink = [248.0, 79.0, 158.0].map(linear);
        let pink = [pink[0], pink[1], pink[2], 1.0];
        let white = [1.0; 4];
        assert_eq!(caret_color(pink, white, 1.0)[3], 1.0);
        assert_eq!(caret_color(pink, white, 0.0)[3], 0.0);
        // Half the caret over white blends to sRGB green 167; TextEdit measures 170.
        let alpha = caret_color(pink, white, 0.5)[3];
        let green = 1.0 + alpha * (pink[1] - 1.0);
        assert!((green - linear(167.5)).abs() < 0.02, "{alpha}");
    }

    /// The opacity and hold in milliseconds `millis` after the caret moved.
    fn blink_at(platform: Platform, millis: f64) -> (f32, Option<f64>) {
        let (opacity, hold) = platform.caret_blink(Duration::from_secs_f64(millis / 1000.0));
        (opacity, hold.map(|hold| hold.as_secs_f64() * 1000.0))
    }

    fn assert_blink(platform: Platform, millis: f64, expected: (f32, Option<f64>)) {
        let (opacity, hold) = blink_at(platform, millis);
        assert_eq!(opacity, expected.0, "{platform:?} at {millis} ms");
        match (hold, expected.1) {
            (Some(hold), Some(expected)) => {
                assert!(
                    (hold - expected).abs() < 0.01,
                    "{platform:?} at {millis} ms holds {hold} ms, not {expected}"
                )
            }
            (hold, expected) => assert_eq!(hold, expected, "{platform:?} at {millis} ms"),
        }
    }

    /// How often the opacity changes over `span` milliseconds from `from`.
    fn blink_changes(platform: Platform, from: u32, span: u32) -> usize {
        let mut previous = blink_at(platform, f64::from(from)).0;
        (from + 1..=from + span)
            .filter(|&millis| {
                let opacity = blink_at(platform, f64::from(millis)).0;
                std::mem::replace(&mut previous, opacity) != opacity
            })
            .count()
    }

    #[test]
    fn caret_blinks_on_appkit_s_timing() {
        let mac = Platform::MacOs;
        assert_blink(mac, 0.0, (1.0, Some(687.5)));
        assert_blink(mac, 687.5, (0.75, Some(37.5)));
        assert_blink(mac, 760.0, (0.5, Some(2.5)));
        assert_blink(mac, 800.0, (0.0, Some(237.5)));
        assert_blink(mac, 1037.5, (0.25, Some(37.5)));
        assert_blink(mac, 1150.0, (1.0, Some(537.5)));
        assert_blink(mac, 1687.5, (0.75, Some(37.5)));
        assert_blink(mac, 3_600_200.0, (1.0, Some(487.5)));
        assert_eq!(
            blink_changes(mac, 1000, 1000),
            8,
            "four fade steps each way per second"
        );
    }

    #[test]
    fn caret_blinks_on_gtk_s_timing() {
        let gtk = Platform::Gtk;
        // Solid for half the 1200 ms cycle and its visible quarter, then fades out over 300 ms.
        assert_blink(gtk, 0.0, (1.0, Some(900.0 + 1000.0 / 60.0)));
        assert_blink(gtk, 1200.0, (0.0, Some(300.0 + 1000.0 / 60.0)));
        assert_blink(gtk, 1800.0, (1.0, Some(300.0 + 1000.0 / 60.0)));
        let (fading, _) = blink_at(gtk, 1050.0);
        assert!((fading - 0.5).abs() <= 1.0 / 18.0, "{fading}");
        // A fade each way per 1.2 s cycle, in 60 Hz steps.
        assert_eq!(blink_changes(gtk, 900, 1200), 2 * 18);
        // Ten seconds without moving leave it solid, cutting short the fade in.
        assert_blink(gtk, 9_700.0, (0.0, Some(216.667)));
        assert_blink(gtk, 9_990.0, (5.0 / 18.0, Some(10.0)));
        assert_blink(gtk, 10_000.0, (1.0, None));
    }

    #[test]
    fn caret_blinks_on_windows_timing() {
        let windows = Platform::Windows;
        assert_blink(windows, 0.0, (1.0, Some(530.0)));
        assert_blink(windows, 530.0, (0.0, Some(530.0)));
        assert_blink(windows, 1100.0, (1.0, Some(490.0)));
        assert_eq!(blink_changes(windows, 530, 1060), 2, "no fades");
        assert_blink(windows, 4800.0, (0.0, Some(200.0)));
        assert_blink(windows, 5000.0, (1.0, None));
    }

    #[test]
    fn carets_and_selections_match_each_platform() {
        assert_eq!(
            [Platform::MacOs, Platform::Gtk, Platform::Windows].map(Platform::caret_width),
            [2.0, 1.0, 1.0]
        );
        for platform in [Platform::MacOs, Platform::Gtk, Platform::Windows] {
            for dark in [false, true] {
                let [caret, focused, unfocused] = platform.text_colors(dark);
                assert!(caret.1 >= 0.8, "{platform:?} carets stand out");
                assert!(focused.1 > unfocused.1 || focused.0 != unfocused.0);
            }
        }
        // libadwaita tints selections with the accent at 30% and follows the text colour.
        let [caret, selection, _] = Platform::Gtk.text_colors(false);
        assert_eq!(selection, ([0x35, 0x84, 0xe4], 0.3));
        assert_eq!(caret, ([0x00, 0x00, 0x06], 0.8));
    }
}
