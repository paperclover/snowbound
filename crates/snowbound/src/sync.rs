//! The sync status at the top right, and the popup it opens: OneNote's Shared Notebook
//! Synchronization for the open section's notebook and each of its sections.

use crate::{Session, State, art, filetime, library, platform};
use notebook::session::SyncStatus;
use std::{error::Error, io::ErrorKind};
use ui::{
    Anchor, Axis, Flags, Id, Overflow, Spec, Theme, Ui, children, fill, fit, px, shell::TOOL,
};

const WIDTH: f32 = 380.0;

fn id() -> Id {
    Id::ROOT.child("sync-status")
}

fn button() -> Id {
    Id::ROOT.child("sync-button")
}

/// The status's label and icon, and what the reader can do about an error.
fn describe(sync: &SyncStatus) -> (&'static str, &'static [&'static str], Option<&'static str>) {
    use ErrorKind::*;
    if library::offline() {
        return (
            "Working offline",
            art::SYNC_OFFLINE,
            Some("Changes stay on this computer until you sync."),
        );
    }
    match sync.error.as_ref().map(std::io::Error::kind) {
        Some(PermissionDenied | ReadOnlyFilesystem) => (
            "Unable to sync",
            art::SYNC_ERROR,
            Some(
                "You can’t change this notebook where it’s stored. Changes stay on this computer.",
            ),
        ),
        Some(WouldBlock | ResourceBusy) => (
            "Section in use",
            art::SYNC_BUSY,
            Some("Someone else is saving this section. Sync continues when they finish."),
        ),
        Some(
            NotFound | ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected
            | TimedOut | HostUnreachable | NetworkUnreachable | NetworkDown | BrokenPipe
            | AddrNotAvailable,
        ) => (
            "Not connected",
            art::SYNC_OFFLINE,
            Some("Changes stay on this computer and sync when the notebook is back."),
        ),
        Some(_) => ("Unable to sync", art::SYNC_ERROR, None),
        None if sync.synced.is_none() || sync.queued > 0 => ("Syncing…", art::SYNC_BUSY, None),
        None => ("Up to date", art::SYNC_DONE, None),
    }
}

/// Labels from the best state to the worst, which a notebook's status shows.
const ORDER: [&str; 5] = [
    "Up to date",
    "Syncing…",
    "Section in use",
    "Not connected",
    "Unable to sync",
];

fn rank(sync: &SyncStatus) -> usize {
    let label = describe(sync).0;
    ORDER.iter().position(|shown| *shown == label).unwrap_or(0)
}

/// Every section of the open section's notebook with its status, in catalog order: the
/// open one's from its session, the others' from the notebook's background sync.
fn sections(session: &Session) -> Vec<(String, SyncStatus)> {
    let open = &session.tabs[session.tab].path;
    let copy = |sync: &SyncStatus| SyncStatus {
        synced: sync.synced,
        error: sync
            .error
            .as_ref()
            .map(|error| std::io::Error::new(error.kind(), error.to_string())),
        queued: sync.queued,
    };
    let mut sections = session
        .library
        .background
        .as_ref()
        .map(|background| background.status())
        .unwrap_or_default();
    match sections.iter_mut().find(|(path, _)| path == open) {
        Some((_, sync)) => *sync = copy(&session.sync),
        None => sections.insert(0, (open.clone(), copy(&session.sync))),
    }
    sections
}

/// The notebook as a whole: the worst section's error, every waiting change, and the
/// oldest last sync.
fn overall(sections: &[(String, SyncStatus)]) -> SyncStatus {
    let worst = sections
        .iter()
        .map(|(_, sync)| sync)
        .filter(|sync| sync.error.is_some())
        .max_by_key(|sync| rank(sync));
    SyncStatus {
        synced: sections
            .iter()
            .map(|(_, sync)| sync.synced)
            .collect::<Option<Vec<_>>>()
            .and_then(|times| times.into_iter().min()),
        error: worst
            .and_then(|sync| sync.error.as_ref())
            .map(|error| std::io::Error::new(error.kind(), error.to_string())),
        queued: sections.iter().map(|(_, sync)| sync.queued).sum(),
    }
}

fn changes(count: u64) -> String {
    match count {
        1 => "1 change".to_owned(),
        count => format!("{count} changes"),
    }
}

/// A FILETIME as OneNote's "Last sync": the time, and the date too before today.
fn when(time: u64) -> String {
    let [_, clock] = platform::date_text(time);
    let day = platform::short_date(time);
    if day == platform::short_date(filetime()) {
        clock
    } else {
        format!("{day} {clock}")
    }
}

/// The status's icon in the toolbar, named in its tooltip, which opens the popup.
pub(crate) fn control(ui: &mut Ui, session: &Session, theme: &Theme) {
    let sync = overall(&sections(session));
    let strong = ui.popup_open(id()) || sync.error.is_some() && !library::offline();
    let (label, icon, _) = describe(&sync);
    ui.open_as(
        button(),
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(TOOL), px(TOOL)],
            icon: Some(icon),
            color: Some(if strong { theme.text } else { theme.text_dim }),
            hover_fill: Some(theme.hover()),
            radius: 4.0,
            center: true,
            ..Spec::default()
        },
    );
    ui.close();
    ui::popup::tooltip(ui, label, "", None);
    if ui.signal(button()).clicked {
        if ui.popup_open(id()) {
            ui.close_popup(id());
        } else {
            ui.open_popup(id());
        }
    }
}

