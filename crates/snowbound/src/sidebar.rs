//! OneNote 2010's navigation bar: the open notebooks with their sections and section
//! groups, down the window's left, beside the section tabs.

use crate::{
    art,
    library::Library,
    menus::{Dragged, Target},
    rename, section_color,
};
use notebook::discover::{Folder, Reason, SectionState};
use std::{collections::HashSet, sync::Arc};
use ui::{Axis, Flags, Id, Signal, Spec, Theme, Ui, fill, fit, px};

pub const WIDTH: f32 = 220.0;
/// The square the sidebar's button stands in at the start of the section tabs.
const RAIL: f32 = crate::TAB_ROW;
/// How far below the row's middle the button and the header's title sit: the tabs stand
/// on the row's foot, so their labels sit 1 to 3 pixels low.
const DROP: f32 = 2.0;
const ROW: f32 = 24.0;
/// The tree's margin, and how far inside a row its icon starts and its label after that; the
/// header's title and the notebook button's icon, while the sidebar is open, line up with them.
const MARGIN: f32 = 4.0;
const ROW_PAD: f32 = 6.0;
const ICON: f32 = 16.0;
/// The Back and Forward buttons' box, with room beside them as the notebook button's square has.
pub const NAV: f32 = 2.0 * ui::shell::TOOL + 1.0 + (RAIL - ui::shell::TOOL);
/// How far each level of the tree sits inside its parent.
const INDENT: f32 = 16.0;

/// What the sidebar was asked to do.
pub enum Action {
    /// Opens a notebook's section by catalog path.
    Open {
        notebook: usize,
        path: String,
    },
    /// Folds or unfolds a notebook's or section group's rows, by `Library::key`.
    Fold(String),
    NewNotebook,
    OpenNotebook,
    /// Signs in again to the notebook opened from its server at this location.
    SignIn(String),
    Options,
    /// A row's context menu, opened here.
    Menu(Target, [f32; 2]),
    /// Ends renaming, with the name typed or without.
    Renamed(bool),
    /// Explains why a section or group can't be opened: the alert's title and message.
    Unavailable(String, String),
}

/// What the tree's rows read and what they were asked.
pub struct Tree<'a> {
    theme: &'a Theme,
    /// The open section's notebook index and catalog path.
    open: Option<(usize, &'a str)>,
    folded: &'a HashSet<String>,
    renaming: Option<&'a mut rename::Renaming>,
    action: Option<Action>,
    /// Each section and group row: where it lies, and what it is.
    rows: Vec<Entry>,
    /// The section or group held down, which a drag moves.
    held: Option<Entry>,
    /// The row dragged, which the tree leaves out for a gap where it would land.
    lifted: Option<(&'a Entry, &'a Landing)>,
    /// The dragged row as it was built, for drawing it over the others.
    ghost: Option<Ghost>,
    /// Where the gap the dragged row would land in stands.
    gap: Option<[f32; 4]>,
    /// The notebooks, groups and sections holding unread pages, by `Library::key`.
    unread: HashSet<String>,
}

/// Where a dragged row would land: before a row of a folder or at its end, or into a group.
#[derive(Clone, PartialEq)]
pub struct Landing {
    pub folder: String,
    pub before: Option<String>,
    pub into: bool,
}

/// The dragged row's look, kept to draw it over the others.
struct Ghost {
    label: String,
    icon: Leading,
    depth: u32,
    dim: bool,
    selected: Option<[f32; 4]>,
    bold: bool,
}

impl<'a> Tree<'a> {
    fn new(
        theme: &'a Theme,
        open: Option<(usize, &'a str)>,
        folded: &'a HashSet<String>,
        renaming: Option<&'a mut rename::Renaming>,
    ) -> Self {
        Self {
            theme,
            open,
            folded,
            renaming,
            action: None,
            rows: Vec::new(),
            held: None,
            lifted: None,
            ghost: None,
            gap: None,
            unread: HashSet::new(),
        }
    }
}

/// A section or group row, for dragging one onto or between the others.
#[derive(Clone, PartialEq)]
pub struct Entry {
    pub notebook: usize,
    pub path: String,
    pub group: bool,
    pub rect: [f32; 4],
    /// The row's box, which a drag draws over the others.
    pub id: Id,
}

impl Entry {
    fn folder(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(folder, _)| folder)
    }

    /// Whether this is the row `lifted`, or one inside it.
    fn within(&self, lifted: &Entry) -> bool {
        self.notebook == lifted.notebook
            && (self.path == lifted.path || self.path.starts_with(&format!("{}/", lifted.path)))
    }
}

/// The header, whose empty space drags the window as the tab row's does.
pub fn header() -> Id {
    Id::ROOT.child("sidebar header")
}

