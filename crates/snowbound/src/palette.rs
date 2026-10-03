//! The palette: the pages and sections shown lately, then the open notebooks, their sections
//! and every page the search index holds to go to, or after a leading `>` the command
//! table's commands, narrowed as typed. ⌘K or a right-click lists what can be done to a row.

use crate::{
    State, art,
    commands::{self, Choice},
    library,
    menus::{self, Action, Target},
};
use std::sync::Arc;
use ui::{
    Anchor, Id,
    popup::{Item, Pick},
};

/// What the palette's query starts with to list commands.
pub(crate) const COMMANDS: &str = ">";

pub(crate) fn id() -> Id {
    Id::ROOT.child("palette")
}

/// A row: its text, the dim text after it, its icon and what it runs; a heading runs nothing.
#[derive(Default)]
struct Row {
    text: String,
    after: String,
    icon: Option<&'static [&'static str]>,
    tint: Option<[f32; 4]>,
    disabled: bool,
    /// Repeats a row listed further on, as the recent ones do.
    repeated: bool,
    target: Option<Target>,
}

impl Row {
    fn heading(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            ..Self::default()
        }
    }

    fn item(&self) -> Item<'_> {
        Item {
            text: &self.text,
            shortcut: &self.after,
            icon: self.icon,
            tint: self.tint,
            disabled: self.disabled,
            heading: self.target.is_none(),
            separated: self.target.is_none(),
            repeated: self.repeated,
            ..Item::default()
        }
    }
}

impl State {
    /// Builds the palette while it is open, with the actions on a row when asked for, and
    /// does what is chosen from either.
    pub(crate) fn palette(&mut self) {
        if !self.ui.popup_open(id()) {
            return;
        }
        let format = self.format_state();
        let mut commands: Vec<Row> = commands::COMMANDS
            .iter()
            .filter(|command| commands::offered(command.id))
            .map(|command| Row {
                text: command.title.to_owned(),
                after: commands::shortcut(command.id),
                icon: crate::artwork(command.id),
                disabled: !self.status(&Choice::Command(command.id), &format).enabled,
                target: Some(Target::Command(command.id)),
                ..Row::default()
            })
            .collect();
        commands.extend(self.tags.iter().enumerate().map(|(place, tag)| {
            let id = commands::Id::Tag(place);
            Row {
                text: tag.label.clone(),
                after: commands::shortcut(id),
                icon: Some(crate::tag_art(tag)),
                tint: Some([1.0; 4]),
                disabled: !self.status(&Choice::Command(id), &format).enabled,
                target: Some(Target::Command(id)),
                ..Row::default()
            }
        }));
        let mut places = self.recent();
        places.push(Row::heading("Notebooks"));
        places.extend(
            (self.notebooks.iter())
                .filter(|library| library.catalog().is_some())
                .map(|library| Row {
                    text: library.name.clone(),
                    icon: Some(art::NOTEBOOK),
                    tint: Some(crate::notebook_color(&self.ui.theme, library.color())),
                    target: Some(Target::Notebook(Arc::clone(library))),
                    ..Row::default()
                }),
        );
        places.push(Row::heading("Sections"));
        let theme = &self.ui.theme;
        for library in &self.notebooks {
            // A section opened on its own has no catalog, and lists itself at "".
            let folders = menus::folders(library);
            let paths: Vec<&str> = if folders.is_empty() {
                vec![""]
            } else {
                folders.iter().map(|folder| folder.path.as_str()).collect()
            };
            for path in paths {
                places.extend(library.tabs(path).into_iter().map(|tab| Row {
                    text: tab.name,
                    after: library.name.clone(),
                    icon: Some(art::SECTION),
                    tint: Some(theme.section(crate::section_color(tab.color)).accent),
                    target: Some(Target::Section {
                        library: Arc::clone(library),
                        path: tab.path,
                    }),
                    ..Row::default()
                }));
            }
        }
        places.push(Row::heading("Pages"));
        let index = self
            .search
            .index
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        places.extend(index.entries().iter().filter_map(|entry| {
            let (location, path) = entry.section.split_once('\n').unwrap_or_default();
            let library = self
                .notebooks
                .iter()
                .find(|library| library.location == location)?;
            let mut section = library::section_name(path, &None);
            if section.is_empty() {
                section = library.name.clone();
            }
            Some(Row {
                text: title(&entry.title),
                after: section,
                icon: Some(art::PAGE),
                target: Some(Target::Page {
                    library: Arc::clone(library),
                    path: path.to_owned(),
                    space: entry.space,
                }),
                ..Row::default()
            })
        }));
        drop(index);
        let mut items =
            [&commands, &places].map(|rows| rows.iter().map(Row::item).collect::<Vec<_>>());
        // The latest page or section starts highlighted, so Enter goes back to it.
        if let Some(latest) = items[1].get_mut(1).filter(|item| item.repeated) {
            latest.current = true;
        }
        let picked = ui::popup::palette(
            &mut self.ui,
            id(),
            &[(COMMANDS, &items[0]), ("", &items[1])],
            "Search pages, sections and notebooks (type > for commands)",
        );
        let mut rows = [commands, places];
        match picked {
            Some(Pick::Run(mode, index)) => {
                if let Some(target) = rows[mode].swap_remove(index).target {
                    let action = match target {
                        Target::Command(_) => Action::Run,
                        _ => Action::Open,
                    };
                    self.act_on(target, action);
                }
            }
            Some(Pick::Actions(mode, index)) => {
                self.actions = rows[mode].swap_remove(index).target;
            }
            None => {}
        }
        let panel = ui::popup::actions(id());
        let Some(target) = self.actions.clone().filter(|_| self.ui.popup_open(panel)) else {
            self.actions = None;
            return;
        };
        // Run or Open leads, as Enter on the row does.
        let (keys, first) = match target {
            Target::Command(id) => (
                commands::shortcut(id),
                (
                    Action::Run,
                    Item {
                        text: "Run",
                        icon: crate::artwork(id),
                        disabled: !self.status(&Choice::Command(id), &format).enabled,
                        ..Item::default()
                    },
                ),
            ),
            _ => (
                String::new(),
                (
                    Action::Open,
                    Item {
                        text: "Open",
                        icon: Action::Open.art(),
                        ..Item::default()
                    },
                ),
            ),
        };
        let first = (
            first.0,
            Item {
                shortcut: &keys,
                ..first.1
            },
        );
        let mut actions = vec![first];
        actions.extend(self.actions(&target).into_iter().enumerate().map(
            |(at, (action, item))| {
                let separated = item.separated || at == 0;
                (action, Item { separated, ..item })
            },
        ));
        let anchor = Anchor::Right(self.ui.rect(id()).unwrap_or_default());
        if let Some(action) =
            self.action_menu(panel, anchor, Some("Search actions"), &target, &actions)
        {
            self.actions = None;
            self.act_on(target, action);
        }
    }

