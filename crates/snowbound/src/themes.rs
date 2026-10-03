//! Style themes in the app (`resources/styles.md`): the page wears its own theme, else its
//! section's, else its notebook's, as its paragraph style objects; the Styles gallery
//! applies a style in the page's theme, and its Customize… opens the Themes dialog, which
//! edits a notebook's own themes with a live preview and gives one to the page, the section
//! or the notebook.

use crate::{Library, State, filetime};
use accesskit::Role;
use notebook::sidecar::themes::{
    self as stored, Assignment, STYLES, Theme, ThemeStyle, Themes, built_in,
};
use onestore::page::Definition;
use std::{error::Error, sync::Arc};
use ui::{Anchor, Axis, Flags, Id, Spec, Ui, children, fill, popup::Item, px};

/// What a theme is chosen for, from where the page is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Page,
    Section,
    Notebook,
}

impl Scope {
    fn name(self) -> &'static str {
        match self {
            Self::Page => "This Page",
            Self::Section => "This Section",
            Self::Notebook => "This Notebook",
        }
    }
}

/// The Themes dialog while it is open.
pub struct Dialog {
    library: Arc<Library>,
    /// Where Save can give the theme, asked with the first offered first; one alone takes it.
    targets: Vec<(Scope, stored::Scope)>,
    /// The built-in themes, then the notebook's own as the dialog edits them.
    themes: Vec<Theme>,
    /// The notebook's own themes changed here, by id.
    changed: Vec<String>,
    /// The notebook's own themes deleted here.
    deleted: Vec<Theme>,
    selected: usize,
    /// The style the controls edit, by its place in the gallery.
    style: usize,
    /// The colour of the section the theme is chosen for, which "Theme" colours take.
    section: Option<u32>,
}

fn id() -> Id {
    Id::ROOT.child("themes")
}

fn popup(name: &str) -> Id {
    id().child(name)
}

/// Spacing the dialog offers above and below a style, in points.
const SPACING: [f32; 9] = [0.0, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0, 12.0, 18.0];

/// The largest a style's text shows in the Styles gallery, in pixels.
const GALLERY_LARGEST: f32 = 24.0;

/// The pixel size `preview` shows `style` at: as a page shows it, up to `largest`.
fn shown_size(style: &ThemeStyle, largest: f32) -> f32 {
    (style.size * 1.2).clamp(10.0, largest)
}

/// A line of `text` in `style` as a page in a section coloured `section` shows it, scaled
/// down to fit a menu row up to `largest` pixels, its colour moved onto the popup's paper so
/// it reads in either scheme.
pub(crate) fn preview(
    ui: &mut Ui,
    part: &str,
    text: &str,
    style: &ThemeStyle,
    section: Option<u32>,
    largest: f32,
) {
    let theme = ui.theme.clone();
    let paper = canvas::gpu::Paper {
        color: theme.popup,
        ink: theme.text,
    };
    let color = style
        .colorref(section)
        .map_or(theme.text, |color| paper.tint(canvas::gpu::colorref(color)));
    ui.leaf(
        part,
        Spec {
            flags: Flags::CLIP,
            size: [fill(), fill()],
            text: Some(text),
            font: Some(&style.font),
            font_size: Some(shown_size(style, largest)),
            bold: style.bold,
            italic: style.italic,
            color: Some(color),
            ..Spec::default()
        },
    );
}

impl State {
    /// Where a theme for `scope` goes from the open page: its notebook and the scope's key.
    pub(crate) fn theme_target(&self, scope: Scope) -> Option<(Arc<Library>, stored::Scope)> {
        let session = self.session.as_ref()?;
        let library = &session.library;
        library.catalog()?;
        let target = match scope {
            Scope::Page => stored::Scope::page(self.view.editor.identity()?),
            Scope::Section => {
                stored::Scope::section(library.section_identity(&session.tabs[session.tab].path)?)
            }
            Scope::Notebook => stored::Scope::Notebook,
        };
        Some((Arc::clone(library), target))
    }

