use crate::{Axis, Event, Flags, Id, Signal, Spec, Ui, fit, px};
use draw::edit::{self, Command, Movement};
use parley::{
    Affinity,
    editing::{Cursor, Selection},
};
use std::hash::Hash;
use winit::{
    event::Ime,
    keyboard::{Key, ModifiersState, NamedKey},
    window::CursorIcon,
};

/// A labelled push button that fills with the accent tint under the pointer.
pub fn button(ui: &mut Ui, part: impl Hash, text: &str) -> Signal {
    let theme = &ui.theme;
    let spec = Spec {
        flags: Flags::CLICKABLE,
        size: [fit(), px(theme.font_size * 2.0)],
        text: Some(text),
        fill: Some(theme.chip),
        hover_fill: Some(theme.hover()),
        hover_border: Some(theme.accent),
        radius: 4.0,
        pad: [theme.font_size * 0.75, 0.0],
        center: true,
        ..Spec::default()
    };
    ui.leaf(part, spec)
}

/// A check box before `label`, `checked` or not; a click on either reports for the caller
/// to toggle.
pub fn check_box(ui: &mut Ui, part: impl Hash, label: &str, checked: bool) -> Signal {
    let theme = ui.theme.clone();
    let [height, side] = [theme.font_size * 2.0, 16.0];
    let margin = (height - side) / 2.0;
    let id = ui.open(
        part,
        Spec {
            flags: Flags::CLICKABLE,
            size: [crate::children(), px(height)],
            gap: 8.0,
            ..Spec::default()
        },
    );
    let hovered = ui.signal(id).hovered;
    ui.leaf(
        "box",
        Spec {
            size: [px(side), px(height)],
            inset: [0.0, margin, 0.0, margin],
            fill: Some(if checked { theme.accent } else { theme.base }),
            border: Some(if checked || hovered {
                theme.accent
            } else {
                theme.chip
            }),
            radius: 3.0,
            icon: checked.then_some(crate::popup::CHECK),
            color: Some([1.0; 4]),
            ..Spec::default()
        },
    );
    ui.leaf(
        "label",
        Spec {
            size: [fit(), px(height)],
            text: Some(label),
            ..Spec::default()
        },
    );
    ui.close();
    ui.signal(id)
}

/// An overlay scrollbar along the far edge of the current box, for content whose scroll
/// offset ranges over `range` while `view` of it shows, all in one unit. Returns the offset
/// a drag of its thumb chose.
pub fn scrollbar(
    ui: &mut Ui,
    part: impl Hash,
    axis: Axis,
    offset: f32,
    range: [f32; 2],
    view: f32,
    color: [f32; 4],
) -> Option<f32> {
    let along = usize::from(axis == Axis::Y);
    let rect = ui.rect(ui.current())?;
    let length = rect[along + 2] - rect[along];
    let cross = rect[3 - along] - rect[1 - along];
    let span = range[1] - range[0];
    // The far end leaves room for the other axis's bar.
    let track = length - 20.0;
    if span <= 0.0 || track <= 0.0 {
        return None;
    }
    let size = (track * view / (view + span)).max(28.0).min(track);
    let start = 4.0 + ((offset - range[0]) / span).clamp(0.0, 1.0) * (track - size);
    let id = ui.id(&part);
    let signal = ui.signal(id);
    let pointer = ui.pointer().map(|pointer| pointer[along] - rect[along]);
    if signal.pressed
        && let Some(pointer) = pointer
    {
        *ui.grab(id) = pointer - start;
    }
    let chosen = signal.dragging.then_some(pointer).flatten().map(|pointer| {
        let travel = (track - size).max(f32::EPSILON);
        let fraction = ((pointer - *ui.grab(id) - 4.0) / travel).clamp(0.0, 1.0);
        range[0] + fraction * span
    });
    let mut position = [0.0; 2];
    position[along] = start;
    position[1 - along] = cross - 10.0;
    let mut extent = [px(6.0); 2];
    extent[along] = px(size);
    ui.leaf(
        &part,
        Spec {
            flags: Flags::CLICKABLE | Flags::FLOAT,
            size: extent,
            position,
            fill: Some(color),
            hover_fill: Some([color[0], color[1], color[2], (color[3] * 1.6).min(1.0)]),
            radius: 3.0,
            cursor: Some(CursorIcon::Default),
            ..Spec::default()
        },
    );
    chosen
}

