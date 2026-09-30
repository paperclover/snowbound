//! OneNote's Options dialog: pages of settings listed on the left as OneNote 2010 lists
//! them, grouped under the same headings, each kept when OK is chosen.

use crate::{State, platform, settings::ColorScheme, update};
use accesskit::Role;
use ui::{Anchor, Axis, Flags, Id, Spec, Theme, Ui, children, fill, popup::Item, px};
use winit::keyboard::NamedKey;

const WIDTH: f32 = 640.0;
/// The pages' area, the same for every page so the dialog keeps its size.
const HEIGHT: f32 = 320.0;
const NAV: f32 = 150.0;
/// The column the fields' labels share.
const LABEL: f32 = 104.0;
const SCHEMES: [(ColorScheme, &str); 3] = [
    (ColorScheme::System, "System"),
    (ColorScheme::Light, "Light"),
    (ColorScheme::Dark, "Dark"),
];

#[derive(Clone, Copy, PartialEq)]
enum Page {
    General,
    Display,
    SaveBackup,
    Advanced,
}

impl Page {
    const ALL: [(Page, &str); 4] = [
        (Page::General, "General"),
        (Page::Display, "Display"),
        (Page::SaveBackup, "Save & Backup"),
        (Page::Advanced, "Advanced"),
    ];
}

/// The page shown and what the fields hold until OK keeps them.
pub struct Options {
    page: Page,
    user_name: String,
    color_scheme: ColorScheme,
    light_pages: bool,
    automatic_updates: bool,
    pen_pressure: bool,
}

fn id() -> Id {
    Id::ROOT.child("options")
}

fn user_name() -> Id {
    id().child("user-name")
}

fn schemes() -> Id {
    id().child("color-schemes")
}

impl State {
    pub(crate) fn open_options(&mut self) {
        self.options = Some(Options {
            page: Page::General,
            user_name: self.author.clone(),
            color_scheme: self.color_scheme,
            light_pages: self.light_pages,
            automatic_updates: self.updates.automatic(),
            pen_pressure: self.pen_pressure,
        });
        self.ui.open_popup(id());
        self.ui.set_focus(Some(user_name()));
    }

