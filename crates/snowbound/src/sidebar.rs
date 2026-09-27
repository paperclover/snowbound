//! OneNote 2010's navigation bar: the open notebooks with their sections and section
//! groups, down the window's left, beside the section tabs.

use crate::{art, library::Library, menus::Target, section_color};
use notebook::discover::{Folder, SectionState};
use std::{collections::HashSet, sync::Arc};
use ui::{Axis, Event, Flags, Id, Signal, Spec, Theme, Ui, fill, fit, px};
use winit::keyboard::{Key, NamedKey};

pub const WIDTH: f32 = 220.0;
/// The collapsed sidebar: a rail as wide as its button, which stays where it is as the
/// sidebar opens.
pub const RAIL: f32 = crate::FRAME + ui::shell::TOOL + 2.0;
const ROW: f32 = 24.0;
/// How far each level of the tree sits inside its parent.
const INDENT: f32 = 16.0;

/// What the sidebar was asked to do.
pub enum Action {
    /// Expands or collapses the sidebar.
    Toggle,
    /// Shows a notebook where it was left.
    Notebook(usize),
    /// Opens a notebook's section by catalog path.
    Open { notebook: usize, path: String },
    /// Folds or unfolds a notebook's or section group's rows, by `fold_key`.
    Fold(String),
    NewNotebook,
    OpenNotebook,
    /// A row's context menu, opened here.
    Menu(Target, [f32; 2]),
    /// Ends renaming, with the name typed or without.
    Renamed(bool),
}

/// A section or group being renamed in place, and the name typed so far.
pub struct Renaming {
    pub library: Arc<Library>,
    pub path: String,
    pub name: String,
}

/// The field a row being renamed shows.
pub fn rename_field() -> Id {
    Id::ROOT.child("rename")
}

/// Names a notebook's or group's rows for folding them.
pub fn fold_key(library: &Library, path: &str) -> String {
    format!("{}\n{path}", library.location)
}

/// What the tree's rows read and what they were asked.
struct Tree<'a> {
    theme: &'a Theme,
    /// The open section's notebook index and catalog path.
    open: Option<(usize, &'a str)>,
    folded: &'a HashSet<String>,
    renaming: Option<&'a mut Renaming>,
    action: Option<Action>,
    /// Each section and group row: where it lies, and what it is.
    rows: Vec<Entry>,
    /// The section or group held down, which a drag moves.
    held: Option<Entry>,
}

/// A section or group row, for dragging one onto or between the others.
#[derive(Clone, PartialEq)]
pub struct Entry {
    pub notebook: usize,
    pub path: String,
    pub group: bool,
    pub rect: [f32; 4],
}

impl Entry {
    fn folder(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(folder, _)| folder)
    }
}

/// The sidebar under a header row `header` tall, listing `notebooks` with the open section
/// marked.
pub fn sidebar(
    ui: &mut Ui,
    theme: &Theme,
    notebooks: &[Arc<Library>],
    open: Option<(usize, &str)>,
    folded: &HashSet<String>,
    renaming: Option<&mut Renaming>,
    header: f32,
) -> (Option<Action>, Vec<Entry>, Option<Entry>) {
    let mut tree = Tree {
        theme,
        open,
        folded,
        renaming,
        action: None,
        rows: Vec::new(),
        held: None,
    };
    ui.open(
        "header",
        Spec {
            size: [fill(), px(header)],
            pad: [crate::FRAME, (header - ui::shell::TOOL) / 2.0],
            gap: 6.0,
            ..Spec::default()
        },
    );
    if ui::shell::tool_button(ui, "toggle", art::NOTEBOOK, theme.text, false).clicked {
        tree.action = Some(Action::Toggle);
    }
    ui.leaf(
        "title",
        Spec {
            size: [fill(), px(ui::shell::TOOL)],
            text: Some("Notebooks"),
            color: Some(theme.text_dim),
            ..Spec::default()
        },
    );
    ui.close();
    ui.open(
        "notebooks",
        Spec {
            flags: Flags::SCROLL | Flags::CLIP,
            axis: Axis::Y,
            size: [fill(), fill()],
            pad: [4.0, 2.0],
            ..Spec::default()
        },
    );
    for (index, library) in notebooks.iter().enumerate() {
        let key = fold_key(library, "");
        let unfolded = !folded.contains(&key);
        let (row, fold) = tree_row(
            ui,
            &mut tree,
            ("notebook", index),
            Row {
                label: &library.name,
                icon: Leading::Icon(art::NOTEBOOK),
                depth: 0,
                dim: library.notebook.is_err(),
                fold: Some(unfolded),
                selected: None,
                renamed: false,
            },
        );
        if fold {
            tree.action = Some(Action::Fold(key));
        } else if row.clicked && open.is_none_or(|(notebook, _)| notebook != index) {
            tree.action = Some(Action::Notebook(index));
        }
        if let Some(point) = row.context {
            tree.action = Some(Action::Menu(Target::Notebook(Arc::clone(library)), point));
        }
        if unfolded && let Some(catalog) = library.catalog() {
            folder(ui, &mut tree, library, index, catalog, 1);
        }
    }
    ui.close();
    ui.open(
        "footer",
        Spec {
            axis: Axis::Y,
            size: [fill(), ui::children()],
            pad: [4.0, 6.0],
            ..Spec::default()
        },
    );
    for (part, icon, label, chosen) in [
        ("new", art::PLUS, "New Notebook", Action::NewNotebook),
        ("open", art::NOTEBOOK, "Open Existing", Action::OpenNotebook),
    ] {
        let (row, _) = tree_row(
            ui,
            &mut tree,
            part,
            Row {
                label,
                icon: Leading::Icon(icon),
                depth: 0,
                dim: false,
                fold: None,
                selected: None,
                renamed: false,
            },
        );
        if row.clicked {
            tree.action = Some(chosen);
        }
    }
    ui.close();
    (tree.action, tree.rows, tree.held)
}

