//! The sync status at the top right, and the popup it opens: OneNote's Shared Notebook
//! Synchronization for the open section.

use crate::{Session, State, filetime, library, platform};
use notebook::session::SyncStatus;
use std::{error::Error, io::ErrorKind};
use ui::{Anchor, Axis, Flags, Id, Overflow, Spec, Theme, Ui, children, fill, fit, px};

const WIDTH: f32 = 380.0;

fn id() -> Id {
    Id::ROOT.child("sync-status")
}

fn button() -> Id {
    Id::ROOT.child("sync-button")
}

/// The status's label, and what the reader can do about an error.
fn describe(sync: &SyncStatus) -> (&'static str, Option<&'static str>) {
    use ErrorKind::*;
    if library::offline() {
        return (
            "Working offline",
            Some("Changes stay on this computer until you sync."),
        );
    }
    match sync.error.as_ref().map(std::io::Error::kind) {
        Some(PermissionDenied | ReadOnlyFilesystem) => (
            "Unable to sync",
            Some(
                "You can’t change this notebook where it’s stored. Changes stay on this computer.",
            ),
        ),
        Some(WouldBlock | ResourceBusy) => (
            "Section in use",
            Some("Someone else is saving this section. Sync continues when they finish."),
        ),
        Some(
            NotFound | ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected
            | TimedOut | HostUnreachable | NetworkUnreachable | NetworkDown | BrokenPipe
            | AddrNotAvailable,
        ) => (
            "Not connected",
            Some("Changes stay on this computer and sync when the notebook is back."),
        ),
        Some(_) => ("Unable to sync", None),
        None if sync.synced.is_none() || sync.queued > 0 => ("Syncing…", None),
        None => ("Up to date", None),
    }
}

pub(crate) fn label(sync: &SyncStatus) -> &'static str {
    describe(sync).0
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

/// The status beside the window's controls, which opens the popup.
pub(crate) fn control(ui: &mut Ui, session: &Session, theme: &Theme, height: f32) {
    let strong = ui.popup_open(id()) || session.sync.error.is_some() && !library::offline();
    ui.open_as(
        button(),
        Spec {
            flags: Flags::CLICKABLE,
            size: [fit(), px(height)],
            text: Some(label(&session.sync)),
            color: Some(if strong { theme.text } else { theme.text_dim }),
            hover_fill: Some(theme.hover()),
            radius: 4.0,
            pad: [8.0, 0.0],
            center: true,
            ..Spec::default()
        },
    );
    ui.close();
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
        let ui = &mut self.ui;
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
        let (progress, advice) = describe(&session.sync);
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
        row(ui, "Section", &session.tabs[session.tab].name);
        row(ui, "Progress", progress);
        if let Some(synced) = session.sync.synced {
            row(ui, "Last sync", &when(synced));
        }
        if session.sync.queued > 0 {
            let queued = match session.sync.queued {
                1 => "1 change".to_owned(),
                count => format!("{count} changes"),
            };
            row(ui, "Not yet synced", &queued);
        }
        if let Some(advice) = advice {
            text(ui, "advice", advice, theme.text, false);
        }
        if let Some(error) = &session.sync.error {
            text(ui, "error", &error.to_string(), theme.text_dim, false);
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
        let now = ui::button(ui, "sync-now", "Sync Now").clicked;
        ui.close();
        ui.close();
        if toggled {
            library::set_offline(!offline);
            session.section.set_offline(!offline);
        }
        if now {
            session.section.wake();
        }
        Ok(())
    }
}
