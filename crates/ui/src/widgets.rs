use crate::{Axis, Event, Flags, Id, Signal, Spec, Ui, fit, px};
use std::hash::Hash;
use winit::{
    event::Ime,
    keyboard::{Key, NamedKey},
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
        hover_fill: Some(theme.hover),
        hover_border: Some(theme.accent),
        radius: 4.0,
        pad: [theme.font_size * 0.75, 0.0],
        center: true,
        ..Spec::default()
    };
    ui.leaf(part, spec)
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

/// A single-line field editing `text`, showing `placeholder` while empty. Typing,
/// Backspace/Delete, Left/Right with Shift to select, Home/End, Command-A and input
/// method commits edit it; a press places the caret.
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
    let clicked_at = signal
        .pressed
        .then(|| ui.pointer().zip(ui.rect(id)))
        .flatten()
        .map(|(pointer, rect)| pointer[0] - rect[0] - pad);
    let command = ui.modifiers().super_key() || ui.modifiers().control_key();
    let shift = ui.modifiers().shift_key();
    let (mut caret, mut mark) = {
        let (caret, mark) = ui.caret(id);
        (*caret, *mark)
    };
    caret = floor_boundary(text, caret);
    mark = floor_boundary(text, mark);
    if let Some(x) = clicked_at {
        let (texts, frame) = ui.texts();
        caret = texts.label(text, size, frame).index_at(x);
        mark = caret;
    }
    for event in &signal.events {
        let selection = caret.min(mark)..caret.max(mark);
        let insert = |text: &mut String, inserted: &str, caret: &mut usize| {
            text.replace_range(selection.clone(), inserted);
            *caret = selection.start + inserted.len();
        };
        match event {
            Event::Ime(Ime::Commit(committed)) => {
                insert(text, committed, &mut caret);
                mark = caret;
            }
            Event::Key { key, text: typed } => match key {
                Key::Named(NamedKey::Backspace | NamedKey::Delete) => {
                    if selection.is_empty() {
                        let edge = if matches!(key, Key::Named(NamedKey::Backspace)) {
                            previous(text, caret)..caret
                        } else {
                            caret..next(text, caret)
                        };
                        text.replace_range(edge.clone(), "");
                        caret = edge.start;
                    } else {
                        insert(text, "", &mut caret);
                    }
                    mark = caret;
                }
                Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight) => {
                    let left = matches!(key, Key::Named(NamedKey::ArrowLeft));
                    caret = match (shift, selection.is_empty(), left) {
                        (false, false, true) => selection.start,
                        (false, false, false) => selection.end,
                        (_, _, true) => previous(text, caret),
                        (_, _, false) => next(text, caret),
                    };
                    if !shift {
                        mark = caret;
                    }
                }
                Key::Named(NamedKey::Home | NamedKey::End) => {
                    caret = if matches!(key, Key::Named(NamedKey::Home)) {
                        0
                    } else {
                        text.len()
                    };
                    if !shift {
                        mark = caret;
                    }
                }
                Key::Character(character) if command => {
                    if character.eq_ignore_ascii_case("a") {
                        mark = 0;
                        caret = text.len();
                    }
                }
                _ => {
                    if let Some(typed) = typed.as_deref().filter(|typed| {
                        !command && !typed.is_empty() && !typed.chars().any(char::is_control)
                    }) {
                        insert(text, typed, &mut caret);
                        mark = caret;
                    }
                }
            },
            _ => {}
        }
    }
    {
        let (stored_caret, stored_mark) = ui.caret(id);
        *stored_caret = caret;
        *stored_mark = mark;
    }
    let theme = ui.theme.clone();
    let shown = if text.is_empty() {
        placeholder
    } else {
        text.as_str()
    };
    let height = spec.size[1];
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
        let (texts, frame) = ui.texts();
        let label = texts.label(text, size, frame);
        let line = label.size[1];
        let [start, end] =
            [caret.min(mark), caret.max(mark)].map(|index| pad + label.caret_x(index));
        let top = match height.size {
            crate::Size::Pixels(height) => (height - line) / 2.0,
            _ => 0.0,
        };
        if end > start {
            ui.mark([start, top, end, top + line], theme.hover);
        }
        let x = pad + label.caret_x(caret);
        ui.mark([x, top, x + 1.0, top + line], theme.text);
    }
    ui.close();
    signal
}

fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn previous(text: &str, index: usize) -> usize {
    text[..index]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next(text: &str, index: usize) -> usize {
    text[index..]
        .chars()
        .next()
        .map_or(index, |character| index + character.len_utf8())
}
