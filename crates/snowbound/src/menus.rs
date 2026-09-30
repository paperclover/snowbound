//! Context menus on pages, sections, section groups and notebooks, with OneNote 2010's
//! commands in its words, and moving pages by dragging their tabs.

use crate::{Command, Library, State, manage::Structure, platform};
use onestore::{ExGuid, PageEdit};
use std::sync::Arc;
use ui::{Anchor, Id, popup::Item};

/// What a drag holds.
pub enum Dragged {
    Page(ExGuid),
    /// A sidebar row, where it would land, and where it came from.
    Entry {
        entry: crate::sidebar::Entry,
        landing: crate::sidebar::Landing,
        home: crate::sidebar::Landing,
    },
    /// A section tab, by index.
    Tab(usize),
}

/// Something held down to drag: where the pointer took it, whether it has moved far enough
/// to lift, and for rows, the index in their list where the gap it would land in opens.
pub struct Drag {
    pub what: Dragged,
    /// The pointer when pressed, and its distance from the box's corner.
    from: [f32; 2],
    grab: [f32; 2],
    pub lifted: bool,
    /// Escape called it off; it ends when the button comes up.
    pub cancelled: bool,
    /// Let go: what it held eases into its place, then the drag ends.
    pub released: bool,
    pub slot: Option<usize>,
}

impl Drag {
    /// Whether it follows the pointer: lifted, held, and not called off.
    pub fn live(&self) -> bool {
        self.lifted && !self.cancelled && !self.released
    }

    /// Where the dragged box's corner follows the pointer at `pointer`.
    pub fn corner(&self, pointer: [f32; 2]) -> [f32; 2] {
        [pointer[0] - self.grab[0], pointer[1] - self.grab[1]]
    }
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
pub(crate) fn folder(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(String::new(), |(folder, _)| folder.to_owned())
}

impl State {
    /// Follows a drag of what `held` names, pressed in its box's rectangle, this frame:
    /// starts one, lifts it once the pointer has moved a few pixels as a click's doesn't,
    /// and says when it was let go while it followed the pointer. `mine` tells this list's
    /// drags from others'. A drag let go stays, as what it held eases into place, until
    /// `settle` ends it.
    pub(crate) fn follow_drag(
        &mut self,
        held: Option<(Dragged, [f32; 4])>,
        mine: impl Fn(&Dragged) -> bool,
    ) -> bool {
        let pointer = self.ui.pointer();
        match (held, &mut self.drag) {
            (Some(_), Some(drag)) if mine(&drag.what) && !drag.released => {
                if let Some([x, y]) = pointer {
                    drag.lifted |= (x - drag.from[0]).hypot(y - drag.from[1]) > 4.0;
                }
                false
            }
            (Some((what, rect)), None | Some(Drag { released: true, .. })) => {
                self.drag = pointer.map(|pointer| Drag {
                    what,
                    from: pointer,
                    grab: [pointer[0] - rect[0], pointer[1] - rect[1]],
                    lifted: false,
                    cancelled: false,
                    released: false,
                    slot: None,
                });
                false
            }
            (None, Some(drag)) if mine(&drag.what) && !drag.released => {
                let dropped = drag.live();
                drag.released = true;
                if !drag.lifted {
                    self.drag = None;
                }
                dropped
            }
            _ => false,
        }
    }

    /// Whether something dragged is lifted, still following the pointer or easing into place,
    /// so letting go of it isn't a click.
    pub(crate) fn dragged(&self) -> bool {
        self.drag.as_ref().is_some_and(|drag| drag.lifted)
    }

