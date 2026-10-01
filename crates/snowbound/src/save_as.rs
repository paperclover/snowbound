//! File, Save As, as OneNote 2010's: the page, its section or its notebook, as a OneNote
//! section (`.one`), a package (`.onepkg`) or a PDF. A page or section saved as a section is
//! a copy outside any notebook; a notebook's package holds its sections as they stand here,
//! the open one's queued edits included (`notebook::package`).

use crate::{Library, State, UserEvent, platform, print};
use accesskit::Role;
use notebook::package;
use std::{error::Error, sync::Arc};
use ui::{Anchor, Axis, Id, Spec, children, fill, popup::Item, px};
use winit::keyboard::NamedKey;

/// What Save As saves.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Page,
    Section,
    Notebook,
}

const SCOPES: [(Scope, &str); 3] = [
    (Scope::Page, "Page"),
    (Scope::Section, "Section"),
    (Scope::Notebook, "Notebook"),
];

/// OneNote 2010's formats for each scope, as Save As lists them.
fn formats(scope: Scope) -> [(&'static str, &'static str); 2] {
    match scope {
        Scope::Notebook => [
            ("OneNote Package (*.onepkg)", "onepkg"),
            ("PDF (*.pdf)", "pdf"),
        ],
        _ => [
            ("OneNote 2010 Section (*.one)", "one"),
            ("PDF (*.pdf)", "pdf"),
        ],
    }
}

/// The dialog's choices while it is open: the section a scope of Section saves, by catalog
/// path, and the place in `formats` chosen.
pub struct Dialog {
    library: Arc<Library>,
    section: Option<String>,
    scope: Scope,
    format: usize,
}

fn id() -> Id {
    Id::ROOT.child("save-as")
}

impl State {
    /// Opens Save As on `scope`: the open page or section, or `section` of `library` when
    /// given, or the notebook.
    pub(crate) fn open_save_as(
        &mut self,
        library: Arc<Library>,
        section: Option<String>,
        scope: Scope,
    ) {
        let section = section.or_else(|| {
            let session = self.session.as_ref()?;
            Some(session.tabs[session.tab].path.clone())
        });
        self.save_as = Some(Dialog {
            library,
            section,
            scope,
            format: 0,
        });
        self.ui.open_popup(id());
    }