/// The sidebar's header row, as tall as the tab row and dragging the window when `drags`, then
/// with `rows` the tree of `notebooks` with the open section marked. The notebook button
/// floats over the header's icon on the left, or its end on the `right`, where the Back and
/// Forward buttons otherwise do.
fn sidebar(
    ui: &mut Ui,
    tree: &mut Tree,
    notebooks: &[Arc<Library>],
    drags: bool,
    rows: bool,
    right: bool,
) {
    let (theme, folded) = (tree.theme, tree.folded);
    ui.open_as(
        self::header(),
        Spec {
            flags: if drags {
                Flags::CLICKABLE
            } else {
                Flags::default()
            },
            size: [fill(), px(RAIL)],
            pad: [MARGIN + ROW_PAD, (RAIL - ui::shell::TOOL) / 2.0 + DROP],
            gap: ROW_PAD,
            ..Spec::default()
        },
    );
    ui.leaf(
        "icon",
        Spec {
            size: [px(ICON), px(ui::shell::TOOL)],
            ..Spec::default()
        },
    );
    ui.leaf(
        "title",
        Spec {
            size: [fill(), px(ui::shell::TOOL)],
            text: Some("My Notebooks"),
            color: Some(theme.text_dim),
            ..Spec::default()
        },
    );
    // Room for what floats over the header's end.
    let end = if right { RAIL } else { NAV + MARGIN };
    ui.leaf(
        "end",
        Spec {
            size: [px(end - MARGIN - 2.0 * ROW_PAD), px(1.0)],
            ..Spec::default()
        },
    );
    ui.close();
    if !rows {
        return;
    }
    ui.open(
        "notebooks",
        Spec {
            flags: Flags::SCROLL | Flags::CLIP,
            axis: Axis::Y,
            size: [fill(), fill()],
            pad: [MARGIN, 2.0],
            ..Spec::default()
        },
    );
    for (index, library) in notebooks.iter().enumerate() {
        let key = library.key("");
        let unfolded = !folded.contains(&key);
        let (row, fold) = tree_row(
            ui,
            tree,
            ui.id(("notebook", index)),
            Row {
                label: &library.name,
                icon: Leading::Notebook(
                    if library.in_icloud() {
                        art::NOTEBOOK_ICLOUD
                    } else {
                        art::NOTEBOOK
                    },
                    library.color(),
                ),
                depth: 0,
                dim: library.notebook.is_err(),
                fold: Some(unfolded),
                selected: None,
                renamed: false,
                bold: tree.unread.contains(&key),
            },
        );
        let unsigned = library.notebook.is_err()
            && crate::library::server_address(&library.location).is_some();
        if row.clicked && unsigned {
            tree.action = Some(Action::SignIn(library.location.clone()));
        } else if row.clicked || fold {
            tree.action = Some(Action::Fold(key));
        }
        if let Some(point) = row.context {
            tree.action = Some(Action::Menu(Target::Notebook(Arc::clone(library)), point));
        }
        if let Some(catalog) = library.catalog()
            && unfolding(ui, ("notebook rows", index), unfolded)
        {
            folder(ui, tree, library, index, catalog, 1);
            ui.close();
            ui.close();
        }
    }
    ui.close();
    ui.open(
        "footer",
        Spec {
            axis: Axis::Y,
            size: [fill(), ui::children()],
            pad: [MARGIN, 6.0],
            ..Spec::default()
        },
    );
    for (part, icon, label, chosen) in [
        (
            "new",
            Leading::Icon(art::PLUS),
            "New Notebook",
            Action::NewNotebook,
        ),
        (
            "open",
            Leading::Notebook(art::NOTEBOOK, None),
            "Open Existing",
            Action::OpenNotebook,
        ),
        (
            "options",
            Leading::Icon(art::OPTIONS),
            "Options",
            Action::Options,
        ),
    ] {
        let (row, _) = tree_row(
            ui,
            tree,
            ui.id(part),
            Row {
                label,
                icon,
                depth: 0,
                dim: false,
                fold: None,
                selected: None,
                renamed: false,
                bold: false,
            },
        );
        if row.clicked {
            tree.action = Some(chosen);
        }
    }
    ui.close();
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
            .is_some_and(|renaming| renaming.entry(library, path, false))
    };
    for section in &folder.sections {
        let (name, color, readable) = match &section.state {
            SectionState::Readable { name, color, .. } => (
                crate::library::section_name(&section.path, name),
                *color,
                true,
            ),
            // Opens on its locked page, which unlocks it.
            SectionState::Locked => (
                crate::library::section_name(&section.path, &None),
                None,
                true,
            ),
            _ => (
                crate::library::section_name(&section.path, &None),
                None,
                false,
            ),
        };
        gap(ui, tree, &folder.path, Some(&section.path));
        let renaming = renamed(tree, &section.path);
        let id = ui.id(("section", notebook, &section.path));
        let row = Row {
            label: &name,
            icon: Leading::Section(section_color(color)),
            depth,
            dim: !readable,
            fold: None,
            selected: (open == Some(section.path.as_str())).then_some(section_color(color)),
            renamed: renaming,
            bold: tree.unread.contains(&library.key(&section.path)),
        };
        if lifts(tree, notebook, &section.path, &row) {
            continue;
        }
        let (row, _) = tree_row(ui, tree, id, row);
        if row.clicked && readable && open != Some(section.path.as_str()) {
            tree.action = Some(Action::Open {
                notebook,
                path: section.path.clone(),
            });
        }
        entry(ui, tree, notebook, &section.path, id, false, &row);
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
        if crate::library::recycle_bin(&group.path) {
            continue;
        }
        let key = library.key(&group.path);
        let unfolded = !tree.folded.contains(&key);
        let name = group.path.rsplit('/').next().unwrap_or_default();
        gap(ui, tree, &folder.path, Some(&group.path));
        let renaming = renamed(tree, &group.path);
        let id = ui.id(("group", notebook, &group.path));
        let row = Row {
            label: name,
            icon: Leading::Icon(art::SECTION_GROUP),
            depth,
            dim: false,
            fold: Some(unfolded),
            selected: None,
            renamed: renaming,
            bold: tree.unread.contains(&key),
        };
        // A dragged group's rows fold away beneath it.
        let lifted = lifts(tree, notebook, &group.path, &row);
        if !lifted {
            let (row, fold) = tree_row(ui, tree, id, row);
            if row.clicked || fold {
                tree.action = Some(Action::Fold(key));
            }
            entry(ui, tree, notebook, &group.path, id, true, &row);
            if let Some(point) = row.context {
                let target = Target::Group {
                    library: Arc::clone(library),
                    path: group.path.clone(),
                };
                tree.action = Some(Action::Menu(target, point));
            }
        }
        if unfolding(
            ui,
            ("group rows", notebook, &group.path),
            unfolded && !lifted,
        ) {
            self::folder(ui, tree, library, notebook, group, depth + 1);
            ui.close();
            ui.close();
        }
    }
    gap(ui, tree, &folder.path, None);
    for entry in &folder.unavailable {
        if crate::library::recycle_bin(&entry.path) {
            continue;
        }
        let name = entry.path.rsplit('/').next().unwrap_or_default();
        let name = if entry.group {
            name.to_owned()
        } else {
            crate::library::section_name(&entry.path, &None)
        };
        let (row, _) = tree_row(
            ui,
            tree,
            ui.id(("unavailable", notebook, &entry.path)),
            Row {
                label: &name,
                icon: match entry.reason {
                    _ if entry.group => Leading::Icon(art::SECTION_GROUP),
                    Reason::Evicted => Leading::Icon(art::ICLOUD),
                    _ => Leading::Section(section_color(None)),
                },
                depth,
                dim: true,
                fold: None,
                selected: None,
                renamed: false,
                bold: false,
            },
        );
        if row.clicked {
            tree.action = Some(match entry.reason {
                Reason::Denied => Action::Unavailable(
                    format!("Can't read \u{201c}{name}\u{201d}"),
                    format!("{}\n\nIt appears here once you have access.", entry.error),
                ),
                Reason::Evicted => Action::Unavailable(
                    format!("Downloading \u{201c}{name}\u{201d}"),
                    "It opens once iCloud Drive brings it to this computer.".into(),
                ),
                _ => Action::Unavailable(
                    format!("Can't open \u{201c}{name}\u{201d}"),
                    format!("{}.", entry.error),
                ),
            });
        }
    }
}