    /// The colour of the open section, which "Theme" colours take.
    pub(crate) fn section_color(&self) -> Option<u32> {
        let session = self.session.as_ref()?;
        session.tabs.get(session.tab)?.color
    }

    /// The theme the open page wears, if any scope names one.
    pub(crate) fn page_theme(&self) -> Option<Theme> {
        let session = self.session.as_ref()?;
        let section = session
            .library
            .section_identity(&session.tabs[session.tab].path);
        session
            .library
            .themes()
            .effective(section, self.view.editor.identity())
    }

    /// The styles the gallery offers: the page's theme's, else OneNote 2010's.
    pub(crate) fn gallery_sheet(&self) -> Theme {
        self.page_theme()
            .unwrap_or_else(|| built_in().swap_remove(0))
    }

    /// Stored style `name` as the gallery applies it on the open page.
    pub(crate) fn gallery_style(&self, name: &str) -> Definition {
        let theme = self.gallery_sheet();
        stored::definition(name, &theme.styles[name], self.section_color())
    }

    /// Dresses the open page in its theme: new text and Enter take the theme's styles, and
    /// style objects the theme gives otherwise are restyled, as one edit. A page with no theme
    /// keeps what it holds.
    pub(crate) fn wear_theme(&mut self) -> Result<(), Box<dyn Error>> {
        let theme = self.page_theme();
        let sheet = (theme.as_ref())
            .map(|theme| theme.sheet(self.section_color()))
            .unwrap_or_default();
        self.view.editor.styles = sheet.clone();
        let Some(session) = &self.session else {
            return Ok(());
        };
        if theme.is_none() || session.read_only() {
            return Ok(());
        }
        let page = session.section.page(session.space)?;
        let ops = onestore::op::restyle(&page, &sheet)?;
        if ops.is_empty() {
            return Ok(());
        }
        self.persist()?;
        self.edit_page(ops)
    }

    /// Opens the Themes dialog on the open page's theme, saving to the page, its section or
    /// its notebook.
    pub(crate) fn open_themes(&mut self) {
        let targets: Vec<(Arc<Library>, (Scope, stored::Scope))> =
            [Scope::Page, Scope::Section, Scope::Notebook]
                .into_iter()
                .filter_map(|scope| {
                    let (library, target) = self.theme_target(scope)?;
                    Some((library, (scope, target)))
                })
                .collect();
        let Some((library, _)) = targets.first() else {
            return;
        };
        let library = Arc::clone(library);
        let (wearing, section) = (self.page_theme(), self.section_color());
        let targets = targets.into_iter().map(|(_, target)| target).collect();
        self.show_themes(library, targets, wearing, section);
    }

    /// Opens the Themes dialog on theme `wearing`, saving to `targets` of `library`, in a
    /// section coloured `section`.
    pub(crate) fn show_themes(
        &mut self,
        library: Arc<Library>,
        targets: Vec<(Scope, stored::Scope)>,
        wearing: Option<Theme>,
        section: Option<u32>,
    ) {
        let mut all = library.themes().all();
        // A retired built-in worn here is listed while it is.
        if let Some(wearing) = &wearing
            && !all.contains(wearing)
        {
            all.push(wearing.clone());
        }
        let selected = wearing
            .and_then(|wearing| all.iter().position(|theme| theme.id == wearing.id))
            .unwrap_or(0);
        self.themes = Some(Dialog {
            library,
            targets,
            themes: all,
            changed: Vec::new(),
            deleted: Vec::new(),
            selected,
            style: 0,
            section,
        });
        self.ui.open_popup(id());
    }