    /// Ends this list's drag, let go, once what it held stands in its place.
    pub(crate) fn settle(&mut self, mine: impl Fn(&Dragged) -> bool, placed: bool) {
        if placed
            && self
                .drag
                .as_ref()
                .is_some_and(|drag| drag.released && mine(&drag.what))
        {
            self.drag = None;
        }
    }

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
                let (versions, shown) = self.session.as_ref().map_or((false, false), |session| {
                    (
                        !session.page_versions(*space).is_empty(),
                        session.shown_history == Some(*space),
                    )
                });
                vec![
                    item("Delete"),
                    Item {
                        separated: true,
                        ..item("Copy Link to Page")
                    },
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
                    Item {
                        separated: true,
                        disabled: !versions,
                        ..item(if shown {
                            "Hide Page Versions"
                        } else {
                            "Show Page Versions"
                        })
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
            (Target::Page(space), "Copy Link to Page") => {
                let copied = self
                    .page_link(space, None)
                    .and_then(|link| self.clipboard.set_text(link));
                if let Err(error) = copied {
                    eprintln!("Copying the link failed: {error}");
                }
                None
            }
            (Target::Page(_), "New Page") => Some(Command::NewPage { under: None }),
            (Target::Page(space), "New Subpage") => Some(Command::NewPage { under: Some(space) }),
            (Target::Page(page), "Show Page Versions") => Some(Command::History { page, show: true }),
            (Target::Page(page), "Hide Page Versions") => Some(Command::History { page, show: false }),
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
                self.rename(crate::rename::Target::Entry {
                    library,
                    path,
                    in_tab: false,
                });
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
        let (
            Some(Drag {
                what: Dragged::Page(_),
                ..
            }),
            Some(session),
            Some([x, y]),
        ) = (
            self.drag.as_ref().filter(|drag| drag.live()),
            &self.session,
            self.ui.pointer(),
        )
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

    /// The page tab dragged, as the page list, built as the current box, shows it.
    pub(crate) fn dragged_page(&self) -> Option<crate::PageDrag> {
        let drag = self.drag.as_ref().filter(|drag| drag.lifted)?;
        let Dragged::Page(space) = drag.what else {
            return None;
        };
        let origin = self.ui.rect(self.ui.id(crate::page_list_top()))?[1];
        Some(crate::PageDrag {
            space,
            top: drag.live().then(|| drag.corner(self.pointer)[1] - origin),
            slot: drag.slot,
        })
    }

    /// Moves a page by dragging its tab, `held` in the page list whose tabs stand at
    /// `places`: let go, it moves where the gap the other tabs opened for it is, at its
    /// level, or under a page at most one level above it. Onto a section tab of those built
    /// as `row`, it moves to the end of that section, as OneNote moves a page dropped on a
    /// section's tab.
    pub(crate) fn drag_pages(
        &mut self,
        held: Option<ExGuid>,
        places: &[f32],
        settled: bool,
        row: Id,
    ) {
        let mine = |what: &Dragged| matches!(what, Dragged::Page(_));
        self.settle(mine, settled);
        let held =
            held.and_then(|space| Some((Dragged::Page(space), self.ui.rect(self.ui.id(space))?)));
        let target = self.page_drop(row);
        let origin = self.ui.rect(self.ui.id(crate::page_list_top()));
        let dropped = self.follow_drag(held, mine);
        let (
            Some(session),
            Some(
                drag @ Drag {
                    what: Dragged::Page(_),
                    ..
                },
            ),
        ) = (&self.session, &mut self.drag)
        else {
            return;
        };
        let Dragged::Page(space) = drag.what else {
            return;
        };
        let Some(from) = session
            .pages
            .iter()
            .position(|(listed, ..)| *listed == space)
        else {
            // The page left the section, dropped on another's tab.
            self.drag = None;
            return;
        };
        if drag.cancelled {
            drag.slot = None;
        } else if drag.live()
            && let Some(origin) = origin
        {
            let middle = drag.corner(self.pointer)[1] - origin[1] + crate::ROW / 2.0;
            let spans: Vec<[f32; 2]> = places.iter().map(|top| [*top, crate::ROW]).collect();
            drag.slot = Some(if target.is_some() {
                from
            } else {
                ui::drop_slot(&spans, from, middle)
            });
        }
        if !dropped {
            return;
        }
        let slot = drag.slot.take();
        if let Some(tab) = target {
            let path = session.tabs[tab].path.clone();
            self.commands.push(Command::MovePage { space, path });
            return;
        }
        let Some(slot) = slot.filter(|slot| *slot != from) else {
            return;
        };
        let others: Vec<_> = session
            .pages
            .iter()
            .filter(|(listed, ..)| *listed != space)
            .collect();
        let before = others.get(slot).map(|(listed, ..)| *listed);
        let level = slot
            .checked_sub(1)
            .map_or(1, |above| session.pages[from].2.min(others[above].2 + 1));
        if let Ok(edit) = PageEdit::move_to(space, before, level) {
            self.commands.push(Command::Pages(vec![edit]));
        }
    }

    /// The section tab dragged, as the tabs built as `row` show it: its index, and its
    /// leading edge following the pointer along the row.
    pub(crate) fn dragged_tab(&self, row: Id) -> Option<ui::shell::Dragged> {
        let drag = self.drag.as_ref().filter(|drag| drag.lifted)?;
        let Dragged::Tab(index) = drag.what else {
            return None;
        };
        let start = drag.corner(self.pointer)[0] - self.ui.rect(row)?[0];
        Some(ui::shell::Dragged {
            index,
            start: drag.live().then_some(start),
        })
    }

    /// Reorders section tabs by dragging one, as browser tabs reorder: let go, the tab
    /// `held` of those built as `row` lands at `slot`, and the folder takes that order. The
    /// tabs take it at once, so none slides back while the notebook changes.
    pub(crate) fn drag_tabs(
        &mut self,
        held: Option<usize>,
        slot: Option<usize>,
        settled: bool,
        row: Id,
    ) {
        let mine = |what: &Dragged| matches!(what, Dragged::Tab(_));
        self.settle(mine, settled);
        let held = held.and_then(|tab| {
            Some((
                Dragged::Tab(tab),
                self.ui.rect(ui::shell::tab_id(row, tab))?,
            ))
        });
        let dropped = self.follow_drag(held, mine);
        let (
            true,
            Some(Drag {
                what: Dragged::Tab(tab),
                ..
            }),
            Some(slot),
            Some(session),
        ) = (dropped, &mut self.drag, slot, &mut self.session)
        else {
            return;
        };
        // It eases into its new place from where it was let go.
        let tab = std::mem::replace(tab, slot);
        if slot == tab {
            return;
        }
        let open = session.tabs[session.tab].path.clone();
        let moved = session.tabs.remove(tab);
        session.tabs.insert(slot, moved);
        session.tab = session
            .tabs
            .iter()
            .position(|tab| tab.path == open)
            .unwrap_or_default();
        let folder = folder(&open);
        let paths = session.tabs.iter().map(|tab| tab.path.clone()).collect();
        self.commands.push(Command::Structure(
            Arc::clone(&session.library),
            Structure::Reorder { folder, paths },
        ));
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
