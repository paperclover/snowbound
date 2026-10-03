use crate::{Axis, Event, Flags, Id, Signal, Spec, Ui, fit, px};
use accesskit::Role;
use draw::RasterImage;
use draw::edit::{self, Command, Movement};
use parley::{
    Affinity,
    editing::{Cursor, Selection},
};
use std::{hash::Hash, time::Duration};
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
        role: Some(Role::Button),
        ..Spec::default()
    };
    ui.leaf(part, spec)
}

/// Cancel and a dialog's default button `text`, which takes no clicks while not `enabled`,
/// in the platform's order: the default last on macOS and GNOME, first on Windows, as their
/// own dialogs place them. The default wears the accent around it. Whether each was clicked,
/// Cancel first.
pub fn dialog_buttons(ui: &mut Ui, text: &str, enabled: bool) -> [bool; 2] {
    let default = |ui: &mut Ui| {
        let theme = &ui.theme;
        let spec = Spec {
            flags: if enabled {
                Flags::CLICKABLE
            } else {
                Flags::default()
            },
            size: [fit(), px(theme.font_size * 2.0)],
            text: Some(text),
            color: (!enabled).then_some(theme.text_dim),
            fill: Some(theme.chip),
            hover_fill: enabled.then(|| theme.hover()),
            border: enabled.then_some(theme.accent),
            radius: 4.0,
            pad: [theme.font_size * 0.75, 0.0],
            center: true,
            role: Some(Role::Button),
            ..Spec::default()
        };
        let clicked = ui.leaf("default", spec).clicked;
        if !enabled && let Some(node) = ui.access(ui.id("default")) {
            node.set_disabled();
        }
        clicked
    };
    let windows = edit::Platform::CURRENT == edit::Platform::Windows;
    let mut chosen = windows && default(ui);
    let cancel = button(ui, "cancel", "Cancel").clicked;
    if !windows {
        chosen = default(ui);
    }
    [cancel, chosen]
}

/// A short tag in the accent's colour, centred in a box `height` tall, as BETA marks a
/// feature still settling beside its name.
pub fn badge(ui: &mut Ui, part: impl Hash, text: &str, height: f32) {
    let theme = &ui.theme;
    let [red, green, blue, _] = theme.accent;
    let tall = (theme.font_size * 1.15).min(height).round();
    let spec = Spec {
        size: [fit(), px(tall)],
        text: Some(text),
        font_size: Some((theme.font_size * 0.7).round()),
        bold: true,
        color: Some(theme.accent),
        fill: Some([red, green, blue, 0.16]),
        radius: 3.0,
        pad: [4.0, 0.0],
        center: true,
        ..Spec::default()
    };
    ui.open(
        part,
        Spec {
            size: [crate::children(), px(height)],
            pad: [0.0, ((height - tall) / 2.0).max(0.0)],
            ..Spec::default()
        },
    );
    ui.leaf("text", spec);
    ui.close();
}

