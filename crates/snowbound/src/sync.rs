//! The sync status at the top right, and the popup it opens: OneNote's Shared Notebook
//! Synchronization for the open section's notebook, led by where its sync stands.

use crate::{Library, Session, State, art, filetime, library, platform, update};
use notebook::session::{SyncState, SyncStatus};
use onestore::ExGuid;
use std::{error::Error, path::Path, sync::Arc, time::Duration};
use ui::{
    Anchor, Axis, Flags, Id, Overflow, Spec, Theme, Ui, children, fill, fit, px, shell::TOOL,
};

const WIDTH: f32 = 340.0;
const PAD: f32 = 16.0;
/// A sync shorter than this never shows, so a quick publish doesn't flash the bar.
const SHOW_AFTER: Duration = Duration::from_millis(400);
/// How long a shown sync's full bar stays before the status reads up to date.
const LINGER: Duration = Duration::from_millis(700);
/// One pass of the sheen along a running bar.
const SHEEN: f32 = 1.4;
const THIS: &str = if cfg!(target_os = "macos") {
    "this Mac"
} else {
    "this computer"
};

fn id() -> Id {
    Id::ROOT.child("sync-status")
}

fn button() -> Id {
    Id::ROOT.child("sync-button")
}

/// The status's headline and icon. `quiet` holds a sync back that hasn't run `SHOW_AFTER`
/// yet: a notebook synced before still reads up to date.
fn describe(
    sync: &SyncStatus,
    offline: bool,
    quiet: bool,
) -> (&'static str, &'static [&'static str]) {
    if offline {
        return ("Offline", art::SYNC_OFFLINE);
    }
    match sync.state() {
        SyncState::ReadOnly => ("Can’t save changes", art::SYNC_ERROR),
        SyncState::InUse => ("Waiting for another device", art::SYNC_BUSY),
        SyncState::NotConnected => ("Waiting for connection", art::SYNC_OFFLINE),
        SyncState::Protected => ("Password-protected section", art::SYNC_ERROR),
        SyncState::Unreadable => ("Can’t read a section", art::SYNC_ERROR),
        SyncState::Failed => ("Unable to sync", art::SYNC_ERROR),
        SyncState::Syncing if !quiet || sync.synced.is_none() => ("Syncing…", art::SYNC_BUSY),
        SyncState::Syncing | SyncState::UpToDate => ("Up to date", art::SYNC_DONE),
    }
}

/// A section's status beside its name.
fn brief(sync: &SyncStatus, offline: bool) -> String {
    let waiting = match sync.queued {
        0 => String::new(),
        queued => format!(" · {} waiting", changes(queued)),
    };
    match sync.state() {
        SyncState::UpToDate => "Up to date".to_owned(),
        SyncState::Syncing if offline => format!("{} waiting", changes(sync.queued)),
        SyncState::Syncing if sync.queued > 0 => format!("Sending {}", changes(sync.queued)),
        SyncState::Syncing => "Syncing…".to_owned(),
        SyncState::InUse => format!("In use elsewhere{waiting}"),
        SyncState::NotConnected => format!("Not connected{waiting}"),
        SyncState::ReadOnly => format!("Read-only{waiting}"),
        SyncState::Protected => "Password protected".to_owned(),
        SyncState::Unreadable => "Can’t read".to_owned(),
        SyncState::Failed => format!("Unable to sync{waiting}"),
    }
}

/// Every section of `library` with its status, in catalog order: the open one's from its
/// session, the others' from the notebook's background sync. OneNote's sync dialog leaves
/// out the recycle bin, unless something is wrong there.
fn sections(library: &Library, session: &Session) -> Vec<(String, SyncStatus)> {
    let open = &session.tabs[session.tab].path;
    let mut sections = library
        .background
        .as_ref()
        .map(|background| background.status())
        .unwrap_or_default();
    if std::ptr::eq(library, &*session.library) {
        match sections.iter_mut().find(|(path, _)| path == open) {
            Some((_, sync)) => *sync = copy(&session.sync),
            None => sections.insert(0, (open.clone(), copy(&session.sync))),
        }
    }
    sections.retain(|(path, sync)| {
        sync.error.is_some()
            || !path
                .rsplit_once('/')
                .is_some_and(|(folder, _)| library::recycle_bin(folder))
    });
    sections
}

/// The notebook as a whole: the worst section's error, every waiting change, and the
/// oldest last sync.
fn overall(sections: &[(String, SyncStatus)]) -> SyncStatus {
    let worst = sections
        .iter()
        .map(|(_, sync)| sync)
        .filter(|sync| sync.error.is_some())
        .max_by_key(|sync| sync.state());
    SyncStatus {
        synced: sections
            .iter()
            .map(|(_, sync)| sync.synced)
            .collect::<Option<Vec<_>>>()
            .and_then(|times| times.into_iter().min()),
        error: worst.and_then(|sync| copy(sync).error),
        queued: sections.iter().map(|(_, sync)| sync.queued).sum(),
    }
}

/// `sync` again; its error, which can't be cloned, as one of the same kind and message.
fn copy(sync: &SyncStatus) -> SyncStatus {
    SyncStatus {
        synced: sync.synced,
        error: (sync.error.as_ref())
            .map(|error| std::io::Error::new(error.kind(), error.to_string())),
        queued: sync.queued,
    }
}

fn changes(count: u64) -> String {
    match count {
        1 => "1 change".to_owned(),
        count => format!("{count} changes"),
    }
}

/// A FILETIME as how long ago it was: minutes within the hour, then the time, and the date
/// too before today.
fn ago(time: u64) -> String {
    let minutes = filetime().saturating_sub(time) / 600_000_000;
    if minutes < 1 {
        return "just now".to_owned();
    }
    if minutes < 60 {
        return format!("{minutes} min ago");
    }
    let [_, clock] = platform::date_text(time);
    let day = platform::short_date(time);
    if day == platform::short_date(filetime()) {
        format!("at {clock}")
    } else {
        format!("{day} at {clock}")
    }
}

/// Whether a sync shows: once it has run `SHOW_AFTER`, and for `LINGER` after it ends up
/// to date where it `shown`, so a quick one never flashes. `id` keeps the timings.
fn shows_sync(ui: &mut Ui, id: Id, syncing: bool, shown: bool) -> bool {
    let running = ui.lasted(id.child("running"), syncing);
    let ended = ui.lasted(id.child("ended"), !syncing);
    let due = match syncing {
        true => SHOW_AFTER.checked_sub(running),
        false if shown => LINGER.checked_sub(ended),
        false => None,
    };
    if let Some(due) = due.filter(|due| !due.is_zero()) {
        ui.wake_after(due);
    }
    running >= SHOW_AFTER || !syncing && shown && ended < LINGER
}

const TITLE: &str = "Notebook Sync Status";

/// What the popup says of a newer build: downloading, or waiting to be installed.
fn update_note(update: &update::Status) -> Option<String> {
    match update {
        update::Status::Downloading(version) => Some(format!("Downloading {version}…")),
        update::Status::Ready(version, ..) => Some(format!("{version} is ready")),
        update::Status::Available(version, ..) => Some(format!("{version} is available")),
        _ => None,
    }
}

/// The status's icon in the toolbar, which opens the popup; its tooltip names the status,
/// and a dot on it says a newer build is ready.
pub(crate) fn control(ui: &mut Ui, session: &Session, update: &update::Status, theme: &Theme) {
    let sync = overall(&sections(&session.library, session));
    let offline = library::offline();
    let syncing = !offline && sync.state() == SyncState::Syncing;
    let quiet = !shows_sync(ui, button(), syncing, false);
    let (label, icon) = describe(&sync, offline, quiet);
    // Through the system's mount, OneNote's locks aren't taken.
    let icon = match session.library.notice {
        Some(_) if icon == art::SYNC_DONE => art::SYNC_WARNING,
        _ => icon,
    };
    ui.open_as(
        button(),
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(TOOL), px(TOOL)],
            icon: Some(icon),
            hover_fill: Some(theme.hover()),
            radius: 4.0,
            center: true,
            role: Some(accesskit::Role::Button),
            ..Spec::default()
        },
    );
    let waiting = matches!(
        update,
        update::Status::Ready(..) | update::Status::Available(..)
    );
    if waiting {
        ui.leaf(
            "update",
            Spec {
                flags: Flags::FLOAT,
                size: [px(7.0), px(7.0)],
                position: [TOOL - 8.0, 1.0],
                fill: Some(theme.accent),
                radius: 3.5,
                ..Spec::default()
            },
        );
    }
    let open = ui.popup_open(id());
    let note = update_note(update).filter(|_| waiting);
    if let Some(node) = ui.access(button()) {
        node.set_label(TITLE);
        node.set_value(label);
        if let Some(note) = &note {
            node.set_description(format!("Snowbound {note}"));
        }
        node.set_has_popup(accesskit::HasPopup::Dialog);
        node.set_expanded(open);
    }
    ui.close();
    ui::popup::tooltip(ui, label, "", note.as_deref());
    if ui.signal(button()).clicked {
        if ui.popup_open(id()) {
            ui.close_popup(id());
        } else {
            ui.open_popup(id());
        }
    }
}

