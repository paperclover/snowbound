//! Notebook Properties, as OneNote 2010's: the notebook's display name on this computer, its
//! colour, and where it lives.

use crate::{Command, Library, State, art, commands, manage::Structure, menus::SECTION_COLORS};
use std::sync::Arc;
use ui::{Anchor, Axis, Id, Spec, children, fill, popup::Item, px};
use winit::keyboard::NamedKey;

const WIDTH: f32 = 380.0;
const TITLE: &str = "Notebook Properties";

/// The dialog's fields until OK keeps them.
pub struct Properties {
    library: Arc<Library>,
    name: String,
    /// A colour picked, COLORREF.
    color: Option<u32>,
    /// Renames the notebook's folder to a name changed.
    rename_folder: bool,
}

fn id() -> Id {
    Id::ROOT.child("notebook-properties")
}

fn name_field() -> Id {
    id().child("name")
}

fn colors() -> Id {
    id().child("colors")
}

impl State {
    /// Opens Notebook Properties on `library`, its display name selected, as OneNote's Rename…
    /// and Properties… both open it.
    pub(crate) fn open_properties(&mut self, library: Arc<Library>) {
        self.properties = Some(Properties {
            name: library.name.clone(),
            library,
            color: None,
            rename_folder: true,
        });
        self.ui.open_popup(id());
        self.ui.focus_all(name_field());
    }

    /// Builds Notebook Properties while it is open. OK, or Enter in the name, keeps a
    /// non-empty name and a colour picked.
    pub(crate) fn properties_dialog(&mut self) {
        let Some(dialog) = &mut self.properties else {
            return;
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.properties = None;
            return;
        }
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        let entered = ui::popup::navigation(ui, &[name_field()], &[NamedKey::Enter])
            .contains(&NamedKey::Enter);
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(WIDTH), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [16.0, 12.0],
                gap: 4.0,
                anchor: Some(Anchor::Dialog),
                role: Some(accesskit::Role::Dialog),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id()) {
            node.set_label(TITLE);
        }
        ui.leaf(
            "title",
            Spec {
                size: [fill(), px(row)],
                text: Some(TITLE),
                bold: true,
                role: Some(accesskit::Role::Heading),
                ..Spec::default()
            },
        );
        let label = |ui: &mut ui::Ui, text: &str| {
            ui.leaf(
                text,
                Spec {
                    size: [fill(), px(row * 0.8)],
                    text: Some(text),
                    ..Spec::default()
                },
            );
        };
        let dim = |ui: &mut ui::Ui, key: &str, text: &str| {
            ui.leaf(
                key,
                Spec {
                    flags: ui::Flags::CLIP,
                    size: [fill(), px(row * 0.8)],
                    text: Some(text),
                    color: Some(theme.text_dim),
                    ..Spec::default()
                },
            );
        };
        label(ui, "Display name:");
        ui::text_field(
            ui,
            name_field(),
            &mut dialog.name,
            "",
            Spec {
                size: [fill(), px(row)],
                fill: Some(theme.base),
                border: Some(theme.accent),
                radius: 4.0,
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(name_field()) {
            node.set_label("Display name");
        }
        let renamable = dialog.name.trim() != dialog.library.name
            && (dialog.library.renamed_location(dialog.name.trim())).is_some();
        if renamable {
            let rename = ui::check_box(
                ui,
                "rename",
                "Rename the folder on disk",
                dialog.rename_folder,
            );
            if rename.clicked {
                dialog.rename_folder = !dialog.rename_folder;
            }
            if dialog.rename_folder && dialog.library.on_smb() {
                ui.leaf(
                    "shared",
                    Spec {
                        flags: ui::Flags::CLIP,
                        size: [fill(), px(row * 0.8)],
                        text: Some("Other computers must reopen it from the new folder"),
                        icon: Some(art::WARNING),
                        ..Spec::default()
                    },
                );
            }
        } else {
            dim(ui, "hint", "Doesn’t change the notebook’s folder name");
        }
        label(ui, "Color:");
        // A notebook without a colour of its own wears the app's.
        let shown = dialog.color.or(dialog.library.color());
        let named = SECTION_COLORS
            .iter()
            .find(|(color, _)| Some(*color) == shown)
            .map_or("Default", |(_, name)| name);
        let combo = ui.id("combo");
        ui::shell::combo(ui, "combo", "Color", named, 160.0, colors(), true);
        let items: Vec<Item> = SECTION_COLORS
            .iter()
            .map(|&(color, text)| Item {
                text,
                icon: Some(art::SWATCH),
                tint: Some(crate::notebook_color(&theme, Some(color))),
                current: Some(color) == shown,
                ..Item::default()
            })
            .collect();
        let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
        if let Some(index) = ui::popup::menu(ui, colors(), anchor, &items, None) {
            dialog.color = Some(SECTION_COLORS[index].0);
        }
        label(ui, "Location:");
        dim(ui, "location", &dialog.library.location);
        let reveal = commands::command(commands::Id::ShowNotebook).title;
        if dialog.library.folder().is_some() && ui::button(ui, "reveal", reveal).clicked {
            crate::platform::reveal(&dialog.library.location);
        }
        crate::buttons(ui);
        let [cancel, ok] = ui::dialog_buttons(ui, "OK", true);
        let ok = ok || entered;
        ui.close();
        ui.close();
        let name = dialog.name.trim();
        if ok && !name.is_empty() && renamable && dialog.rename_folder {
            let (library, name, color) =
                (Arc::clone(&dialog.library), name.to_owned(), dialog.color);
            self.ui.close_popup(id());
            self.properties = None;
            return self.rename_notebook(library, name, color);
        }
        if ok && !name.is_empty() {
            if name != dialog.library.name || dialog.color.is_some() {
                self.commands.push(Command::Structure(
                    Arc::clone(&dialog.library),
                    Structure::Properties {
                        name: name.to_owned(),
                        color: dialog.color,
                    },
                ));
            }
        } else if !cancel {
            return;
        }
        self.ui.close_popup(id());
        self.properties = None;
    }
}