    /// Under a heading, the pages shown lately but the one shown, their sections but the one
    /// open, the notebooks of those since closed, and the servers saved to reconnect to;
    /// latest first, and none before any. Pages and sections since gone are forgotten.
    fn recent(&mut self) -> Vec<Row> {
        let index = self
            .search
            .index
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let notebooks = &self.notebooks;
        let library = |location: &str| {
            (notebooks.iter())
                .find(|library| library.location == location)
                .map(Arc::clone)
        };
        // A closed notebook keeps its places to open it again, an unreadable one until it reads.
        let pruned = self.trail.prune(&index, |place| {
            library(&place.notebook)
                .is_none_or(|library| library.notebook.is_err() || library.contains(&place.section))
        });
        let shown = self.session.as_ref().map(|session| {
            (
                session.library.location.as_str(),
                session.tabs[session.tab].path.as_str(),
                session.space,
            )
        });
        let theme = &self.ui.theme;
        let [mut pages, mut sections, mut closed] = [(); 3].map(|()| Vec::<Row>::new());
        let listed = |rows: &[Row], target: &Target| {
            rows.iter().any(|row| match (&row.target, target) {
                (
                    Some(Target::Section { library, path }),
                    Target::Section {
                        library: other,
                        path: at,
                    },
                ) => library.location == other.location && path == at,
                (Some(Target::Closed(location)), Target::Closed(other)) => location == other,
                _ => false,
            })
        };
        for place in &self.trail.recent {
            let Some(library) = library(&place.notebook) else {
                let target = Target::Closed(place.notebook.clone());
                if !listed(&closed, &target) {
                    closed.push(Row {
                        text: (place.notebook.rsplit(['/', '\\']))
                            .find(|name| !name.is_empty())
                            .unwrap_or(&place.notebook)
                            .to_owned(),
                        after: "Closed".to_owned(),
                        icon: Some(art::NOTEBOOK),
                        tint: Some(crate::notebook_color(&self.ui.theme, None)),
                        target: Some(target),
                        ..Row::default()
                    });
                }
                continue;
            };
            if library.notebook.is_err() {
                continue;
            }
            let here = (place.notebook.as_str(), place.section.as_str());
            let section = Target::Section {
                library: Arc::clone(&library),
                path: place.section.clone(),
            };
            if shown.is_none_or(|(location, path, _)| (location, path) != here)
                && !listed(&sections, &section)
                && let Some(tab) = (library.tabs(&menus::folder(&place.section)).into_iter())
                    .find(|tab| tab.path == place.section)
            {
                sections.push(Row {
                    text: tab.name,
                    after: library.name.clone(),
                    icon: Some(art::SECTION),
                    tint: Some(theme.section(crate::section_color(tab.color)).accent),
                    repeated: true,
                    target: Some(section),
                    ..Row::default()
                });
            }
            let key = library.key(&place.section);
            // A page the index hasn't read yet waits for its title.
            let Some(entry) = (index.entries().iter())
                .find(|entry| entry.space == place.page && entry.section == key)
                .filter(|_| shown != Some((here.0, here.1, place.page)))
            else {
                continue;
            };
            pages.push(Row {
                text: title(&entry.title),
                after: library::section_name(&place.section, &None),
                icon: Some(art::PAGE),
                repeated: true,
                target: Some(Target::Page {
                    library,
                    path: place.section.clone(),
                    space: place.page,
                }),
                ..Row::default()
            });
        }
        drop(index);
        if pruned {
            self.save_settings();
        }
        let servers = self.servers.iter().map(|address| Row {
            text: address.strip_prefix("smb://").unwrap_or(address).to_owned(),
            after: "Server".to_owned(),
            icon: Some(art::SERVER),
            target: Some(Target::Server(address.clone())),
            ..Row::default()
        });
        let rows: Vec<Row> = pages
            .into_iter()
            .chain(sections)
            .chain(closed)
            .chain(servers)
            .collect();
        if rows.is_empty() {
            return rows;
        }
        std::iter::once(Row::heading("Recent"))
            .chain(rows)
            .collect()
    }
}

/// A page's title as the palette lists it.
fn title(title: &str) -> String {
    if title.is_empty() {
        "Untitled page".to_owned()
    } else {
        title.to_owned()
    }
}