/// A single-line field editing `text`, showing `placeholder` while empty. Presses, drags
/// and keys select and edit as on the page: a double press selects a word, a triple press
/// everything, and dragging extends by that unit.
pub fn text_field(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    placeholder: &str,
    spec: Spec<'_>,
) -> Signal {
    let signal = ui.signal(id);
    let pad = spec.pad[0];
    let size = ui.theme.font_size;
    let modifiers = edit_modifiers(ui.modifiers());
    let pointer = ui
        .pointer()
        .zip(ui.rect(id))
        .map(|(pointer, rect)| pointer[0] - rect[0] - pad)
        .filter(|_| signal.pressed || signal.dragging);
    let (mut selection, mut press) = {
        let (selection, press) = ui.field(id);
        (*selection, *press)
    };
    let before = [selection.anchor(), selection.focus()];
    let (texts, frame) = ui.texts();
    let mut label = texts.label(text, size, false, None, frame);
    if let Some(x) = pointer {
        let layout = &label.layout;
        let unit = if signal.pressed { signal.unit } else { press.1 };
        let hit = Cursor::from_point(layout, x, label.size[1] / 2.0);
        let target = edit::selection_at(layout, hit, x, unit);
        selection = if signal.pressed {
            let pressed = if modifiers.shift {
                Selection::new(selection.anchor(), target.focus())
            } else {
                target
            };
            press = (pressed, unit);
            pressed
        } else {
            let [anchor, focus] = edit::drag(
                [press.0.anchor(), press.0.focus()],
                [target.anchor(), target.focus()],
                unit,
                |cursor| cursor.index(),
            );
            Selection::new(anchor, focus)
        };
    }
    selection = selection.refresh(&label.layout);
    for event in &signal.events {
        let layout = &label.layout;
        let (range, inserted) = match event {
            Event::Ime(Ime::Commit(committed)) => (selection.text_range(), committed.as_str()),
            Event::Key { key, text: typed } => {
                let key = edit_key(key);
                // One line has no break for Control-K to join, nor room for Home, End and the
                // page keys to scroll.
                let command = match (Command::from_key(&key, modifiers), &key) {
                    (Some(Command::Kill), _) => Some(Command::DeleteTo(Movement::LineEnd)),
                    (Some(Command::ScrollPage { .. } | Command::MovePage { .. }), _) => None,
                    (None, edit::Key::Named(edit::NamedKey::Home)) => {
                        Some(Command::Move(Movement::DocumentStart))
                    }
                    (None, edit::Key::Named(edit::NamedKey::End)) => {
                        Some(Command::Move(Movement::DocumentEnd))
                    }
                    (command, _) => command,
                };
                let collapsed = selection.is_collapsed();
                match (command, key) {
                    (Some(Command::Move(movement)), _) => {
                        selection = edit::step(layout, selection, movement, modifiers.shift);
                        continue;
                    }
                    (Some(Command::DeleteTo(movement)), _) if collapsed => (
                        edit::step(layout, selection, movement, true).text_range(),
                        "",
                    ),
                    (Some(Command::Delete { backward }), _) if collapsed => {
                        let caret = selection.focus();
                        let cluster = caret.logical_clusters(layout)[usize::from(!backward)];
                        (
                            cluster.map_or(caret.index()..caret.index(), |cluster| {
                                cluster.text_range()
                            }),
                            "",
                        )
                    }
                    (Some(_), _) => (selection.text_range(), ""),
                    (None, edit::Key::Character(character))
                        if modifiers.command && character.eq_ignore_ascii_case("a") =>
                    {
                        selection = Selection::new(
                            Cursor::from_byte_index(layout, 0, Affinity::Downstream),
                            Cursor::from_byte_index(layout, usize::MAX, Affinity::Upstream),
                        );
                        continue;
                    }
                    (None, _) => match typed.as_deref().filter(|typed| {
                        !modifiers.command
                            && !modifiers.control
                            && !typed.is_empty()
                            && !typed.chars().any(char::is_control)
                    }) {
                        Some(typed) => (selection.text_range(), typed),
                        None => continue,
                    },
                }
            }
            _ => continue,
        };
        text.replace_range(range.clone(), inserted);
        label = texts.label(text, size, false, None, frame);
        selection = Selection::from_byte_index(
            &label.layout,
            range.start + inserted.len(),
            Affinity::Downstream,
        );
    }
    let moved = signal.pressed || before != [selection.anchor(), selection.focus()];
    {
        let (stored, stored_press) = ui.field(id);
        *stored = selection;
        *stored_press = press;
    }
    let theme = ui.theme.clone();
    let shown = if text.is_empty() {
        placeholder
    } else {
        text.as_str()
    };
    let height = spec.size[1];
    let backdrop = spec.fill.unwrap_or(theme.base);
    ui.open_as(
        id,
        Spec {
            flags: spec.flags | Flags::CLICKABLE | Flags::FOCUSABLE,
            text: Some(shown),
            color: Some(if text.is_empty() {
                theme.text_dim
            } else {
                theme.text
            }),
            cursor: Some(CursorIcon::Text),
            ..spec
        },
    );
    if signal.focused {
        let line = label.size[1];
        let top = match height.size {
            crate::Size::Pixels(height) => (height - line) / 2.0,
            _ => 0.0,
        };
        let fill = if ui.window_focused {
            theme.selection
        } else {
            theme.inactive_selection
        };
        for (rect, _) in selection.geometry(&label.layout) {
            ui.mark(
                [pad + rect.x0 as f32, top, pad + rect.x1 as f32, top + line],
                fill,
                0.0,
            );
        }
        if ui.window_focused && selection.is_collapsed() {
            let opacity = ui.blink(id, moved);
            let x = pad + selection.focus().geometry(&label.layout, 0.0).x0 as f32;
            let half = edit::CARET_WIDTH / 2.0;
            ui.mark(
                [x - half, top, x + half, top + line],
                edit::caret_color(theme.caret, backdrop, opacity),
                half,
            );
        }
    }
    ui.close();
    signal
}