    /// Builds the Themes dialog while it is open. Save keeps the edits and gives the theme to
    /// the scope it then asks for, This Page first; No Theme takes that scope's own away;
    /// Cancel, Escape or a press outside leave everything as it was.
    pub(crate) fn themes_dialog(&mut self) {
        let Some(dialog) = &mut self.themes else {
            return;
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.themes = None;
            return;
        }
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(760.0), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [10.0, 10.0],
                gap: 8.0,
                anchor: Some(Anchor::Dialog),
                role: Some(Role::Dialog),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id()) {
            node.set_label("Themes");
        }
        let title = match &dialog.targets[..] {
            [(scope, _)] => format!("Theme for {}", scope.name()),
            _ => "Themes".to_owned(),
        };
        ui.leaf(
            "title",
            Spec {
                size: [fill(), px(row)],
                text: Some(&title),
                bold: true,
                role: Some(Role::Heading),
                ..Spec::default()
            },
        );
        ui.open(
            "body",
            Spec {
                size: [fill(), px(420.0)],
                gap: 10.0,
                ..Spec::default()
            },
        );
        // The themes, built-in first.
        ui.open(
            "list",
            Spec {
                axis: Axis::Y,
                size: [px(170.0), fill()],
                fill: Some(theme.panel),
                radius: 5.0,
                pad: [6.0, 6.0],
                gap: 2.0,
                role: Some(Role::ListBox),
                ..Spec::default()
            },
        );
        for (index, listed) in dialog.themes.iter().enumerate() {
            let shown = index == dialog.selected;
            let item = ui.open(
                ("theme", index),
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fill(), px(row)],
                    fill: shown.then(|| theme.hover()),
                    hover_fill: Some(theme.hover()),
                    radius: 4.0,
                    pad: [8.0, 0.0],
                    role: Some(Role::ListBoxOption),
                    ..Spec::default()
                },
            );
            preview(
                ui,
                "name",
                &listed.name,
                &listed.styles["h2"],
                dialog.section,
                15.0,
            );
            if let Some(node) = ui.access(item) {
                node.set_label(listed.name.as_str());
                node.set_selected(shown);
            }
            ui.close();
            if ui.signal(item).clicked {
                dialog.selected = index;
            }
        }
        ui.leaf(
            "space",
            Spec {
                size: [fill(), fill()],
                ..Spec::default()
            },
        );
        ui.open(
            "list buttons",
            Spec {
                size: [fill(), children()],
                gap: 6.0,
                ..Spec::default()
            },
        );
        let built = stored::is_built_in(&dialog.themes[dialog.selected].id);
        let duplicate = ui::button(ui, "duplicate", "Duplicate").clicked;
        let delete = !built && ui::button(ui, "delete", "Delete").clicked;
        ui.close();
        ui.close();
        // The selected theme's styles.
        ui.open(
            "editor",
            Spec {
                axis: Axis::Y,
                size: [px(270.0), fill()],
                gap: 6.0,
                ..Spec::default()
            },
        );
        if built {
            ui.leaf(
                "locked",
                Spec {
                    size: [fill(), px(row * 1.5)],
                    text: Some("Changing a built-in theme edits a copy."),
                    color: Some(theme.text_dim),
                    ..Spec::default()
                },
            );
        } else {
            let edited = &mut dialog.themes[dialog.selected];
            let before = edited.name.clone();
            ui::text_field(
                ui,
                popup("name"),
                &mut edited.name,
                "Theme name",
                Spec {
                    size: [fill(), px(row)],
                    fill: Some(theme.base),
                    border: Some(theme.chip),
                    radius: 4.0,
                    pad: [6.0, 0.0],
                    ..Spec::default()
                },
            );
            if edited.name != before && !dialog.changed.contains(&edited.id) {
                dialog.changed.push(edited.id.clone());
            }
        }
        // Each control edits the style shown; a built-in theme is copied first.
        let mut change: Option<Change> = None;
        let shown = dialog.themes[dialog.selected].clone();
        let (name, label) = STYLES[dialog.style];
        let style = &shown.styles[name];
        let field = |ui: &mut Ui, label: &str, build: &mut dyn FnMut(&mut Ui)| {
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
                    size: [px(64.0), px(row)],
                    text: Some(label),
                    ..Spec::default()
                },
            );
            build(ui);
            ui.close();
        };
        field(ui, "Style:", &mut |ui| {
            let combo = ui.id("combo");
            ui::shell::combo(ui, "combo", "Style", label, 180.0, popup("styles"), true);
            let items: Vec<Item> = STYLES
                .iter()
                .enumerate()
                .map(|(at, (_, label))| Item {
                    text: label,
                    checked: Some(at == dialog.style),
                    current: at == dialog.style,
                    ..Item::default()
                })
                .collect();
            let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
            if let Some(index) = ui::popup::menu(ui, popup("styles"), anchor, &items, None) {
                dialog.style = index;
            }
        });
        let fonts = &self.fonts;
        field(ui, "Font:", &mut |ui| {
            let combo = ui.id("combo");
            ui::shell::combo(
                ui,
                "combo",
                "Font",
                &style.font,
                180.0,
                popup("fonts"),
                true,
            );
            let mut names: Vec<&str> = crate::FONTS.to_vec();
            names.push("Georgia");
            names.extend(fonts.iter().map(String::as_str));
            let items: Vec<Item> = names
                .iter()
                .map(|font| Item {
                    text: font,
                    font: Some(font),
                    current: *font == style.font,
                    ..Item::default()
                })
                .collect();
            let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
            if let Some(index) = ui::popup::menu(ui, popup("fonts"), anchor, &items, Some("Font")) {
                let font = names[index].to_owned();
                change = Some(Change::Font(font));
            }
        });
        field(ui, "Size:", &mut |ui| {
            let combo = ui.id("combo");
            let size = format!("{}", style.size);
            ui::shell::combo(ui, "combo", "Size", &size, 60.0, popup("sizes"), true);
            let labels: Vec<String> = crate::SIZES.iter().map(|size| format!("{size}")).collect();
            let items: Vec<Item> = labels
                .iter()
                .map(|label| Item {
                    text: label,
                    checked: Some(*label == size),
                    current: *label == size,
                    ..Item::default()
                })
                .collect();
            let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
            if let Some(index) = ui::popup::menu(ui, popup("sizes"), anchor, &items, None) {
                let size = crate::SIZES[index];
                change = Some(Change::Size(size));
            }
        });
        field(ui, "", &mut |ui| {
            if ui::check_box(ui, "bold", "Bold", style.bold).clicked {
                change = Some(Change::Bold);
            }
            if ui::check_box(ui, "italic", "Italic", style.italic).clicked {
                change = Some(Change::Italic);
            }
        });
        field(ui, "Color:", &mut |ui| {
            let button = ui.id("color");
            let shown = match (&style.color, style.accent) {
                (_, true) => "Theme",
                (Some(color), false) => color,
                (None, false) => "Automatic",
            };
            ui::shell::combo(ui, "color", "Color", shown, 120.0, popup("colors"), true);
            let swatches: Vec<_> = crate::FONT_COLORS
                .iter()
                .map(|&(color, name)| (canvas::gpu::colorref(color), name))
                .collect();
            let anchor = Anchor::Below(ui.rect(button).unwrap_or_default());
            let buttons = ["Automatic", "Theme"];
            if let Some(picked) =
                ui::popup::color_grid(ui, popup("colors"), anchor, &buttons, &swatches, 10)
            {
                change = match picked {
                    Ok(chosen) => (swatches.iter())
                        .position(|(swatch, _)| *swatch == chosen)
                        .map(|at| Change::Color(Some(stored::color_hex(crate::FONT_COLORS[at].0)))),
                    Err(0) => Some(Change::Color(None)),
                    Err(_) => Some(Change::Accent),
                };
            }
        });
        for (part, label, value) in [
            ("before", "Above:", style.before),
            ("after", "Below:", style.after),
        ] {
            field(ui, label, &mut |ui| {
                let combo = ui.id("combo");
                let shown = format!("{value} pt");
                ui::shell::combo(ui, "combo", label, &shown, 80.0, popup(part), true);
                let labels: Vec<String> = SPACING
                    .iter()
                    .map(|points| format!("{points} pt"))
                    .collect();
                let items: Vec<Item> = labels
                    .iter()
                    .map(|text| Item {
                        text,
                        checked: Some(*text == shown),
                        current: *text == shown,
                        ..Item::default()
                    })
                    .collect();
                let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
                if let Some(index) = ui::popup::menu(ui, popup(part), anchor, &items, None) {
                    let points = SPACING[index];
                    change = Some(Change::Spacing(part == "before", points));
                }
            });
        }
        ui.close();
        // The preview: every style as a page in the theme shows it.
        ui.open(
            "preview",
            Spec {
                axis: Axis::Y,
                size: [fill(), fill()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                radius: 5.0,
                pad: [14.0, 10.0],
                role: Some(Role::Group),
                ..Spec::default()
            },
        );
        for (at, (name, label)) in STYLES.iter().enumerate() {
            let style = &shown.styles[*name];
            let height = (style.size * 1.2).clamp(10.0, 26.0) * 1.35
                + (style.before + style.after).min(12.0);
            let line = ui.open(
                ("line", at),
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fill(), px(height)],
                    fill: (at == dialog.style).then(|| theme.hover()),
                    radius: 3.0,
                    pad: [4.0, (style.before.min(12.0)) / 2.0],
                    ..Spec::default()
                },
            );
            preview(ui, "text", label, style, dialog.section, 26.0);
            ui.close();
            if ui.signal(line).clicked {
                dialog.style = at;
            }
        }
        ui.close();
        ui.close();
        ui.open(
            "buttons",
            Spec {
                size: [fill(), children()],
                gap: 8.0,
                ..Spec::default()
            },
        );
        // Each button gives its choice to the one place there is, else asks where.
        let targets = &dialog.targets;
        let place = |ui: &mut Ui, part: &str, label: &str| -> Option<usize> {
            let clicked = ui::button(ui, part, label).clicked;
            if targets.len() == 1 {
                return clicked.then_some(0);
            }
            let menu = popup(part);
            if clicked {
                ui.open_popup(menu);
            }
            let items: Vec<Item> = targets
                .iter()
                .enumerate()
                .map(|(at, (scope, _))| Item {
                    text: scope.name(),
                    current: at == 0,
                    ..Item::default()
                })
                .collect();
            let anchor = Anchor::Below(ui.rect(ui.id(part)).unwrap_or_default());
            ui::popup::menu(ui, menu, anchor, &items, None)
        };
        let none = place(ui, "none", "No Theme");
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let save = place(ui, "save", "Save");
        ui.close();
        ui.close();

        let now = filetime();
        let copy = |dialog: &mut Dialog, name: String| {
            let mut copy = dialog.themes[dialog.selected].clone();
            copy.id = fresh_id(now, dialog.themes.len());
            copy.name = name;
            copy.modified = now;
            dialog.changed.push(copy.id.clone());
            dialog.themes.push(copy);
            dialog.selected = dialog.themes.len() - 1;
        };
        if duplicate {
            let name = format!("{} Copy", dialog.themes[dialog.selected].name);
            copy(dialog, name);
        }
        if let Some(change) = change {
            if built {
                let name = format!("My {}", dialog.themes[dialog.selected].name);
                copy(dialog, name);
            }
            let edited = &mut dialog.themes[dialog.selected];
            change.apply(
                edited
                    .styles
                    .get_mut(name)
                    .expect("a theme has every style"),
            );
            if !dialog.changed.contains(&edited.id) {
                dialog.changed.push(edited.id.clone());
            }
        }
        if delete {
            let mut gone = dialog.themes.remove(dialog.selected);
            gone.deleted = true;
            dialog.deleted.push(gone);
            dialog.selected = 0;
        }
        let chosen = save.map(|at| (at, true)).or(none.map(|at| (at, false)));
        if chosen.is_none() && !cancel {
            return;
        }
        if let Some((at, apply)) = chosen {
            let themes = dialog
                .themes
                .iter()
                .filter(|listed| dialog.changed.contains(&listed.id))
                .chain(&dialog.deleted)
                .map(|listed| Theme {
                    modified: now,
                    ..listed.clone()
                })
                .collect();
            let chosen = &dialog.themes[dialog.selected];
            let assignment = Assignment {
                scope: dialog.targets[at].1.clone(),
                theme: apply.then(|| chosen.id.clone()),
                assigned: now,
            };
            let wrote = Themes {
                themes,
                assignments: vec![assignment],
            };
            let before = dialog.library.themes();
            dialog.library.save_themes(wrote.clone());
            // A page's theme is taken back in its section, others anywhere in the notebook.
            let section = (dialog.targets[at].0 == Scope::Page)
                .then(|| {
                    let session = self.session.as_ref()?;
                    (session.library).section_identity(&session.tabs[session.tab].path)
                })
                .flatten();
            let undo = crate::undo::themes_undo(&dialog.library, section, &before, wrote);
            self.undo.record(undo);
        }
        self.ui.close_popup(id());
        self.themes = None;
        if chosen.is_some()
            && let Err(error) = self.wear_theme()
        {
            eprintln!("Restyling the page failed: {error}");
        }
    }
}