impl State {
    /// Builds the sync status popup while it is open.
    pub(crate) fn sync_popup(&mut self) -> Result<(), Box<dyn Error>> {
        let (ui, notebooks) = (&mut self.ui, &self.notebooks);
        let Some(session) = &mut self.session else {
            ui.close_popup(id());
            return Ok(());
        };
        if !ui.popup_open(id()) {
            return Ok(());
        }
        // Read afresh while shown, so the last sync time follows each poll.
        session.sync = session.section.sync_status()?;
        let theme = ui.theme.clone();
        let offline = library::offline();
        let sections = sections(session);
        let sync = overall(&sections);
        let (progress, _, advice) = describe(&sync);
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(WIDTH), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [14.0, 10.0],
                gap: 2.0,
                anchor: Some(Anchor::Below(ui.rect(button()).unwrap_or_default())),
                ..Spec::default()
            },
        );
        let text = |ui: &mut Ui, part: &str, text: &str, color: [f32; 4], bold: bool| {
            ui.leaf(
                part,
                Spec {
                    size: [fill(), fit()],
                    text: Some(text),
                    color: Some(color),
                    bold,
                    overflow: Overflow::Wrap,
                    pad: [0.0, 3.0],
                    ..Spec::default()
                },
            );
        };
        let row = |ui: &mut Ui, key: &str, value: &str| {
            ui.open(
                key,
                Spec {
                    size: [fill(), children()],
                    gap: 8.0,
                    ..Spec::default()
                },
            );
            ui.leaf(
                "key",
                Spec {
                    size: [px(92.0), px(theme.font_size * 1.6)],
                    text: Some(key),
                    color: Some(theme.text_dim),
                    ..Spec::default()
                },
            );
            ui.leaf(
                "value",
                Spec {
                    size: [fill(), px(theme.font_size * 1.6)],
                    text: Some(value),
                    overflow: Overflow::Ellipsis,
                    ..Spec::default()
                },
            );
            ui.close();
        };
        text(ui, "title", "Notebook Sync Status", theme.text, true);
        row(ui, "Notebook", &session.library.name);
        row(ui, "Location", &session.library.location);
        row(ui, "Connection", &session.library.transport());
        row(ui, "Progress", progress);
        if let Some(synced) = sync.synced {
            row(ui, "Last sync", &when(synced));
        }
        if sync.queued > 0 {
            row(ui, "Not yet synced", &changes(sync.queued));
        }
        if let Some(advice) = advice {
            text(ui, "advice", advice, theme.text, false);
        }
        text(ui, "sections", "Sections", theme.text, true);
        for (index, (path, sync)) in sections.iter().enumerate() {
            ui.open(
                format!("section-{index}"),
                Spec {
                    size: [fill(), children()],
                    gap: 8.0,
                    ..Spec::default()
                },
            );
            let name = path.strip_suffix(".one").unwrap_or(path);
            ui.leaf(
                "name",
                Spec {
                    size: [fill(), px(theme.font_size * 1.6)],
                    text: Some(name),
                    overflow: Overflow::Ellipsis,
                    ..Spec::default()
                },
            );
            let status = match sync.queued {
                0 => describe(sync).0.to_owned(),
                queued => format!("{}, {}", describe(sync).0, changes(queued)),
            };
            ui.leaf(
                "status",
                Spec {
                    size: [fit(), px(theme.font_size * 1.6)],
                    text: Some(&status),
                    color: Some(theme.text_dim),
                    ..Spec::default()
                },
            );
            ui.close();
            if let Some(error) = &sync.error {
                let part = format!("section-{index}-error");
                text(ui, &part, &error.to_string(), theme.text_dim, false);
            }
        }
        if let Some(notice) = &session.library.notice {
            let notice = format!("Snowbound’s SMB client couldn’t sign in: {notice}");
            text(ui, "notice", &notice, theme.text_dim, false);
        }
        ui.open(
            "controls",
            Spec {
                size: [fill(), children()],
                pad: [0.0, 6.0],
                gap: 8.0,
                ..Spec::default()
            },
        );
        let toggled = ui::check_box(ui, "offline", "Work offline", offline).clicked;
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let file = session.library.local(session.section.file());
        let show = match file {
            Some(_) => ui::button(ui, "show-file", platform::SHOW_FILE).clicked,
            None => {
                ui.leaf(
                    "show-file",
                    Spec {
                        size: [fit(), px(theme.font_size * 2.0)],
                        text: Some(platform::SHOW_FILE),
                        color: Some(theme.text_dim),
                        fill: Some(theme.chip),
                        radius: 4.0,
                        pad: [theme.font_size * 0.75, 0.0],
                        center: true,
                        ..Spec::default()
                    },
                );
                false
            }
        };
        let now = ui::button(ui, "sync-now", "Sync Now").clicked;
        ui.close();
        ui.close();
        let backgrounds = notebooks
            .iter()
            .filter_map(|library| library.background.as_ref());
        if toggled {
            library::set_offline(!offline);
            session.section.set_offline(!offline);
            backgrounds.for_each(|background| background.set_offline(!offline));
        } else if now {
            session.section.wake();
            backgrounds.for_each(|background| background.wake());
        }
        if let Some(file) = file.filter(|_| show) {
            platform::show_file(&file);
        }
        Ok(())
    }
}
