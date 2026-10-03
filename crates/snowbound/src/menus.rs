//! Context menus on pages, sections, section groups and notebooks, with OneNote 2010's
//! commands in its words, and moving pages by dragging their tabs.

use crate::{
    Command, Library, State,
    commands::{self, Choice},
    manage::Structure,
    platform,
    recycle::Request,
};
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
    /// A notebook's sidebar row, by its index among the notebooks.
    Notebook(crate::sidebar::Entry),
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

/// What a context menu, or the palette's actions, are on.
#[derive(Clone)]
pub enum Target {
    /// A page of a section, open or not.
    Page {
        library: Arc<Library>,
        path: String,
        space: ExGuid,
    },
    Section {
        library: Arc<Library>,
        path: String,
    },
    Group {
        library: Arc<Library>,
        path: String,
    },
    Notebook(Arc<Library>),
    /// A notebook shown lately but closed since, by location.
    Closed(String),
    /// A server saved to reconnect to, by address.
    Server(String),
    /// A page to add to the open section, by its title.
    NewPage(String),
    /// The page's context menu at the caret or the file selected, by an item's text.
    Text(&'static str),
    Command(commands::Id),
}

impl Target {
    /// What it is, as a palette command acting on it names it.
    fn noun(&self) -> &'static str {
        match self {
            Target::Page { .. } => "Page",
            Target::Section { .. } => "Section",
            Target::Group { .. } => "Section Group",
            Target::Notebook(_) => "Notebook",
            Target::Closed(_)
            | Target::Server(_)
            | Target::Command(_)
            | Target::NewPage(_)
            | Target::Text(_) => "",
        }
    }
}

/// What can be done to a target, as its context menu and the palette's actions list it.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open,
    Run,
    Rename,
    Delete,
    /// Opens the submenu of the places `MoveTo` takes it.
    Move,
    /// Moves it to a section, or into a folder, by catalog path.
    MoveTo(String),
    /// Opens the submenu of the sections `CopyTo` copies a page of the recycle bin to.
    Copy,
    CopyTo(String),
    /// Save As on a section or notebook.
    SaveAs,
    /// Shows the notebook's recycle bin.
    RecycleBin,
    EmptyRecycleBin,
    MarkNotebookRead,
    CopyLink,
    NewPage,
    NewSubpage,
    MakeSubpage,
    PromoteSubpage,
    Versions(bool),
    NewSection,
    NewGroup,
    Reveal,
    Close,
    /// Opens the Themes dialog for it.
    Theme,
    /// Opens the notebook's sync status.
    SyncStatus,
    /// Opens the submenu of the colours `Color` gives a section.
    Colors,
    /// A section's colour, COLORREF; none is OneNote's None.
    Color(Option<u32>),
    Sync,
    /// Moves a notebook up the notebook list, or down.
    Raise(bool),
    Properties,
    /// Opens Password Protection on a section.
    Password,
    /// Opens Live Share on a notebook.
    #[cfg(feature = "live")]
    LiveShare,
}

impl Action {
    /// The artwork its menu item shows; none for a place or colour, which shows its own.
    pub(crate) fn art(&self) -> Option<&'static [&'static str]> {
        use crate::art;
        Some(match self {
            Action::Open => art::OPEN,
            Action::Rename => art::RENAME,
            Action::Delete => art::DELETE,
            Action::Move => art::MOVE,
            Action::Copy => art::COPY,
            Action::SaveAs => art::SAVE,
            Action::RecycleBin => art::RECYCLE_BIN,
            Action::EmptyRecycleBin => art::EMPTY_RECYCLE_BIN,
            Action::MarkNotebookRead => art::MARK_NOTEBOOK_READ,
            Action::CopyLink => art::COPY_LINK,
            Action::NewPage => art::NEW_PAGE,
            Action::NewSubpage => art::NEW_SUBPAGE,
            Action::MakeSubpage => art::SUBPAGE,
            Action::PromoteSubpage => art::OUTDENT,
            Action::Versions(_) => art::PAGE_VERSIONS,
            Action::NewSection => art::NEW_SECTION,
            Action::NewGroup => art::NEW_SECTION_GROUP,
            Action::Reveal => art::FOLDER,
            Action::Close => art::CLOSE_NOTEBOOK,
            Action::Theme => art::STYLES,
            Action::Colors => art::SECTION_COLOR,
            Action::Sync => art::SYNC_NOW,
            Action::Raise(true) => art::MOVE_UP,
            Action::Raise(false) => art::MOVE_DOWN,
            Action::Properties => art::PROPERTIES,
            Action::Password => art::PASSWORD,
            #[cfg(feature = "live")]
            Action::LiveShare => art::LIVE_SHARE,
            Action::Run
            | Action::MoveTo(_)
            | Action::CopyTo(_)
            | Action::SyncStatus
            | Action::Color(_) => return None,
        })
    }
}