/// An edit the dialog's controls make to the style they show.
enum Change {
    Font(String),
    Size(f32),
    Bold,
    Italic,
    Color(Option<String>),
    /// The section's accent, the "Theme" colour.
    Accent,
    /// Space above (true) or below, in points.
    Spacing(bool, f32),
}

impl Change {
    fn apply(self, style: &mut ThemeStyle) {
        match self {
            Self::Font(font) => style.font = font,
            Self::Size(size) => style.size = size,
            Self::Bold => style.bold = !style.bold,
            Self::Italic => style.italic = !style.italic,
            Self::Color(color) => {
                style.color = color;
                style.accent = false;
            }
            Self::Accent => {
                style.color = Some(stored::color_hex(stored::accent(None)));
                style.accent = true;
            }
            Self::Spacing(true, points) => style.before = points,
            Self::Spacing(false, points) => style.after = points,
        }
    }
}

/// A new theme's id: unique to this machine's clock and the themes it knows.
fn fresh_id(now: u64, count: usize) -> String {
    format!("{now:x}-{count}")
}

/// The Styles gallery under the toolbar's Styles button: the eleven styles drawn in
/// `sheet` in a section coloured `section`, the one at the caret outlined, then Customize….
/// Returns the command chosen.
pub(crate) fn gallery(
    ui: &mut Ui,
    menu: Id,
    anchor: Anchor,
    sheet: &Theme,
    section: Option<u32>,
    current: Option<&str>,
) -> Option<crate::commands::Id> {
    use crate::commands::Id as Cmd;
    // Each style's row is as tall as its text, without the style's spacing.
    let row = |height: f32| [240.0, height + 8.0];
    let groups: Vec<ui::popup::Group> = STYLES
        .iter()
        .enumerate()
        .map(|(at, (name, _))| ui::popup::Group {
            heading: if at == 0 { &sheet.name } else { "" },
            cells: 1,
            columns: 1,
            size: row(shown_size(&sheet.styles[*name], GALLERY_LARGEST) * 1.25),
        })
        .chain([ui::popup::Group {
            heading: "",
            cells: 1,
            columns: 1,
            size: row(16.0),
        }])
        .collect();
    let shown: Vec<usize> = STYLES
        .iter()
        .position(|(name, _)| Some(*name) == current)
        .into_iter()
        .collect();
    let chosen = ui::popup::gallery(ui, menu, anchor, &groups, &shown, |ui, index| match STYLES
        .get(index)
    {
        Some((name, label)) => {
            preview(
                ui,
                "preview",
                label,
                &sheet.styles[*name],
                section,
                GALLERY_LARGEST,
            );
            if let Some(node) = ui.access(ui.id("preview")) {
                node.set_label(*label);
            }
        }
        None => {
            ui.leaf(
                "customize",
                Spec {
                    size: [fill(), fill()],
                    text: Some("Customize…"),
                    ..Spec::default()
                },
            );
        }
    })?;
    Some(if chosen < STYLES.len() {
        Cmd::Style(chosen)
    } else {
        Cmd::Themes
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::{
        document::TextPosition,
        editor::{CanvasEditor, Formatting},
        layout::TextEngine,
    };
    use onestore::{
        RevisionIndex, Store,
        document::{Document, Kind},
        op,
        page::Page,
    };

    fn page(bytes: &[u8]) -> (onestore::ExGuid, Page) {
        let store = Store::parse(bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (space, _) = document.pages().unwrap()[0];
        (space, Page::from_space(&document, space).unwrap())
    }

    /// The name of each paragraph's style on `page`, with its definition, in page order.
    fn styled(page: &Page) -> Vec<(String, Definition)> {
        let mut out = Vec::new();
        let mut pending: Vec<&onestore::page::PageParagraph> = Vec::new();
        for object in &page.objects {
            match object {
                onestore::page::PageObject::Outline(outline) => {
                    pending.extend(outline.paragraphs.iter());
                }
                onestore::page::PageObject::Title(title) => {
                    pending.extend(title.outlines.iter().flat_map(|o| &o.paragraphs));
                }
                _ => {}
            }
        }
        for paragraph in pending {
            let Some(style) = paragraph.style else {
                continue;
            };
            let definition = page.definitions[&style].clone();
            if let Kind::Style {
                name: Some(name), ..
            } = &definition.kind
            {
                out.push((name.clone(), definition));
            }
        }
        out
    }

    /// A section per built-in theme, each a page with a paragraph in every gallery style,
    /// applied through the editor as the gallery does and the title restyled as opening the
    /// page does: every style object holds the theme's definition under OneNote's name, the
    /// next paragraph after a heading is Normal, and a second theme restyles the page whole.
    /// `SNOWBOUND_STYLES_EXPORT` names a new directory receiving the notebook, its themes
    /// assigned by section, for a cold open in OneNote (`corpus/styles`).
    #[test]
    fn themed_pages_hold_their_theme_under_onenote_s_names() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-styles-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&temporary);
        let root = temporary.join("Styles");
        notebook::fs::create_dir_all(&root).unwrap();
        let mut engine = TextEngine::default();
        let mut sections = Vec::new();
        for theme in built_in() {
            let file = format!("{}.one", theme.name);
            let source = onestore::create_section(&file, "", "Author").unwrap();
            let (space, before) = page(&source);
            let mut editor = CanvasEditor::from_page(before, &mut engine).unwrap();
            let sheet = theme.sheet(None);
            editor.styles = sheet.clone();
            let body = editor
                .outlines()
                .iter()
                .find(|outline| !outline.title)
                .unwrap()
                .id;
            editor.focus_outline(body).unwrap();
            for (paragraph, (name, label)) in STYLES.iter().enumerate() {
                if paragraph > 0 {
                    editor.enter(&mut engine, false).unwrap();
                }
                editor
                    .insert(&mut engine, &format!("{label} in {}", theme.name))
                    .unwrap();
                editor
                    .select(
                        [TextPosition {
                            paragraph,
                            offset: 0,
                        }; 2]
                            .into(),
                    )
                    .unwrap();
                editor
                    .format(&mut engine, Formatting::Style(sheet[*name].clone()))
                    .unwrap();
                let offset = editor.outlines()[editor
                    .outlines()
                    .iter()
                    .position(|outline| outline.id == body)
                    .unwrap()]
                .document()
                .paragraphs()
                .nth(paragraph)
                .unwrap()
                .text()
                .encode_utf16()
                .count() as u32;
                editor
                    .select([TextPosition { paragraph, offset }; 2].into())
                    .unwrap();
            }
            let arena = onestore::Arena::default();
            let mut section = onestore::Section::open(&arena, source.to_vec()).unwrap();
            let edit = |ops: Vec<op::PageOp>, at| op::Edit {
                at,
                ops: ops
                    .into_iter()
                    .map(|op| op::Op::Page { space, op })
                    .collect(),
            };
            section
                .apply(
                    "Author",
                    &edit(editor.take_ops().unwrap(), 134_000_000_000_000_000),
                )
                .unwrap();
            let typed = section.page(space).unwrap();
            let restyle = op::restyle(&typed, &sheet).unwrap();
            section
                .apply("Author", &edit(restyle, 134_000_000_000_000_001))
                .unwrap();
            section.seal().unwrap();
            let written = section.image();
            let (_, stored) = page(&written);
            let styles: Vec<_> = styled(&stored)
                .into_iter()
                .filter(|(name, _)| sheet.contains_key(name))
                .collect();
            let names: std::collections::BTreeSet<&str> =
                styles.iter().map(|(name, _)| name.as_str()).collect();
            assert_eq!(names.len(), STYLES.len(), "{}", theme.name);
            for (name, definition) in &styles {
                assert_eq!(definition, &sheet[name], "{} {name}", theme.name);
            }
            assert!(op::restyle(&stored, &sheet).unwrap().is_empty());
            // Another theme restyles the page whole, keeping the names.
            let other = built_in().swap_remove(3).sheet(None);
            let mut copy = onestore::Section::open(&arena, written.clone()).unwrap();
            copy.apply(
                "Author",
                &edit(
                    op::restyle(&stored, &other).unwrap(),
                    134_000_000_000_000_002,
                ),
            )
            .unwrap();
            for (name, definition) in styled(&copy.page(space).unwrap()) {
                if !other.contains_key(&name) {
                    continue;
                }
                assert_eq!(definition, other[&name], "{} to modern {name}", theme.name);
            }
            notebook::fs::write(root.join(&file), &written).unwrap();
            let file_id = Store::parse(&written).unwrap().header.file_id;
            sections.push((file, file_id, theme.id));
        }
        let listed: Vec<(&str, [u8; 16])> = sections
            .iter()
            .map(|(file, id, _)| (file.as_str(), *id))
            .collect();
        let toc = onestore::create_table_of_contents("Open Notebook.onetoc2", &listed).unwrap();
        notebook::fs::write(root.join("Open Notebook.onetoc2"), toc).unwrap();
        let notebook = notebook::session::Notebook::open(&root, temporary.join("cache")).unwrap();
        let themes = notebook
            .save_themes(Themes {
                assignments: sections
                    .iter()
                    .map(|(_, id, theme)| Assignment {
                        scope: stored::Scope::section(*id),
                        theme: Some(theme.clone()),
                        assigned: 134_000_000_000_000_000,
                    })
                    .collect(),
                ..Themes::default()
            })
            .unwrap();
        for (_, id, theme) in &sections {
            assert_eq!(&themes.effective(Some(*id), None).unwrap().id, theme);
        }
        if let Some(directory) = std::env::var_os("SNOWBOUND_STYLES_EXPORT") {
            let status = std::process::Command::new("cp")
                .arg("-R")
                .arg(&root)
                .arg(directory)
                .status()
                .unwrap();
            assert!(status.success());
        }
        notebook::fs::remove_dir_all(&temporary).unwrap();
    }
}