/// A notebook's or group's sections, then its groups, as OneNote lists them.
fn folder(
    ui: &mut Ui,
    tree: &mut Tree,
    library: &Arc<Library>,
    notebook: usize,
    folder: &Folder,
    depth: u32,
) {
    let open = tree
        .open
        .filter(|(index, _)| *index == notebook)
        .map(|(_, path)| path);
    let renamed = |tree: &Tree, path: &str| {
        tree.renaming
            .as_ref()
            .is_some_and(|renaming| Arc::ptr_eq(&renaming.library, library) && renaming.path == path)
    };
    for section in &folder.sections {
        let (name, color, readable) = match &section.state {
            SectionState::Readable { name, color } => (
                crate::library::section_name(&section.path, name),
                *color,
                true,
            ),
            _ => (
                crate::library::section_name(&section.path, &None),
                None,
                false,
            ),
        };
        let renaming = renamed(tree, &section.path);
        let (row, _) = tree_row(
            ui,
            tree,
            ("section", notebook, &section.path),
            Row {
                label: &name,
                icon: Leading::Swatch(section_color(color)),
                depth,
                dim: !readable,
                fold: None,
                selected: (open == Some(section.path.as_str())).then_some(section_color(color)),
                renamed: renaming,
            },
        );
        if row.clicked && readable && open != Some(section.path.as_str()) {
            tree.action = Some(Action::Open {
                notebook,
                path: section.path.clone(),
            });
        }
        entry(ui, tree, notebook, &section.path, false, &row);
        if let Some(point) = row.context {
            let target = Target::Section {
                library: Arc::clone(library),
                path: section.path.clone(),
            };
            tree.action = Some(Action::Menu(target, point));
        }
    }
    for group in &folder.groups {
        // OneNote lists deleted sections under its own command, not as a group.
        if crate::library::recycle_bin(group) {
            continue;
        }
        let key = fold_key(library, &group.path);
        let unfolded = !tree.folded.contains(&key);
        let name = group.path.rsplit('/').next().unwrap_or_default();
        let renaming = renamed(tree, &group.path);
        let (row, fold) = tree_row(
            ui,
            tree,
            ("group", notebook, &group.path),
            Row {
                label: name,
                icon: Leading::Icon(art::SECTION_GROUP),
                depth,
                dim: false,
                fold: Some(unfolded),
                selected: None,
                renamed: renaming,
            },
        );
        if row.clicked || fold {
            tree.action = Some(Action::Fold(key));
        }
        entry(ui, tree, notebook, &group.path, true, &row);
        if let Some(point) = row.context {
            let target = Target::Group {
                library: Arc::clone(library),
                path: group.path.clone(),
            };
            tree.action = Some(Action::Menu(target, point));
        }
        if unfolded {
            self::folder(ui, tree, library, notebook, group, depth + 1);
        }
    }
}