/// OneNote 2010's section and notebook colours, COLORREF, with their names in its Section
/// Color menu's order (`corpus/section-color/native`).
pub(crate) const SECTION_COLORS: [(u32, &str); 16] = [
    (0xe4a88a, "Blue"),
    (0x69d8ff, "Yellow"),
    (0x97c9b7, "Green"),
    (0x9795ee, "Red"),
    (0xde9eb4, "Purple"),
    (0xaeba91, "Cyan"),
    (0x78b0f6, "Orange"),
    (0xbba4d5, "Magenta"),
    (0xd2bb9b, "Blue Mist"),
    (0xb79cab, "Purple Mist"),
    (0x99d1e8, "Tan"),
    (0x6ff9f5, "Lemon"),
    (0x92e7ad, "Apple"),
    (0xcabc4d, "Teal"),
    (0x7575ba, "Red Chalk"),
    (0xaa9595, "Silver"),
];

/// Whether a command does `action` to `target` when it is the one shown, so the palette's
/// commands need no row of their own for it.
pub(crate) fn covered(target: &Target, action: &Action) -> bool {
    use Action::*;
    match target {
        Target::Page { .. } => matches!(action, CopyLink | NewPage | NewSubpage | Theme),
        Target::Section { .. } => matches!(
            action,
            SaveAs | NewSection | NewGroup | Password | Theme | EmptyRecycleBin
        ),
        Target::Group { .. } => matches!(action, NewSection | NewGroup),
        Target::Notebook(_) => {
            #[cfg(feature = "live")]
            if *action == LiveShare {
                return true;
            }
            matches!(
                action,
                SaveAs
                    | Close
                    | NewSection
                    | NewGroup
                    | MarkNotebookRead
                    | Reveal
                    | RecycleBin
                    | Theme
            )
        }
        Target::Closed(_)
        | Target::Server(_)
        | Target::Command(_)
        | Target::NewPage(_)
        | Target::Text(_) => false,
    }
}

/// How the palette's commands name `action` on `target`, shown in its context menu as
/// `label`: with the target's noun where the label lacks it, as Rename becomes Rename Page.
pub(crate) fn command_title(target: &Target, action: &Action, label: &str) -> String {
    let noun = target.noun();
    if label.to_lowercase().contains(&noun.to_lowercase()) {
        return label.to_owned();
    }
    match label.split_once(' ') {
        _ if *action == Action::Properties => format!("{noun} {label}"),
        Some((verb, rest)) => format!("{verb} {noun} {rest}"),
        None => format!("{label} {noun}"),
    }
}

