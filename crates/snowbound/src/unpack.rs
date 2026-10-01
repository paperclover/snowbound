//! OneNote 2010's Unpack Notebook: opening a package (`.onepkg`) asks for the notebook's name,
//! colour and the folder it goes in, then writes it there (`notebook::package::unpack`) and
//! opens it.

use crate::{State, UserEvent, art, menus::SECTION_COLORS, platform};
use accesskit::Role;
use notebook::session::Notebook;
use std::path::{Path, PathBuf};
use ui::{Anchor, Axis, Id, Spec, children, fill, popup::Item, px};
use winit::keyboard::NamedKey;

const TITLE: &str = "Unpack Notebook";
/// The most a package may unpack to.
const LIMIT: u64 = 16 << 30;

/// The dialog's fields while it is open.
pub struct Dialog {
    package: PathBuf,
    name: String,
    /// A colour picked, COLORREF; none keeps the package's.
    color: Option<u32>,
    folder: PathBuf,
}

fn id() -> Id {
    Id::ROOT.child("unpack")
}

fn name_field() -> Id {
    id().child("name")
}

impl State {
    /// Opens Unpack Notebook on the package at `package`, named for its file and going in the
    /// documents folder, as OneNote's starts in its notebooks folder.
    pub(crate) fn open_unpack(&mut self, package: &Path) {
        let name = package
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        let folder = (platform::documents_dir())
            .or_else(|| package.parent().map(Path::to_owned))
            .unwrap_or_default();
        self.unpacking = Some(Dialog {
            package: package.to_owned(),
            name,
            color: None,
            folder,
        });
        self.ui.open_popup(id());
        self.ui.focus_all(name_field());
    }

    /// Builds Unpack Notebook while it is open. Create, or Enter in the name, unpacks into a
    /// new folder of that name.
    pub(crate) fn unpack_dialog(&mut self) {
        let Some(dialog) = &mut self.unpacking else {
            return;
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.unpacking = None;
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
                size: [px(420.0), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [16.0, 12.0],
                gap: 4.0,
                anchor: Some(Anchor::Dialog),
                role: Some(Role::Dialog),
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
                role: Some(Role::Heading),
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
        label(ui, "Name:");
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
            node.set_label("Name");
        }
        label(ui, "Color:");
        let colors = id().child("colors");
        let named = (SECTION_COLORS.iter())
            .find(|(color, _)| Some(*color) == dialog.color)
            .map_or("As packaged", |(_, name)| name);
        let combo = ui.id("combo");
        ui::shell::combo(ui, "combo", "Color", named, 160.0, colors, true);
        let items: Vec<Item> = (SECTION_COLORS.iter())
            .map(|&(color, text)| Item {
                text,
                icon: Some(art::SECTION),
                tint: Some(theme.section(crate::section_color(Some(color))).accent),
                current: Some(color) == dialog.color,
                ..Item::default()
            })
            .collect();
        let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
        if let Some(index) = ui::popup::menu(ui, colors, anchor, &items, None) {
            dialog.color = Some(SECTION_COLORS[index].0);
        }
        label(ui, "Path:");
        let folder = dialog.folder.to_string_lossy().into_owned();
        ui.leaf(
            "folder",
            Spec {
                flags: ui::Flags::CLIP,
                size: [fill(), px(row * 0.8)],
                text: Some(&folder),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        );
        let browse = ui::button(ui, "browse", "Browse…").clicked;
        let root = dialog.folder.join(dialog.name.trim());
        let full = format!("Full path: {}", root.display());
        ui.leaf(
            "full",
            Spec {
                flags: ui::Flags::CLIP,
                size: [fill(), px(row * 0.8)],
                text: Some(&full),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        );
        crate::buttons(ui);
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let create = ui::button(ui, "create", "Create").clicked || entered;
        ui.close();
        ui.close();
        if browse
            && let Some(chosen) =
                platform::pick_new(TITLE, dialog.name.trim(), "Choose", Some(&dialog.folder))
        {
            if let Some(name) = chosen.file_name() {
                dialog.name = name.to_string_lossy().into_owned();
            }
            if let Some(parent) = chosen.parent() {
                dialog.folder = parent.to_owned();
            }
        }
        let name = dialog.name.trim();
        let valid =
            !name.is_empty() && !name.contains(['/', '\\', ':']) && name != "." && name != "..";
        if create && !valid {
            return platform::alert("Couldn't unpack the notebook", "Give the notebook a name.");
        }
        if create && notebook::fs::metadata(&root).is_ok() {
            return platform::alert(
                "Couldn't unpack the notebook",
                &format!(
                    "{} already exists. Choose another name or path.",
                    root.display()
                ),
            );
        }
        if !create && !cancel {
            return;
        }
        let dialog = self.unpacking.take().expect("The dialog is open");
        self.ui.close_popup(id());
        if create {
            self.unpack(dialog.package, root, dialog.color);
        }
    }

    /// Unpacks `package` into the new folder `root` on a thread of its own, colours it
    /// `color` where one was picked, and opens it.
    fn unpack(&mut self, package: PathBuf, root: PathBuf, color: Option<u32>) {
        let (cache, proxy) = (self.cache.clone(), self.proxy.clone());
        crate::spawn(move || {
            let unpacked = (|| -> Result<String, Box<dyn std::error::Error>> {
                let files = notebook::package::read(&notebook::fs::read(&package)?, LIMIT)?;
                notebook::package::unpack(files, &root)?;
                let location = notebook::fs::absolute(&root)?
                    .to_string_lossy()
                    .into_owned();
                if let Some(color) = color {
                    Notebook::open(&location, &cache)?.set_color(color)?;
                }
                Ok(location)
            })()
            .map_err(|error| error.to_string());
            let _ = proxy.send_event(UserEvent::Then(Box::new(move |state: &mut State| {
                match unpacked {
                    Ok(location) => state.open_notebook(location, None),
                    Err(error) => platform::alert("Couldn't unpack the notebook", &error),
                }
                Ok(())
            })));
        });
    }
}
