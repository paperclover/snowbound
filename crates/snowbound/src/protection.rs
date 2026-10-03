//! Password-protected sections as OneNote 2010 shows them: a locked section's page, the
//! dialogs that unlock one and set, change or remove its password, Lock All, and locking a
//! section after a while unused or as it is left. Keys live in memory only (`Library`).

use crate::{Command, Library, State, art, manage::Structure};
use accesskit::Role;
use std::{error::Error, sync::Arc, time::Duration};
use ui::{Anchor, Axis, Flags, Id, Spec, Ui, children, fill, px};
use winit::keyboard::NamedKey;
use zeroize::Zeroizing;

const WIDTH: f32 = 440.0;

/// What Options' "Lock password protected sections after…" offers, in minutes, as
/// OneNote 2010's list does.
pub const AFTER: [(u32, &str); 11] = [
    (1, "1 Minute"),
    (5, "5 Minutes"),
    (10, "10 Minutes"),
    (15, "15 Minutes"),
    (30, "30 Minutes"),
    (60, "1 Hour"),
    (120, "2 Hours"),
    (240, "4 Hours"),
    (480, "8 Hours"),
    (720, "12 Hours"),
    (1440, "1 Day"),
];

/// A password-protected section shown locked, in place of its pages.
pub struct Locked {
    pub library: Arc<Library>,
    pub path: String,
}

/// A password dialog on a section.
pub struct Asking {
    library: Arc<Library>,
    path: String,
    dialog: Dialog,
}

enum Dialog {
    /// Protected Section: the password that unlocks it; whether the last was wrong.
    Unlock(Zeroizing<String>, bool),
    /// Password Protection: whether it is protected, and the ways to change that.
    Protection,
    /// Sets a password: it, its confirmation, and why the last try was refused.
    Set([Zeroizing<String>; 2], Option<&'static str>),
    /// Change Password: the old password, the new one and its confirmation.
    Change([Zeroizing<String>; 3], Option<&'static str>),
    /// Remove Password: the current password.
    Remove(Zeroizing<String>, bool),
}

fn id() -> Id {
    Id::ROOT.child("password")
}

fn field(index: usize) -> Id {
    id().child(("field", index))
}

/// The locked page's notice, which a click or Enter unlocks.
fn notice() -> Id {
    Id::ROOT.child("locked-notice")
}

fn section_name(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".one").unwrap_or(name)
}

impl State {
    /// Shows the locked section `path` of `library` in place of the open section, which stays
    /// open a while for coming back to.
    pub(crate) fn show_locked(
        &mut self,
        library: Arc<Library>,
        path: String,
    ) -> Result<(), Box<dyn Error>> {
        self.stop_loading();
        self.persist()?;
        if let Some(previous) = self.session.take() {
            let path = &previous.tabs[previous.tab].path;
            if self.leave(&previous.library, path) {
                previous.section.close()?;
            } else {
                previous.library.keep(path, previous.section);
            }
        }
        self.sectionless = None;
        self.locked = Some(Locked { library, path });
        self.ui.set_focus(Some(notice()));
        self.title();
        Ok(())
    }

    /// Locks `path` of `library` as it is left, where Options asks for that; whether it did.
    pub(crate) fn leave(&mut self, library: &Arc<Library>, path: &str) -> bool {
        if self.passwords.lock_on_leave && library.protected(path) {
            library.lock(|locked, _| locked == path);
            self.prefetch.forget(&library.key(path));
            return true;
        }
        false
    }

