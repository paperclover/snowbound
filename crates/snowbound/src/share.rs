//! Live Share's dialogs: sharing the notebook shown from this computer, with its code and the
//! people connected, and opening a notebook another computer shares by its code.

use crate::{Library, State};
use accesskit::Role;
use notebook::live::{
    Relayed,
    share::{self, Refusal, Sharing},
    wire::Welcome,
};
use std::sync::{Arc, mpsc};
use ui::{Anchor, Axis, Id, Spec, Ui, children, fill, fit, px};
use winit::keyboard::NamedKey;

const WIDTH: f32 = 420.0;
/// What marks Live Share as still settling, wherever it is offered.
pub(crate) const BETA: &str = "BETA";

fn share_id() -> Id {
    Id::ROOT.child("live-share")
}

fn join_id() -> Id {
    Id::ROOT.child("open-shared")
}

fn password_field() -> Id {
    share_id().child("password")
}

fn code_field() -> Id {
    join_id().child("code")
}

fn join_password() -> Id {
    join_id().child("password")
}

/// Live Share on one notebook while it is open.
pub(crate) struct ShareDialog {
    library: Arc<Library>,
    protect: bool,
    password: String,
    copied: bool,
}

/// Open Shared Notebook while it is open.
pub(crate) struct JoinDialog {
    code: String,
    password: String,
    status: Status,
    replies: crate::live::Channel<Result<Welcome, Refusal>>,
}

enum Status {
    Idle,
    Waiting,
    Failed(String),
}

/// What to tell someone whose code didn't open a notebook.
fn refusal(refusal: &Refusal) -> String {
    match refusal {
        Refusal::Malformed => "Check the code. It looks like 7KQ-4MZ-9XR.".into(),
        Refusal::Version { theirs_newer: true } => {
            "The person sharing has a newer Snowbound. Update Snowbound, then try again.".into()
        }
        Refusal::Version {
            theirs_newer: false,
        } => "The person sharing has an older \
                                                     Snowbound. Ask them to update it."
            .into(),
        Refusal::Wrong => "That code or password doesn’t open a notebook. Check it with the \
                           person sharing."
            .into(),
        Refusal::NoOne => "No one is sharing with that code. Check the number, and that \
                           Snowbound is open on their computer."
            .into(),
        Refusal::Expired => {
            "That code expired after too many wrong tries. Ask for a new one.".into()
        }
        Refusal::TooMany(wait) => match wait.map(|wait| wait.as_secs().div_ceil(60)) {
            Some(1) => "Too many wrong codes from this network. Try again in a minute.".into(),
            Some(minutes) => {
                format!("Too many wrong codes from this network. Try again in {minutes} minutes.")
            }
            None => "Too many wrong codes from this network. Try again later.".into(),
        },
        Refusal::Busy => "The Live Share relay is busy. Try again in a minute.".into(),
        Refusal::Unreachable => {
            "Can’t reach the Live Share relay. Check your internet connection.".into()
        }
        Refusal::TimedOut => "The computer sharing didn’t answer. Try again.".into(),
    }
}