/// Where the row `lifted` lands with the pointer at height `y` among `rows`: onto a
/// group's middle, into it; above or below a row's middle, before or after it in its
/// folder; into another folder, at its end.
fn landing_at(rows: &[Entry], lifted: &Entry, y: f32) -> Option<Landing> {
    let candidates: Vec<&Entry> = rows
        .iter()
        .filter(|row| row.notebook == lifted.notebook && !row.within(lifted))
        .collect();
    let under = candidates
        .iter()
        .find(|row| y >= row.rect[1] && y < row.rect[3])
        .or_else(|| candidates.last().filter(|row| y >= row.rect[3]))?;
    let [top, bottom] = [under.rect[1], under.rect[3]];
    let quarter = (bottom - top) / 4.0;
    if under.group && y > top + quarter && y < bottom - quarter {
        return Some(Landing {
            folder: under.path.clone(),
            before: None,
            into: true,
        });
    }
    let folder = under.folder();
    let before = if y < (top + bottom) / 2.0 {
        Some(under.path.clone())
    } else {
        candidates
            .iter()
            .skip_while(|row| row.path != under.path)
            .skip(1)
            .find(|row| !row.within(under))
            .filter(|row| row.folder() == folder)
            .map(|row| row.path.clone())
    };
    Some(Landing {
        folder: folder.to_owned(),
        before: before.filter(|_| folder == lifted.folder()),
        into: false,
    })
}

/// Whether the row of `path` in `notebook` is the one dragged, which the tree leaves out,
/// keeping its look to draw it over the others.
fn lifts(tree: &mut Tree, notebook: usize, path: &str, row: &Row) -> bool {
    let lifted = tree
        .lifted
        .is_some_and(|(lifted, _)| lifted.notebook == notebook && lifted.path == path);
    if lifted {
        tree.ghost = Some(Ghost {
            label: row.label.to_owned(),
            icon: row.icon,
            depth: row.depth,
            dim: row.dim,
            selected: row.selected,
            bold: row.bold,
        });
    }
    lifted
}

/// The gap before the row of `before` in `folder`, or at its end, which opens where a
/// dragged row would land and closes where it no longer would.
fn gap(ui: &mut Ui, tree: &mut Tree, folder: &str, before: Option<&str>) {
    let Some((_, landing)) = tree.lifted else {
        return;
    };
    let here = !landing.into && landing.folder == folder && landing.before.as_deref() == before;
    let id = ui.id(("gap", before));
    let height = ui.animate(id, if here { ROW } else { 0.0 });
    if here {
        tree.gap = ui.rect(id);
    }
    if height > 0.0 {
        ui.leaf(
            ("gap", before),
            Spec {
                size: [fill(), px(height)],
                ..Spec::default()
            },
        );
    }
}