    /// The section tabs of the locked section's folder, its own shown.
    pub(crate) fn locked_tabs(
        &mut self,
        row: Id,
        section: &ui::Section,
        strip: [f32; 4],
    ) -> (Option<Command>, Id) {
        let Some(locked) = &self.locked else {
            return (None, row);
        };
        let library = Arc::clone(&locked.library);
        let tabs = library.tabs(&crate::menus::folder(&locked.path));
        let shown = tabs
            .iter()
            .position(|tab| tab.path == locked.path)
            .unwrap_or(0);
        let names: Vec<_> = tabs
            .iter()
            .map(|tab| (tab.name.as_str(), crate::section_color(tab.color), false))
            .collect();
        let drawn = ui::shell::section_tabs(
            &mut self.ui,
            row,
            &names,
            shown,
            None,
            None,
            section,
            crate::TAB_ROW,
            strip,
        );
        if let Some((tab, point)) = drawn.context {
            self.menu = Some((
                crate::menus::Target::Section {
                    library: Arc::clone(&library),
                    path: tabs[tab].path.clone(),
                },
                point,
            ));
            self.ui.open_popup(crate::menus::id());
        }
        let clicked = drawn
            .clicked
            .filter(|tab| *tab != shown)
            .map(|tab| Command::OpenSection(library, tabs[tab].path.clone()));
        (clicked, drawn.open)
    }

