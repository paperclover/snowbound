//! Options' Keyboard: every command in the table under its menu with its chords, which a
//! click records anew, and which OK puts in use.

use crate::commands::{self, COMMANDS, Chord, Id as Command, Keymap, Unusable};
use crate::options::found;
use accesskit::Role;
use canvas::editor::Toggle;
use draw::edit::{Key, NamedKey, Platform};
use ui::{Axis, Flags, Spec, Ui, children, fill, fit, px};

/// The menus the table's commands fall under, each from its first command on.
const GROUPS: [(&str, Command); 9] = [
    ("Snowbound", Command::Settings),
    ("File", Command::NewNotebook),
    ("Edit", Command::Undo),
    ("View", Command::GoTo),
    ("Insert", Command::Table),
    ("Draw", Command::SelectType),
    ("Format", Command::Toggle(Toggle::Bold)),
    ("Tags", Command::CustomizeTags),
    ("Help", Command::Help),
];

/// The Keyboard section's keymap until OK keeps it, and the chord being recorded.
pub struct Keyboard {
    pub keymap: Keymap,
    recording: Option<Recording>,
}

struct Recording {
    command: Command,
    /// The place of the chord the new one replaces; `None` adds one.
    replacing: Option<usize>,
    refused: Option<Refusal>,
}

enum Refusal {
    Unusable(Chord, Unusable),
    /// Run by another command, or a tag.
    Taken(Chord, Command),
}

/// Whether every word of `query` is in command `id`'s title, menu or `chords`.
fn hit(query: &[String], id: Command, chords: &[Chord]) -> bool {
    let labels: Vec<String> = chords.iter().map(|chord| label(*chord)).collect();
    let mut texts = vec![commands::command(id).title, group(id)];
    texts.extend(labels.iter().map(String::as_str));
    found(query, &texts)
}

/// Whether the Options search finds a command by `query`, with the chords in use.
pub fn searched(query: &[String]) -> bool {
    let keymap = Keymap::current();
    (COMMANDS.iter()).any(|command| {
        hit(
            query,
            command.id,
            keymap.chords(command.id, Platform::CURRENT),
        )
    })
}

/// The heading of the menu `id` falls under.
fn group(id: Command) -> &'static str {
    let at = |id| COMMANDS.iter().position(|command| command.id == id);
    let place = at(id).unwrap_or_default();
    GROUPS
        .iter()
        .rev()
        .find(|(_, first)| at(*first).is_some_and(|first| first <= place))
        .map_or("", |(name, _)| name)
}

fn label(chord: Chord) -> String {
    chord.label(commands::shown())
}

fn title(id: Command) -> String {
    match id {
        Command::Tag(place) => format!("Tag {}", place + 1),
        id => commands::command(id).title.to_owned(),
    }
}

impl Keyboard {
    pub fn new() -> Self {
        Self {
            keymap: Keymap::current(),
            recording: None,
        }
    }

    pub fn recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Takes a key pressed while recording: Escape stops, and a chord is given the command
    /// unless it is unusable or another's. Reports whether the key was taken.
    pub fn key(&mut self, key: &Key, modifiers: draw::edit::Modifiers) -> bool {
        let Some(recording) = &mut self.recording else {
            return false;
        };
        let chord = match key {
            Key::Named(NamedKey::Escape) => {
                self.recording = None;
                return true;
            }
            Key::Named(NamedKey::Modifier | NamedKey::Other) => return true,
            key => commands::pressed(key, modifiers),
        };
        let Some(chord) = chord else {
            return true;
        };
        let platform = Platform::CURRENT;
        if let Some(why) = chord.unusable(platform) {
            recording.refused = Some(Refusal::Unusable(chord, why));
            return true;
        }
        match self.keymap.ran(chord, platform) {
            Some(owner) if owner == recording.command => self.recording = None,
            Some(owner) => recording.refused = Some(Refusal::Taken(chord, owner)),
            None => self.bind(chord),
        }
        true
    }

