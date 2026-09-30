//! Style themes in the app (`resources/styles.md`): the page wears its own theme, else its
//! section's, else its notebook's, as its paragraph style objects; the Styles gallery
//! applies a style in the page's theme; the Themes dialog picks a theme for the page, the
//! section or the notebook, and edits a notebook's own themes with a live preview.

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
    scope: Scope,
    library: Arc<Library>,
    target: stored::Scope,
    /// The built-in themes, then the notebook's own as the dialog edits them.
    themes: Vec<Theme>,
    /// The notebook's own themes changed here, by id.
    changed: Vec<String>,
    /// The notebook's own themes deleted here.
    deleted: Vec<Theme>,
    selected: usize,
    /// The style the controls edit, by its place in the gallery.
    style: usize,
}

fn id() -> Id {
    Id::ROOT.child("themes")
}

fn popup(name: &str) -> Id {
    id().child(name)
}

/// Spacing the dialog offers above and below a style, in points.
const SPACING: [f32; 9] = [0.0, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0, 12.0, 18.0];

/// A style's colour as `#rrggbb` from a COLORREF, and back.
fn hex(colorref: u32) -> String {
    let [red, green, blue, _] = colorref.to_le_bytes();
    format!("#{red:02X}{green:02X}{blue:02X}")
}

fn colorref(hex: &str) -> Option<u32> {
    let rgb = u32::from_str_radix(hex.strip_prefix('#')?, 16).ok()?;
    Some((rgb >> 16) | (rgb & 0xff00) | ((rgb & 0xff) << 16))
}