    /// Builds Save As while it is open; Save As… asks where, then writes on a thread.
    pub(crate) fn save_as_dialog(&mut self) {
        let Some(dialog) = &mut self.save_as else {
            return;
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.save_as = None;
            return;
        }
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        let entered = ui::popup::navigation(ui, &[], &[NamedKey::Enter]).contains(&NamedKey::Enter);
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(360.0), children()],
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
            node.set_label("Save As");
        }
        ui.leaf(
            "title",
            Spec {
                size: [fill(), px(row)],
                text: Some("Save As"),
                bold: true,
                role: Some(Role::Heading),
                ..Spec::default()
            },
        );
        // A page or section is only there to save with one open, or picked.
        let offered: Vec<(Scope, &str)> = (SCOPES.iter().copied())
            .filter(|(scope, _)| match scope {
                Scope::Page => self.session.is_some(),
                Scope::Section => dialog.section.is_some(),
                Scope::Notebook => dialog.library.catalog().is_some(),
            })
            .collect();
        let current = offered.iter().position(|(scope, _)| *scope == dialog.scope);
        let names: Vec<&str> = offered.iter().map(|(_, name)| *name).collect();
        if let Some(at) = choose(ui, "Save current", &names, current.unwrap_or(0)) {
            dialog.scope = offered[at].0;
            dialog.format = 0;
        }
        let names = formats(dialog.scope).map(|(name, _)| name);
        if let Some(at) = choose(ui, "Format", &names, dialog.format) {
            dialog.format = at;
        }
        crate::buttons(ui);
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let go = ui::button(ui, "go", "Save As…").clicked || entered;
        ui.close();
        ui.close();
        if !go && !cancel {
            return;
        }
        let dialog = self.save_as.take().expect("The dialog is open");
        self.ui.close_popup(id());
        if go && let Err(error) = self.save(dialog) {
            platform::alert("Couldn't save", &error.to_string());
        }
    }

    /// Asks where to save what `dialog` chose, then writes it on a thread of its own; a PDF
    /// goes through Export as PDF's settings.
    fn save(&mut self, dialog: Dialog) -> Result<(), Box<dyn Error>> {
        let (_, extension) = formats(dialog.scope)[dialog.format];
        if extension == "pdf" {
            self.export_pdf(match dialog.scope {
                Scope::Page => print::Scope::Page,
                Scope::Section => print::Scope::Section,
                Scope::Notebook => print::Scope::Notebook,
            });
            return Ok(());
        }
        self.persist()?;
        let library = dialog.library;
        let open = (self.session.as_ref())
            .filter(|session| Arc::ptr_eq(&session.library, &library))
            .map(|session| {
                let path = session.tabs[session.tab].path.clone();
                (path, Arc::clone(session.section.replica()), session.space)
            });
        let section = dialog.section.unwrap_or_default();
        let title = match dialog.scope {
            Scope::Page => self.view.editor.page()?.title,
            Scope::Section => crate::library::section_name(&section, &None),
            Scope::Notebook => library.name.clone(),
        };
        let name = format!("{}.{extension}", print::file_name(&title));
        let Some(mut path) = platform::pick_new("Save As", &name, "Save", None) else {
            return Ok(());
        };
        if path.extension().is_none() {
            path.set_extension(extension);
        }
        let (scope, author, proxy) = (dialog.scope, self.author.clone(), self.proxy.clone());
        let color = (library
            .tabs(crate::menus::folder(&section).as_str())
            .into_iter())
        .find(|tab| tab.path == section)
        .and_then(|tab| tab.color);
        std::thread::spawn(move || {
            let written = (|| -> Result<(), Box<dyn Error>> {
                let image = |path: &str| {
                    let (_, replica, _) = open.as_ref().filter(|(open, ..)| open == path)?;
                    replica.snapshot().ok()
                };
                let bytes = match scope {
                    Scope::Page => {
                        let (_, replica, space) = open.as_ref().ok_or("No page is open")?;
                        let page = replica.page(*space)?;
                        let file = file_name(&path);
                        package::page_section(&page, &file, color, &author)?
                    }
                    Scope::Section => {
                        let stored = match image(&section) {
                            Some(image) => image,
                            None => library.reopen()?.read_section(&section)?,
                        };
                        package::section_copy(stored)?
                    }
                    Scope::Notebook => {
                        let notebook = library.reopen()?;
                        package::pack(package::notebook_files(&notebook, image)?)?
                    }
                };
                std::fs::write(&path, bytes)?;
                Ok(())
            })();
            let written = written.map_err(|error| error.to_string());
            let _ = proxy.send_event(UserEvent::Then(Box::new(move |_| {
                written.map_err(|error| {
                    platform::alert("Couldn't save", &error);
                    error.into()
                })
            })));
        });
        Ok(())
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A labelled combo listing `names`, choosing among them.
fn choose(ui: &mut ui::Ui, label: &str, names: &[&str], current: usize) -> Option<usize> {
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
            size: [px(110.0), px(row)],
            text: Some(label),
            ..Spec::default()
        },
    );
    let menu = id().child(label);
    let combo = ui.id(label);
    let shown = names.get(current).copied().unwrap_or_default();
    ui::shell::combo(ui, label, label, shown, 210.0, menu, true);
    let items: Vec<Item> = (names.iter().enumerate())
        .map(|(at, text)| Item {
            text,
            checked: Some(at == current),
            current: at == current,
            ..Item::default()
        })
        .collect();
    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
    let chosen = ui::popup::menu(ui, menu, anchor, &items, None);
    ui.close();
    chosen
}