/// A place Move offers.
struct Destination {
    name: String,
    path: String,
    icon: &'static [&'static str],
    tint: Option<[f32; 4]>,
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
        let (target, point) = (target.clone(), *point);
        let mut actions: Vec<(Action, Item)> = self.actions(&target);
        // A notebook's menu heads with its sync status in a line, which opens the whole.
        let status = match &target {
            Target::Notebook(library) => self.session.as_ref().map(|session| {
                let (text, icon) = crate::sync::line(library, session);
                (Arc::clone(library), text, icon)
            }),
            _ => None,
        };
        if let Some((_, text, icon)) = &status {
            actions[0].1.separated = true;
            let item = Item {
                text,
                icon: Some(*icon),
                ..Item::default()
            };
            actions.insert(0, (Action::SyncStatus, item));
        }
        let action = self.action_menu(id(), Anchor::Point(point), None, &target, &actions);
        match (action, status) {
            (Some(Action::SyncStatus), Some((library, ..))) => {
                self.menu = None;
                self.ui.close_popup(id());
                self.show_sync(library, point);
            }
            (Some(action), _) => {
                self.menu = None;
                match target {
                    // From its tab, as a double click there renames it.
                    Target::Section { library, path }
                        if action == Action::Rename && self.on_tab(&library, &path, point) =>
                    {
                        self.rename(crate::rename::Target::Entry {
                            library,
                            path,
                            in_tab: true,
                        });
                    }
                    target => self.act_on(target, action),
                }
            }
            (None, _) => {}
        }
    }

    /// Whether `point`, where a context menu opened, is on the tab of section `path`.
    fn on_tab(&self, library: &Arc<Library>, path: &str, point: [f32; 2]) -> bool {
        let shown = self.session.as_ref().is_some_and(|session| {
            Arc::ptr_eq(&session.library, library)
                && session.tabs.iter().any(|tab| tab.path == path)
        });
        let [x, y] = point;
        shown
            && self
                .ui
                .rect(crate::sections())
                .is_some_and(|[left, top, right, bottom]| {
                    x >= left && x < right && y >= top && y < bottom
                })
    }

    /// What can be done to `target`, in its context menu's order, as each shows there.
    pub(crate) fn actions(&self, target: &Target) -> Vec<(Action, Item<'static>)> {
        // An action, its label, whether it is disabled, and whether a rule starts its group.
        let item = |action: Action, text, disabled, separated| {
            let submenu = matches!(action, Action::Move | Action::Copy | Action::Colors);
            #[cfg(feature = "live")]
            let badge = (action == Action::LiveShare).then_some(crate::share::BETA);
            #[cfg(not(feature = "live"))]
            let badge = None;
            let item = Item {
                text,
                icon: action.art(),
                disabled,
                separated,
                submenu,
                badge,
                ..Item::default()
            };
            (action, item)
        };
        let nowhere = || self.destinations(target).is_empty();
        // Last on a page, section or notebook, under a rule.
        let styles = |library: &Library| {
            let title = commands::command(commands::Id::Themes).title;
            item(Action::Theme, title, library.catalog().is_none(), true)
        };
        match target {
            // OneNote's Move or Copy takes a page out of the bin; nothing else changes it.
            Target::Page { path, .. } if crate::recycle::binned(path) => vec![
                item(Action::Delete, "Delete", false, false),
                item(Action::Move, "Restore To", nowhere(), true),
                item(Action::Copy, "Copy To", nowhere(), false),
            ],
            Target::Section { path, .. } if crate::recycle::binned(path) => vec![
                item(Action::SaveAs, "Save As", false, false),
                item(Action::Move, "Restore To", nowhere(), false),
                item(Action::EmptyRecycleBin, "Empty Recycle Bin", false, true),
                item(Action::Colors, "Section Color", false, true),
            ],
            Target::Page {
                library,
                path,
                space,
            } => {
                let mut actions = vec![
                    item(Action::Rename, "Rename", false, false),
                    item(Action::Delete, "Delete", false, false),
                    item(Action::Move, "Move to Section", nowhere(), false),
                    item(Action::CopyLink, "Copy Link to Page", false, true),
                    item(Action::NewPage, "New Page", false, true),
                    // OneNote offers this beside New Page, which is a plain + here.
                    item(Action::NewSubpage, "New Subpage", false, false),
                ];
                // Levels and versions are known once the section is open.
                let Some(session) = self.session.as_ref().filter(|_| self.open(library, path))
                else {
                    actions.push(styles(library));
                    return actions;
                };
                let pages = &session.pages;
                let at = pages.iter().position(|(listed, ..)| listed == space);
                let level = at.map_or(1, |at| pages[at].2);
                let above = at
                    .and_then(|at| at.checked_sub(1))
                    .map_or(0, |above| pages[above].2);
                let shown = session.shown_history == Some(*space);
                let versions = if shown {
                    "Hide Page Versions"
                } else {
                    "Show Page Versions"
                };
                actions.extend([
                    item(
                        Action::MakeSubpage,
                        "Make Subpage",
                        level > above || level >= 3,
                        true,
                    ),
                    item(Action::PromoteSubpage, "Promote Subpage", level <= 1, false),
                    item(
                        Action::Versions(!shown),
                        versions,
                        session.page_versions(*space).is_empty(),
                        true,
                    ),
                    styles(library),
                ]);
                actions
            }
            Target::Section { library, path } | Target::Group { library, path } => {
                let section = matches!(target, Target::Section { .. });
                let cataloged = library.catalog().is_some();
                let new = [
                    item(Action::NewSection, "New Section", !cataloged, false),
                    item(Action::NewGroup, "New Section Group", !cataloged, false),
                ];
                // A group's menu leads with what it makes, as Windows' menus do.
                let mut actions = if section { Vec::new() } else { new.to_vec() };
                actions.push(item(Action::Rename, "Rename", false, !section));
                if section {
                    actions.push(item(Action::SaveAs, "Save As", false, false));
                }
                actions.extend([
                    item(Action::Delete, "Delete", false, false),
                    item(Action::Move, "Move", nowhere(), false),
                ]);
                if section {
                    let link = self.section_link(library, path).is_none();
                    actions.push(item(Action::CopyLink, "Copy Link to Section", link, true));
                }
                if section {
                    let [mut first, rest] = new;
                    first.1.separated = true;
                    actions.extend([first, rest]);
                }
                if section {
                    actions.extend([
                        item(
                            Action::Password,
                            "Password Protect This Section",
                            !cataloged,
                            true,
                        ),
                        item(Action::Colors, "Section Color", false, false),
                        item(
                            Action::Reveal,
                            platform::SHOW_FILE,
                            section_file(library, path).is_none(),
                            false,
                        ),
                        item(
                            Action::Theme,
                            commands::command(commands::Id::Themes).title,
                            library.section_identity(path).is_none(),
                            true,
                        ),
                    ]);
                }
                actions
            }
            Target::Notebook(library) => {
                let listed = (self.notebooks.iter()).position(|open| Arc::ptr_eq(open, library));
                let last = self.notebooks.len().saturating_sub(1);
                let unread = self.unread_notebook(library);
                // It leads with what it makes, as Windows' menus do.
                let mut actions = vec![
                    item(
                        Action::NewSection,
                        "New Section",
                        library.catalog().is_none(),
                        false,
                    ),
                    item(
                        Action::NewGroup,
                        "New Section Group",
                        library.catalog().is_none(),
                        false,
                    ),
                    item(Action::Rename, "Rename", false, true),
                    item(
                        Action::SaveAs,
                        "Save As",
                        library.catalog().is_none(),
                        false,
                    ),
                    item(
                        Action::Sync,
                        "Sync This Notebook Now",
                        library.background.is_none(),
                        false,
                    ),
                    // The app's iCloud Drive folder lists every notebook in it.
                    item(
                        Action::Close,
                        "Close This Notebook",
                        crate::manage::in_icloud_folder(&library.location),
                        false,
                    ),
                ];
                // Its folder to the Trash, as the Finder deletes it; a server's stays there.
                if cfg!(not(target_arch = "wasm32"))
                    && library.folder().is_some()
                    && !library.on_smb()
                    && !matches!(library.notebook, Ok(None))
                {
                    actions.push(item(Action::Delete, "Delete Notebook", false, false));
                }
                actions.extend([
                    item(Action::CopyLink, "Copy Link to Notebook", false, true),
                    #[cfg(feature = "live")]
                    item(
                        Action::LiveShare,
                        commands::command(commands::Id::LiveShare).title,
                        library.catalog().is_none() || library.joined.is_some(),
                        false,
                    ),
                    item(
                        Action::MarkNotebookRead,
                        "Mark Notebook as Read",
                        !unread,
                        true,
                    ),
                    item(
                        Action::Raise(true),
                        "Move Up",
                        listed.is_none_or(|at| at == 0),
                        true,
                    ),
                    item(
                        Action::Raise(false),
                        "Move Down",
                        listed.is_none_or(|at| at == last),
                        false,
                    ),
                    item(
                        Action::Reveal,
                        commands::command(commands::Id::ShowNotebook).title,
                        library.folder().is_none(),
                        true,
                    ),
                    item(
                        Action::RecycleBin,
                        "Notebook Recycle Bin",
                        !self.recycle_bin_available(library),
                        false,
                    ),
                    item(
                        Action::Properties,
                        "Properties",
                        library.catalog().is_none(),
                        false,
                    ),
                    styles(library),
                ]);
                actions
            }
            Target::Closed(_) | Target::Server(_) => {
                vec![item(Action::Delete, "Remove from Recent", false, false)]
            }
            Target::Command(_) | Target::NewPage(_) | Target::Text(_) => Vec::new(),
        }
    }

    /// Builds menu `id` of `actions` on `target` at `anchor`, under a filter field showing
    /// `filter` when given, with Move's places in its submenu: the action chosen.
    pub(crate) fn action_menu(
        &mut self,
        id: Id,
        anchor: Anchor,
        filter: Option<&str>,
        target: &Target,
        actions: &[(Action, Item)],
    ) -> Option<Action> {
        let items: Vec<Item> = actions.iter().map(|(_, item)| *item).collect();
        let chosen = ui::popup::menu(&mut self.ui, id, anchor, &items, filter);
        let submenu = |action: &Action| match action {
            Action::Move => Some(id.child("move")),
            Action::Copy => Some(id.child("copy")),
            Action::Colors => Some(id.child("colors")),
            _ => None,
        };
        ui::popup::submenus(&mut self.ui, id, &items, |index| submenu(&actions[index].0));
        let mut chosen = chosen.map(|index| actions[index].0.clone());
        for action in [Action::Move, Action::Copy, Action::Colors] {
            if let Some(menu) = submenu(&action) {
                chosen = chosen.or_else(|| self.submenu(menu, &action, target, anchor));
            }
        }
        chosen
    }

    /// Builds submenu `id` of `action`, Move, Copy or Colors, on `target` while it is open:
    /// the place or colour chosen, as the action it takes.
    pub(crate) fn submenu(
        &mut self,
        id: Id,
        action: &Action,
        target: &Target,
        anchor: Anchor,
    ) -> Option<Action> {
        if !self.ui.popup_open(id) {
            return None;
        }
        match (action, target) {
            (Action::Move | Action::Copy, _) => {
                let destinations = self.destinations(target);
                let items: Vec<Item> = (destinations.iter())
                    .map(|place| Item {
                        text: &place.name,
                        icon: Some(place.icon),
                        tint: place.tint,
                        ..Item::default()
                    })
                    .collect();
                let index = ui::popup::menu(&mut self.ui, id, anchor, &items, None)?;
                let path = destinations[index].path.clone();
                Some(match action {
                    Action::Move => Action::MoveTo(path),
                    _ => Action::CopyTo(path),
                })
            }
            (Action::Colors, Target::Section { library, path }) => {
                let current = (library.tabs(&folder(path)).into_iter())
                    .find(|tab| tab.path == *path)
                    .and_then(|tab| tab.color);
                let theme = &self.ui.theme;
                let mut items: Vec<Item> = (SECTION_COLORS.iter())
                    .map(|&(color, text)| Item {
                        text,
                        icon: Some(crate::art::SECTION),
                        tint: Some(theme.section(crate::section_color(Some(color))).accent),
                        checked: Some(current == Some(color)),
                        ..Item::default()
                    })
                    .collect();
                items.push(Item {
                    text: "None",
                    checked: Some(current.is_none()),
                    separated: true,
                    ..Item::default()
                });
                ui::popup::menu(&mut self.ui, id, anchor, &items, None)
                    .map(|index| Action::Color(SECTION_COLORS.get(index).map(|(color, _)| *color)))
            }
            _ => None,
        }
    }

    /// Where Move takes `target`: the other sections of a page's folder, or the folders a
    /// section or group may move into.
    fn destinations(&self, target: &Target) -> Vec<Destination> {
        let theme = &self.ui.theme;
        match target {
            Target::Page { library, path, .. } if crate::recycle::binned(path) => {
                (folders(library).into_iter())
                    .flat_map(|place| library.tabs(&place.path))
                    .map(|tab| Destination {
                        tint: Some(theme.section(crate::section_color(tab.color)).accent),
                        name: tab.name,
                        path: tab.path,
                        icon: crate::art::SECTION,
                    })
                    .collect()
            }
            Target::Page { library, path, .. } => (library.tabs(&folder(path)).into_iter())
                .filter(|tab| tab.path != *path)
                .map(|tab| Destination {
                    tint: Some(theme.section(crate::section_color(tab.color)).accent),
                    name: tab.name,
                    path: tab.path,
                    icon: crate::art::SECTION,
                })
                .collect(),
            Target::Section { library, path } | Target::Group { library, path } => {
                let within = format!("{path}/");
                let home = folder(path);
                (folders(library).into_iter())
                    .filter(|place| {
                        place.path != home
                            && place.path != *path
                            && !place.path.starts_with(&within)
                    })
                    .map(|place| Destination {
                        name: match place.path.rsplit_once('/') {
                            _ if place.path.is_empty() => library.name.clone(),
                            Some((_, name)) => name.to_owned(),
                            None => place.path.clone(),
                        },
                        path: place.path.clone(),
                        icon: if place.path.is_empty() {
                            crate::art::NOTEBOOK
                        } else {
                            crate::art::SECTION_GROUP
                        },
                        tint: place
                            .path
                            .is_empty()
                            .then(|| crate::notebook_color(theme, library.color())),
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Does `action` to `target`. An action on a page of a section not open opens it there
    /// first, then acts.
    pub(crate) fn act_on(&mut self, target: Target, action: Action) {
        let command = match (target, action) {
            (Target::Command(id), _) => {
                self.choose(Choice::Command(id));
                None
            }
            (Target::NewPage(title), _) => Some(Command::NewPage { under: None, title }),
            (Target::Text(text), _) => Some(Command::Text(text)),
            (
                Target::Page {
                    library,
                    path,
                    space,
                },
                action,
            ) if action == Action::Open || !self.open(&library, &path) => {
                self.go(library, path, space);
                self.after_open = (action != Action::Open).then_some((space, action));
                None
            }
            (Target::Page { space, .. }, action) => self.page_command(space, action),
            (Target::Section { library, path }, Action::Open) => {
                Some(Command::OpenSection(library, path))
            }
            (Target::Section { library, path }, Action::SaveAs) => {
                self.open_save_as(library, Some(path), crate::save_as::Scope::Section);
                None
            }
            (Target::Notebook(library), Action::SaveAs) => {
                self.open_save_as(library, None, crate::save_as::Scope::Notebook);
                None
            }
            (
                Target::Section { library, .. } | Target::Notebook(library),
                Action::EmptyRecycleBin,
            ) => {
                self.empty_recycle_bin(library);
                None
            }
            (Target::Notebook(library), Action::RecycleBin) => {
                self.toggle_recycle_bin(library);
                None
            }
            (Target::Notebook(library), Action::MarkNotebookRead) => {
                self.mark_notebook_read(&library);
                None
            }
            (Target::Section { library, path }, Action::Password) => {
                self.password_protection(library, path);
                None
            }
            (Target::Notebook(library), Action::Open) => {
                self.open_notebook(library.location.clone(), None);
                None
            }
            (
                Target::Section { library, path } | Target::Group { library, path },
                Action::Rename,
            ) => {
                self.rename(crate::rename::Target::Entry {
                    library,
                    path,
                    in_tab: false,
                });
                None
            }
            (Target::Section { library, path }, Action::Delete) => {
                let name = crate::library::entry_name(&path).to_owned();
                platform::confirm(
                    "Are you sure you want to move this section to this notebook's Recycle Bin?",
                    &name,
                    "Cancel",
                    "Delete",
                    self.reply(|state, ()| {
                        let change = Structure::Delete { path };
                        state.commands.push(Command::Structure(library, change));
                        Ok(())
                    }),
                );
                None
            }
            (Target::Group { library, path }, Action::Delete) => {
                let name = crate::library::entry_name(&path).to_owned();
                platform::confirm(
                    "Are you sure you want to move the sections in this section group to this notebook's Recycle Bin?",
                    &name,
                    "Cancel",
                    "Delete",
                    self.reply(|state, ()| {
                        let change = Structure::Delete { path };
                        state.commands.push(Command::Structure(library, change));
                        Ok(())
                    }),
                );
                None
            }
            (
                Target::Section { library, path } | Target::Group { library, path },
                Action::MoveTo(folder),
            ) => Some(Command::Structure(
                library,
                Structure::Move { path, folder },
            )),
            (Target::Section { library, path }, Action::Theme) => {
                if let Some(identity) = library.section_identity(&path) {
                    let scope = notebook::sidecar::themes::Scope::section(identity);
                    let (wearing, color) = (
                        library.themes().assigned(&scope),
                        library.section_color(&path),
                    );
                    let targets = vec![(crate::themes::Scope::Section, scope)];
                    self.show_themes(library, targets, wearing, color);
                }
                None
            }
            (Target::Notebook(library), Action::Theme) => {
                let scope = notebook::sidecar::themes::Scope::Notebook;
                let wearing = library.themes().assigned(&scope);
                let targets = vec![(crate::themes::Scope::Notebook, scope)];
                self.show_themes(library, targets, wearing, None);
                None
            }
            (Target::Section { library, path }, Action::Color(color)) => Some(Command::Structure(
                library,
                Structure::Color { path, color },
            )),
            (Target::Section { library, path }, Action::CopyLink) => {
                if let Some(link) = self.section_link(&library, &path)
                    && let Err(error) = self.clipboard.set_text(link)
                {
                    eprintln!("Copying the link failed: {error}");
                }
                None
            }
            (Target::Section { library, path }, Action::Reveal) => {
                if let Some(file) = section_file(&library, &path) {
                    platform::show_file(&file);
                }
                None
            }
            (Target::Section { library, path }, action) => {
                new(&action, folder(&path)).map(|structure| Command::Structure(library, structure))
            }
            (Target::Group { library, path }, action) => {
                new(&action, path).map(|structure| Command::Structure(library, structure))
            }
            (Target::Notebook(library), Action::Reveal) => {
                platform::reveal(&library.location);
                None
            }
            (Target::Notebook(library), Action::Close) => Some(Command::CloseNotebook(library)),
            #[cfg(not(target_arch = "wasm32"))]
            (Target::Notebook(library), Action::Delete) => {
                self.delete_notebook(library);
                None
            }
            #[cfg(feature = "live")]
            (Target::Notebook(library), Action::LiveShare) => {
                self.open_live_share(library);
                None
            }
            (Target::Notebook(library), Action::Rename | Action::Properties) => {
                self.open_properties(library);
                None
            }
            (Target::Notebook(library), Action::Sync) => {
                if let Some(session) = &self.session
                    && Arc::ptr_eq(&session.library, &library)
                {
                    session.section.wake();
                }
                library
                    .background
                    .iter()
                    .for_each(|background| background.wake());
                None
            }
            (Target::Notebook(library), Action::CopyLink) => {
                // OneNote 2010's Copy Link to Notebook: the folder's path, spaces escaped.
                let link = format!("onenote:///{}", library.location.replace(' ', "%20"));
                if let Err(error) = self.clipboard.set_text(link) {
                    eprintln!("Copying the link failed: {error}");
                }
                None
            }
            (Target::Notebook(library), Action::Raise(up)) => {
                let at = (self.notebooks.iter()).position(|open| Arc::ptr_eq(open, &library));
                let to = at.and_then(|at| if up { at.checked_sub(1) } else { Some(at + 1) });
                if let (Some(at), Some(to)) = (at, to.filter(|to| *to < self.notebooks.len())) {
                    self.notebooks.swap(at, to);
                    self.save_settings();
                }
                None
            }
            (Target::Notebook(library), action) => {
                new(&action, String::new()).map(|structure| Command::Structure(library, structure))
            }
            (Target::Closed(location), Action::Delete) => {
                self.trail.recent.retain(|place| place.notebook != location);
                self.save_settings();
                None
            }
            (Target::Closed(location), _) => {
                self.open_notebook(location, None);
                None
            }
            (Target::Server(address), action) => {
                if let Some(index) = self.servers.iter().position(|saved| *saved == address) {
                    self.saved_server((index, action == Action::Delete));
                }
                None
            }
        };
        self.commands.extend(command);
    }

    /// What `action` on page `space` of the open section does.
    fn page_command(&mut self, space: ExGuid, action: Action) -> Option<Command> {
        match action {
            Action::Rename => {
                self.rename(crate::rename::Target::Page(space));
                None
            }
            Action::Delete if self.in_recycle_bin() => {
                Some(Command::Recycle(Request::Purge(vec![space])))
            }
            Action::Delete => Some(Command::DeletePages(vec![space])),
            Action::MoveTo(path) if self.in_recycle_bin() => {
                Some(Command::Recycle(Request::Restore {
                    space,
                    path,
                    copy: false,
                }))
            }
            Action::CopyTo(path) => Some(Command::Recycle(Request::Restore {
                space,
                path,
                copy: true,
            })),
            Action::MoveTo(path) => Some(Command::MovePage { space, path }),
            Action::CopyLink => {
                let copied = self
                    .page_link(space, None)
                    .and_then(|link| self.clipboard.set_text(link));
                if let Err(error) = copied {
                    eprintln!("Copying the link failed: {error}");
                }
                None
            }
            Action::NewPage => Some(Command::NewPage {
                under: None,
                title: String::new(),
            }),
            Action::NewSubpage => Some(Command::NewPage {
                under: Some(space),
                title: String::new(),
            }),
            Action::Theme => {
                let session = self.session.as_ref()?;
                let identity = session.section.page(space).ok()?.identity?;
                let library = Arc::clone(&session.library);
                let scope = notebook::sidecar::themes::Scope::page(identity);
                let (wearing, color) = (library.themes().assigned(&scope), self.section_color());
                let targets = vec![(crate::themes::Scope::Page, scope)];
                self.show_themes(library, targets, wearing, color);
                None
            }
            Action::Versions(show) => Some(Command::History { page: space, show }),
            Action::MakeSubpage | Action::PromoteSubpage => {
                let session = self.session.as_ref()?;
                let level = session
                    .pages
                    .iter()
                    .find(|(listed, ..)| *listed == space)?
                    .2;
                let level = if action == Action::MakeSubpage {
                    level + 1
                } else {
                    level - 1
                };
                Some(Command::Pages(vec![
                    PageEdit::set_level(space, level).ok()?,
                ]))
            }
            _ => None,
        }
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

/// The file of the section at catalog `path` on this computer; none on a server reached
/// without a mount.
fn section_file(library: &Library, path: &str) -> Option<std::path::PathBuf> {
    Some(library.folder()?.join(path))
}

/// The new section or group `action` asks for in `folder`.
fn new(action: &Action, folder: String) -> Option<Structure> {
    match action {
        Action::NewSection => Some(Structure::NewSection { folder }),
        Action::NewGroup => Some(Structure::NewGroup { folder }),
        _ => None,
    }
}

/// `library`'s folders of sections, the notebook's first, then its section groups breadth
/// first, but for the recycle bin; none for a section opened on its own.
pub(crate) fn folders(library: &Library) -> Vec<&notebook::discover::Folder> {
    library.catalog().map_or_else(Vec::new, |catalog| {
        crate::library::folders(catalog, |group| !crate::library::recycle_bin(&group.path))
    })
}

#[cfg(test)]
mod tests {
    use super::SECTION_COLORS;
    use notebook::{discover::SectionState, session::Notebook};

    /// Section Color and Notebook Properties' colour as the app stores them: each of OneNote's
    /// colours and None on a section of its own, and Teal on the notebook.
    /// `SNOWBOUND_SECTION_COLOR_EXPORT` names a new directory receiving the notebook for a
    /// cold reopen in OneNote 2010 (`corpus/section-color`).
    #[test]
    fn sections_and_the_notebook_take_onenotes_colours() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-colors-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&temporary);
        let root = temporary.join("Colors");
        let cache = temporary.join("cache");
        notebook::fs::create_dir_all(&temporary).unwrap();
        let page = || onestore::PageCreation::new(None, Some(""), "Author").unwrap();
        let mut notebook = Notebook::create(&root, &cache, Notebook::NEW_COLOR, &page()).unwrap();
        let named: Vec<(Option<u32>, &str)> = (SECTION_COLORS.iter())
            .map(|&(color, name)| (Some(color), name))
            .chain([(None, "None")])
            .collect();
        for (color, name) in &named {
            let path = notebook.create_section("", name, &page()).unwrap();
            notebook.set_section_color(&path, *color).unwrap();
        }
        notebook.set_color(0xcabc4d).unwrap();

        let reopened = Notebook::open(&root, &cache).unwrap();
        let catalog = reopened.catalog();
        assert_eq!(catalog.toc.as_ref().unwrap().color, Some(0xcabc4d));
        let colors: Vec<(String, Option<u32>)> = (catalog.sections.iter())
            .map(|section| match &section.state {
                SectionState::Readable { color, .. } => (section.path.clone(), *color),
                _ => panic!("{}", section.path),
            })
            .collect();
        // The section a new notebook starts with keeps the colour it was made in.
        let expected: Vec<(String, Option<u32>)> = [(Some(0xe4a88a), "New Section 1")]
            .iter()
            .chain(&named)
            .map(|(color, name)| (format!("{name}.one"), *color))
            .collect();
        assert_eq!(colors, expected);
        if let Some(directory) = std::env::var_os("SNOWBOUND_SECTION_COLOR_EXPORT") {
            let directory = std::path::Path::new(&directory);
            notebook::fs::create_dir_all(directory).unwrap();
            for entry in notebook::fs::read_dir(&root).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_file() {
                    notebook::fs::copy(entry.path(), directory.join(entry.file_name())).unwrap();
                }
            }
        }
        notebook::fs::remove_dir_all(&temporary).unwrap();
    }
}
