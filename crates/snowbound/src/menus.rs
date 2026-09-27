//! Context menus on pages, sections, section groups and notebooks, with OneNote 2010's
//! commands in its words, and moving pages by dragging their tabs.

use crate::{Command, Library, State, manage::Structure, platform};
use onestore::{ExGuid, PageEdit};
use std::sync::Arc;
use ui::{Anchor, Id, popup::Item};

/// What a drag holds.
pub enum Drag {
    Page(ExGuid),
    /// A sidebar row.
    Entry(crate::sidebar::Entry),
    /// A section tab, by index.
    Tab(usize),
}

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
                    // OneNote offers this beside New Page, which is a plain + here.
                    item("New Subpage"),
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
            (Target::Page(_), "New Page") => Some(Command::NewPage { under: None }),
            (Target::Page(space), "New Subpage") => Some(Command::NewPage { under: Some(space) }),
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

    /// The section tab, of those built as `row`, under the pointer while a page is dragged
    /// there, which a drop moves the page into.
    pub(crate) fn page_drop(&self, row: Id) -> Option<usize> {
        let (Some(Drag::Page(_)), Some(session), Some([x, y])) =
            (&self.drag, &self.session, self.ui.pointer())
        else {
            return None;
        };
        (0..session.tabs.len())
            .filter(|tab| *tab != session.tab)
            .find(|tab| {
                self.ui.rect(ui::shell::tab_id(row, *tab)).is_some_and(
                    |[left, top, right, bottom]| x >= left && x < right && y >= top && y < bottom,
                )
            })
    }

    /// Moves a page by dragging its tab: while `held`, a line shows where it would go; let
    /// go, it moves there at its level, or under a page at most one level above it. Onto a
    /// section tab of those built as `row`, it moves to the end of that section, as
    /// OneNote moves a page dropped on a section's tab. `panel` is the page list's rectangle.
    pub(crate) fn drag_pages(
        &mut self,
        held: Option<ExGuid>,
        section: &ui::Section,
        panel: [f32; 4],
        row: Id,
    ) {
        let Some(session) = &self.session else {
            return;
        };
        if held.is_some() && !self.filter.is_empty() {
            return;
        }
        let dropped = match (&self.drag, held) {
            (_, Some(space)) => Some(space),
            (Some(Drag::Page(space)), None) => Some(*space),
            _ => None,
        };
        let target = self.page_drop(row);
        if let Some(space) = held {
            self.drag = Some(Drag::Page(space));
        } else if matches!(self.drag, Some(Drag::Page(_))) {
            self.drag = None;
        }
        let (Some(space), Some([_, y])) = (dropped, self.ui.pointer()) else {
            return;
        };
        if let Some(tab) = target {
            if held.is_none() {
                let path = session.tabs[tab].path.clone();
                self.commands.push(Command::MovePage { space, path });
            }
            return;
        }
        // Rows other than the dragged one, with where each lies.
        let rows: Vec<(ExGuid, u32, [f32; 4])> = session
            .pages
            .iter()
            .filter(|(listed, ..)| *listed != space)
            .filter_map(|(listed, _, level)| {
                Some((*listed, *level, self.ui.rect(self.ui.id(listed))?))
            })
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
        } else if let Ok(edit) = PageEdit::move_to(space, before, level) {
            self.commands.push(Command::Pages(vec![edit]));
        }
    }

    /// Reorders section tabs by dragging one: while `held`, a line shows where it would go
    /// among the tabs built as `row` in the tab row `bar`; let go, the folder takes that
    /// order.
    pub(crate) fn drag_tabs(
        &mut self,
        held: Option<usize>,
        row: Id,
        bar: [f32; 4],
        accent: [f32; 4],
    ) {
        let Some(session) = &self.session else {
            return;
        };
        let dragged = match (&self.drag, held) {
            (_, Some(tab)) => Some(tab),
            (Some(Drag::Tab(tab)), None) => Some(*tab),
            _ => None,
        };
        if let Some(tab) = held {
            self.drag = Some(Drag::Tab(tab));
        } else if matches!(self.drag, Some(Drag::Tab(_))) {
            self.drag = None;
        }
        let (Some(tab), Some([x, _])) = (dragged, self.ui.pointer()) else {
            return;
        };
        let others: Vec<(usize, [f32; 4])> = (0..session.tabs.len())
            .filter(|other| *other != tab)
            .filter_map(|other| Some((other, self.ui.rect(ui::shell::tab_id(row, other))?)))
            .collect();
        let at = others
            .iter()
            .position(|(_, rect)| x < (rect[0] + rect[2]) / 2.0)
            .unwrap_or(others.len());
        let mut order: Vec<usize> = others.iter().map(|(other, _)| *other).collect();
        order.insert(at, tab);
        if order.iter().copied().eq(0..session.tabs.len()) {
            return;
        }
        if held.is_some() {
            let edge = match (
                at.checked_sub(1).map(|before| others[before].1),
                others.get(at),
            ) {
                (_, Some((_, after))) => after[0],
                (Some(before), None) => before[2],
                (None, None) => return,
            } - bar[0];
            self.ui
                .mark([edge - 1.5, 4.0, edge + 1.5, bar[3] - bar[1]], accent, 1.5);
        } else {
            let path = &session.tabs[tab].path;
            let folder = folder(path);
            let paths = order
                .into_iter()
                .map(|index| session.tabs[index].path.clone())
                .collect();
            self.commands.push(Command::Structure(
                Arc::clone(&session.library),
                Structure::Reorder { folder, paths },
            ));
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
