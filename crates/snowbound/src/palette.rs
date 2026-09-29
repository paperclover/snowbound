//! The command palette: the command table's commands, then the open notebooks' sections and
//! every page the search index holds to go to, narrowed as typed.

use crate::{
    Command, Library, State,
    commands::{self, COMMANDS, Choice},
    library,
};
use onestore::ExGuid;
use std::sync::Arc;
use ui::{Id, popup::Item};

pub(crate) fn id() -> Id {
    Id::ROOT.child("palette")
}

enum Target {
    Command(commands::Id),
    Section(Arc<Library>, String),
    Page(String, ExGuid),
}

/// A row: its text, the dim text after it, and what it runs; a heading runs nothing.
struct Row {
    text: String,
    after: String,
    disabled: bool,
    target: Option<Target>,
}

impl State {
    /// Builds the palette while it is open, and runs what is chosen from it.
    pub(crate) fn palette(&mut self) {
        if !self.ui.popup_open(id()) {
            return;
        }
        let format = self.format_state();
        let mut rows: Vec<Row> = COMMANDS
            .iter()
            .map(|command| Row {
                text: command.title.to_owned(),
                after: commands::shortcut(command.id),
                disabled: !self.status(&Choice::Command(command.id), &format).enabled,
                target: Some(Target::Command(command.id)),
            })
            .collect();
        rows.extend(self.tags.iter().enumerate().map(|(place, tag)| {
            let id = commands::Id::Tag(place);
            Row {
                text: tag.label.clone(),
                after: commands::shortcut(id),
                disabled: !self.status(&Choice::Command(id), &format).enabled,
                target: Some(Target::Command(id)),
            }
        }));
        let heading = |text: &str| Row {
            text: text.to_owned(),
            after: String::new(),
            disabled: false,
            target: None,
        };
        rows.push(heading("Sections"));
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
                rows.extend(library.tabs(path).into_iter().map(|tab| Row {
                    text: tab.name,
                    after: library.name.clone(),
                    disabled: false,
                    target: Some(Target::Section(Arc::clone(library), tab.path)),
                }));
            }
        }
        rows.push(heading("Pages"));
        let index = self
            .search
            .index
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        rows.extend(index.entries().iter().map(|entry| {
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
                disabled: false,
                target: Some(Target::Page(entry.section.clone(), entry.space)),
            }
        }));
        drop(index);
        let items: Vec<Item> = rows
            .iter()
            .map(|row| Item {
                text: &row.text,
                shortcut: &row.after,
                disabled: row.disabled,
                heading: row.target.is_none(),
                separated: row.target.is_none(),
                ..Item::default()
            })
            .collect();
        let chosen = ui::popup::palette(
            &mut self.ui,
            id(),
            &items,
            "Search commands, sections and pages",
        );
        match chosen.and_then(|index| rows.swap_remove(index).target) {
            Some(Target::Command(command)) => self.choose(Choice::Command(command)),
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