/// Opens the boxes a notebook's or group's rows ease open and shut in, where they show:
/// the caller builds the rows and closes both. Rows appearing with the tree, as the sidebar
/// opens or a notebook's sections arrive, show as they stand; only a toggle eases.
fn unfolding(ui: &mut Ui, part: impl std::hash::Hash, unfolded: bool) -> bool {
    let outer = ui.id(part);
    let height = if ui.lasted(outer.child("shown"), true).is_zero() {
        if !unfolded {
            return false;
        }
        ui::children()
    } else {
        let rows = ui
            .rect(outer.child("rows"))
            .map_or(0.0, |rect| rect[3] - rect[1]);
        let height = ui.animate(outer, if unfolded { rows } else { 0.0 });
        if !unfolded && height == 0.0 {
            return false;
        }
        px(height)
    };
    ui.open_as(
        outer,
        Spec {
            flags: Flags::CLIP,
            axis: Axis::Y,
            size: [fill(), height],
            ..Spec::default()
        },
    );
    ui.open(
        "rows",
        Spec {
            axis: Axis::Y,
            size: [fill(), ui::children()],
            ..Spec::default()
        },
    );
    true
}

/// Records a section or group row, built as `id`, for dragging.
fn entry(ui: &Ui, tree: &mut Tree, notebook: usize, path: &str, id: Id, group: bool, row: &Signal) {
    let Some(rect) = ui.rect(id) else {
        return;
    };
    let entry = Entry {
        notebook,
        path: path.to_owned(),
        group,
        rect,
        id,
    };
    if row.dragging {
        tree.held = Some(entry.clone());
    }
    tree.rows.push(entry);
}

#[derive(Clone, Copy)]
enum Leading {
    Icon(&'static [&'static str]),
    /// A notebook's glyph, drawn in its colour (COLORREF).
    Notebook(&'static [&'static str], Option<u32>),
    /// A section's tab, drawn in its colour.
    Section([f32; 4]),
}

impl Leading {
    /// The art and the colour its `currentColor` paints in `theme`.
    fn art(self, theme: &Theme) -> (&'static [&'static str], [f32; 4]) {
        match self {
            Self::Icon(icon) => (icon, theme.text),
            Self::Notebook(icon, color) => (icon, crate::notebook_color(theme, color)),
            Self::Section(section) => (art::SECTION, theme.section(section).accent),
        }
    }
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
    /// It holds unread pages, which OneNote sets bold.
    bold: bool,
}