    /// Builds the Options dialog while it is open. OK, or Enter in a field, keeps its choices
    /// with a non-empty user name; Cancel, Escape or a press outside leave them.
    pub(crate) fn options_dialog(&mut self) {
        let Some(options) = &mut self.options else {
            return;
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.options = None;
            return;
        }
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        let entered = ui::popup::navigation(ui, &[user_name()], &[NamedKey::Enter])
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
                pad: [6.0, 6.0],
                anchor: Some(Anchor::Dialog),
                role: Some(Role::Dialog),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id()) {
            node.set_label("Options");
        }
        ui.open(
            "body",
            Spec {
                size: [fill(), px(HEIGHT)],
                ..Spec::default()
            },
        );
        let pages = ui.open(
            "pages",
            Spec {
                axis: Axis::Y,
                size: [px(NAV), fill()],
                fill: Some(theme.panel),
                radius: 5.0,
                pad: [6.0, 6.0],
                gap: 2.0,
                role: Some(Role::TabList),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(pages) {
            node.set_orientation(accesskit::Orientation::Vertical);
        }
        for (page, name) in Page::ALL {
            let shown = page == options.page;
            let spec = Spec {
                flags: Flags::CLICKABLE,
                size: [fill(), px(row)],
                text: Some(name),
                bold: shown,
                fill: shown.then(|| theme.hover()),
                hover_fill: Some(theme.hover()),
                radius: 4.0,
                pad: [8.0, 0.0],
                role: Some(Role::Tab),
                ..Spec::default()
            };
            let tab = ui.open(name, spec);
            if let Some(node) = ui.access(tab) {
                node.set_selected(shown);
            }
            ui.close();
            if ui.signal(tab).clicked {
                options.page = page;
            }
        }
        ui.close();
        ui.open(
            "page",
            Spec {
                axis: Axis::Y,
                size: [fill(), fill()],
                pad: [16.0, 10.0],
                gap: 6.0,
                ..Spec::default()
            },
        );
        match options.page {
            Page::General => {
                heading(ui, &theme, "User Interface Options");
                field(ui, "Color scheme:", |ui| {
                    let combo = ui.id("combo");
                    let current = SCHEMES
                        .iter()
                        .find(|(scheme, _)| *scheme == options.color_scheme)
                        .map_or("", |(_, name)| name);
                    ui::shell::combo(ui, "combo", "Color scheme", current, 140.0, schemes(), true);
                    let items = SCHEMES.map(|(scheme, name)| Item {
                        text: name,
                        checked: Some(scheme == options.color_scheme),
                        current: scheme == options.color_scheme,
                        ..Item::default()
                    });
                    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
                    if let Some(index) = ui::popup::menu(ui, schemes(), anchor, &items, None) {
                        options.color_scheme = SCHEMES[index].0;
                    }
                });
                heading(ui, &theme, "Personalize");
                field(ui, "User name:", |ui| {
                    ui::text_field(
                        ui,
                        user_name(),
                        &mut options.user_name,
                        "",
                        Spec {
                            size: [px(260.0), px(row)],
                            fill: Some(theme.base),
                            border: Some(theme.accent),
                            radius: 4.0,
                            pad: [6.0, 0.0],
                            ..Spec::default()
                        },
                    );
                    if let Some(node) = ui.access(user_name()) {
                        node.set_label("User name");
                    }
                });
                heading(ui, &theme, "Updates");
                ui.leaf(
                    "version",
                    Spec {
                        size: [fill(), px(row)],
                        text: Some(&update::describe_running()),
                        pad: [8.0, 0.0],
                        ..Spec::default()
                    },
                );
                if ui::check_box(
                    ui,
                    "automatic-updates",
                    "Check for updates automatically",
                    options.automatic_updates,
                )
                .clicked
                {
                    options.automatic_updates = !options.automatic_updates;
                }
                #[cfg(target_os = "linux")]
                if crate::desktop::uninstallable() {
                    field(ui, "Installed:", |ui| {
                        let binary = crate::desktop::binary().unwrap_or_default();
                        ui.leaf(
                            "path",
                            Spec {
                                flags: Flags::CLIP,
                                size: [fill(), px(row)],
                                text: Some(&binary.to_string_lossy()),
                                color: Some(theme.text_dim),
                                ..Spec::default()
                            },
                        );
                        if ui::button(ui, "uninstall", "Uninstall…").clicked {
                            crate::desktop::uninstall();
                        }
                    });
                }
            }
            Page::Display => {
                heading(ui, &theme, "Display");
                let dark = !options.light_pages;
                if ui::check_box(
                    ui,
                    "dark-pages",
                    "Pages appear dark in the Dark color scheme",
                    dark,
                )
                .clicked
                {
                    options.light_pages = dark;
                }
            }
            Page::Advanced => {
                heading(ui, &theme, "Pen");
                if ui::check_box(
                    ui,
                    "pen-pressure",
                    "Use pen pressure sensitivity",
                    options.pen_pressure,
                )
                .clicked
                {
                    options.pen_pressure = !options.pen_pressure;
                }
            }
            Page::SaveBackup => {
                heading(ui, &theme, "Cache file location");
                let cache = &self.cache;
                field(ui, "Path:", |ui| {
                    ui.leaf(
                        "path",
                        Spec {
                            flags: Flags::CLIP,
                            size: [fill(), px(row)],
                            text: Some(&cache.to_string_lossy()),
                            color: Some(theme.text_dim),
                            ..Spec::default()
                        },
                    );
                    let label = if cfg!(target_os = "macos") {
                        "Show in Finder"
                    } else {
                        "Open Folder"
                    };
                    if ui::button(ui, "reveal", label).clicked {
                        platform::reveal(cache);
                    }
                });
            }
        }
        ui.close();
        ui.close();
        ui.open(
            "buttons",
            Spec {
                size: [fill(), children()],
                pad: [10.0, 8.0],
                gap: 8.0,
                ..Spec::default()
            },
        );
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let ok = ui::button(ui, "ok", "OK").clicked || entered;
        ui.close();
        ui.close();
        let name = options.user_name.trim();
        if ok && !name.is_empty() {
            self.author = name.to_owned();
            self.color_scheme = options.color_scheme;
            self.light_pages = options.light_pages;
            self.pen_pressure = options.pen_pressure;
            self.updates.set_automatic(options.automatic_updates);
            self.follow_color_scheme();
            self.save_settings();
        } else if !cancel {
            return;
        }
        self.ui.close_popup(id());
        self.options = None;
    }
}

/// A group's heading on a band, as OneNote heads the groups of its Options pages.
fn heading(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.leaf(
        text,
        Spec {
            size: [fill(), px(theme.font_size * 1.8)],
            text: Some(text),
            bold: true,
            fill: Some(ui::mix(theme.panel, theme.chip, 0.4)),
            radius: 4.0,
            pad: [8.0, 0.0],
            role: Some(Role::Heading),
            ..Spec::default()
        },
    );
}

/// A row of `label` in the labels' column and the controls `build` adds after it.
fn field(ui: &mut Ui, label: &str, build: impl FnOnce(&mut Ui)) {
    let row = ui.theme.font_size * 2.0;
    ui.open(
        label,
        Spec {
            size: [fill(), px(row)],
            pad: [8.0, 0.0],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "label",
        Spec {
            size: [px(LABEL), px(row)],
            text: Some(label),
            ..Spec::default()
        },
    );
    build(ui);
    ui.close();
}