/// A winit key in the editing vocabulary text fields and the page share.
pub fn edit_key(key: &Key) -> edit::Key {
    use edit::NamedKey as Edit;
    match key {
        Key::Character(text) => edit::Key::Character(text.to_string()),
        Key::Named(named) => edit::Key::Named(match named {
            NamedKey::Escape => Edit::Escape,
            NamedKey::Tab => Edit::Tab,
            NamedKey::Space => Edit::Space,
            NamedKey::Enter => Edit::Enter,
            NamedKey::Backspace => Edit::Backspace,
            NamedKey::Delete => Edit::Delete,
            NamedKey::ArrowLeft => Edit::ArrowLeft,
            NamedKey::ArrowRight => Edit::ArrowRight,
            NamedKey::ArrowUp => Edit::ArrowUp,
            NamedKey::ArrowDown => Edit::ArrowDown,
            NamedKey::Home => Edit::Home,
            NamedKey::End => Edit::End,
            NamedKey::PageUp => Edit::PageUp,
            NamedKey::PageDown => Edit::PageDown,
            NamedKey::Alt
            | NamedKey::AltGraph
            | NamedKey::Control
            | NamedKey::Shift
            | NamedKey::Super
            | NamedKey::Meta => Edit::Modifier,
            _ => Edit::Other,
        }),
        _ => edit::Key::Named(Edit::Other),
    }
}

pub fn edit_modifiers(modifiers: ModifiersState) -> edit::Modifiers {
    edit::Modifiers {
        shift: modifiers.shift_key(),
        control: modifiers.control_key(),
        option: modifiers.alt_key(),
        command: if edit::Platform::CURRENT == edit::Platform::MacOs {
            modifiers.super_key()
        } else {
            modifiers.control_key()
        },
    }
}