    /// Gives the recorded command `chord` in place of the one it replaces.
    fn bind(&mut self, chord: Chord) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        let mut chords = self.chords(recording.command);
        match recording.replacing {
            Some(place) => chords[place] = chord,
            None => chords.push(chord),
        }
        self.keymap.set(recording.command, chords);
    }

    fn chords(&self, id: Command) -> Vec<Chord> {
        self.keymap.chords(id, Platform::CURRENT).to_vec()
    }

    /// Takes `chord` from its owner, or with `swap` gives the owner the chord replaced, then
    /// binds it.
    fn take(&mut self, chord: Chord, owner: Command, swap: bool) {
        let replaced = (self.recording.as_ref())
            .and_then(|recording| Some(self.chords(recording.command)[recording.replacing?]));
        // A tag yields its chord to the command given it by itself.
        if !matches!(owner, Command::Tag(_)) {
            let chords = self
                .chords(owner)
                .into_iter()
                .filter_map(|held| match (held == chord, replaced) {
                    (false, _) => Some(held),
                    (true, Some(replaced)) if swap => Some(replaced),
                    (true, _) => None,
                })
                .collect();
            self.keymap.set(owner, chords);
        }
        self.bind(chord);
    }

    /// Builds the section in the box open in `ui`: the commands matching every word of
    /// `query` in their title, menu or chords, or all where none match.
    pub fn build(&mut self, ui: &mut Ui, query: &[String]) {
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        ui.open(
            "keyboard",
            Spec {
                axis: Axis::Y,
                size: [fill(), children()],
                gap: 2.0,
                ..Spec::default()
            },
        );
        ui.open(
            "bar",
            Spec {
                size: [fill(), px(row)],
                gap: 8.0,
                ..Spec::default()
            },
        );
        ui.leaf(
            "hint",
            Spec {
                flags: Flags::CLIP,
                size: [fill(), px(row)],
                text: Some("Click a shortcut to change it"),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        );
        if ui::button(ui, "reset-all", "Reset All").clicked {
            self.keymap = Keymap::default();
            self.recording = None;
        }
        ui.close();
        let listed: Vec<(Command, Vec<Chord>)> = COMMANDS
            .iter()
            .filter(|command| commands::offered(command.id))
            .map(|command| (command.id, self.chords(command.id)))
            .collect();
        let matching = |(id, chords): &&(Command, Vec<Chord>)| hit(query, *id, chords);
        let some = listed.iter().any(|command| matching(&command));
        let mut heading = "";
        for (id, chords) in listed.iter().filter(|command| !some || matching(command)) {
            let group = group(*id);
            if group != heading {
                heading = group;
                ui.leaf(
                    ("group", group),
                    Spec {
                        size: [fill(), px(row)],
                        text: Some(group),
                        bold: true,
                        color: Some(theme.text_dim),
                        role: Some(Role::Heading),
                        ..Spec::default()
                    },
                );
            }
            self.command_row(ui, *id, chords);
            if self
                .recording
                .as_ref()
                .is_some_and(|recording| recording.command == *id)
            {
                self.status(ui);
            }
        }
        ui.close();
    }

    /// A command's row: its title, a chip per chord to record anew, one to add a chord, and
    /// Reset where the user changed its chords.
    fn command_row(&mut self, ui: &mut Ui, id: Command, chords: &[Chord]) {
        let theme = ui.theme.clone();
        let height = theme.font_size * 1.9;
        let row = ui.open(
            ("command", format!("{id:?}")),
            Spec {
                size: [fill(), px(height)],
                gap: 4.0,
                pad: [6.0, 2.0],
                radius: 4.0,
                hover_fill: Some(theme.hover()),
                role: Some(Role::ListItem),
                ..Spec::default()
            },
        );
        let title = title(id);
        if let Some(node) = ui.access(row) {
            node.set_label(title.as_str());
        }
        ui.leaf(
            "title",
            Spec {
                flags: Flags::CLIP,
                size: [fill(), px(height - 4.0)],
                text: Some(&title),
                bold: self.keymap.customized(id),
                ..Spec::default()
            },
        );
        let recording = self
            .recording
            .as_ref()
            .filter(|recording| recording.command == id)
            .map(|recording| recording.replacing);
        let chip = |ui: &mut Ui, part: (usize, bool), text: &str, live: bool| {
            ui.leaf(
                part,
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fit(), px(height - 4.0)],
                    text: Some(text),
                    color: Some(if live { theme.accent } else { theme.text }),
                    fill: Some(theme.chip),
                    hover_border: Some(theme.accent),
                    border: live.then_some(theme.accent),
                    radius: 4.0,
                    pad: [8.0, 0.0],
                    center: true,
                    role: Some(Role::Button),
                    ..Spec::default()
                },
            )
            .clicked
        };
        for (place, chord) in chords.iter().enumerate() {
            let live = recording == Some(Some(place));
            let text = if live {
                "Type shortcut…".to_owned()
            } else {
                label(*chord)
            };
            if chip(ui, (place, false), &text, live) {
                self.record(ui, id, Some(place));
            }
        }
        let adding = recording == Some(None);
        let text = if adding { "Type shortcut…" } else { "+" };
        if chip(ui, (chords.len(), true), text, adding) {
            self.record(ui, id, None);
        }
        if let Some(node) = ui.access(ui.id((chords.len(), true))) {
            node.set_label(format!("Add shortcut for {title}"));
        }
        if self.keymap.customized(id) && ui::button(ui, "reset", "Reset").clicked {
            self.keymap
                .set(id, commands::command(id).chords(Platform::CURRENT).to_vec());
            self.recording = None;
        }
        ui.close();
    }

    /// Records a chord for `id`, replacing its chord at `replacing` or adding one; the
    /// keys go to the recording, not the search.
    fn record(&mut self, ui: &mut Ui, id: Command, replacing: Option<usize>) {
        self.recording = Some(Recording {
            command: id,
            replacing,
            refused: None,
        });
        ui.set_focus(None);
    }

    /// The line under the list: what the recording waits for, or why it refused a chord,
    /// with what can be done about it.
    fn status(&mut self, ui: &mut Ui) {
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        // A Mac's browser takes the PC's chords, Option for Alt.
        let modifiers = match (Platform::CURRENT, commands::shown()) {
            (Platform::MacOs, _) => "⌘ or ⌃",
            (_, Platform::MacOs) => "⌘ or ⌥",
            _ => "Ctrl or Alt",
        };
        let text = match &self.recording {
            None => "Click a shortcut to change it".to_owned(),
            Some(recording) => match recording.refused {
                None => format!("Type a shortcut for {}", title(recording.command)),
                Some(Refusal::Unusable(chord, Unusable::Typing)) => {
                    format!("{} types text. Include {}", label(chord), modifiers)
                }
                Some(Refusal::Unusable(chord, Unusable::Editing)) => {
                    format!("{} moves through text. Type another", label(chord))
                }
                Some(Refusal::Unusable(chord, Unusable::System(item))) => {
                    format!("{} is macOS's {item}. Type another", label(chord))
                }
                Some(Refusal::Taken(chord, owner)) => {
                    format!("{} already runs {}", label(chord), title(owner))
                }
            },
        };
        let status = ui.open(
            "status",
            Spec {
                size: [fill(), px(row)],
                gap: 8.0,
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(status) {
            node.set_live(accesskit::Live::Polite);
        }
        ui.leaf(
            "text",
            Spec {
                flags: Flags::CLIP,
                size: [fill(), px(row)],
                text: Some(&text),
                color: Some(theme.text_dim),
                pad: [6.0, 0.0],
                role: Some(Role::Label),
                ..Spec::default()
            },
        );
        if let Some(recording) = &self.recording {
            let taken = match recording.refused {
                Some(Refusal::Taken(chord, owner)) => Some((chord, owner)),
                _ => None,
            };
            let (command, replacing) = (recording.command, recording.replacing);
            if let Some((chord, owner)) = taken {
                // Only a command, not a tag, can take the chord replaced in exchange.
                if replacing.is_some()
                    && !matches!(owner, Command::Tag(_))
                    && ui::button(ui, "swap", "Swap").clicked
                {
                    self.take(chord, owner, true);
                }
                if ui::button(ui, "replace", "Replace").clicked {
                    self.take(chord, owner, false);
                }
            } else if let Some(place) = replacing
                && ui::button(ui, "remove", "Remove").clicked
            {
                let mut chords = self.chords(command);
                chords.remove(place);
                self.keymap.set(command, chords);
                self.recording = None;
            }
            if self.recording.is_some() && ui::button(ui, "cancel", "Cancel").clicked {
                self.recording = None;
            }
        }
        ui.close();
    }
}

impl crate::State {
    /// Gives a key pressed while Options records a chord to the recording.
    pub(crate) fn record_chord(&mut self, event: &ui::Event) -> bool {
        let (Some(options), ui::Event::Key { key, .. }) = (&mut self.options, event) else {
            return false;
        };
        // A field focused since recording began takes the keys instead.
        if self.ui.focused().is_some() {
            options.keyboard.recording = None;
            return false;
        }
        let modifiers = ui::edit_modifiers(self.ui.modifiers());
        let taken = options.keyboard.key(&ui::edit_key(key), modifiers);
        if taken {
            self.window.request_redraw();
        }
        taken
    }

    /// Puts `keymap` in use, and in the menu bar where it changes.
    pub(crate) fn install_keymap(&self, keymap: Keymap) {
        if keymap.install() {
            crate::platform::install_menu();
            crate::platform::update_tag_menu(&self.tags);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draw::edit::Modifiers;

    fn shortcut() -> Modifiers {
        Modifiers {
            command: true,
            control: Platform::CURRENT != Platform::MacOs,
            ..Modifiers::default()
        }
    }

    fn typed(keyboard: &mut Keyboard, key: &str, modifiers: Modifiers) {
        assert!(keyboard.key(&Key::Character(key.into()), modifiers));
    }

    fn recording(id: Command, replacing: Option<usize>) -> Keyboard {
        Keyboard {
            keymap: Keymap::default(),
            recording: Some(Recording {
                command: id,
                replacing,
                refused: None,
            }),
        }
    }

    #[test]
    fn a_recorded_chord_replaces_adds_or_swaps_and_refuses_typing() {
        let bold = Command::Toggle(Toggle::Bold);
        let italic = Command::Toggle(Toggle::Italic);
        let platform = Platform::CURRENT;
        let mut keyboard = recording(Command::PageList, None);
        typed(&mut keyboard, "j", Modifiers::default());
        assert!(matches!(
            keyboard.recording.as_ref().unwrap().refused,
            Some(Refusal::Unusable(_, Unusable::Typing))
        ));
        typed(&mut keyboard, "j", shortcut());
        assert!(!keyboard.recording());
        let chord = commands::cmd('j');
        assert_eq!(keyboard.keymap.chords(Command::PageList, platform), [chord]);
        assert_eq!(
            keyboard.keymap.ran(chord, platform),
            Some(Command::PageList)
        );

        // Bold's chord, taken from Italic, which gets Bold's in exchange.
        let mut keyboard = recording(bold, Some(0));
        typed(&mut keyboard, "i", shortcut());
        assert!(matches!(
            keyboard.recording.as_ref().unwrap().refused,
            Some(Refusal::Taken(_, owner)) if owner == italic
        ));
        keyboard.take(commands::cmd('i'), italic, true);
        assert_eq!(keyboard.keymap.chords(bold, platform), [commands::cmd('i')]);
        assert_eq!(
            keyboard.keymap.chords(italic, platform),
            [commands::cmd('b')]
        );

        // A tag's chord, which the tag then goes without.
        let mut keyboard = recording(Command::PageList, None);
        typed(&mut keyboard, "1", shortcut());
        keyboard.take(commands::cmd('1'), Command::Tag(0), false);
        assert_eq!(
            keyboard.keymap.ran(commands::cmd('1'), platform),
            Some(Command::PageList)
        );

        // Chords set back to the defaults keep nothing.
        let mut keyboard = recording(bold, Some(0));
        typed(&mut keyboard, "b", shortcut());
        assert_eq!(keyboard.keymap, Keymap::default());
    }

    #[test]
    fn every_command_falls_under_a_menu() {
        assert_eq!(group(Command::Settings), "Snowbound");
        assert_eq!(group(Command::Paste), "Edit");
        assert_eq!(group(Command::Sidebar), "View");
        assert_eq!(group(Command::Toggle(Toggle::Italic)), "Format");
        assert_eq!(group(Command::CheckForUpdates), "Help");
    }
}