/// One row of the tree: its signal, and whether its fold arrow was clicked.
fn tree_row(ui: &mut Ui, tree: &mut Tree, id: Id, row: Row) -> (Signal, bool) {
    let theme = tree.theme;
    let lit = row
        .selected
        .map(|color| ui::mix(theme.section(color).tab, theme.base, 0.35));
    ui.open_as(
        id,
        Spec {
            flags: Flags::CLICKABLE,
            size: [fill(), px(ROW)],
            fill: lit,
            hover_fill: Some(ui::mix(lit.unwrap_or(theme.sidebar), theme.hover(), 0.6)),
            radius: 4.0,
            pad: [ROW_PAD + INDENT * row.depth as f32, 0.0],
            // The field's text stands where the label did.
            gap: if row.renamed {
                ROW_PAD - rename::PAD
            } else {
                ROW_PAD
            },
            role: Some(accesskit::Role::TreeItem),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(id) {
        node.set_label(row.label);
        node.set_level(row.depth as usize + 1);
        node.set_selected(row.selected.is_some());
        if let Some(unfolded) = row.fold {
            node.set_expanded(unfolded);
        }
    }
    let color = if row.dim { theme.text_dim } else { theme.text };
    let (icon, [red, green, blue, _]) = row.icon.art(theme);
    // Coloured art keeps its colours, so a dim row fades its icon.
    let alpha = if row.dim { 0.5 } else { 1.0 };
    ui.leaf(
        "icon",
        Spec {
            size: [px(ICON), px(ROW)],
            icon: Some(icon),
            color: Some([red, green, blue, alpha]),
            ..Spec::default()
        },
    );
    match (row.renamed, tree.renaming.as_deref_mut()) {
        (true, Some(renaming)) => {
            if let Some(keep) = rename::edit(ui, theme, &mut renaming.name, ROW - 4.0) {
                tree.action = Some(Action::Renamed(keep));
            }
        }
        _ => {
            ui.leaf(
                "label",
                Spec {
                    size: [fill(), px(ROW)],
                    text: Some(row.label),
                    bold: row.bold,
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
                role: Some(accesskit::Role::Button),
                ..Spec::default()
            },
        )
        .clicked
    });
    if let Some(unfolded) = row.fold
        && let Some(node) = ui.access(id.child("fold"))
    {
        node.set_label(if unfolded { "Collapse" } else { "Expand" });
    }
    ui.close();
    (ui.signal(id), folded)
}

impl crate::State {
    /// How wide the sidebar stands this frame as it eases open and shut.
    pub(crate) fn sidebar_width(&mut self) -> f32 {
        if self.temporary {
            return 0.0;
        }
        self.ui.animate(
            self.ui.id("sidebar"),
            if self.sidebar && !self.full_page {
                WIDTH
            } else {
                0.0
            },
        )
    }

    /// The sidebar `width` wide beside the section tabs, on the left, or the right as
    /// OneNote's "Navigation bar appears on the left" turned off puts it.
    pub(crate) fn sidebar(&mut self, theme: &Theme, width: f32) {
        if self.temporary {
            return;
        }
        self.ui.open(
            "sidebar",
            Spec {
                flags: Flags::CLIP,
                size: [px(width), fill()],
                fill: Some(theme.sidebar),
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
                role: Some(accesskit::Role::Tree),
                ..Spec::default()
            },
        );
        crate::name(&mut self.ui, rows_id, "My Notebooks");
        let shown = match (&self.session, &self.locked) {
            (Some(session), _) => Some((&session.library, session.tabs[session.tab].path.as_str())),
            (None, Some(locked)) => Some((&locked.library, locked.path.as_str())),
            (None, None) => None,
        };
        let open = shown.and_then(|(shown, path)| {
            let index = self
                .notebooks
                .iter()
                .position(|library| Arc::ptr_eq(library, shown))?;
            Some((index, path))
        });
        let drags = self.chrome_drags();
        let unread = self.unread_keys();
        let mut tree = Tree::new(theme, open, &self.folded, self.renaming.as_mut());
        tree.unread = unread;
        tree.lifted = self
            .drag
            .as_ref()
            .filter(|drag| drag.lifted)
            .and_then(|drag| match &drag.what {
                Dragged::Entry { entry, landing, .. } => Some((entry, landing)),
                _ => None,
            });
        sidebar(
            &mut self.ui,
            &mut tree,
            &self.notebooks,
            drags,
            // Opening, the rows are built at once, so a rename field there takes the focus.
            width > 0.5 || self.sidebar && !self.full_page,
            self.navigation_bar_right,
        );
        let Tree {
            action,
            rows,
            mut held,
            ghost,
            gap,
            ..
        } = tree;
        let corner = self.ui.rect(rows_id).unwrap_or_default();
        let found = ghost.is_some();
        let (ghost_held, settled) = self.lifted_row(theme, ghost, gap, &rows, corner);
        held = held.or(ghost_held);
        self.drag_entries(held, &rows, gap, settled || !found, corner, theme);
        self.ui.close();
        self.ui.close();
        match action {
            Some(Action::Fold(key)) => {
                if !self.folded.remove(&key) {
                    self.folded.insert(key);
                }
            }
            Some(Action::Open { notebook, path }) if !self.dragged() => {
                let library = Arc::clone(&self.notebooks[notebook]);
                self.commands
                    .push(crate::Command::OpenSection(library, path));
            }
            Some(Action::Menu(target, point)) => {
                self.menu = Some((target, point));
                self.ui.open_popup(crate::menus::id());
            }
            Some(Action::Renamed(keep)) => self.finish_renaming(keep),
            Some(Action::NewNotebook) => self.commands.push(crate::Command::NewNotebook),
            Some(Action::OpenNotebook) => self.commands.push(crate::Command::OpenNotebook),
            Some(Action::SignIn(location)) => self
                .commands
                .push(crate::Command::OpenFromServer(Some(location))),
            Some(Action::Options) => self.open_options(),
            Some(Action::Unavailable(title, message)) => crate::platform::alert(&title, &message),
            Some(Action::Open { .. }) | None => {}
        }
    }

    /// Back, Forward and the notebook button, floating at the body's corner over the section
    /// tabs' row, `height` tall as it eases. While the sidebar, `width` wide as it eases, is
    /// open on the left, the notebook button closes it from its header's icon, and Back and
    /// Forward ride its end.
    pub(crate) fn sidebar_button(&mut self, height: f32, width: f32) {
        use crate::commands::{Choice, Id as Cmd};
        if self.temporary {
            return;
        }
        let (nav, toggle) = if self.navigation_bar_right {
            // The notebook button stands at the body's far edge, as laid out last frame.
            let edge = self
                .ui
                .rect(self.ui.current())
                .map_or(0.0, |[left, _, right, _]| right - left - RAIL);
            (0.0, edge)
        } else {
            let open = MARGIN + ROW_PAD - (RAIL - ICON) / 2.0;
            let toggle = NAV + (open - NAV) * width / WIDTH;
            ((width - NAV - MARGIN).max(0.0), toggle)
        };
        let pad = [
            (RAIL - ui::shell::TOOL) / 2.0,
            (RAIL - ui::shell::TOOL) / 2.0 + DROP,
        ];
        self.ui.open(
            "back and forward",
            Spec {
                flags: Flags::FLOAT | Flags::CLIP,
                size: [px(NAV), px(height)],
                position: [nav, 0.0],
                pad,
                gap: 1.0,
                ..Spec::default()
            },
        );
        let format = self.format_state();
        for id in [Cmd::Back, Cmd::Forward] {
            let status = self.status(&Choice::Command(id), &format);
            if let Some(choice) = crate::tool(&mut self.ui, id, status) {
                self.choose(choice);
            }
        }
        self.ui.close();
        self.ui.open(
            "toggle",
            Spec {
                flags: Flags::FLOAT | Flags::CLIP,
                size: [px(RAIL), px(height)],
                position: [toggle, 0.0],
                pad,
                ..Spec::default()
            },
        );
        let (icon, tint) = if self.sidebar {
            (art::CLOSE, self.ui.theme.text)
        } else {
            let color = self.notebook().and_then(|library| library.color());
            (art::NOTEBOOK, crate::notebook_color(&self.ui.theme, color))
        };
        if ui::shell::tool_button(&mut self.ui, "button", icon, tint, None).clicked {
            self.sidebar = !self.sidebar;
            self.save_settings();
        }
        crate::tip(&mut self.ui, Cmd::Sidebar);
        self.ui.close();
    }

    /// The dragged row, `ghost`, drawn over the others in the rows' box `corner`: following
    /// the pointer, or once let go easing into the `gap` it lands in, or onto the group it
    /// lands in among `rows`. Returns it while held, and whether it stands in its place.
    fn lifted_row(
        &mut self,
        theme: &Theme,
        ghost: Option<Ghost>,
        gap: Option<[f32; 4]>,
        rows: &[Entry],
        corner: [f32; 4],
    ) -> (Option<Entry>, bool) {
        let (
            Some(ghost),
            Some(
                drag @ crate::menus::Drag {
                    what: Dragged::Entry { entry, landing, .. },
                    ..
                },
            ),
        ) = (ghost, &self.drag)
        else {
            return (None, true);
        };
        let key = self.ui.id("lifted");
        let (top, settled) = if drag.live() {
            let top = drag.corner(self.pointer)[1] - corner[1];
            (self.ui.hold(key, top), false)
        } else {
            let place = gap
                .or_else(|| {
                    rows.iter()
                        .find(|row| landing.into && row.path == landing.folder)
                        .map(|row| row.rect)
                })
                .unwrap_or(entry.rect);
            let target = place[1] - corner[1];
            let top = self.ui.animate(key, target);
            (top, (top - target).abs() < 0.5)
        };
        let entry = entry.clone();
        self.ui.open(
            "lifted",
            Spec {
                flags: Flags::FLOAT,
                size: [px(entry.rect[2] - entry.rect[0]), px(ROW)],
                position: [entry.rect[0] - corner[0], top],
                fill: Some(theme.base),
                shadow: Some([0.0, 0.0, 0.0, 0.3]),
                radius: 4.0,
                ..Spec::default()
            },
        );
        let mut tree = Tree::new(theme, None, &self.folded, None);
        let (row, _) = tree_row(
            &mut self.ui,
            &mut tree,
            entry.id,
            Row {
                label: &ghost.label,
                icon: ghost.icon,
                depth: ghost.depth,
                dim: ghost.dim,
                fold: None,
                selected: ghost.selected,
                renamed: false,
                bold: ghost.bold,
            },
        );
        self.ui.close();
        (row.dragging.then_some(entry), settled)
    }

    /// Moves a section or group by dragging its row, `held`, among `rows` in the rows' box
    /// `corner`: the others open a `gap` where it would land. Onto a group's middle it moves
    /// into the group, as OneNote moves one dropped on a group's tab; between rows of its
    /// folder it takes that place; into another folder it moves there, last. The drag ends
    /// once the row, let go, is `settled`.
    fn drag_entries(
        &mut self,
        held: Option<Entry>,
        rows: &[Entry],
        gap: Option<[f32; 4]>,
        settled: bool,
        corner: [f32; 4],
        theme: &Theme,
    ) {
        let mine = |what: &Dragged| matches!(what, Dragged::Entry { .. });
        self.settle(mine, settled);
        let held = held.map(|entry| {
            let rect = entry.rect;
            let home = Landing {
                folder: entry.folder().to_owned(),
                before: rows
                    .iter()
                    .skip_while(|row| row.path != entry.path)
                    .skip(1)
                    .find(|row| !row.within(&entry) && row.notebook == entry.notebook)
                    .filter(|row| row.folder() == entry.folder())
                    .map(|row| row.path.clone()),
                into: false,
            };
            let what = Dragged::Entry {
                landing: home.clone(),
                home,
                entry,
            };
            (what, rect)
        });
        let dropped = self.follow_drag(held, mine);
        let pointer = self.pointer;
        let Some(
            drag @ crate::menus::Drag {
                what: Dragged::Entry { .. },
                ..
            },
        ) = &mut self.drag
        else {
            return;
        };
        let live = drag.live();
        let middle = drag.corner(pointer)[1] + ROW / 2.0;
        let Dragged::Entry {
            entry,
            landing,
            home,
        } = &mut drag.what
        else {
            return;
        };
        if drag.cancelled {
            *landing = home.clone();
        } else if live {
            // Where the rows stand with no gap open, so the gap follows the dragged row
            // rather than the rows it pushed aside.
            let closed: Vec<Entry> = rows
                .iter()
                .map(|row| {
                    let mut row = row.clone();
                    if let Some([_, top, _, bottom]) = gap
                        && row.rect[1] >= bottom - 0.5
                    {
                        row.rect[1] -= bottom - top;
                        row.rect[3] -= bottom - top;
                    }
                    row
                })
                .collect();
            if let Some(found) = landing_at(&closed, entry, middle) {
                *landing = found;
            }
        }
        if live && landing.into {
            let accent = theme.accent;
            if let Some(group) = rows.iter().find(|row| row.path == landing.folder) {
                let [left, top, right, bottom] = group.rect;
                self.ui.mark(
                    [
                        left - corner[0],
                        top - corner[1],
                        right - corner[0],
                        bottom - corner[1],
                    ],
                    [accent[0], accent[1], accent[2], 0.3],
                    4.0,
                );
            }
        }
        if !dropped || landing == home {
            return;
        }
        let change = if landing.into || landing.folder != entry.folder() {
            crate::manage::Structure::Move {
                path: entry.path.clone(),
                folder: landing.folder.clone(),
            }
        } else {
            // The folder's order with the dragged entry where it was dropped.
            let mut paths: Vec<String> = rows
                .iter()
                .filter(|row| row.notebook == entry.notebook && row.folder() == landing.folder)
                .filter(|row| !row.within(entry))
                .map(|row| row.path.clone())
                .collect();
            let at = landing
                .before
                .as_ref()
                .and_then(|before| paths.iter().position(|path| path == before))
                .unwrap_or(paths.len());
            paths.insert(at, entry.path.clone());
            crate::manage::Structure::Reorder {
                folder: landing.folder.clone(),
                paths,
            }
        };
        let library = Arc::clone(&self.notebooks[entry.notebook]);
        self.commands
            .push(crate::Command::Structure(library, change));
    }

    /// Opens what the open panel chose: a notebook folder, a notebook's table of contents,
    /// or a section, in its notebook where it has one.
    pub(crate) fn open_path(&mut self, path: &std::path::Path) {
        match crate::library::locate(path) {
            crate::library::Located::Notebook { root, section } => {
                self.open_notebook(root.to_string_lossy().into_owned(), section)
            }
            crate::library::Located::Section(file) => {
                let library = Arc::new(Library::section(&file, &self.cache));
                let path = library.location.clone();
                self.commands
                    .push(crate::Command::OpenSection(library, path));
            }
            crate::library::Located::Package(package) => self.open_unpack(&package),
            crate::library::Located::Nothing => crate::platform::alert(
                "Couldn't open",
                "Choose a notebook folder, its Open Notebook file, a section file or a package.",
            ),
        }
    }

    /// Shows the notebook at `location` at `section`, or its first section, opening it
    /// unless it is open.
    pub(crate) fn open_notebook(&mut self, location: String, section: Option<String>) {
        self.open_notebook_with(location, section, |location, cache| {
            Ok(Library::notebook(location, cache))
        });
    }

    /// Shows the notebook at `location` as `open_notebook` does, reading it with `read` unless
    /// it is open and readable.
    pub(crate) fn open_notebook_with(
        &mut self,
        location: String,
        section: Option<String>,
        read: impl FnOnce(&str, &std::path::Path) -> Result<Library, String> + Send + 'static,
    ) {
        let open = self
            .notebooks
            .iter()
            .find(|library| library.location == location && library.notebook.is_ok())
            .cloned();
        // The notebook shown already shows a section, which only one reader may hold.
        if let Some(session) = &self.session
            && session.library.location == location
            && section
                .as_ref()
                .is_none_or(|section| *section == session.tabs[session.tab].path)
        {
            return;
        }
        self.read_notebook(location, section, open, read);
    }

    /// Shows `section`, or where it was left, or the first section, of the notebook at
    /// `location`, `open` or read with `read`, in place of the notebook listed there.
    pub(crate) fn read_notebook(
        &mut self,
        location: String,
        section: Option<String>,
        open: Option<Arc<Library>>,
        read: impl FnOnce(&str, &std::path::Path) -> Result<Library, String> + Send + 'static,
    ) {
        let section = section.or_else(|| {
            (self.trail.recent.iter())
                .find(|place| place.notebook == location)
                .map(|place| place.section.clone())
        });
        let left = section
            .as_ref()
            .and_then(|path| self.last_pages.get(&crate::library::key(&location, path)))
            .copied();
        let (cache, notify) = (self.cache.clone(), crate::notify(self.proxy.clone()));
        self.load(move || {
            let library = match open {
                Some(library) => library,
                None => {
                    let library = Arc::new(read(&location, &cache)?);
                    library.purge_recycle_bin();
                    library
                }
            };
            if let Err(error) = &library.notebook {
                return Err(error.clone().into());
            }
            let Some((path, left)) = section
                .filter(|path| library.contains(path))
                .map(|path| (path, left))
                .or_else(|| Some((library.first_section()?, None)))
            else {
                // Shown once its sections arrive.
                if library.downloading() {
                    return Ok(crate::Loaded::Library(library, None));
                }
                return Err("This folder holds no notebook sections.".into());
            };
            let section = library.open(&path, notify)?;
            let (session, page) = crate::read_session(section, library, path, left)?;
            Ok(crate::Loaded::Section(Box::new(session), page))
        });
    }

    /// The window below the title bar on the first run, or once every notebook is closed:
    /// what is missing, and the ways to start.
    pub(crate) fn welcome(&mut self) {
        let id = self.ui.id("welcome");
        #[cfg(target_os = "linux")]
        let install = crate::desktop::installable().then_some((
            "install",
            Leading::Icon(art::PLUS),
            "Install Snowbound",
            crate::Command::Install,
        ));
        #[cfg(not(target_os = "linux"))]
        let install = None;
        let servers = self.servers.clone();
        self.notice(
            id,
            "No notebooks open",
            &servers,
            [
                (
                    "new",
                    Leading::Icon(art::PLUS),
                    "New Notebook",
                    crate::Command::NewNotebook,
                ),
                (
                    "open",
                    Leading::Notebook(art::NOTEBOOK, None),
                    "Open Existing",
                    crate::Command::OpenNotebook,
                ),
                (
                    "server",
                    Leading::Icon(art::SERVER),
                    "Open Notebook from Server…",
                    crate::Command::OpenFromServer(None),
                ),
                #[cfg(feature = "live")]
                (
                    "shared",
                    Leading::Icon(art::LINK),
                    "Open Shared Notebook…",
                    crate::Command::OpenShared,
                ),
            ]
            .into_iter()
            .chain(crate::guide::OFFERED.then_some((
                "guide",
                Leading::Icon(art::PAGE),
                "Open the Snowbound Guide",
                crate::Command::OpenGuide,
            )))
            .chain(install)
            .collect(),
        );
    }

    /// The page's place while notebook `library` has no sections, as OneNote 2010 shows
    /// it, with a button to add one; or while none is on this computer yet.
    pub(crate) fn no_sections(&mut self, library: Arc<Library>) {
        let downloading = library.downloading();
        let renaming = self.folder_renaming(&library.location);
        let new = crate::Command::Structure(
            library,
            crate::manage::Structure::NewSection {
                folder: String::new(),
            },
        );
        let id = self.ui.id("no sections");
        let (title, buttons) = if renaming {
            ("Renaming the folder…", Vec::new())
        } else if downloading {
            ("Downloading from iCloud Drive…", Vec::new())
        } else {
            (
                "No sections in this notebook",
                vec![("new", Leading::Icon(art::PLUS), "New Section", new)],
            )
        };
        self.notice(id, title, &[], buttons);
    }

    /// A box `id` in the page's colours, showing `title` above `buttons` in its middle: each
    /// a part, an icon, a label and what it does; then the saved `servers` to reconnect to.
    fn notice(
        &mut self,
        id: ui::Id,
        title: &str,
        servers: &[String],
        buttons: Vec<(&str, Leading, &str, crate::Command)>,
    ) {
        const BUTTON: [f32; 2] = [240.0, 32.0];
        let theme = self.page_area_theme();
        let chrome = std::mem::replace(&mut self.ui.theme, theme.clone());
        let [left, top, right, bottom] = self.ui.rect(id).unwrap_or_default();
        self.ui.open_as(
            id,
            Spec {
                size: [fill(), fill()],
                fill: Some(theme.paper),
                ..Spec::default()
            },
        );
        self.ui.open(
            "content",
            Spec {
                flags: Flags::FLOAT,
                axis: Axis::Y,
                size: [px(BUTTON[0]), ui::children()],
                position: [
                    ((right - left - BUTTON[0]) / 2.0).max(0.0),
                    ((bottom - top) * 0.4 - 60.0).max(0.0),
                ],
                gap: 10.0,
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "title",
            Spec {
                size: [fill(), px(28.0)],
                text: Some(title),
                center: true,
                ..Spec::default()
            },
        );
        let mut chosen = None;
        for (part, icon, label, command) in buttons {
            let (icon, tint) = icon.art(&theme);
            let button = self.ui.leaf(
                part,
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fill(), px(BUTTON[1])],
                    icon: Some(icon),
                    tint: Some(tint),
                    text: Some(label),
                    fill: Some(theme.chip),
                    hover_fill: Some(theme.hover()),
                    hover_border: Some(theme.accent),
                    radius: 6.0,
                    center: true,
                    role: Some(accesskit::Role::Button),
                    ..Spec::default()
                },
            );
            if button.clicked {
                chosen = Some(command);
            }
        }
        self.commands.extend(chosen);
        let saved = crate::server::saved_servers(&mut self.ui, servers);
        self.ui.close();
        self.ui.close();
        self.ui.theme = chrome;
        if let Some(saved) = saved {
            self.saved_server(saved);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, group: bool, top: f32) -> Entry {
        Entry {
            notebook: 0,
            path: path.to_owned(),
            group,
            rect: [0.0, top, 200.0, top + ROW],
            id: Id::ROOT.child(path),
        }
    }

    /// A dragged row lands before or after a row by its middle, into a group by the group's
    /// middle half, and at the end of another folder.
    #[test]
    fn a_dragged_row_lands_between_rows_into_groups_and_last_in_other_folders() {
        let rows = [
            row("A.one", false, 0.0),
            row("B.one", false, 24.0),
            row("G", true, 48.0),
            row("G/C.one", false, 72.0),
        ];
        let lifted = row("Z.one", false, 200.0);
        let landing = |y| landing_at(&rows, &lifted, y).unwrap();
        let at = |folder: &str, before: Option<&str>, into| Landing {
            folder: folder.to_owned(),
            before: before.map(str::to_owned),
            into,
        };
        assert!(landing(4.0) == at("", Some("A.one"), false));
        assert!(landing(20.0) == at("", Some("B.one"), false));
        assert!(landing(60.0) == at("G", None, true));
        assert!(landing(50.0) == at("", Some("G"), false));
        assert!(landing(80.0) == at("G", None, false));
        assert!(landing(500.0) == at("G", None, false));
    }
}