/// The width `badge` takes for `text`.
pub fn badge_width(ui: &mut Ui, text: &str) -> f32 {
    let size = (ui.theme.font_size * 0.7).round();
    ui.texts.label(text, size, ui.frame).size[0] + 8.0
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
            role: Some(Role::CheckBox),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(id) {
        node.set_toggled(checked.into());
    }
    // A box's signal is taken once, so its hover and click are read together.
    let signal = ui.signal(id);
    ui.leaf(
        "box",
        Spec {
            size: [px(side), px(height)],
            inset: [0.0, margin, 0.0, margin],
            fill: Some(if checked { theme.accent } else { theme.base }),
            border: Some(if checked || signal.hovered {
                theme.accent
            } else {
                crate::mix(theme.chip, theme.text, 0.45)
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
    signal
}

/// How a segmented control draws its choice, as the platform draws its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Segments {
    /// A raised face sliding along a sunken track: macOS since 11, libadwaita's toggle
    /// groups and Windows 11's segmented control.
    #[default]
    Track,
    /// Buttons joined in a row, the chosen one pressed in: Mac OS X 10.6's segmented control
    /// and Windows 7's toggle groups.
    Joined,
}

/// `choices` side by side, equally wide, with `chosen` picked: a radio group named `label`
/// for assistive technology. Tab enters at the choice, and arrows pick the one beside it.
/// Returns the choice a click or the keyboard picks.
pub fn segmented(
    ui: &mut Ui,
    part: impl Hash,
    label: &str,
    choices: &[&str],
    chosen: usize,
) -> Option<usize> {
    let theme = ui.theme.clone();
    let joined = ui.segments == Segments::Joined;
    let dark = draw::oklab(theme.text)[0] > 0.5;
    let [red, green, blue, _] = theme.text;
    let ink = |alpha| [red, green, blue, alpha];
    let height = theme.font_size * 2.0;
    let widest = (choices.iter()).fold(0.0_f32, |widest, choice| widest.max(ui.measure(choice)[0]));
    let width = (widest + 2.0 * theme.font_size).round();
    // A track shows around its face; joined buttons fill their border.
    let inset = if joined { 1.0 } else { 2.0 };
    let button = |shade: f32| {
        if dark {
            crate::mix(theme.popup, theme.text, shade)
        } else {
            crate::mix([1.0; 4], theme.chip, 0.75 - shade * 4.0)
        }
    };
    let group = ui.open(
        part,
        Spec {
            size: [crate::children(), px(height)],
            fill: Some(if joined {
                button(0.12)
            } else {
                ink(if dark { 0.1 } else { 0.07 })
            }),
            gradient: joined.then(|| button(0.06)),
            border: joined.then_some(theme.chip),
            radius: if joined { 4.0 } else { 6.0 },
            pad: [inset, inset],
            role: Some(Role::RadioGroup),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(group) {
        node.set_label(label);
        node.set_orientation(accesskit::Orientation::Horizontal);
    }
    let target = chosen as f32 * width;
    let slid = if joined {
        ui.hold(group.child("slide"), target)
    } else {
        ui.animate(group.child("slide"), target)
    };
    ui.leaf(
        "thumb",
        Spec {
            flags: Flags::FLOAT,
            size: [px(width), px(height - 2.0 * inset)],
            position: [inset + slid, inset],
            fill: Some(match (joined, dark) {
                (true, _) => theme.accent,
                (false, true) => crate::mix(theme.popup, theme.text, 0.3),
                (false, false) => [1.0; 4],
            }),
            gradient: joined.then(|| crate::mix(theme.accent, [0.0, 0.0, 0.0, 1.0], 0.25)),
            border: (!joined && !dark).then(|| ink(0.08)),
            shadow: (!joined && !dark).then_some([0.0, 0.0, 0.0, 0.12]),
            radius: if joined { 3.0 } else { 4.0 },
            ..Spec::default()
        },
    );
    // Rules between the choices; a track leaves them out beside its face.
    for between in 1..choices.len() {
        if !joined && (between == chosen || between == chosen + 1) {
            continue;
        }
        let length = if joined { height - 2.0 } else { height * 0.5 };
        ui.leaf(
            ("rule", between),
            Spec {
                flags: Flags::FLOAT,
                size: [px(1.0), px(length)],
                position: [
                    inset + between as f32 * width - 0.5,
                    (height - length) / 2.0,
                ],
                fill: Some(if joined { theme.chip } else { ink(0.15) }),
                ..Spec::default()
            },
        );
    }
    let mut picked = None;
    for (index, choice) in choices.iter().enumerate() {
        let on = index == chosen;
        let id = ui.open(
            index,
            Spec {
                flags: Flags::CLICKABLE,
                size: [px(width), crate::fill()],
                text: Some(choice),
                color: Some(if on && joined { [1.0; 4] } else { theme.text }),
                hover_fill: (!on).then(|| ink(0.06)),
                radius: if joined { 3.0 } else { 4.0 },
                center: true,
                role: Some(Role::RadioButton),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id) {
            node.set_toggled(on.into());
        }
        ui.close();
        let signal = ui.signal(id);
        // The keyboard's arrows pick the segment they move to, as a radio group's do.
        let arrived = ui.lasted(id.child("focus"), signal.focused) == Duration::ZERO;
        if !on && (signal.clicked || signal.focused && arrived) {
            picked = Some(index);
        }
    }
    ui.close();
    picked
}

/// Scrollers the platform paints, fixed along the content's edges, as Mac OS X 10.6's are.
pub struct Scrollers {
    /// Across a scroller, in logical pixels.
    pub thickness: f32,
    pub paint: Box<dyn FnMut(&Scroller) -> PaintedScroller>,
}

/// A scroller as it is to be painted, in logical pixels.
pub struct Scroller {
    pub axis: Axis,
    pub length: f32,
    pub scale: f32,
    /// Where the offset lies in its range, from 0 to 1.
    pub value: f32,
    /// How much of the content shows, from 0 to 1.
    pub proportion: f32,
    /// The part held down.
    pub held: Option<ScrollerPart>,
    /// Whether the window is key, which colours the knob.
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollerPart {
    Decrement,
    Increment,
    Knob,
    /// The track either side of the knob, which pages.
    Slot,
}

/// A painted scroller and where its parts lie along it, from its start in logical pixels.
#[derive(Clone)]
pub struct PaintedScroller {
    /// `length` by `thickness`, at the scroller's scale.
    pub image: RasterImage,
    pub knob: [f32; 2],
    pub slot: [f32; 2],
    pub decrement: [f32; 2],
    pub increment: [f32; 2],
}

/// How long a held arrow or track waits before repeating, and then between steps.
const REPEAT_DELAY: Duration = Duration::from_millis(300);
const REPEAT: Duration = Duration::from_millis(50);

/// A scrollbar along the far edge of the current box, for content whose scroll offset
/// ranges over `range` while `view` of it shows, all in one unit: the platform's scroller
/// where `Ui::scrollers` has one, otherwise a thumb in `color` over the content. With a
/// `corner`, its far end leaves room for the other axis's scrollbar. Returns the offset
/// the scrollbar chose.
#[allow(clippy::too_many_arguments)]
pub fn scrollbar(
    ui: &mut Ui,
    part: impl Hash,
    axis: Axis,
    offset: f32,
    range: [f32; 2],
    view: f32,
    corner: bool,
    color: [f32; 4],
) -> Option<f32> {
    if ui.scrollers.is_some() {
        return system_scrollbar(ui, part, axis, offset, range, view, corner);
    }
    let along = usize::from(axis == Axis::Y);
    let rect = ui.rect(ui.current())?;
    let length = rect[along + 2] - rect[along];
    let cross = rect[3 - along] - rect[1 - along];
    let span = range[1] - range[0];
    // A thumb keeps 4 from each end and the box's far edge, and is 6 across.
    let track = length - 8.0 - if corner { 10.0 } else { 0.0 };
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
        ui.state(id).grab = pointer - start;
    }
    let chosen = signal.dragging.then_some(pointer).flatten().map(|pointer| {
        let travel = (track - size).max(f32::EPSILON);
        let fraction = ((pointer - ui.state(id).grab - 4.0) / travel).clamp(0.0, 1.0);
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

fn system_scrollbar(
    ui: &mut Ui,
    part: impl Hash,
    axis: Axis,
    offset: f32,
    range: [f32; 2],
    view: f32,
    corner: bool,
) -> Option<f32> {
    let along = usize::from(axis == Axis::Y);
    let rect = ui.rect(ui.current())?;
    let span = range[1] - range[0];
    let thickness = ui.scrollers.as_ref()?.thickness;
    let length = rect[along + 2] - rect[along] - if corner { thickness } else { 0.0 };
    if span <= 0.0 || length <= 0.0 {
        return None;
    }
    let id = ui.id(&part);
    let signal = ui.signal(id);
    let pointer = ui.pointer().map(|pointer| pointer[along] - rect[along]);
    let now = ui.now;
    if !signal.pressed && !signal.dragging {
        ui.state(id).held = None;
    }
    let request = Scroller {
        axis,
        length,
        scale: ui.scale,
        value: ((offset - range[0]) / span).clamp(0.0, 1.0),
        proportion: (view / (view + span)).clamp(0.0, 1.0),
        held: ui.state(id).held.map(|(part, _)| part),
        active: ui.window_focused,
    };
    let painted = (ui.scrollers.as_mut()?.paint)(&request);
    let within = |[start, end]: [f32; 2], at: f32| start <= at && at < end;
    let line = (view / 20.0).max(1.0);
    let mut chosen = None;
    if let Some(at) = pointer
        && (signal.pressed || signal.dragging)
    {
        if signal.pressed {
            let hit = [
                (ScrollerPart::Decrement, painted.decrement),
                (ScrollerPart::Increment, painted.increment),
                (ScrollerPart::Knob, painted.knob),
                (ScrollerPart::Slot, painted.slot),
            ]
            .into_iter()
            .find(|(_, extent)| within(*extent, at));
            ui.state(id).held = hit.map(|(part, _)| (part, now));
            ui.state(id).grab = at - painted.knob[0];
        }
        match ui.state(id).held {
            Some((ScrollerPart::Knob, _)) => {
                let travel =
                    (painted.slot[1] - painted.slot[0]) - (painted.knob[1] - painted.knob[0]);
                let from = at - ui.state(id).grab - painted.slot[0];
                let fraction = (from / travel.max(f32::EPSILON)).clamp(0.0, 1.0);
                chosen = Some(range[0] + fraction * span);
            }
            Some((part, due)) if now >= due => {
                let step = match part {
                    ScrollerPart::Decrement => -line,
                    ScrollerPart::Increment => line,
                    // The track pages towards the pointer and stops once the knob is under it.
                    _ if within(painted.knob, at) => 0.0,
                    _ if at < painted.knob[0] => line - view,
                    _ => view - line,
                };
                let delay = if signal.pressed { REPEAT_DELAY } else { REPEAT };
                ui.state(id).held = Some((part, now + delay));
                chosen = Some((offset + step).clamp(range[0], range[1]));
            }
            _ => {}
        }
        // A held part repeats on later frames.
        ui.animating = true;
    }
    let mut position = [0.0; 2];
    position[1 - along] = rect[3 - along] - rect[1 - along] - thickness;
    let mut extent = [px(thickness); 2];
    extent[along] = px(length);
    ui.leaf(
        &part,
        Spec {
            flags: Flags::CLICKABLE | Flags::FLOAT,
            size: extent,
            position,
            image: Some(&painted.image),
            cursor: Some(CursorIcon::Default),
            ..Spec::default()
        },
    );
    if axis == Axis::Y && corner {
        // The corner between the two scrollers shows the content's white, as AppKit's does.
        ui.leaf(
            ("corner", &part),
            Spec {
                flags: Flags::FLOAT,
                size: [px(thickness); 2],
                position: [position[0], length],
                fill: Some([1.0; 4]),
                ..Spec::default()
            },
        );
    }
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
    field(ui, id, text, placeholder, spec, false)
}

/// A text field for a password, showing and announcing a bullet for each character.
pub fn password_field(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    placeholder: &str,
    spec: Spec<'_>,
) -> Signal {
    field(ui, id, text, placeholder, spec, true)
}

fn field(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    placeholder: &str,
    spec: Spec<'_>,
    secret: bool,
) -> Signal {
    const BULLET: &str = "\u{2022}";
    // The text laid out, and where its offsets fall in `text`: a secret shows bullets.
    let masked = |text: &str| {
        if secret {
            BULLET.repeat(text.chars().count())
        } else {
            text.to_owned()
        }
    };
    let to_text = |text: &str, index: usize| match secret {
        true => (text.char_indices().map(|(at, _)| at))
            .nth(index / BULLET.len())
            .unwrap_or(text.len()),
        false => index,
    };
    let to_shown = |text: &str, index: usize| match secret {
        true => text[..index].chars().count() * BULLET.len(),
        false => index,
    };
    let mut shown_text = masked(text);
    let signal = ui.signal(id);
    let pad = spec.pad[0];
    let size = ui.theme.font_size;
    let modifiers = edit_modifiers(ui.modifiers());
    let pointer = ui
        .pointer()
        .zip(ui.rect(id))
        .map(|(pointer, rect)| pointer[0] - rect[0] - pad)
        .filter(|_| signal.pressed || signal.dragging);
    let state = ui.state(id);
    let (mut selection, mut press, select) = (state.selection, state.press, state.select.take());
    let before = [selection.anchor(), selection.focus()];
    let (texts, frame) = (&mut ui.texts, ui.frame);
    let mut label = texts.label(&shown_text, size, frame);
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
    selection = if let Some([anchor, focus]) = select {
        Selection::new(
            Cursor::from_byte_index(&label.layout, anchor, Affinity::Downstream),
            Cursor::from_byte_index(&label.layout, focus, Affinity::Upstream),
        )
    } else {
        selection.refresh(&label.layout)
    };
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
                    (
                        Some(
                            Command::ScrollPage { .. }
                            | Command::MovePage { .. }
                            | Command::MoveParagraphs { .. },
                        ),
                        _,
                    ) => None,
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
        let range = to_text(text, range.start)..to_text(text, range.end);
        text.replace_range(range.clone(), inserted);
        shown_text = masked(text);
        label = texts.label(&shown_text, size, frame);
        selection = Selection::from_byte_index(
            &label.layout,
            to_shown(text, range.start + inserted.len()),
            Affinity::Downstream,
        );
    }
    let moved = signal.pressed || before != [selection.anchor(), selection.focus()];
    let state = ui.state(id);
    state.selection = selection;
    state.press = press;
    let theme = ui.theme.clone();
    let shown = if text.is_empty() {
        placeholder
    } else {
        shown_text.as_str()
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
            role: spec.role.or(Some(if secret {
                Role::PasswordInput
            } else {
                Role::TextInput
            })),
            ..spec
        },
    );
    if let Some(node) = ui.access(id) {
        node.set_value(shown_text.as_str());
        if !placeholder.is_empty() {
            node.set_placeholder(placeholder);
        }
    }
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
            NamedKey::F7 => Edit::F7,
            NamedKey::F11 => Edit::F11,
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
