//! Snowbound's own dialogs on Linux, drawn with the interface kit in the window: messages,
//! questions and a line of text, and a file chooser where no desktop portal answers.

use crate::{Reply, State, UserEvent, art};
use accesskit::Role;
use std::path::{Path, PathBuf};
use ui::{Anchor, Axis, Flags, Id, Spec, children, fill, px};
use winit::keyboard::NamedKey;

/// What a dialog asks, and where its answer goes; one dismissed drops its reply.
pub enum Ask {
    /// A message with an OK button.
    Message,
    /// Whether to go ahead: buttons for `cancel` and for `action`.
    Question {
        cancel: String,
        action: String,
        reply: Reply<()>,
    },
    /// A line of text, starting as `text`.
    Entry { text: String, reply: Reply<String> },
    /// A file in `folder`: one of `types` (extensions) unless empty, to open; or with
    /// `save`, a new one named `name`.
    File {
        folder: PathBuf,
        types: Vec<String>,
        save: bool,
        name: String,
        reply: Reply<PathBuf>,
    },
}

/// A dialog waiting its turn, or shown.
pub struct Dialog {
    title: String,
    detail: String,
    ask: Ask,
    /// A file chooser's folder: its folders, then the files it offers, each with whether it
    /// is a folder; or why the folder can't be read.
    entries: Result<Vec<(String, bool)>, String>,
    shown: bool,
}

/// Shows `ask`, titled `title` and explained by `detail`, once the dialogs before it are
/// answered. Any thread may ask.
pub fn show(title: String, detail: String, ask: Ask) {
    let entries = match &ask {
        Ask::File { folder, types, .. } => listing(folder, types),
        _ => Ok(Vec::new()),
    };
    let dialog = Dialog {
        title,
        detail,
        ask,
        entries,
        shown: false,
    };
    if let Some(proxy) = crate::platform::PROXY.get() {
        let _ = proxy.send_event(UserEvent::Then(Box::new(move |state| {
            state.asking.push_back(dialog);
            Ok(())
        })));
    }
}

/// The folder a file chooser starts in where it is given none.
pub fn start_folder() -> PathBuf {
    crate::platform::documents_dir()
        .filter(|folder| folder.is_dir())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| "/".into())
}

/// `folder`'s folders, then its files of `types` (extensions; all where empty), each by
/// name without regard to case; hidden ones left out.
fn listing(folder: &Path, types: &[String]) -> Result<Vec<(String, bool)>, String> {
    let mut entries: Vec<(String, bool)> = std::fs::read_dir(folder)
        .map_err(|error| format!("Couldn't read {}: {error}", folder.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            let is_folder = entry.path().is_dir();
            let offered = is_folder
                || types.is_empty()
                || Path::new(&name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        types
                            .iter()
                            .any(|kind| kind.eq_ignore_ascii_case(extension))
                    });
            (offered && !name.starts_with('.')).then_some((name, is_folder))
        })
        .collect();
    entries.sort_by_cached_key(|(name, is_folder)| (!is_folder, name.to_lowercase()));
    Ok(entries)
}

fn id() -> Id {
    Id::ROOT.child("own dialog")
}

fn field() -> Id {
    id().child("field")
}

/// How the user answered a dialog this frame.
enum Answer {
    Cancel,
    Accept,
    /// A file chooser moves to this folder.
    Open(PathBuf),
    /// A file chooser's file to open, or the name to save as.
    Choose(String),
}