    /// The locked section's page, as OneNote 2010 draws it: a click or Enter unlocks.
    pub(crate) fn locked_page(&mut self) {
        let Some(locked) = &self.locked else {
            return;
        };
        let (library, path) = (Arc::clone(&locked.library), locked.path.clone());
        let theme = &self.page_area_theme();
        let chrome = std::mem::replace(&mut self.ui.theme, theme.clone());
        let ui = &mut self.ui;
        let entered =
            ui::popup::navigation(ui, &[notice()], &[NamedKey::Enter]).contains(&NamedKey::Enter);
        let area = ui.open(
            "locked",
            Spec {
                size: [fill(), fill()],
                fill: Some(ui::mix(theme.paper, theme.chip, 0.6)),
                ..Spec::default()
            },
        );
        let [left, top, right, bottom] = ui.rect(area).unwrap_or_default();
        let width = 380.0_f32.min(right - left);
        let open = ui.open_as(
            notice(),
            Spec {
                flags: Flags::FLOAT | Flags::CLICKABLE | Flags::FOCUSABLE,
                size: [px(width), children()],
                position: [
                    ((right - left - width) / 2.0).max(0.0),
                    ((bottom - top) * 0.2).max(0.0),
                ],
                gap: 12.0,
                role: Some(Role::Button),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(open) {
            node.set_label("This section is password protected. Unlock it");
        }
        ui.leaf(
            "icon",
            Spec {
                size: [px(20.0), px(theme.font_size * 1.6)],
                icon: Some(art::INFO),
                ..Spec::default()
            },
        );
        ui.open(
            "words",
            Spec {
                axis: Axis::Y,
                size: [fill(), children()],
                gap: 14.0,
                ..Spec::default()
            },
        );
        let line = |ui: &mut Ui, part, text, bold| {
            ui.leaf(
                part,
                Spec {
                    size: [fill(), px(theme.font_size * 1.6)],
                    text: Some(text),
                    bold,
                    font_size: Some(theme.font_size * 1.15),
                    ..Spec::default()
                },
            );
        };
        line(ui, "title", "This section is password protected.", true);
        line(
            ui,
            "how",
            "Click here or press ENTER to unlock this section.",
            false,
        );
        ui.close();
        ui.close();
        ui.close();
        ui.theme = chrome;
        if ui.signal(open).clicked || entered {
            self.ask_password(library, path, Dialog::Unlock(Zeroizing::default(), false));
        }
    }

    /// The section shown, open or locked, with its notebook.
    pub(crate) fn shown_section(&self) -> Option<(Arc<Library>, String)> {
        match (&self.session, &self.locked) {
            (Some(session), _) if session.library.catalog().is_some() => Some((
                Arc::clone(&session.library),
                session.tabs[session.tab].path.clone(),
            )),
            (None, Some(locked)) => Some((Arc::clone(&locked.library), locked.path.clone())),
            _ => None,
        }
    }

    /// Opens Password Protection on the section at `path` of `library`, as its menu's
    /// Password Protect This Section… does.
    pub(crate) fn password_protection(&mut self, library: Arc<Library>, path: String) {
        self.ask_password(library, path, Dialog::Protection);
    }

    fn ask_password(&mut self, library: Arc<Library>, path: String, dialog: Dialog) {
        self.password = Some(Asking {
            library,
            path,
            dialog,
        });
        self.ui.open_popup(id());
    }

    /// Locks every protected section unlocked this run, as Lock All and Ctrl+Alt+L do.
    pub(crate) fn lock_all(&mut self) -> Result<(), Box<dyn Error>> {
        let mut locked = Vec::new();
        for library in self.notebooks.clone() {
            let paths = library.lock(|_, _| true);
            locked.extend(paths.iter().map(|path| library.key(path)));
        }
        self.sync_index(true, locked);
        self.relock()
    }

    /// Locks the sections not worked in for as long as Options says, and wakes in time to
    /// lock the next.
    pub(crate) fn lock_idle(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(minutes) = self.passwords.lock_after else {
            return Ok(());
        };
        let after = Duration::from_secs(u64::from(minutes) * 60);
        // Working in the open section keeps it unlocked.
        if self.changed
            && let Some(session) = &self.session
        {
            session.library.touch(&session.tabs[session.tab].path);
        }
        let (mut waiting, mut locked) = (false, Vec::new());
        for library in self.notebooks.clone() {
            let paths = library.lock(|_, idle| {
                waiting |= idle < after;
                idle >= after
            });
            locked.extend(paths.iter().map(|path| library.key(path)));
        }
        if waiting {
            self.ui.wake_after(Duration::from_secs(30));
        }
        if !locked.is_empty() {
            self.sync_index(true, locked);
        }
        self.relock()
    }

    /// Shows the open section locked once its key is gone.
    fn relock(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(session) = &self.session else {
            return Ok(());
        };
        let (library, path) = (
            Arc::clone(&session.library),
            session.tabs[session.tab].path.clone(),
        );
        if !library.locked(&path) {
            return Ok(());
        }
        self.persist()?;
        if let Some(session) = self.session.take() {
            session.section.close()?;
        }
        self.prefetch.forget(&library.key(&path));
        self.password = None;
        self.ui.close_popup(id());
        self.show_locked(library, path)
    }

    /// The open password dialog; what its OK does.
    pub(crate) fn password_dialog(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(mut asking) = self.password.take() else {
            return Ok(());
        };
        if !self.ui.popup_open(id()) {
            return Ok(());
        }
        let theme = self.ui.theme.clone();
        let row = theme.font_size * 2.0;
        let ui = &mut self.ui;
        let fields = match &asking.dialog {
            Dialog::Unlock(..) | Dialog::Remove(..) => 1,
            Dialog::Protection => 0,
            Dialog::Set(..) => 2,
            Dialog::Change(..) => 3,
        };
        let owners: Vec<Id> = (0..fields).map(field).collect();
        // A dialog asking for a password has one of its fields focused, the first at first.
        if !owners.is_empty() && !ui.focused().is_some_and(|focus| owners.contains(&focus)) {
            ui.set_focus(Some(field(0)));
        }
        let entered =
            ui::popup::navigation(ui, &owners, &[NamedKey::Enter]).contains(&NamedKey::Enter);
        let protected = asking.library.protected(&asking.path);
        let title = match asking.dialog {
            Dialog::Unlock(..) => "Protected Section",
            Dialog::Protection | Dialog::Set(..) => "Password Protection",
            Dialog::Change(..) => "Change Password",
            Dialog::Remove(..) => "Remove Password",
        };
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(WIDTH), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [16.0, 12.0],
                gap: 8.0,
                anchor: Some(Anchor::Dialog),
                role: Some(Role::Dialog),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id()) {
            node.set_label(title);
        }
        let text = |ui: &mut Ui, part: &str, text: &str, bold| {
            ui.leaf(
                part,
                Spec {
                    size: [fill(), px(row)],
                    text: Some(text),
                    bold,
                    ..Spec::default()
                },
            );
        };
        text(ui, "title", title, true);
        let name = section_name(&asking.path);
        let field_spec = Spec {
            size: [fill(), px(row)],
            fill: Some(theme.base),
            border: Some(theme.accent),
            radius: 4.0,
            pad: [6.0, 0.0],
            ..Spec::default()
        };
        let password = |ui: &mut Ui, index, label: &str, value: &mut String| {
            text(ui, &format!("label {index}"), label, false);
            ui::password_field(ui, field(index), value, "", field_spec.clone());
            if let Some(node) = ui.access(field(index)) {
                node.set_label(label.trim_end_matches(':'));
            }
        };
        let problem = |ui: &mut Ui, problem: &str| {
            ui.leaf(
                "problem",
                Spec {
                    size: [fill(), px(row)],
                    icon: Some(art::WARNING),
                    text: Some(problem),
                    bold: true,
                    gap: 6.0,
                    role: Some(Role::Alert),
                    ..Spec::default()
                },
            );
        };
        // Password Protection's own buttons; the others' are Cancel and OK.
        let mut buttons: &[&str] = &[];
        match &mut asking.dialog {
            Dialog::Unlock(value, wrong) => {
                text(
                    ui,
                    "about",
                    &format!("Section \u{201c}{name}\u{201d} is password protected."),
                    false,
                );
                if *wrong {
                    problem(ui, "Password is incorrect.");
                }
                password(ui, 0, "Enter Password:", value);
            }
            Dialog::Protection => {
                let status = if protected {
                    format!("Section \u{201c}{name}\u{201d} is password protected.")
                } else {
                    format!("Section \u{201c}{name}\u{201d} is not password protected.")
                };
                text(ui, "status", &status, false);
                buttons = if protected {
                    &["Change Password", "Remove Password", "Lock All", "Close"]
                } else {
                    &["Set Password", "Lock All", "Close"]
                };
            }
            Dialog::Set([value, confirm], refused) => {
                if let Some(refused) = refused {
                    problem(ui, refused);
                }
                password(ui, 0, "Enter Password:", value);
                password(ui, 1, "Confirm Password:", confirm);
                text(ui, "caution", "Caution", true);
                ui.leaf(
                    "caution-text",
                    Spec {
                        size: [fill(), ui::fit()],
                        text: Some(
                            "If you lose or forget the password, Snowbound cannot recover \
                             your data. (Remember that passwords are case-sensitive.)",
                        ),
                        overflow: ui::Overflow::Wrap,
                        ..Spec::default()
                    },
                );
            }
            Dialog::Change([old, value, confirm], refused) => {
                if let Some(refused) = refused {
                    problem(ui, refused);
                }
                password(ui, 0, "Old Password:", old);
                password(ui, 1, "Enter New Password:", value);
                password(ui, 2, "Confirm Password:", confirm);
            }
            Dialog::Remove(value, wrong) => {
                if *wrong {
                    problem(ui, "Password is incorrect.");
                }
                password(
                    ui,
                    0,
                    "To remove password protection, enter the current password:",
                    value,
                );
            }
        }
        ui.open(
            "buttons",
            Spec {
                size: [fill(), children()],
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
        let pressed = if buttons.is_empty() {
            match ui::dialog_buttons(ui, "OK", true) {
                [true, _] => Some("Cancel"),
                [_, true] => Some("OK"),
                _ => None,
            }
        } else {
            (buttons.iter())
                .find(|label| ui::button(ui, **label, label).clicked)
                .copied()
        };
        ui.close();
        ui.close();
        let chosen = pressed.or(entered.then_some("OK"));
        let Asking {
            library,
            path,
            dialog,
        } = asking;
        let next = match (chosen, dialog) {
            (None, dialog) => {
                self.password = Some(Asking {
                    library,
                    path,
                    dialog,
                });
                return Ok(());
            }
            (Some("Cancel" | "Close"), _) => None,
            (Some("Lock All"), _) => {
                self.ui.close_popup(id());
                return self.lock_all();
            }
            (Some("Set Password"), _) => Some(Dialog::Set(Default::default(), None)),
            (Some("Change Password"), _) => Some(Dialog::Change(Default::default(), None)),
            (Some("Remove Password"), _) => Some(Dialog::Remove(Zeroizing::default(), false)),
            (Some(_), Dialog::Unlock(value, _)) => match library.unlock(&path, &value) {
                Ok(()) => {
                    self.commands
                        .push(Command::OpenSection(Arc::clone(&library), path.clone()));
                    None
                }
                Err(notebook::Error::Protected(_)) => {
                    Some(Dialog::Unlock(Zeroizing::default(), true))
                }
                Err(error) => {
                    crate::platform::alert(
                        "Couldn't unlock the section",
                        &crate::plain(&error, "section"),
                    );
                    None
                }
            },
            (Some(_), Dialog::Set([value, confirm], _)) => {
                if value.is_empty() {
                    Some(Dialog::Set(Default::default(), Some("Enter a password.")))
                } else if *value != *confirm {
                    Some(Dialog::Set(
                        Default::default(),
                        Some("The passwords don\u{2019}t match."),
                    ))
                } else {
                    self.set_password(library.clone(), path.clone(), None, Some(value));
                    None
                }
            }
            (Some(_), Dialog::Change([old, value, confirm], _)) => {
                if value.is_empty() {
                    Some(Dialog::Change(
                        Default::default(),
                        Some("Enter a password."),
                    ))
                } else if *value != *confirm {
                    Some(Dialog::Change(
                        Default::default(),
                        Some("The new passwords don\u{2019}t match."),
                    ))
                } else {
                    match current(&library, &path, &old) {
                        Some(key) => {
                            self.set_password(
                                library.clone(),
                                path.clone(),
                                Some(key),
                                Some(value),
                            );
                            None
                        }
                        None => Some(Dialog::Change(
                            Default::default(),
                            Some("Password is incorrect."),
                        )),
                    }
                }
            }
            (Some(_), Dialog::Remove(value, _)) => match current(&library, &path, &value) {
                Some(key) => {
                    self.set_password(library.clone(), path.clone(), Some(key), None);
                    None
                }
                None => Some(Dialog::Remove(Zeroizing::default(), true)),
            },
            (Some(_), Dialog::Protection) => None,
        };
        match next {
            Some(dialog) => self.ask_password(library, path, dialog),
            None => self.ui.close_popup(id()),
        }
        Ok(())
    }

    /// Sets, changes or removes the password of `path` in `library`, closing its section
    /// first: the section is written anew, and its replica goes with the old file.
    fn set_password(
        &mut self,
        library: Arc<Library>,
        path: String,
        key: Option<onestore::protected::Key>,
        password: Option<Zeroizing<String>>,
    ) {
        let open = self.session.as_ref().is_some_and(|session| {
            Arc::ptr_eq(&session.library, &library) && session.tabs[session.tab].path == path
        });
        if open {
            let closed = self.persist().and_then(|()| {
                if let Some(session) = self.session.take() {
                    session.section.close()?;
                }
                Ok(())
            });
            if let Err(error) = closed {
                return crate::platform::alert(
                    "Couldn't set the password",
                    &crate::plain(&*error, "section"),
                );
            }
            self.prefetch.forget(&library.key(&path));
        }
        library.lock(|locked, _| locked == path);
        self.restructure(
            library,
            Structure::Password {
                path,
                key,
                password,
            },
        );
    }
}

/// The key `password` opens the section at `path` with, if it does.
fn current(library: &Library, path: &str, password: &str) -> Option<onestore::protected::Key> {
    library.unlock(path, password).ok()?;
    library.unlocked(path)
}
