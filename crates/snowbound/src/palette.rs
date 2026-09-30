//! The palette: the open notebooks, their sections and every page the search index holds to
//! go to, or after a leading `>` the command table's commands, narrowed as typed.

use crate::{
    Command, Library, State, art,
    commands::{self, Choice},
    library,
};
use onestore::ExGuid;
use std::sync::Arc;
use ui::{Id, popup::Item};

/// What the palette's query starts with to list commands.
pub(crate) const COMMANDS: &str = ">";

pub(crate) fn id() -> Id {
    Id::ROOT.child("palette")
}

enum Target {
    Command(commands::Id),
    Notebook(String),
    Section(Arc<Library>, String),
    Page(String, ExGuid),
}

/// A row: its text, the dim text after it, its icon and what it runs; a heading runs nothing.
struct Row {
    text: String,
    after: String,
    icon: Option<&'static [&'static str]>,
    tint: Option<[f32; 4]>,
    disabled: bool,
    target: Option<Target>,
}

impl Row {
    fn heading(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            after: String::new(),
            icon: None,
            tint: None,
            disabled: false,
            target: None,
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
            ..Item::default()
        }
    }
}

impl State {
    /// Builds the palette while it is open, and runs what is chosen from it.
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
                tint: None,
                disabled: !self.status(&Choice::Command(command.id), &format).enabled,
                target: Some(Target::Command(command.id)),
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
            }
        }));
        let mut places = vec![Row::heading("Notebooks")];
        places.extend(
            (self.notebooks.iter())
                .filter(|library| library.catalog().is_some())
                .map(|library| Row {
                    text: library.name.clone(),
                    after: String::new(),
                    icon: Some(art::NOTEBOOK),
                    tint: None,
                    disabled: false,
                    target: Some(Target::Notebook(library.location.clone())),
                }),
        );
        places.push(Row::heading("Sections"));
        let theme = &self.ui.theme;
        for library in &self.notebooks {
            // A section opened on its own has no catalog, and lists itself at "".
            let mut folders: Vec<_> = library.catalog().into_iter().collect();
            let mut at = 0;
            while let Some(&folder) = folders.get(at) {
                folders.extend(
                    folder
                        .groups
                        .iter()
                        .filter(|group| !library::recycle_bin(&group.path)),
                );
                at += 1;
            }
            let paths = folders.iter().map(|folder| folder.path.as_str());
            let paths: Vec<&str> = if folders.is_empty() {
                vec![""]
            } else {
                paths.collect()
            };
            for path in paths {
                places.extend(library.tabs(path).into_iter().map(|tab| Row {
                    text: tab.name,
                    after: library.name.clone(),
                    icon: Some(art::SECTION),
                    tint: Some(theme.section(crate::section_color(tab.color)).accent),
                    disabled: false,
                    target: Some(Target::Section(Arc::clone(library), tab.path)),
                }));
            }
        }
        places.push(Row::heading("Pages"));
        let index = self
            .search
            .index
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        places.extend(index.entries().iter().map(|entry| {
            let (location, path) = entry.section.split_once('\n').unwrap_or_default();
            let mut section = library::section_name(path, &None);
            if section.is_empty() {
                section = (self.notebooks.iter())
                    .find(|library| library.location == location)
                    .map_or_else(String::new, |library| library.name.clone());
            }
            Row {
                text: if entry.title.is_empty() {
                    "Untitled page".to_owned()
                } else {
                    entry.title.clone()
                },
                after: section,
                icon: Some(art::PAGE),
                tint: None,
                disabled: false,
                target: Some(Target::Page(entry.section.clone(), entry.space)),
            }
        }));
        drop(index);
        let items = [&commands, &places].map(|rows| rows.iter().map(Row::item).collect::<Vec<_>>());
        let chosen = ui::popup::palette(
            &mut self.ui,
            id(),
            &[(COMMANDS, &items[0]), ("", &items[1])],
            "Search pages, sections and notebooks (type > for commands)",
        );
        let mut rows = [commands, places];
        match chosen.and_then(|(mode, index)| rows[mode].swap_remove(index).target) {
            Some(Target::Command(command)) => self.choose(Choice::Command(command)),
            Some(Target::Notebook(location)) => self.open_notebook(location, None),
            Some(Target::Section(library, path)) => {
                self.commands.push(Command::OpenSection(library, path));
            }
            Some(Target::Page(section, space)) => {
                let open = (self.session.as_ref())
                    .map(|session| session.library.key(&session.tabs[session.tab].path));
                let (location, path) = section.split_once('\n').unwrap_or_default();
                if open.as_ref() == Some(&section) {
                    self.commands.push(Command::OpenPage(space));
                } else if let Some(library) =
                    (self.notebooks.iter()).find(|library| library.location == location)
                {
                    self.commands
                        .push(Command::OpenSection(Arc::clone(library), path.to_owned()));
                    self.last_pages.insert(section, space);
                }
            }
            None => {}
        }
    }
}