/// Records a section or group row for dragging.
fn entry(ui: &Ui, tree: &mut Tree, notebook: usize, path: &str, group: bool, row: &Signal) {
    let Some(rect) = ui.rect(ui.id(("section", notebook, path)))
        .or_else(|| ui.rect(ui.id(("group", notebook, path))))
    else {
        return;
    };
    let entry = Entry {
        notebook,
        path: path.to_owned(),
        group,
        rect,
    };
    if row.dragging {
        tree.held = Some(entry.clone());
    }
    tree.rows.push(entry);
}

enum Leading {
    Icon(&'static [&'static str]),
    /// A section's colour, as OneNote's section icon shows it.
    Swatch([f32; 4]),
}

struct Row<'a> {
    label: &'a str,
    icon: Leading,
    depth: u32,
    dim: bool,
    /// Whether the rows beneath are unfolded, for rows that fold.
    fold: Option<bool>,
    /// The open section's colour.
    selected: Option<[f32; 4]>,
    /// The row is being renamed: its label is the tree's rename field.
    renamed: bool,
}

/// One row of the tree: its signal, and whether its fold arrow was clicked.
fn tree_row(ui: &mut Ui, tree: &mut Tree, part: impl std::hash::Hash, row: Row) -> (Signal, bool) {
    let theme = tree.theme;
    let lit = row
        .selected
        .map(|color| ui::mix(theme.section(color).tab, theme.base, 0.35));
    let id = ui.open(
        part,
        Spec {
            flags: Flags::CLICKABLE,
            size: [fill(), px(ROW)],
            fill: lit,
            hover_fill: Some(ui::mix(lit.unwrap_or(theme.strip), theme.hover(), 0.6)),
            radius: 4.0,
            pad: [6.0 + INDENT * row.depth as f32, 0.0],
            gap: 6.0,
            ..Spec::default()
        },
    );
    let color = if row.dim { theme.text_dim } else { theme.text };
    match row.icon {
        Leading::Icon(icon) => {
            ui.leaf(
                "icon",
                Spec {
                    size: [px(16.0), px(ROW)],
                    icon: Some(icon),
                    color: Some(color),
                    ..Spec::default()
                },
            );
        }
        Leading::Swatch(swatch) => {
            ui.open(
                "swatch",
                Spec {
                    size: [px(16.0), px(ROW)],
                    pad: [3.0, 6.0],
                    ..Spec::default()
                },
            );
            ui.leaf(
                "color",
                Spec {
                    size: [fill(), fill()],
                    fill: Some(theme.section(swatch).frame[0]),
                    border: Some(theme.section(swatch).edge),
                    radius: 2.0,
                    ..Spec::default()
                },
            );
            ui.close();
        }
    }
    match (row.renamed, tree.renaming.as_deref_mut()) {
        (true, Some(renaming)) => {
            let field = ui::text_field(
                ui,
                rename_field(),
                &mut renaming.name,
                "",
                Spec {
                    size: [fill(), px(ROW - 4.0)],
                    fill: Some(theme.base),
                    border: Some(theme.accent),
                    radius: 3.0,
                    pad: [4.0, 0.0],
                    ..Spec::default()
                },
            );
            for event in &field.events {
                if let Event::Key {
                    key: Key::Named(key @ (NamedKey::Enter | NamedKey::Escape)),
                    ..
                } = event
                {
                    tree.action = Some(Action::Renamed(*key == NamedKey::Enter));
                }
            }
        }
        _ => {
            ui.leaf(
                "label",
                Spec {
                    size: [fill(), px(ROW)],
                    text: Some(row.label),
                    color: Some(color),
                    ..Spec::default()
                },
            );
        }
    }
    let folded = row.fold.is_some_and(|unfolded| {
        ui.leaf(
            "fold",
            Spec {
                flags: Flags::CLICKABLE,
                size: [fit(), px(ROW)],
                icon: Some(if unfolded {
                    art::CHEVRON_UP
                } else {
                    ui::shell::CHEVRON
                }),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        )
        .clicked
    });
    ui.close();
    (ui.signal(id), folded)
}

impl crate::State {
    /// The sidebar left of the section tabs, easing open and shut.
    pub(crate) fn sidebar(&mut self, theme: &Theme) {
        if self.temporary {
            return;
        }
        let width = self
            .ui
            .animate(self.ui.id("sidebar"), if self.sidebar { WIDTH } else { RAIL });
        self.ui.open(
            "sidebar",
            Spec {
                flags: Flags::CLIP,
                size: [px(width), fill()],
                fill: Some(theme.strip),
                ..Spec::default()
            },
        );
        // The rows keep their width as the sidebar eases, so their labels never reflow.
        let rows_id = self.ui.id("rows");
        self.ui.open(
            "rows",
            Spec {
                axis: Axis::Y,
                size: [px(WIDTH), fill()],
                ..Spec::default()
            },
        );
        let open = self.session.as_ref().and_then(|session| {
            let index = self
                .notebooks
                .iter()
                .position(|library| Arc::ptr_eq(library, &session.library))?;
            Some((index, session.tabs[session.tab].path.as_str()))
        });
        let (action, rows, held) = sidebar(
            &mut self.ui,
            theme,
            &self.notebooks,
            open,
            &self.folded,
            self.renaming.as_mut(),
            crate::TAB_ROW,
        );
        let corner = self.ui.rect(rows_id).unwrap_or_default();
        self.drag_entries(held, &rows, corner, theme);
        self.ui.close();
        self.ui.close();
        match action {
            Some(Action::Toggle) => {
                self.sidebar = !self.sidebar;
                self.save_settings();
            }
            Some(Action::Fold(key)) => {
                if !self.folded.remove(&key) {
                    self.folded.insert(key);
                }
            }
            Some(Action::Notebook(index)) => {
                let library = Arc::clone(&self.notebooks[index]);
                if let Some(path) = library.first_section() {
                    self.commands.push(crate::Command::OpenSection(library, path));
                }
            }
            Some(Action::Open { notebook, path }) => {
                let library = Arc::clone(&self.notebooks[notebook]);
                self.commands.push(crate::Command::OpenSection(library, path));
            }
            Some(Action::Menu(target, point)) => {
                self.menu = Some((target, point));
                self.ui.open_popup(crate::menus::id());
            }
            Some(Action::Renamed(keep)) => {
                if let Some(renaming) = self.renaming.take()
                    && keep
                {
                    let name = renaming.name.trim().to_owned();
                    let old = renaming.path.rsplit('/').next().unwrap_or_default();
                    if !name.is_empty() && name != old.strip_suffix(".one").unwrap_or(old) {
                        self.commands.push(crate::Command::Structure(
                            renaming.library,
                            crate::manage::Structure::Rename {
                                path: renaming.path,
                                name,
                            },
                        ));
                    }
                }
                self.ui.set_focus(Some(crate::page()));
            }
            Some(Action::NewNotebook) => self.commands.push(crate::Command::NewNotebook),
            Some(Action::OpenNotebook) => self.commands.push(crate::Command::OpenNotebook),
            None => {}
        }
    }

    /// Moves a section or group by dragging its row: onto a group's middle it moves into the
    /// group, as OneNote moves one dropped on a group's tab; between rows of its folder it
    /// takes that place; between rows of another folder it moves there, last. `corner` is
    /// the rows' box.
    fn drag_entries(&mut self, held: Option<Entry>, rows: &[Entry], corner: [f32; 4], theme: &Theme) {
        let dragged = held.clone().or(self.dragging_entry.take());
        self.dragging_entry = held.clone();
        let (Some(dragged), Some([_, y])) = (dragged, self.ui.pointer()) else {
            return;
        };
        let inside = |entry: &Entry| {
            entry.path == dragged.path || entry.path.starts_with(&format!("{}/", dragged.path))
        };
        let candidates: Vec<&Entry> = rows
            .iter()
            .filter(|entry| entry.notebook == dragged.notebook && !inside(entry))
            .collect();
        let Some(under) = candidates
            .iter()
            .find(|entry| y >= entry.rect[1] && y < entry.rect[3])
            .or_else(|| candidates.last().filter(|entry| y >= entry.rect[3]))
        else {
            return;
        };
        let [top, bottom] = [under.rect[1], under.rect[3]];
        let quarter = (bottom - top) / 4.0;
        let into = under.group && y > top + quarter && y < bottom - quarter;
        let (folder, before) = if into {
            (under.path.as_str(), None)
        } else if y < (top + bottom) / 2.0 {
            (under.folder(), Some(under.path.as_str()))
        } else {
            let next = candidates
                .iter()
                .skip_while(|entry| entry.path != under.path)
                .nth(1)
                .filter(|entry| entry.folder() == under.folder());
            (under.folder(), next.map(|entry| entry.path.as_str()))
        };
        let change = if folder == dragged.folder() {
            if into {
                return;
            }
            // The folder's order with the dragged entry where it was dropped.
            let mut paths: Vec<String> = candidates
                .iter()
                .filter(|entry| entry.folder() == folder)
                .map(|entry| entry.path.clone())
                .collect();
            let at = before
                .and_then(|before| paths.iter().position(|path| path == before))
                .unwrap_or(paths.len());
            paths.insert(at, dragged.path.clone());
            let current: Vec<&str> = rows
                .iter()
                .filter(|entry| entry.notebook == dragged.notebook && entry.folder() == folder)
                .map(|entry| entry.path.as_str())
                .collect();
            if paths.iter().map(String::as_str).eq(current) {
                return;
            }
            crate::manage::Structure::Reorder {
                folder: folder.to_owned(),
                paths,
            }
        } else {
            crate::manage::Structure::Move {
                path: dragged.path.clone(),
                folder: folder.to_owned(),
            }
        };
        if held.is_some() {
            let accent = theme.accent;
            if into {
                let [left, top, right, bottom] = under.rect;
                self.ui.mark(
                    [left - corner[0], top - corner[1], right - corner[0], bottom - corner[1]],
                    [accent[0], accent[1], accent[2], 0.3],
                    4.0,
                );
            } else {
                let line = if before == Some(under.path.as_str()) { top } else { bottom } - corner[1];
                self.ui.mark(
                    [under.rect[0] - corner[0], line - 1.5, under.rect[2] - corner[0], line + 1.5],
                    accent,
                    1.5,
                );
            }
        } else {
            let library = std::sync::Arc::clone(&self.notebooks[dragged.notebook]);
            self.commands.push(crate::Command::Structure(library, change));
        }
    }

    /// Shows the notebook at `location` at its first section, opening it unless it is open.
    pub(crate) fn open_notebook(&mut self, location: String) {
        let open = self
            .notebooks
            .iter()
            .find(|library| library.location == location)
            .cloned();
        let (cache, notify) = (self.cache.clone(), crate::notify(self.proxy.clone()));
        self.load(move || {
            let library = open.unwrap_or_else(|| Arc::new(Library::notebook(&location, &cache)));
            if let Err(error) = &library.notebook {
                return Err(error.clone().into());
            }
            let path = library
                .first_section()
                .ok_or("This folder holds no notebook sections.")?;
            let section = library.open(&path, notify)?;
            let (session, page) = crate::read_session(section, library, path, None)?;
            Ok((crate::Loaded::Section(Box::new(session)), page))
        });
    }

    /// The page's place on the first run, or once every notebook is closed.
    pub(crate) fn welcome(&mut self, theme: &Theme) {
        const SIZE: [f32; 2] = [420.0, 120.0];
        let [left, top, right, bottom] = self.ui.rect(crate::page()).unwrap_or_default();
        self.ui.open_as(
            crate::page(),
            Spec {
                size: [fill(), fill()],
                fill: Some(theme.paper),
                ..Spec::default()
            },
        );
        self.ui.open(
            "welcome",
            Spec {
                flags: Flags::FLOAT,
                axis: Axis::Y,
                size: [px(SIZE[0]), px(SIZE[1])],
                position: [
                    ((right - left - SIZE[0]) / 2.0).max(0.0),
                    ((bottom - top) * 0.4 - SIZE[1] / 2.0).max(0.0),
                ],
                gap: 4.0,
                ..Spec::default()
            },
        );
        let ink = theme.paper_ink;
        for (part, text, color) in [
            ("title", "No notebooks open", ink),
            (
                "description",
                "Open a notebook folder, or start a new notebook.",
                ui::mix(ink, theme.paper, 0.45),
            ),
        ] {
            self.ui.leaf(
                part,
                Spec {
                    size: [fill(), px(26.0)],
                    text: Some(text),
                    color: Some(color),
                    center: true,
                    ..Spec::default()
                },
            );
        }
        let labels = ["Open Notebook…", "New Notebook…"];
        let pad = 0.75 * theme.font_size;
        let width: f32 = labels
            .iter()
            .map(|label| self.ui.measure(label)[0] + 2.0 * pad)
            .sum::<f32>()
            + 8.0;
        self.ui.open(
            "actions",
            Spec {
                size: [fill(), ui::children()],
                pad: [((SIZE[0] - width) / 2.0).max(0.0), 12.0],
                gap: 8.0,
                ..Spec::default()
            },
        );
        let mut chosen = None;
        for (label, command) in labels
            .into_iter()
            .zip([crate::Command::OpenNotebook, crate::Command::NewNotebook])
        {
            if ui::button(&mut self.ui, label, label).clicked {
                chosen = Some(command);
            }
        }
        self.ui.close();
        self.commands.extend(chosen);
        self.ui.close();
        self.ui.close();
    }
}