impl State {
    /// Builds the first dialog waiting, and answers it once the user does.
    pub(crate) fn own_dialog(&mut self) {
        let Some(dialog) = self.asking.front_mut() else {
            return;
        };
        let ui = &mut self.ui;
        if !dialog.shown {
            dialog.shown = true;
            ui.open_popup(id());
            if let Ask::Entry { .. } | Ask::File { save: true, .. } = dialog.ask {
                ui.focus_all(field());
            }
        } else if !ui.popup_open(id()) {
            self.asking.pop_front();
            return;
        }
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        let entered = ui::popup::navigation(ui, &[id(), field()], &[NamedKey::Enter])
            .contains(&NamedKey::Enter);
        let file = matches!(dialog.ask, Ask::File { .. });
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(if file { 480.0 } else { 420.0 }), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [16.0, 12.0],
                gap: 6.0,
                anchor: Some(Anchor::Dialog),
                role: Some(Role::Dialog),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id()) {
            node.set_label(dialog.title.as_str());
        }
        let text = |ui: &mut ui::Ui, part, text: &str, bold: bool, color| {
            ui.leaf(
                part,
                Spec {
                    size: [fill(), ui::fit()],
                    text: Some(text),
                    bold,
                    color,
                    overflow: ui::Overflow::Wrap,
                    role: bold.then_some(Role::Heading),
                    ..Spec::default()
                },
            );
        };
        text(ui, "title", &dialog.title, true, None);
        if !dialog.detail.is_empty() {
            text(ui, "detail", &dialog.detail, false, None);
        }
        let field_spec = Spec {
            size: [fill(), px(row)],
            fill: Some(theme.base),
            border: Some(theme.accent),
            radius: 4.0,
            pad: [6.0, 0.0],
            ..Spec::default()
        };
        // Return goes ahead, but for a question, whose safe answer is the default as
        // AppKit's and Windows' are.
        let mut answer = entered.then_some(match dialog.ask {
            Ask::Question { .. } => Answer::Cancel,
            _ => Answer::Accept,
        });
        match &mut dialog.ask {
            Ask::Entry { text, .. } => {
                ui::text_field(ui, field(), text, "", field_spec);
                crate::name(ui, field(), &dialog.title);
            }
            Ask::File {
                folder, save, name, ..
            } => {
                text(
                    ui,
                    "place",
                    &folder.to_string_lossy(),
                    false,
                    Some(theme.text_dim),
                );
                let list = ui.open(
                    "entries",
                    Spec {
                        flags: Flags::SCROLL | Flags::CLIP,
                        axis: Axis::Y,
                        size: [fill(), px(row * 10.0 + 8.0)],
                        fill: Some(theme.base),
                        border: Some(theme.chip),
                        radius: 4.0,
                        pad: [4.0, 4.0],
                        role: Some(Role::List),
                        ..Spec::default()
                    },
                );
                if let Some(node) = ui.access(list) {
                    node.set_label("Files");
                }
                match &dialog.entries {
                    Ok(entries) => {
                        for (entry, is_folder) in entries {
                            let signal = ui.leaf(
                                ("entry", entry),
                                Spec {
                                    flags: Flags::CLICKABLE,
                                    size: [fill(), px(row)],
                                    icon: Some(if *is_folder { art::FOLDER } else { art::PAGE }),
                                    text: Some(entry),
                                    fill: (!is_folder && entry == name).then_some(theme.chip),
                                    hover_fill: Some(theme.hover()),
                                    radius: 4.0,
                                    pad: [8.0, 0.0],
                                    gap: 6.0,
                                    role: Some(Role::ListItem),
                                    ..Spec::default()
                                },
                            );
                            if *is_folder && signal.clicked {
                                answer = Some(Answer::Open(folder.join(entry)));
                            } else if !is_folder && signal.pressed {
                                // A double click opens or saves as the file.
                                answer =
                                    Some(if signal.unit == draw::edit::SelectionUnit::Grapheme {
                                        Answer::Choose(entry.clone())
                                    } else {
                                        Answer::Accept
                                    });
                            }
                        }
                    }
                    Err(reason) => text(ui, "unread", reason, false, Some(theme.text_dim)),
                }
                ui.close();
                if *save {
                    ui::text_field(ui, field(), name, "", field_spec);
                    crate::name(ui, field(), "Name");
                }
            }
            Ask::Message | Ask::Question { .. } => {}
        }
        ui.open(
            "buttons",
            Spec {
                size: [fill(), children()],
                pad: [0.0, 8.0],
                gap: 8.0,
                ..Spec::default()
            },
        );
        if let Ask::File { folder, .. } = &dialog.ask
            && let Some(parent) = folder.parent()
            && ui::button(ui, "up", "Up").clicked
        {
            answer = Some(Answer::Open(parent.to_owned()));
        }
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let (cancel, action) = match &dialog.ask {
            Ask::Message => (None, "OK"),
            Ask::Question { cancel, action, .. } => (Some(cancel.as_str()), action.as_str()),
            Ask::Entry { .. } => (Some("Cancel"), "OK"),
            Ask::File { save: false, .. } => (Some("Cancel"), "Open"),
            Ask::File { save: true, .. } => (Some("Cancel"), "Save"),
        };
        if cancel.is_some_and(|cancel| ui::button(ui, "cancel", cancel).clicked) {
            answer = Some(Answer::Cancel);
        }
        if ui::button(ui, "action", action).clicked {
            answer = Some(Answer::Accept);
        }
        ui.close();
        ui.close();

        if let Ask::File {
            folder,
            types,
            save,
            name,
            ..
        } = &mut dialog.ask
        {
            match answer {
                Some(Answer::Open(to)) => {
                    dialog.entries = listing(&to, types);
                    *folder = to;
                    if !*save {
                        name.clear();
                    }
                    return;
                }
                Some(Answer::Choose(entry)) => {
                    *name = entry;
                    return;
                }
                Some(Answer::Accept) if name.trim().is_empty() => return,
                // A folder typed in opens.
                Some(Answer::Accept) if folder.join(name.trim()).is_dir() => {
                    let to = folder.join(name.trim());
                    dialog.entries = listing(&to, types);
                    *folder = to;
                    name.clear();
                    return;
                }
                _ => {}
            }
        }
        let Some(answer) = answer else {
            return;
        };
        let dialog = self.asking.pop_front().expect("A dialog is shown");
        self.ui.close_popup(id());
        if let Answer::Cancel = answer {
            return;
        }
        match dialog.ask {
            Ask::Message => {}
            Ask::Question { reply, .. } => reply.send(()),
            Ask::Entry { text, reply } => reply.send(text),
            Ask::File {
                folder,
                name,
                reply,
                ..
            } => reply.send(folder.join(name.trim())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_chooser_lists_folders_then_the_files_it_offers() {
        let folder = std::env::temp_dir().join(format!("snowbound-own-{}", std::process::id()));
        for name in ["b", "A", ".hidden"] {
            std::fs::create_dir_all(folder.join(name)).unwrap();
        }
        for name in ["z.one", "Notes.ONETOC2", "picture.png", ".x.one"] {
            std::fs::write(folder.join(name), b"").unwrap();
        }
        let types = ["one", "onetoc2"].map(String::from);
        let expected = [
            ("A", true),
            ("b", true),
            ("Notes.ONETOC2", false),
            ("z.one", false),
        ];
        assert_eq!(
            listing(&folder, &types).unwrap(),
            expected.map(|(name, is_folder)| (name.to_owned(), is_folder))
        );
        assert_eq!(listing(&folder, &[]).unwrap().len(), 5);
        assert!(listing(&folder.join("missing"), &[]).is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