/// A dialog's frame, titled `title` with the BETA badge.
fn frame(ui: &mut Ui, id: Id, title: &str) {
    let theme = ui.theme.clone();
    let row = theme.font_size * 2.0;
    ui.open_as(
        id,
        Spec {
            axis: Axis::Y,
            size: [px(WIDTH), children()],
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
    if let Some(node) = ui.access(id) {
        node.set_label(title);
    }
    ui.open(
        "title",
        Spec {
            size: [fill(), px(row)],
            gap: 6.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "text",
        Spec {
            size: [fit(), px(row)],
            text: Some(title),
            bold: true,
            role: Some(Role::Heading),
            ..Spec::default()
        },
    );
    ui::badge(ui, "beta", BETA, row);
    ui.close();
}

/// Text wrapped across the dialog, dim where `dim`.
fn text(ui: &mut Ui, part: &str, text: &str, dim: bool) {
    let color = if dim {
        ui.theme.text_dim
    } else {
        ui.theme.text
    };
    ui.leaf(
        part,
        Spec {
            size: [fill(), fit()],
            text: Some(text),
            overflow: ui::Overflow::Wrap,
            color: Some(color),
            ..Spec::default()
        },
    );
}

fn status(ui: &mut Ui, message: &str) {
    ui.leaf(
        "status",
        Spec {
            size: [fill(), fit()],
            text: Some(message),
            overflow: ui::Overflow::Wrap,
            pad: [0.0, 4.0],
            role: Some(Role::Status),
            ..Spec::default()
        },
    );
}

fn field_spec(ui: &Ui) -> Spec<'static> {
    let theme = &ui.theme;
    Spec {
        size: [fill(), px(theme.font_size * 2.0)],
        fill: Some(theme.base),
        border: Some(theme.accent),
        radius: 4.0,
        pad: [6.0, 0.0],
        ..Spec::default()
    }
}

/// `label` before what `build` makes, in one row.
fn labelled(ui: &mut Ui, label: &str, build: impl FnOnce(&mut Ui)) {
    let row = ui.theme.font_size * 2.0;
    ui.open(
        label,
        Spec {
            size: [fill(), px(row)],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "label",
        Spec {
            size: [px(80.0), px(row)],
            text: Some(label),
            ..Spec::default()
        },
    );
    build(ui);
    ui.close();
}

/// Opens a row of buttons, a space taking what they leave.
fn buttons(ui: &mut Ui) {
    ui.open(
        "buttons",
        Spec {
            size: [fill(), children()],
            pad: [0.0, 8.0],
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
}

impl State {
    /// Opens Live Share on `library`.
    pub(crate) fn open_live_share(&mut self, library: Arc<Library>) {
        self.peers.share = Some(ShareDialog {
            library,
            protect: false,
            password: String::new(),
            copied: false,
        });
        self.ui.open_popup(share_id());
    }

    /// Opens Open Shared Notebook.
    pub(crate) fn open_shared(&mut self) {
        self.peers.join = Some(JoinDialog {
            code: String::new(),
            password: String::new(),
            status: Status::Idle,
            replies: mpsc::channel(),
        });
        self.ui.open_popup(join_id());
        self.ui.focus_all(code_field());
    }

    pub(crate) fn live_dialogs(&mut self) {
        self.share_dialog();
        self.join_dialog();
    }

    fn share_dialog(&mut self) {
        let Some(mut dialog) = self.peers.share.take() else {
            return;
        };
        if !self.ui.popup_open(share_id()) {
            return;
        }
        let location = dialog.library.location.clone();
        let host = self.peers.hosts.get(&location).cloned();
        let starting = self.peers.starting.get(&location).cloned();
        let ui = &mut self.ui;
        let theme = ui.theme.clone();
        // Enter takes the dialog's default, Start Sharing or else Done, wherever the focus is
        // but on another button.
        let entered =
            ui::popup::navigation(ui, &[share_id(), password_field()], &[NamedKey::Enter])
                .contains(&NamedKey::Enter);
        frame(ui, share_id(), "Live Share");
        text(ui, "notebook", &dialog.library.name, true);
        let (mut start, mut stop, mut copy) = (false, false, None);
        match (&host, &starting) {
            (Some(host), _) => {
                let code = host.code();
                ui.open(
                    "code",
                    Spec {
                        size: [fill(), px(theme.font_size * 3.0)],
                        gap: 8.0,
                        ..Spec::default()
                    },
                );
                ui.leaf(
                    "text",
                    Spec {
                        size: [fill(), fill()],
                        text: Some(code.as_deref().unwrap_or("Getting a code…")),
                        font_size: Some(if code.is_some() {
                            theme.font_size * 1.6
                        } else {
                            theme.font_size
                        }),
                        bold: code.is_some(),
                        color: code.is_none().then_some(theme.text_dim),
                        role: Some(Role::Label),
                        ..Spec::default()
                    },
                );
                if let Some(code) = &code {
                    let label = if dialog.copied { "Copied" } else { "Copy" };
                    if ui::button(ui, "copy", label).clicked {
                        copy = Some(code.clone());
                    }
                }
                ui.close();
                let protected = !host.sharing().password.is_empty();
                text(
                    ui,
                    "how",
                    if protected {
                        "Others open it with Open Shared Notebook, this code and the password."
                    } else {
                        "Others open it with Open Shared Notebook and this code."
                    },
                    true,
                );
                match host.relayed() {
                    Relayed::Unreachable | Relayed::Refused(..) => status(
                        ui,
                        "Can’t reach the Live Share relay. Only people on this network can join.",
                    ),
                    _ => {}
                }
                ui.leaf(
                    "people",
                    Spec {
                        size: [fill(), px(theme.font_size * 2.0)],
                        text: Some("Connected"),
                        bold: true,
                        role: Some(Role::Heading),
                        ..Spec::default()
                    },
                );
                let guests = host.guests();
                if guests.is_empty() {
                    text(ui, "nobody", "No one has joined yet.", true);
                }
                for guest in &guests {
                    ui.leaf(
                        ("guest", guest.hello.peer),
                        Spec {
                            size: [fill(), px(theme.font_size * 1.6)],
                            text: Some(&guest.hello.name),
                            role: Some(Role::ListItem),
                            ..Spec::default()
                        },
                    );
                }
                buttons(ui);
                stop = ui::button(ui, "stop", "Stop Sharing").clicked;
            }
            (None, Some(None)) => {
                text(ui, "starting", "Starting…", true);
                buttons(ui);
            }
            (None, failed) => {
                text(
                    ui,
                    "what",
                    "People you give the code to can open and edit this notebook while \
                     Snowbound is open on this computer.",
                    false,
                );
                if ui::check_box(ui, "protect", "Require a password", dialog.protect).clicked {
                    dialog.protect = !dialog.protect;
                    if dialog.protect {
                        ui.focus_all(password_field());
                    }
                }
                if dialog.protect {
                    labelled(ui, "Password:", |ui| {
                        let spec = field_spec(ui);
                        ui::password_field(ui, password_field(), &mut dialog.password, "", spec);
                        if let Some(node) = ui.access(password_field()) {
                            node.set_label("Password");
                        }
                    });
                }
                if let Some(Some(error)) = failed {
                    status(ui, &format!("Couldn’t start sharing: {error}"));
                }
                buttons(ui);
                start = ui::button(ui, "start", "Start Sharing").clicked || entered;
            }
        }
        let offered = host.is_none() && !matches!(starting, Some(None));
        let done = ui::button(ui, "done", "Done").clicked || entered && !offered;
        ui.close();
        ui.close();
        if let Some(code) = copy {
            dialog.copied = self.clipboard.set_text(code).is_ok();
        }
        if stop {
            self.stop_sharing(&location);
        }
        if start && !(dialog.protect && dialog.password.is_empty()) {
            let password = if dialog.protect {
                dialog.password.clone()
            } else {
                String::new()
            };
            match Sharing::new(&password) {
                Ok(sharing) => self.start_sharing(&dialog.library, sharing),
                Err(error) => {
                    self.peers
                        .starting
                        .insert(location, Some(error.to_string()));
                }
            }
        }
        if done {
            self.ui.close_popup(share_id());
            self.ui.set_focus(Some(crate::page()));
            return;
        }
        self.peers.share = Some(dialog);
    }

    fn join_dialog(&mut self) {
        let Some(mut dialog) = self.peers.join.take() else {
            return;
        };
        if !self.ui.popup_open(join_id()) {
            return;
        }
        let mut welcomed = None;
        for reply in dialog.replies.1.try_iter() {
            match reply {
                Ok(welcome) => welcomed = Some(welcome),
                Err(refused) => dialog.status = Status::Failed(refusal(&refused)),
            }
        }
        if let Some(welcome) = welcomed {
            match crate::live::joined(&self.cache, welcome) {
                Ok(location) => {
                    self.ui.close_popup(join_id());
                    self.open_notebook(location, None);
                    return;
                }
                Err(error) => {
                    dialog.status = Status::Failed(format!("Couldn’t keep the code: {error}"));
                }
            }
        }
        let ui = &mut self.ui;
        let owners = [join_id(), code_field(), join_password()];
        let entered =
            ui::popup::navigation(ui, &owners, &[NamedKey::Enter]).contains(&NamedKey::Enter);
        frame(ui, join_id(), "Open Shared Notebook");
        text(ui, "what", "Enter the code from the person sharing.", false);
        labelled(ui, "Code:", |ui| {
            let spec = field_spec(ui);
            ui::text_field(ui, code_field(), &mut dialog.code, "7KQ-4MZ-9XR", spec);
            if let Some(node) = ui.access(code_field()) {
                node.set_label("Code");
            }
        });
        labelled(ui, "Password:", |ui| {
            let spec = field_spec(ui);
            ui::password_field(
                ui,
                join_password(),
                &mut dialog.password,
                "If they set one",
                spec,
            );
            if let Some(node) = ui.access(join_password()) {
                node.set_label("Password");
            }
        });
        match &dialog.status {
            Status::Idle => {}
            Status::Waiting => status(ui, "Connecting…"),
            Status::Failed(message) => status(ui, message),
        }
        buttons(ui);
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let open = ui::button(ui, "open", "Open").clicked || entered;
        ui.close();
        ui.close();
        if cancel {
            self.ui.close_popup(join_id());
            self.ui.set_focus(Some(crate::page()));
            return;
        }
        if open && !matches!(dialog.status, Status::Waiting) {
            match share::code(&dialog.code) {
                None => dialog.status = Status::Failed(refusal(&Refusal::Malformed)),
                Some(code) => {
                    dialog.status = Status::Waiting;
                    let (replies, redraw) = (dialog.replies.0.clone(), self.redraw.clone());
                    let password = dialog.password.clone();
                    crate::spawn(move || {
                        let reply = crate::live::join(&code, &password);
                        let _ = replies.send(reply);
                        redraw.wake();
                    });
                }
            }
        }
        self.peers.join = Some(dialog);
    }
}