/// A line of `text` in `style` as a page shows it, scaled down to fit a menu row up to
/// `largest` pixels, its colour moved onto the popup's paper so it reads in either scheme.
pub(crate) fn preview(ui: &mut Ui, part: &str, text: &str, style: &ThemeStyle, largest: f32) {
    let theme = ui.theme.clone();
    let paper = canvas::gpu::Paper {
        color: theme.popup,
        ink: theme.text,
    };
    let color = style
        .color
        .as_deref()
        .and_then(colorref)
        .map_or(theme.text, |color| paper.tint(canvas::gpu::colorref(color)));
    ui.leaf(
        part,
        Spec {
            flags: Flags::CLIP,
            size: [fill(), fill()],
            text: Some(text),
            font: Some(&style.font),
            font_size: Some((style.size * 1.2).clamp(10.0, largest)),
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
        stored::definition(name, &theme.styles[name])
    }

    /// Dresses the open page in its theme: new text and Enter take the theme's styles, and
    /// style objects the theme gives otherwise are restyled, as one edit. A page with no theme
    /// keeps what it holds.
    pub(crate) fn wear_theme(&mut self) -> Result<(), Box<dyn Error>> {
        let theme = self.page_theme();
        let sheet = theme.as_ref().map(Theme::sheet).unwrap_or_default();
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
        let Some(session) = &self.session else {
            return Ok(());
        };
        let space = session.space;
        session.section.apply(
            &self.author,
            onestore::op::Edit {
                at: filetime(),
                ops: ops
                    .into_iter()
                    .map(|op| onestore::op::Op::Page { space, op })
                    .collect(),
            },
        )?;
        self.edited(vec![space]);
        self.refresh()
    }

    /// Opens the Themes dialog choosing a theme for `scope` of the open page.
    pub(crate) fn open_themes(&mut self, scope: Scope) {
        let Some((library, target)) = self.theme_target(scope) else {
            return;
        };
        self.show_themes(scope, library, target);
    }

    /// Opens the Themes dialog choosing a theme for `target`, which `scope` names.
    pub(crate) fn show_themes(
        &mut self,
        scope: Scope,
        library: Arc<Library>,
        target: stored::Scope,
    ) {
        let themes = library.themes();
        let assigned = themes.assigned(&target).map(|theme| theme.id);
        let all = themes.all();
        let selected = assigned
            .and_then(|id| all.iter().position(|theme| theme.id == id))
            .unwrap_or(0);
        self.themes = Some(Dialog {
            scope,
            library,
            target,
            themes: all,
            changed: Vec::new(),
            deleted: Vec::new(),
            selected,
            style: 0,
        });
        self.ui.open_popup(id());
    }

    /// Builds the Themes dialog while it is open. Apply keeps the edits and gives the theme
    /// to the dialog's scope; No Theme takes the scope's own away; Cancel, Escape or a press
    /// outside leave everything as it was.
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
        let title = format!("Theme for {}", dialog.scope.name());
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
            preview(ui, "name", &listed.name, &listed.styles["h2"], 15.0);
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
        let duplicate = ui::button(ui, "duplicate", "Duplicate").clicked;
        let delete =
            dialog.selected >= built_in().len() && ui::button(ui, "delete", "Delete").clicked;
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
        let built = dialog.selected < built_in().len();
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
            let shown = style.color.clone().unwrap_or_else(|| "Automatic".into());
            ui::shell::combo(ui, "color", "Color", &shown, 120.0, popup("colors"), true);
            let swatches: Vec<_> = crate::FONT_COLORS
                .iter()
                .map(|&(color, name)| (canvas::gpu::colorref(color), name))
                .collect();
            let anchor = Anchor::Below(ui.rect(button).unwrap_or_default());
            if let Some(chosen) =
                ui::popup::colors(ui, popup("colors"), anchor, "Automatic", &swatches, 10)
            {
                let color = chosen.and_then(|chosen| {
                    crate::FONT_COLORS
                        .iter()
                        .zip(&swatches)
                        .find(|(_, (swatch, _))| *swatch == chosen)
                        .map(|((color, _), _)| hex(*color))
                });
                change = Some(Change::Color(color));
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
            preview(ui, "text", label, style, 26.0);
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
        let none = ui::button(ui, "none", "No Theme").clicked;
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let apply = ui::button(ui, "apply", "Apply").clicked;
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
            if dialog.selected < built_in().len() {
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
        if delete && dialog.selected >= built_in().len() {
            let mut gone = dialog.themes.remove(dialog.selected);
            gone.deleted = true;
            dialog.deleted.push(gone);
            dialog.selected = 0;
        }
        if !(apply || none || cancel) {
            return;
        }
        if !cancel {
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
                scope: dialog.target.clone(),
                theme: apply.then(|| chosen.id.clone()),
                assigned: now,
            };
            dialog.library.save_themes(Themes {
                themes,
                assignments: vec![assignment],
            });
        }
        self.ui.close_popup(id());
        self.themes = None;
        if !cancel && let Err(error) = self.wear_theme() {
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
            Self::Color(color) => style.color = color,
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
/// `sheet`, the one at the caret outlined, then the theme commands. Returns the command chosen.
pub(crate) fn gallery(
    ui: &mut Ui,
    menu: Id,
    anchor: Anchor,
    sheet: &Theme,
    current: Option<&str>,
) -> Option<crate::commands::Id> {
    use crate::commands::Id as Cmd;
    let scopes = [Scope::Page, Scope::Section, Scope::Notebook];
    let groups = [
        ui::popup::Group {
            heading: &sheet.name,
            cells: STYLES.len(),
            columns: 1,
            size: [240.0, 34.0],
        },
        ui::popup::Group {
            heading: "Theme",
            cells: scopes.len(),
            columns: 1,
            size: [240.0, 24.0],
        },
    ];
    let shown: Vec<usize> = STYLES
        .iter()
        .position(|(name, _)| Some(*name) == current)
        .into_iter()
        .collect();
    let chosen = ui::popup::gallery(ui, menu, anchor, &groups, &shown, |ui, index| match STYLES
        .get(index)
    {
        Some((name, label)) => {
            preview(ui, "preview", label, &sheet.styles[*name], 24.0);
            if let Some(node) = ui.access(ui.id("preview")) {
                node.set_label(*label);
            }
        }
        None => {
            let label = format!("{}…", scopes[index - STYLES.len()].name());
            ui.leaf(
                "scope",
                Spec {
                    size: [fill(), fill()],
                    text: Some(&label),
                    ..Spec::default()
                },
            );
        }
    })?;
    Some(match scopes.get(chosen.wrapping_sub(STYLES.len())) {
        Some(scope) => Cmd::Theme(*scope),
        None => Cmd::Style(chosen),
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
        let _ = std::fs::remove_dir_all(&temporary);
        let root = temporary.join("Styles");
        std::fs::create_dir_all(&root).unwrap();
        let mut engine = TextEngine::default();
        let mut sections = Vec::new();
        for theme in built_in() {
            let file = format!("{}.one", theme.name);
            let source = onestore::create_section(&file, "", "Author").unwrap();
            let (space, before) = page(&source);
            let mut editor = CanvasEditor::from_page(before, &mut engine).unwrap();
            let sheet = theme.sheet();
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
            let other = built_in().swap_remove(3).sheet();
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
            std::fs::write(root.join(&file), &written).unwrap();
            let file_id = Store::parse(&written).unwrap().header.file_id;
            sections.push((file, file_id, theme.id));
        }
        let listed: Vec<(&str, [u8; 16])> = sections
            .iter()
            .map(|(file, id, _)| (file.as_str(), *id))
            .collect();
        let toc = onestore::create_table_of_contents("Open Notebook.onetoc2", &listed).unwrap();
        std::fs::write(root.join("Open Notebook.onetoc2"), toc).unwrap();
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
        std::fs::remove_dir_all(&temporary).unwrap();
    }
}
