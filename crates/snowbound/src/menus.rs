//! Context menus on pages, sections, section groups and notebooks, with OneNote 2010's
//! commands in its words, and moving pages by dragging their tabs.

use crate::{Command, Library, State, manage::Structure, platform};
use onestore::{ExGuid, PageEdit};
use std::sync::Arc;
use ui::{Anchor, Id, popup::Item};

/// What a context menu was opened on.
pub enum Target {
    Page(ExGuid),
    Section { library: Arc<Library>, path: String },
    Group { library: Arc<Library>, path: String },
    Notebook(Arc<Library>),
}

pub fn id() -> Id {
    Id::ROOT.child("context-menu")
}

/// The catalog folder holding `path`.
fn folder(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(String::new(), |(folder, _)| folder.to_owned())
}

impl State {
    /// The open context menu, and what its choice does.
    pub(crate) fn context_menu(&mut self) {
        let Some((target, point)) = &self.menu else {
            return;
        };
        if !self.ui.popup_open(id()) {
            self.menu = None;
            return;
        }
        let item = |text| Item {
            text,
            ..Default::default()
        };
        let items: Vec<Item> = match target {
            Target::Page(space) => {
                let pages = self
                    .session
                    .as_ref()
                    .map_or(&[][..], |session| &session.pages[..]);
                let at = pages.iter().position(|(listed, ..)| listed == space);
                let level = at.map_or(1, |at| pages[at].2);
                let above = at
                    .and_then(|at| at.checked_sub(1))
                    .map_or(0, |above| pages[above].2);
                vec![
                    item("Delete"),
                    Item {
                        separated: true,
                        ..item("New Page")
                    },
                    Item {
                        separated: true,
                        disabled: level > above || level >= 3,
                        ..item("Make Subpage")
                    },
                    Item {
                        disabled: level <= 1,
                        ..item("Promote Subpage")
                    },
                ]
            }
            Target::Section { .. } | Target::Group { .. } => vec![
                item("Rename"),
                item("Delete"),
                Item {
                    separated: true,
                    ..item("New Section")
                },
                item("New Section Group"),
            ],
            Target::Notebook(_) => vec![
                item("New Section"),
                item("New Section Group"),
                Item {
                    separated: true,
                    ..item("Close This Notebook")
                },
            ],
        };
        let Some(chosen) = ui::popup::menu(&mut self.ui, id(), Anchor::Point(*point), &items, None)
        else {
            return;
        };
        let Some((target, _)) = self.menu.take() else {
            return;
        };
        let command = match (target, items[chosen].text) {
            (Target::Page(space), "Delete") => Some(Command::DeletePages(vec![space])),
            (Target::Page(_), "New Page") => Some(Command::NewPage { subpage: false }),
            (Target::Page(space), text) => self.session.as_ref().and_then(|session| {
                let level = session
                    .pages
                    .iter()
                    .find(|(listed, ..)| *listed == space)?
                    .2;
                let level = if text == "Make Subpage" {
                    level + 1
                } else {
                    level - 1
                };
                Some(Command::Pages(vec![PageEdit::set_level(space, level).ok()?]))
            }),
            (
                Target::Section { library, path } | Target::Group { library, path },
                "Rename",
            ) => {
                let name = path.rsplit('/').next().unwrap_or_default();
                self.renaming = Some(crate::sidebar::Renaming {
                    library,
                    name: name.strip_suffix(".one").unwrap_or(name).to_owned(),
                    path,
                });
                self.sidebar = true;
                None
            }
            (Target::Section { library, path }, "Delete") => platform::confirm(
                "Are you sure you want to move this section to this notebook's Recycle Bin?",
                &path,
                "Cancel",
                "Delete",
            )
            .then(|| Command::Structure(library, Structure::Delete { path })),
            (Target::Group { library, path }, "Delete") => platform::confirm(
                "Are you sure you want to move the sections in this section group to this notebook's Recycle Bin?",
                path.rsplit('/').next().unwrap_or_default(),
                "Cancel",
                "Delete",
            )
            .then(|| Command::Structure(library, Structure::Delete { path })),
            (Target::Section { library, path }, text) => {
                let folder = folder(&path);
                Some(Command::Structure(library, new(text, folder)))
            }
            (Target::Group { library, path }, text) => {
                Some(Command::Structure(library, new(text, path)))
            }
            (Target::Notebook(library), "Close This Notebook") => {
                Some(Command::CloseNotebook(library))
            }
            (Target::Notebook(library), text) => {
                Some(Command::Structure(library, new(text, String::new())))
            }
        };
        self.commands.extend(command);
    }

    /// Moves a page by dragging its tab: while `held`, a line shows where it would go; let
    /// go, it moves there at its level, or under a page at most one level above it.
    /// `panel` is the page list's rectangle.
    pub(crate) fn drag_pages(
        &mut self,
        held: Option<ExGuid>,
        section: &ui::Section,
        panel: [f32; 4],
    ) {
        let Some(session) = &self.session else {
            return;
        };
        if held.is_some() && !self.filter.is_empty() {
            return;
        }
        let dragged = held.or(self.dragging.take());
        self.dragging = held;
        let (Some(space), Some([_, y])) = (dragged, self.ui.pointer()) else {
            return;
        };
        // Rows other than the dragged one, with where each lies.
        let rows: Vec<(ExGuid, u32, [f32; 4])> = session
            .pages
            .iter()
            .filter(|(listed, ..)| *listed != space)
            .filter_map(|(listed, _, level)| Some((*listed, *level, self.ui.rect(self.ui.id(listed))?)))
            .collect();
        let Some(level) = session
            .pages
            .iter()
            .find(|(listed, ..)| *listed == space)
            .map(|(.., level)| *level)
        else {
            return;
        };
        let at = rows
            .iter()
            .position(|(.., rect)| y < (rect[1] + rect[3]) / 2.0)
            .unwrap_or(rows.len());
        let before = rows.get(at).map(|(listed, ..)| *listed);
        let level = at
            .checked_sub(1)
            .map_or(1, |above| level.min(rows[above].1 + 1));
        let stays = session
            .pages
            .iter()
            .skip_while(|(listed, ..)| *listed != space)
            .nth(1)
            .map(|(listed, ..)| *listed)
            == before;
        if stays {
            return;
        }
        if held.is_some() {
            // The line lies between the rows the page would go between.
            let line = match (at.checked_sub(1).map(|above| rows[above].2), rows.get(at)) {
                (_, Some((.., below))) => below[1],
                (Some(above), None) => above[3],
                (None, None) => return,
            } - panel[1];
            let indent = 10.0 + 16.0 * (level - 1) as f32;
            self.ui.mark(
                [indent, line - 1.5, crate::PAGE_LIST - 6.0, line + 1.5],
                section.accent,
                1.5,
            );
            self.dragging = held;
        } else if let Ok(edit) = PageEdit::move_to(space, before, level) {
            self.commands.push(Command::Pages(vec![edit]));
        }
    }
}

/// The new section or group a menu's `text` asks for in `folder`.
fn new(text: &str, folder: String) -> Structure {
    if text == "New Section Group" {
        Structure::NewGroup { folder }
    } else {
        Structure::NewSection { folder }
    }
}