/// What the popup shows of the notebook.
struct Facts<'a> {
    name: &'a str,
    /// The notebook's folder or address in full.
    location: &'a str,
    place: Vec<String>,
    /// Why the notebook syncs through the system's mount of its share.
    notice: Option<&'a str>,
    sections: Vec<(String, SyncStatus)>,
    /// The open section's pages with conflict pages, and their titles.
    conflicts: Vec<(ExGuid, &'a str)>,
    offline: bool,
    update: &'a update::Status,
    /// Whether the update's changes are listed.
    changes_listed: bool,
    /// Whether the open section's file can be shown in the file manager.
    local: bool,
}

/// What the reader picked in the popup.
#[derive(Default)]
struct Picked {
    offline: bool,
    sync_now: bool,
    show_file: bool,
    sign_in: bool,
    conflict: Option<ExGuid>,
    build_folder: bool,
    restart: bool,
    list_changes: bool,
}

/// What a newer build changes: `summary`, which unfolds the titles under their kinds when
/// `listed`. Returns whether the summary was clicked.
fn changes_list(ui: &mut Ui, summary: &str, changes: &[update::Change], listed: bool) -> bool {
    let theme = ui.theme.clone();
    let line = theme.font_size * 1.6;
    let toggle = ui.open(
        "summary",
        Spec {
            flags: Flags::CLICKABLE,
            size: [fill(), px(line)],
            gap: 4.0,
            role: Some(accesskit::Role::Button),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(toggle) {
        node.set_label(summary);
        node.set_expanded(listed);
    }
    ui.leaf(
        "label",
        Spec {
            size: [fit(), px(line)],
            text: Some(summary),
            overflow: Overflow::Ellipsis,
            ..Spec::default()
        },
    );
    ui.leaf(
        "chevron",
        Spec {
            size: [fit(), px(line)],
            icon: Some(if listed {
                art::CHEVRON_UP
            } else {
                ui::shell::CHEVRON
            }),
            color: Some(theme.text_dim),
            ..Spec::default()
        },
    );
    ui.close();
    // Unfolds to the titles' height, scrolling past a few dozen lines.
    let list = ui.id("changes");
    let rows = ui
        .rect(list.child("rows"))
        .map_or(0.0, |rect| rect[3] - rect[1]);
    let height = ui.animate(list, if listed { rows.min(line * 9.0) } else { 0.0 });
    if listed || height > 0.0 {
        ui.open_as(
            list,
            Spec {
                flags: Flags::SCROLL | Flags::CLIP,
                axis: Axis::Y,
                size: [fill(), px(height)],
                role: Some(accesskit::Role::List),
                ..Spec::default()
            },
        );
        ui.open(
            "rows",
            Spec {
                axis: Axis::Y,
                size: [fill(), children()],
                pad: [0.0, 2.0],
                gap: 2.0,
                ..Spec::default()
            },
        );
        let mut kind = None;
        for (index, change) in changes.iter().enumerate() {
            if kind != Some(change.kind) {
                kind = Some(change.kind);
                ui.leaf(
                    ("heading", index),
                    Spec {
                        size: [fill(), px(line)],
                        text: Some(change.kind.heading()),
                        color: Some(theme.text_dim),
                        bold: true,
                        ..Spec::default()
                    },
                );
            }
            ui.open(
                ("change", index),
                Spec {
                    size: [fill(), children()],
                    gap: 6.0,
                    role: Some(accesskit::Role::ListItem),
                    ..Spec::default()
                },
            );
            ui.leaf(
                "bullet",
                Spec {
                    size: [fit(), fit()],
                    text: Some("•"),
                    color: Some(theme.text_dim),
                    ..Spec::default()
                },
            );
            ui.leaf(
                "title",
                Spec {
                    size: [fill(), fit()],
                    text: Some(&change.title),
                    overflow: Overflow::Wrap,
                    ..Spec::default()
                },
            );
            ui.close();
        }
        ui.close();
        ui.close();
    }
    ui.signal(toggle).clicked
}

/// Lays out the open popup's contents.
fn build(ui: &mut Ui, facts: &Facts) -> Picked {
    let theme = ui.theme.clone();
    let mut picked = Picked::default();
    let line = theme.font_size * 1.6;
    let text = |ui: &mut Ui, part: &str, text: &str, color: [f32; 4]| {
        ui.leaf(
            part,
            Spec {
                size: [fill(), fit()],
                text: Some(text),
                color: Some(color),
                overflow: Overflow::Wrap,
                pad: [0.0, 2.0],
                ..Spec::default()
            },
        );
    };
    let rule = |ui: &mut Ui, part: &str| {
        ui.leaf(
            part,
            Spec {
                size: [fill(), px(1.0)],
                fill: Some(theme.chip),
                ..Spec::default()
            },
        );
    };
    let sync = overall(&facts.sections);
    let total = facts.sections.len();
    let done = facts
        .sections
        .iter()
        .filter(|(_, sync)| sync.state() == SyncState::UpToDate)
        .count();
    let state = sync.state();
    let syncing = !facts.offline && state == SyncState::Syncing;
    let bar = id().child("status").child("bar");
    let shown = ui.rect(bar).is_some() && state == SyncState::UpToDate;
    let showing = !facts.offline && shows_sync(ui, button(), syncing, shown);
    let (mut headline, mut icon) = describe(&sync, facts.offline, !showing);
    let host = facts.place.first().map_or("the notebook", String::as_str);
    let waiting = changes(sync.queued);
    let conflict;
    let advice = if facts.offline {
        None
    } else {
        match state {
            SyncState::NotConnected => Some(match sync.queued {
                0 => format!("Can’t reach {host}. Sync continues when it’s back."),
                _ => format!("Can’t reach {host}. {waiting} will sync when it’s back."),
            }),
            SyncState::InUse => {
                Some("Someone else is saving a section. Sync continues when they finish.".into())
            }
            SyncState::ReadOnly => Some(format!(
                "This location is read-only. Your changes stay on {THIS}."
            )),
            SyncState::Protected => {
                Some("Snowbound can’t open password-protected sections yet.".into())
            }
            SyncState::Unreadable | SyncState::Failed => {
                Some("Snowbound keeps trying. Sync Now tries again at once.".into())
            }
            _ => None,
        }
    };
    if !showing && headline == "Up to date" && !facts.conflicts.is_empty() {
        conflict = match facts.conflicts.as_slice() {
            [(_, title)] => format!("Conflict on “{title}”"),
            pages => format!("Conflicts on {} pages", pages.len()),
        };
        headline = &conflict;
        icon = art::SYNC_WARNING;
    }

    // The status: headline and Work offline's switch, what to do, then one line of when it
    // last synced or how a sync runs, over a running sync's bar. Its height holds as Work
    // offline turns on and off.
    ui.open(
        "status",
        Spec {
            axis: Axis::Y,
            size: [fill(), children()],
            gap: 2.0,
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(ui.current()) {
        node.set_live(accesskit::Live::Polite);
    }
    let height = theme.font_size * 2.0;
    ui.open(
        "header",
        Spec {
            size: [fill(), px(height)],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "headline",
        Spec {
            size: [fill(), px(height)],
            text: Some(headline),
            icon: Some(icon),
            font_size: Some(theme.font_size * 1.2),
            bold: true,
            overflow: Overflow::Ellipsis,
            ..Spec::default()
        },
    );
    let switch = ui.open(
        "offline",
        Spec {
            flags: Flags::CLICKABLE,
            size: [children(), px(height)],
            gap: 8.0,
            role: Some(accesskit::Role::Switch),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(switch) {
        node.set_label("Work offline");
        node.set_toggled(facts.offline.into());
    }
    let signal = ui.signal(switch);
    picked.offline = signal.clicked;
    ui.leaf(
        "label",
        Spec {
            size: [fit(), px(height)],
            text: Some("Work offline"),
            color: Some(theme.text_dim),
            ..Spec::default()
        },
    );
    let [width, side] = [30.0, 18.0];
    let on = ui.animate(switch.child("on"), f32::from(u8::from(facts.offline)));
    let mix = |from: [f32; 4], to: [f32; 4]| {
        std::array::from_fn(|at| from[at] + (to[at] - from[at]) * on)
    };
    ui.open(
        "track",
        Spec {
            size: [px(width), px(height)],
            inset: [0.0, (height - side) / 2.0, 0.0, (height - side) / 2.0],
            fill: Some(mix(theme.chip, theme.accent)),
            border: signal.hovered.then_some(theme.accent),
            radius: side / 2.0,
            ..Spec::default()
        },
    );
    let knob = side - 4.0;
    let left = 2.0 + (width - side) * on;
    let top = (height - side) / 2.0 + 2.0;
    ui.mark([left, top, left + knob, top + knob], [1.0; 4], knob / 2.0);
    ui.close();
    ui.close();
    ui.close();
    if let Some(advice) = &advice {
        text(ui, "advice", advice, theme.text);
    } else if !facts.conflicts.is_empty() && !showing {
        text(
            ui,
            "advice",
            "Both versions are kept. Open the page to compare them.",
            theme.text,
        );
    }
    let fraction = match total {
        0 => 1.0,
        _ if state == SyncState::UpToDate => 1.0,
        _ => done as f32 / total as f32,
    };
    let subtitle = if showing {
        let mut caption = match total {
            0 | 1 => String::new(),
            _ => format!("{done} of {total} sections synced"),
        };
        if sync.queued > 0 {
            if !caption.is_empty() {
                caption += " · ";
            }
            caption += &format!("{waiting} to send");
        }
        caption
    } else if facts.offline && sync.queued > 0 {
        format!("{waiting} waiting")
    } else if let Some(synced) = sync.synced {
        ui.wake_after(Duration::from_secs(30));
        let prefix = match advice.is_some() || facts.offline {
            true => "Last synced",
            false => "Synced",
        };
        format!("{prefix} {}", ago(synced))
    } else {
        String::new()
    };
    let subtitle = match subtitle.is_empty() {
        true => "Checking for changes…",
        false => &subtitle,
    };
    ui.leaf(
        "subtitle",
        Spec {
            size: [fill(), px(theme.font_size * 1.5)],
            text: Some(subtitle),
            color: Some(theme.text_dim),
            overflow: Overflow::Ellipsis,
            ..Spec::default()
        },
    );
    ui.close();

    // A running sync's bar takes the rule's place, which keeps the bar's height.
    ui.open(
        "progress",
        Spec {
            size: [fill(), px(6.0)],
            ..Spec::default()
        },
    );
    if showing {
        let fraction = ui.animate(bar.child("fraction"), fraction);
        let running = ui.lasted(bar.child("running"), true).as_secs_f32();
        ui.open_as(
            bar,
            Spec {
                size: [fill(), px(6.0)],
                fill: Some(theme.chip),
                radius: 3.0,
                role: Some(accesskit::Role::ProgressIndicator),
                ..Spec::default()
            },
        );
        let width = ui.rect(bar).map_or(0.0, |rect| rect[2] - rect[0]);
        let filled = width * fraction;
        ui.mark([0.0, 0.0, filled, 6.0], theme.accent, 3.0);
        // A sheen sweeps the filled part while the sync runs, so a long section still moves.
        if fraction < 1.0 && filled > 0.0 {
            let band = 36.0_f32.min(filled);
            let at = (running % SHEEN) / SHEEN * (filled + band) - band;
            let [r, g, b, _] = theme.popup;
            ui.mark(
                [at.max(0.0), 0.0, (at + band).min(filled), 6.0],
                [r, g, b, 0.28],
                3.0,
            );
            ui.wake_after(Duration::from_millis(16));
        }
        if let Some(node) = ui.access(bar) {
            node.set_label("Sync progress");
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(100.0);
            node.set_numeric_value(f64::from((fraction * 100.0).round()));
        }
        ui.close();
    } else {
        ui.leaf(
            "rule",
            Spec {
                size: [fill(), px(6.0)],
                inset: [0.0, 2.5, 0.0, 2.5],
                fill: Some(theme.chip),
                ..Spec::default()
            },
        );
    }
    ui.close();
    // Which notebook, and where it lives; the full location in the tooltip.
    ui.open(
        "identity",
        Spec {
            axis: Axis::Y,
            size: [fill(), children()],
            ..Spec::default()
        },
    );
    ui.leaf(
        "name",
        Spec {
            size: [fill(), px(line)],
            text: Some(facts.name),
            icon: Some(art::NOTEBOOK),
            bold: true,
            overflow: Overflow::Ellipsis,
            ..Spec::default()
        },
    );
    if !facts.place.is_empty() {
        ui.leaf(
            "place",
            Spec {
                size: [fill(), px(line)],
                text: Some(&facts.place.join(" › ")),
                color: Some(theme.text_dim),
                overflow: Overflow::Ellipsis,
                pad: [22.0, 0.0],
                ..Spec::default()
            },
        );
        ui::popup::tooltip(ui, facts.location, "", None);
    }
    if let Some(notice) = facts.notice {
        ui.open(
            "notice",
            Spec {
                size: [fill(), children()],
                gap: 8.0,
                pad: [22.0, 4.0],
                ..Spec::default()
            },
        );
        text(
            ui,
            "text",
            &format!("Using the system’s connection. Snowbound couldn’t sign in: {notice}"),
            theme.text_dim,
        );
        picked.sign_in = ui::button(ui, "sign-in", "Sign In…").clicked;
        ui.close();
    }
    ui.close();

    rule(ui, "sections-rule");
    // Only what isn't up to date, and how many others are.
    ui.open(
        "sections",
        Spec {
            axis: Axis::Y,
            size: [fill(), children()],
            ..Spec::default()
        },
    );
    // Where the whole notebook can't be reached, the headline says so for each section
    // without changes waiting.
    let unreached = |sync: &SyncStatus| sync.state() == SyncState::NotConnected && sync.queued == 0;
    let behind: Vec<_> = facts
        .sections
        .iter()
        .filter(|(_, sync)| sync.state() != SyncState::UpToDate)
        .filter(|(_, sync)| !(state == SyncState::NotConnected && unreached(sync)))
        .collect();
    let unreached = match state {
        SyncState::NotConnected => facts
            .sections
            .iter()
            .filter(|(_, sync)| unreached(sync))
            .count(),
        _ => 0,
    };
    for (index, (path, sync)) in behind.iter().enumerate() {
        ui.open(
            ("section", index),
            Spec {
                size: [fill(), children()],
                gap: 8.0,
                ..Spec::default()
            },
        );
        let name = path
            .strip_suffix(".one")
            .unwrap_or(path)
            .replace('/', " › ");
        ui.leaf(
            "name",
            Spec {
                size: [fill(), px(line)],
                text: Some(&name),
                overflow: Overflow::Ellipsis,
                ..Spec::default()
            },
        );
        let color = match sync.state() {
            SyncState::Syncing => theme.text_dim,
            _ => theme.text,
        };
        ui.leaf(
            "status",
            Spec {
                size: [fit(), px(line)],
                text: Some(&brief(sync, facts.offline)),
                color: Some(color),
                ..Spec::default()
            },
        );
        ui.close();
        if let Some(error) = sync
            .error
            .as_ref()
            .filter(|_| matches!(sync.state(), SyncState::Failed | SyncState::Unreadable))
        {
            text(
                ui,
                &format!("section-{index}-error"),
                &error.to_string(),
                theme.text_dim,
            );
        }
    }
    for (space, title) in &facts.conflicts {
        ui.open(
            ("conflict", space),
            Spec {
                size: [fill(), children()],
                gap: 8.0,
                ..Spec::default()
            },
        );
        ui.leaf(
            "name",
            Spec {
                size: [fill(), px(line)],
                text: Some(title),
                icon: Some(art::SYNC_WARNING),
                overflow: Overflow::Ellipsis,
                ..Spec::default()
            },
        );
        if ui::button(ui, "open", "Open Page").clicked {
            picked.conflict = Some(*space);
        }
        ui.close();
    }
    let summaries = [
        match (behind.len() + unreached, done) {
            (_, 0) | (0, 1) => None,
            (0, all) => Some(format!("All {all} sections up to date")),
            (_, 1) => Some("1 other section up to date".to_owned()),
            (_, others) => Some(format!("{others} other sections up to date")),
        },
        match (unreached, behind.is_empty() && done == 0) {
            (0, _) | (1, true) => None,
            (count, true) => Some(format!("All {count} sections waiting for connection")),
            (1, false) => Some("1 other section waiting for connection".to_owned()),
            (count, false) => Some(format!("{count} other sections waiting for connection")),
        },
    ];
    for (index, summary) in summaries.iter().flatten().enumerate() {
        ui.leaf(
            ("summary", index),
            Spec {
                size: [fill(), px(line)],
                text: Some(summary),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        );
    }
    ui.close();

    if let Some(note) = update_note(facts.update) {
        rule(ui, "update-rule");
        ui.open(
            "update",
            Spec {
                axis: Axis::Y,
                size: [fill(), children()],
                gap: 4.0,
                ..Spec::default()
            },
        );
        text(ui, "note", &format!("Snowbound {note}"), theme.text);
        if let update::Status::Ready(_, _, changes) | update::Status::Available(_, changes) =
            facts.update
            && let Some(summary) = update::summary(changes)
        {
            picked.list_changes = changes_list(ui, &summary, changes, facts.changes_listed);
        }
        ui.open(
            "actions",
            Spec {
                size: [fill(), children()],
                pad: [0.0, 4.0],
                gap: 8.0,
                ..Spec::default()
            },
        );
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        picked.build_folder = ui::button(ui, "build-folder", "Build Folder").clicked;
        if matches!(facts.update, update::Status::Ready(..)) {
            picked.restart = ui::button(ui, "restart", "Restart to Update").clicked;
        }
        ui.close();
        ui.close();
    }

    ui.open(
        "controls",
        Spec {
            size: [fill(), children()],
            pad: [0.0, 4.0],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "space",
        Spec {
            size: [fill(), px(1.0)],
            ..Spec::default()
        },
    );
    // A button that can't act now, dimmed.
    let button = |ui: &mut Ui, part: &str, label: &str, enabled: bool| {
        if enabled {
            return ui::button(ui, part, label).clicked;
        }
        ui.leaf(
            part,
            Spec {
                size: [fit(), px(theme.font_size * 2.0)],
                text: Some(label),
                color: Some(theme.text_dim),
                fill: Some(theme.chip),
                radius: 4.0,
                pad: [theme.font_size * 0.75, 0.0],
                center: true,
                role: Some(accesskit::Role::Button),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(ui.id(part)) {
            node.set_disabled();
        }
        false
    };
    picked.show_file = button(ui, "show-file", platform::SHOW_FILE, facts.local);
    picked.sync_now = button(ui, "sync-now", "Sync Now", !facts.offline);
    ui.close();
    picked
}

/// Opens the popup's box at `anchor`.
fn open(ui: &mut Ui, anchor: Anchor) {
    let theme = ui.theme.clone();
    ui.open_as(
        id(),
        Spec {
            axis: Axis::Y,
            size: [px(WIDTH), children()],
            fill: Some(theme.popup),
            border: Some(theme.chip),
            shadow: Some(theme.shadow),
            radius: 8.0,
            pad: [PAD, 12.0],
            gap: 10.0,
            anchor: Some(anchor),
            role: Some(accesskit::Role::Dialog),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(id()) {
        node.set_label(TITLE);
    }
}

/// `library`'s status in a line, as its context menu heads with it.
pub(crate) fn line(library: &Library, session: &Session) -> (String, &'static [&'static str]) {
    let sections = sections(library, session);
    let sync = overall(&sections);
    let offline = library::offline();
    let (headline, icon) = describe(&sync, offline, false);
    let done = sections
        .iter()
        .filter(|(_, sync)| sync.state() == SyncState::UpToDate)
        .count();
    let text = match sync.state() {
        _ if offline && sync.queued > 0 => format!("Offline · {} waiting", changes(sync.queued)),
        SyncState::Syncing if !offline && sections.len() > 1 => {
            format!("Syncing {done} of {} sections…", sections.len())
        }
        SyncState::UpToDate if !offline => match sync.synced {
            Some(synced) => format!("Up to date · {}", ago(synced)),
            None => headline.to_owned(),
        },
        _ if sync.queued > 0 => format!("{headline} · {} waiting", changes(sync.queued)),
        _ => headline.to_owned(),
    };
    (text, icon)
}

impl State {
    /// Opens the sync status popup on `library`, at `at` in the window.
    pub(crate) fn show_sync(&mut self, library: Arc<Library>, at: [f32; 2]) {
        self.sync_notebook = Some((library, at));
        self.ui.open_popup(id());
    }

    /// Builds the sync status popup while it is open: on the notebook its context menu
    /// opened it for, or the open section's.
    pub(crate) fn sync_popup(&mut self) -> Result<(), Box<dyn Error>> {
        let (ui, notebooks) = (&mut self.ui, &self.notebooks);
        let update = self.updates.status();
        let Some(session) = &mut self.session else {
            ui.close_popup(id());
            return Ok(());
        };
        if !ui.popup_open(id()) {
            self.sync_notebook = None;
            return Ok(());
        }
        // Read afresh while shown, so the last sync time follows each poll.
        session.sync = session.section.sync_status()?;
        let (library, anchor) = match &self.sync_notebook {
            Some((library, at)) => (Arc::clone(library), Anchor::Point(*at)),
            None => (
                Arc::clone(&session.library),
                Anchor::Below(ui.rect(button()).unwrap_or_default()),
            ),
        };
        let own = Arc::ptr_eq(&library, &session.library);
        let offline = library::offline();
        let file = match own {
            true => library.local(session.section.file()),
            false => library.folder().map(Path::to_owned),
        };
        let facts = Facts {
            name: &library.name,
            location: &library.location,
            place: library.place(),
            notice: library.notice.as_deref(),
            sections: sections(&library, session),
            conflicts: session
                .conflicts
                .iter()
                .filter(|(_, pages)| own && !pages.is_empty())
                .filter_map(|(space, _)| {
                    let (_, title, _) = session.pages.iter().find(|page| page.0 == *space)?;
                    Some((*space, title.as_str()))
                })
                .collect(),
            offline,
            update: &update,
            changes_listed: self.updates.changes_listed,
            local: file.is_some(),
        };
        open(ui, anchor);
        let picked = build(ui, &facts);
        ui.close();
        let backgrounds = notebooks
            .iter()
            .filter_map(|library| library.background.as_ref());
        if picked.offline {
            library::set_offline(!offline);
            session.section.set_offline(!offline);
            notebooks
                .iter()
                .for_each(|library| library.set_offline(!offline));
        } else if picked.sync_now {
            if own {
                session.section.wake();
            }
            backgrounds.for_each(|background| background.wake());
        }
        if let Some(file) = file.filter(|_| picked.show_file) {
            platform::show_file(&file);
        }
        if picked.sign_in {
            let location = library.location.clone();
            self.ui.close_popup(id());
            self.commands
                .push(crate::Command::OpenFromServer(Some(location)));
        }
        if let Some(space) = picked.conflict {
            self.ui.close_popup(id());
            self.commands.push(crate::Command::OpenPage(space));
        }
        match update {
            update::Status::Downloading(version)
            | update::Status::Ready(version, ..)
            | update::Status::Available(version, ..)
                if picked.build_folder =>
            {
                update::show_build(&version)
            }
            _ if picked.restart => self.restart_to_update(),
            _ if picked.list_changes => self.updates.changes_listed ^= true,
            _ => {}
        }
        Ok(())
    }
}

/// The popup in each state, laid out with made-up statuses on a made-up clock. A quick sync
/// never shows, a longer one shows its bar and fills it before reading up to date. Where
/// wgpu paints, `SNOWBOUND_SYNC_RENDER` names a directory for PNGs of each state in both
/// appearances, and `frames-light`/`frames-dark` of a sync running, for a GIF.
#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, time::Instant};
    use winit::window::Theme as Appearance;

    const SIZE: [f32; 2] = [380.0, 640.0];
    const SCALE: f32 = 2.0;
    const NAMES: [&str; 5] = [
        "Recipes",
        "Travel",
        "Work/Meetings",
        "Work/Projects",
        "Journal",
    ];

    fn status(synced: bool, queued: u64, error: Option<(io::ErrorKind, &str)>) -> SyncStatus {
        SyncStatus {
            synced: synced.then(|| filetime() - 3 * 600_000_000),
            error: error.map(|(kind, text)| io::Error::new(kind, text)),
            queued,
        }
    }

    fn sections(each: impl Fn(usize) -> SyncStatus) -> Vec<(String, SyncStatus)> {
        NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| (format!("{name}.one"), each(index)))
            .collect()
    }

    static IDLE: update::Status = update::Status::Idle;

    fn facts(sections: Vec<(String, SyncStatus)>) -> Facts<'static> {
        Facts {
            name: "OneNote",
            location: "/Volumes/clover/Documents/OneNote",
            place: ["zenith", "clover", "Documents"]
                .map(str::to_owned)
                .to_vec(),
            notice: None,
            sections,
            conflicts: Vec::new(),
            offline: false,
            update: &IDLE,
            changes_listed: false,
            local: true,
        }
    }

    /// One frame at `now`: the toolbar's button at the top right and the popup open below it.
    fn frame(ui: &mut Ui, now: Instant, facts: &Facts) {
        ui.begin(SIZE, SCALE, now);
        ui.leaf(
            "toolbar",
            Spec {
                size: [fill(), px(TOOL + 12.0)],
                fill: Some(ui.theme.strip),
                ..Spec::default()
            },
        );
        ui.open_as(
            button(),
            Spec {
                flags: Flags::FLOAT,
                size: [px(TOOL), px(TOOL)],
                position: [SIZE[0] - TOOL - 10.0, 6.0],
                icon: Some(art::SYNC_DONE),
                ..Spec::default()
            },
        );
        ui.close();
        if !ui.popup_open(id()) {
            ui.open_popup(id());
        }
        open(ui, Anchor::Below(ui.rect(button()).unwrap_or_default()));
        build(ui, facts);
        ui.close();
        ui.end();
    }

    fn ui(appearance: Appearance) -> Ui {
        let theme = crate::theme(appearance, false, false);
        let mut ui = Ui::new(theme, Duration::from_millis(500));
        ui.icon_palette = ui.theme.icon_palette(
            ui.theme.section(crate::section_color(None)).accent,
            [1.0; 4],
        );
        ui
    }

    #[cfg(all(feature = "wgpu", not(windows)))]
    struct Gpu {
        renderer: draw::Renderer,
        device: wgpu::Device,
        queue: wgpu::Queue,
    }

    #[cfg(all(feature = "wgpu", not(windows)))]
    impl Gpu {
        fn new() -> Self {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter =
                pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
            let (device, queue) =
                pollster::block_on(adapter.request_device(&Default::default())).unwrap();
            let renderer = draw::Renderer::new(
                device.clone(),
                queue.clone(),
                wgpu::TextureFormat::Rgba8UnormSrgb,
            );
            Self {
                renderer,
                device,
                queue,
            }
        }

        /// Writes the frame `ui` laid out to `path`, cropped to the popup and its shadow, or
        /// to `bottom` where given so a GIF's frames are one size.
        fn save(&mut self, ui: &Ui, path: &Path, bottom: Option<f32>) {
            let size = SIZE.map(|side| (side * SCALE) as u32);
            let interface = ui.layers();
            let layers: Vec<_> = interface
                .iter()
                .filter_map(|layer| match layer {
                    ui::Layer::Primitives(primitives) => Some(primitives.layer(SCALE)),
                    ui::Layer::Custom { .. } => None,
                })
                .collect();
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Sync popup"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.renderer
                .draw(
                    &texture.create_view(&Default::default()),
                    size,
                    ui.theme.base,
                    &layers,
                )
                .unwrap();
            let row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Sync popup readback"),
                size: u64::from(row) * u64::from(size[1]),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = self.device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(size[1]),
                    },
                },
                texture.size(),
            );
            self.queue.submit([encoder.finish()]);
            buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(5)),
                })
                .unwrap();
            let mapped = buffer.get_mapped_range(..).unwrap();
            let bottom = bottom
                .unwrap_or(ui.rect(id()).unwrap()[3] + 24.0)
                .min(SIZE[1]);
            let height = (bottom * SCALE) as u32;
            let pixels: Vec<u8> = mapped
                .chunks(row as usize)
                .take(height as usize)
                .flat_map(|line| &line[..size[0] as usize * 4])
                .copied()
                .collect();
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = png::Encoder::new(file, size[0], height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
        }
    }

    /// Lays out `facts` until the popup has opened and settled, from `now`.
    fn settle(ui: &mut Ui, now: &mut Instant, facts: &Facts) {
        for _ in 0..90 {
            *now += Duration::from_millis(16);
            frame(ui, *now, facts);
        }
    }

    fn bar(ui: &Ui) -> bool {
        ui.rect(id().child("status").child("bar")).is_some()
    }

    #[test]
    fn a_quick_sync_never_shows_and_a_long_one_fills_before_up_to_date() {
        let mut ui = ui(Appearance::Light);
        let mut now = Instant::now();
        let done = facts(sections(|_| status(true, 0, None)));
        settle(&mut ui, &mut now, &done);
        let busy = facts(sections(|index| status(true, u64::from(index == 1), None)));
        for _ in 0..12 {
            now += Duration::from_millis(16);
            frame(&mut ui, now, &busy);
            assert!(!bar(&ui), "a sync under {SHOW_AFTER:?} shows");
        }
        settle(&mut ui, &mut now, &done);
        assert!(!bar(&ui));
        settle(&mut ui, &mut now, &busy);
        assert!(bar(&ui), "a running sync shows its bar");
        now += Duration::from_millis(16);
        frame(&mut ui, now, &done);
        assert!(bar(&ui), "a finished sync's bar stays a moment, full");
        now += LINGER + Duration::from_millis(50);
        frame(&mut ui, now, &done);
        frame(&mut ui, now, &done);
        assert!(!bar(&ui));
    }

    type Made = fn() -> Facts<'static>;

    /// Each state the popup shows, by name.
    fn states() -> Vec<(&'static str, Made)> {
        vec![
            ("up-to-date", || facts(sections(|_| status(true, 0, None)))),
            ("syncing", || {
                facts(sections(|index| match index {
                    0 | 1 => status(true, 0, None),
                    2 => status(true, 3, None),
                    _ => status(false, 0, None),
                }))
            }),
            ("offline", || Facts {
                offline: true,
                ..facts(sections(|index| status(true, [0, 4, 0, 1, 0][index], None)))
            }),
            ("not-connected", || {
                facts(sections(|index| {
                    status(
                        true,
                        [0, 2, 0, 0, 0][index],
                        Some((io::ErrorKind::TimedOut, "The server didn’t answer")),
                    )
                }))
            }),
            ("conflict", || Facts {
                conflicts: vec![(ExGuid::default(), "Packing list")],
                ..facts(sections(|_| status(true, 0, None)))
            }),
            ("error", || {
                facts(sections(|index| match index {
                    3 => status(
                        true,
                        1,
                        Some((io::ErrorKind::Other, "The section file ended early.")),
                    ),
                    _ => status(true, 0, None),
                }))
            }),
            ("in-use", || {
                facts(sections(|index| match index {
                    2 => status(true, 2, Some((io::ErrorKind::ResourceBusy, "In use"))),
                    _ => status(true, 0, None),
                }))
            }),
            ("read-only", || {
                facts(sections(|index| {
                    let denied = (io::ErrorKind::PermissionDenied, "Read-only");
                    status(true, u64::from(index == 0), (index == 0).then_some(denied))
                }))
            }),
            ("system-connection", || Facts {
                notice: Some("the Keychain has no password for clover on zenith"),
                place: vec!["clover".into(), "Documents".into()],
                ..facts(sections(|_| status(true, 0, None)))
            }),
            ("icloud", || Facts {
                name: "Personal",
                location: "/Users/clover/Library/Mobile Documents/com~apple~CloudDocs/Notes/Personal",
                place: vec!["iCloud Drive".into(), "Notes".into()],
                ..facts(sections(|_| status(true, 0, None)))
            }),
            ("update", || Facts {
                update: ready(),
                ..facts(sections(|_| status(true, 0, None)))
            }),
            ("update-listed", || Facts {
                update: ready(),
                changes_listed: true,
                ..facts(sections(|_| status(true, 0, None)))
            }),
        ]
    }

    /// A staged build that brings a little of each kind.
    fn ready() -> &'static update::Status {
        let changes = serde_json::from_value(serde_json::json!([
            {"version": "2026-10-01-r3", "kind": "feature", "title": "Styles and themes"},
            {"version": "2026-10-01-r3", "kind": "feature", "title": "Settings redesign with search"},
            {"version": "2026-10-01-r3", "kind": "feature", "title": "Undo across pages"},
            {"version": "2026-10-01-r4", "kind": "fix", "title": "Section copies keep their own TOC entries"},
            {"version": "2026-10-01-r4", "kind": "fix", "title": "Pasted pictures keep the size OneNote 2010 gives them"},
            {"version": "2026-10-01-r5", "kind": "other", "title": "Drop leftover debug output"},
        ]))
        .unwrap();
        Box::leak(Box::new(update::Status::Ready(
            update::Version::parse("2026-10-01-r5").unwrap(),
            std::path::PathBuf::new(),
            changes,
        )))
    }

    /// Work offline's switch stays put as it turns on and off, and so do the buttons below
    /// but where a problem's advice, which offline leaves out, goes and comes back.
    #[test]
    fn toggling_offline_moves_nothing() {
        let switch = id().child("status").child("header").child("offline");
        let controls = id().child("controls");
        for (name, made) in states() {
            let advised = !matches!(
                overall(&made().sections).state(),
                SyncState::UpToDate | SyncState::Syncing
            );
            let mut places = Vec::new();
            for offline in [false, true, false] {
                let mut ui = ui(Appearance::Light);
                let mut now = Instant::now();
                let facts = Facts { offline, ..made() };
                settle(&mut ui, &mut now, &facts);
                places.push((ui.rect(switch).unwrap(), ui.rect(controls).unwrap()));
            }
            for pair in places.windows(2) {
                assert_eq!(pair[0].0, pair[1].0, "{name}");
                assert!(advised || pair[0].1 == pair[1].1, "{name}: {places:?}");
            }
        }
    }

    #[cfg(all(feature = "wgpu", not(windows)))]
    #[test]
    fn popup_renders_each_state() {
        let output = std::env::var_os("SNOWBOUND_SYNC_RENDER").map(std::path::PathBuf::from);
        let mut gpu = output.as_ref().map(|output| {
            std::fs::create_dir_all(output).unwrap();
            Gpu::new()
        });
        let states = states();
        for (appearance, theme) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
            for (name, facts) in &states {
                let mut ui = ui(appearance);
                let mut now = Instant::now();
                let facts = facts();
                settle(&mut ui, &mut now, &facts);
                assert!(ui.rect(id()).is_some(), "{name}");
                if let (Some(output), Some(gpu)) = (&output, &mut gpu) {
                    gpu.save(&ui, &output.join(format!("{name}-{theme}.png")), None);
                }
            }
            // Changes made offline sending once the share is back, a section at a time, at
            // 30 frames a second; a quick one first never shows.
            let Some((output, gpu)) = output.as_ref().zip(gpu.as_mut()) else {
                continue;
            };
            let frames = output.join(format!("frames-{theme}"));
            std::fs::create_dir_all(&frames).unwrap();
            let mut ui = ui(appearance);
            let mut now = Instant::now();
            settle(
                &mut ui,
                &mut now,
                &facts(sections(|_| status(true, 0, None))),
            );
            let mut shot = 0;
            for tick in 0..300 {
                let seconds = tick as f32 / 60.0;
                let quick = (0.2..0.45).contains(&seconds);
                // Each section's waiting changes, and when they're sent.
                let sending = [(0, 0.0), (3, 1.4), (1, 2.6), (0, 0.0), (2, 3.3)];
                let facts = facts(sections(|index| {
                    let (queued, sent) = sending[index];
                    let queued = match seconds {
                        _ if index == 0 && quick => 1,
                        seconds if (0.8..sent).contains(&seconds) => queued,
                        _ => 0,
                    };
                    status(true, queued, None)
                }));
                now += Duration::from_micros(16_667);
                frame(&mut ui, now, &facts);
                if tick % 2 == 0 {
                    gpu.save(&ui, &frames.join(format!("{shot:03}.png")), Some(360.0));
                    shot += 1;
                }
            }
            // Work offline on, an edit waiting, then off: the edit sends and nothing moves.
            let frames = output.join(format!("toggle-{theme}"));
            std::fs::create_dir_all(&frames).unwrap();
            let mut ui = self::ui(appearance);
            let mut now = Instant::now();
            settle(
                &mut ui,
                &mut now,
                &facts(sections(|_| status(true, 0, None))),
            );
            for tick in 0..360 {
                let seconds = tick as f32 / 60.0;
                let queued = u64::from((2.0..4.6).contains(&seconds)) * 2;
                let facts = Facts {
                    offline: (1.0..3.2).contains(&seconds),
                    ..facts(sections(|index| {
                        status(true, u64::from(index == 1) * queued, None)
                    }))
                };
                now += Duration::from_micros(16_667);
                frame(&mut ui, now, &facts);
                if tick % 2 == 0 {
                    let path = frames.join(format!("{:03}.png", tick / 2));
                    gpu.save(&ui, &path, Some(360.0));
                }
            }
        }
    }
}
