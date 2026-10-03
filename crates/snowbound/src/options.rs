//! OneNote's Options dialog as one scrolling list of sections (`resources/settings.md`):
//! indexed on the left, filtered by the search field above, each choice kept when OK is
//! chosen.

use crate::{State, platform, settings::ColorScheme, update};
use accesskit::Role;
use canvas::editor::DefaultFont;
use ui::{Anchor, Axis, Extent, Flags, Id, Size, Spec, Theme, Ui, fill, popup::Item, px};
use winit::keyboard::NamedKey;

const WIDTH: f32 = 680.0;
const NAV: f32 = 150.0;
/// The column the fields' labels share.
const LABEL: f32 = 104.0;
const SCHEMES: [(ColorScheme, &str); 3] = [
    (ColorScheme::System, "System"),
    (ColorScheme::Light, "Light"),
    (ColorScheme::Dark, "Dark"),
];
/// The sizes Default font offers, OneNote 2010's list in points.
const SIZES: [f32; 19] = [
    8.0, 9.0, 9.5, 10.0, 10.5, 11.0, 11.5, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0,
    36.0, 48.0, 72.0,
];

/// An entry of the index and its part of the list.
struct Section {
    name: &'static str,
    groups: &'static [Group],
}

/// Rows under a banded heading, as OneNote groups an Options page; an empty heading has no
/// band.
struct Group {
    heading: &'static str,
    rows: &'static [Row],
}

struct Row {
    label: &'static str,
    /// Other words search finds the row by.
    keywords: &'static str,
    control: Control,
}

enum Control {
    /// A check box before the label, bound to a choice.
    Check(fn(&mut Options) -> &mut bool),
    /// The label in the labels' column, then what the function builds.
    Field(fn(&mut State, &mut Options)),
    /// What `build` makes across the row, unlabelled, from the search's words; `finds` tells
    /// whether they find anything in it. Words found in the row's own texts give it none.
    Block {
        build: fn(&mut State, &mut Options, &[String]),
        finds: fn(&[String]) -> bool,
    },
}

/// Every section, in the list's order.
const SECTIONS: &[Section] = &[
    Section {
        name: "General",
        groups: &[
            Group {
                heading: "User Interface Options",
                rows: &[
                    Row {
                        label: "Appearance:",
                        keywords: "color colour scheme dark light mode system theme",
                        control: Control::Field(appearance),
                    },
                    Row {
                        label: "Pages match UI theme",
                        keywords: "dark white paper background",
                        control: Control::Check(|options| &mut options.pages_match),
                    },
                ],
            },
            Group {
                heading: "Personalize",
                rows: &[Row {
                    label: "User name:",
                    keywords: "author name",
                    control: Control::Field(user_name_field),
                }],
            },
        ],
    },
    Section {
        name: "Editing",
        groups: &[
            Group {
                heading: "Default font",
                rows: &[
                    Row {
                        label: "Font:",
                        keywords: "typeface text new",
                        control: Control::Field(font_face),
                    },
                    Row {
                        label: "Size:",
                        keywords: "font points text new",
                        control: Control::Field(font_size),
                    },
                    Row {
                        label: "Font color:",
                        keywords: "colour text new automatic",
                        control: Control::Field(font_color),
                    },
                ],
            },
            Group {
                heading: "Default style theme",
                rows: &[Row {
                    label: "New notebooks:",
                    keywords: "default for new notebooks styles headings modern",
                    control: Control::Field(notebook_theme),
                }],
            },
            Group {
                heading: "Proofing",
                rows: &[Row {
                    label: "Hide spelling errors",
                    keywords: "spell check misspelled words",
                    control: Control::Check(|options| &mut options.hide_spelling),
                }],
            },
            Group {
                heading: "AutoFormat",
                rows: &[Row {
                    label: "Markdown shortcuts",
                    keywords: "markdown autoformat heading bullet numbering list to do quote \
                               bold italic strikethrough code typing",
                    control: Control::Check(|options| &mut options.markdown),
                }],
            },
            Group {
                heading: "Pen",
                rows: &[Row {
                    label: "Use pen pressure sensitivity",
                    keywords: "tablet stylus ink stroke width drawing",
                    control: Control::Check(|options| &mut options.pen_pressure),
                }],
            },
            Group {
                heading: "Passwords",
                rows: &[
                    Row {
                        label: "Lock password protected sections after I have not worked in \
                                them for:",
                        keywords: "protect protected lock unlock idle minutes timeout security",
                        control: Control::Check(|options| &mut options.lock_idle),
                    },
                    Row {
                        label: "Amount of time:",
                        keywords: "password protect protected lock idle minutes hours timeout",
                        control: Control::Field(lock_after),
                    },
                    Row {
                        label: "Lock password protected sections as soon as I navigate away \
                                from them",
                        keywords: "password protect protected lock leave switch security",
                        control: Control::Check(|options| &mut options.lock_on_leave),
                    },
                ],
            },
        ],
    },
    Section {
        name: "Display",
        groups: &[Group {
            heading: "",
            rows: &[
                Row {
                    label: "Page tabs appear on the left",
                    keywords: "pages list side layout",
                    control: Control::Check(|options| &mut options.page_tabs_left),
                },
                Row {
                    label: "Navigation bar appears on the left",
                    keywords: "notebooks sidebar side right layout",
                    control: Control::Check(|options| &mut options.navigation_bar_left),
                },
            ],
        }],
    },
    Section {
        name: "Sync & Storage",
        groups: &[
            Group {
                heading: "Cache file location",
                rows: &[Row {
                    label: "Path:",
                    keywords: "folder replica data disk",
                    control: Control::Field(cache),
                }],
            },
            #[cfg(feature = "live")]
            Group {
                heading: "Live Share",
                rows: &[
                    Row {
                        label: "Show others where I am in shared notebooks",
                        keywords: "presence privacy cursor caret avatar collaborate people",
                        control: Control::Check(|options| &mut options.presence),
                    },
                    Row {
                        label: "Show my account picture with my name",
                        keywords: "presence privacy photo avatar",
                        control: Control::Check(|options| &mut options.picture),
                    },
                    Row {
                        label: "Relay:",
                        keywords: "live share server internet network presence",
                        control: Control::Field(relay),
                    },
                ],
            },
        ],
    },
    Section {
        name: "Updates",
        groups: &[Group {
            heading: "",
            rows: &[
                Row {
                    label: "Version:",
                    keywords: "about build release",
                    control: Control::Field(version),
                },
                Row {
                    label: "Check for updates automatically",
                    keywords: "update download",
                    control: Control::Check(|options| &mut options.automatic_updates),
                },
                #[cfg(target_os = "linux")]
                Row {
                    label: "Installed:",
                    keywords: "install uninstall app menu remove",
                    control: Control::Field(installed),
                },
            ],
        }],
    },
    Section {
        name: "Keyboard",
        groups: &[Group {
            heading: "",
            rows: &[Row {
                label: "Shortcuts",
                keywords: "keys chords hotkeys bindings commands menus",
                control: Control::Block {
                    build: |state, options, query| options.keyboard.build(&mut state.ui, query),
                    finds: crate::keys::searched,
                },
            }],
        }],
    },
];

/// What the fields hold until OK keeps them, and where the list is.
pub struct Options {
    query: String,
    /// The section last picked in the index, marked until the list is scrolled.
    picked: Option<&'static str>,
    user_name: String,
    color_scheme: ColorScheme,
    pages_match: bool,
    hide_spelling: bool,
    markdown: bool,
    automatic_updates: bool,
    pen_pressure: bool,
    default_font: DefaultFont,
    page_tabs_left: bool,
    navigation_bar_left: bool,
    lock_idle: bool,
    /// Minutes, as `protection::AFTER` lists them.
    lock_minutes: u32,
    lock_on_leave: bool,
    presence: bool,
    picture: bool,
    /// Live Share's relay; empty uses Snowbound's.
    relay: String,
    notebook_theme: Option<String>,
    pub(crate) keyboard: crate::keys::Keyboard,
}

fn id() -> Id {
    Id::ROOT.child("options")
}

fn search() -> Id {
    id().child("search")
}

fn list() -> Id {
    id().child("list")
}

fn user_name() -> Id {
    id().child("user-name")
}

fn schemes() -> Id {
    id().child("color-schemes")
}

fn fonts() -> Id {
    id().child("fonts")
}

fn sizes() -> Id {
    id().child("sizes")
}

fn lock_times() -> Id {
    id().child("lock-times")
}

fn font_colors() -> Id {
    id().child("font-colors")
}

fn notebook_themes() -> Id {
    id().child("notebook-themes")
}

/// Whether `word`, lowercase, starts a word of `text`: "pen" finds "Pen" and "pen-like",
/// not "Open".
fn starts_word(text: &str, word: &str) -> bool {
    let text = text.to_lowercase();
    text.match_indices(word).any(|(at, _)| {
        text[..at]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_alphanumeric())
    })
}

/// Whether every word of `query`, lowercase, starts a word in one of `texts`.
pub(crate) fn found(query: &[String], texts: &[&str]) -> bool {
    query
        .iter()
        .all(|word| texts.iter().any(|text| starts_word(text, word)))
}

/// Whether any word of `query` starts a word of `text`, which search then highlights.
fn lit(query: &[String], text: &str) -> bool {
    query.iter().any(|word| starts_word(text, word))
}

/// Whether every word of the query is in `row`'s section name, group heading, label or
/// keywords.
fn titled(query: &[String], section: &Section, group: &Group, row: &Row) -> bool {
    found(
        query,
        &[section.name, group.heading, row.label, row.keywords],
    )
}

/// Whether search shows `row`: its own texts, or a block's contents, hold the query.
fn shown(query: &[String], section: &Section, group: &Group, row: &Row) -> bool {
    titled(query, section, group, row)
        || matches!(row.control, Control::Block { finds, .. } if finds(query))
}

impl State {
    pub(crate) fn open_options(&mut self) {
        self.options = Some(Options {
            query: String::new(),
            picked: None,
            user_name: self.author.clone(),
            color_scheme: self.color_scheme,
            pages_match: !self.light_pages,
            hide_spelling: self.hide_spelling,
            markdown: self.view.editor.markdown.is_some(),
            automatic_updates: self.updates.automatic(),
            pen_pressure: self.pen_pressure,
            default_font: self.view.editor.default_font.clone(),
            page_tabs_left: self.page_tabs_left,
            navigation_bar_left: !self.navigation_bar_right,
            lock_idle: self.passwords.lock_after.is_some(),
            lock_minutes: self.passwords.lock_after.unwrap_or(10),
            lock_on_leave: self.passwords.lock_on_leave,
            presence: self.live_options.presence,
            picture: self.live_options.picture,
            relay: self.live_options.relay.clone().unwrap_or_default(),
            notebook_theme: self.notebook_theme.clone(),
            keyboard: crate::keys::Keyboard::new(),
        });
        self.ui.open_popup(id());
        self.ui.set_focus(Some(search()));
    }

    /// Builds the Options dialog while it is open. OK, or Enter in the user name, keeps its
    /// choices with a non-empty user name; Cancel, Escape or a press outside leave them.
    pub(crate) fn options_dialog(&mut self) {
        let Some(mut options) = self.options.take() else {
            return;
        };
        if !self.ui.popup_open(id()) {
            return;
        }
        let theme = self.ui.theme.clone();
        let row = row_height(&theme);
        let ui = &mut self.ui;
        let entered = ui::popup::navigation(ui, &[user_name()], &[NamedKey::Enter])
            .contains(&NamedKey::Enter);
        // As tall as its contents up to the window, which the list then scrolls within.
        let loose = Extent {
            size: Size::Children,
            strictness: 0.0,
        };
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(WIDTH), loose],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [6.0, 6.0],
                gap: 6.0,
                anchor: Some(Anchor::Dialog),
                role: Some(Role::Dialog),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id()) {
            node.set_label("Options");
        }
        let before = options.query.clone();
        ui::text_field(
            ui,
            search(),
            &mut options.query,
            "Search options",
            Spec {
                size: [fill(), px(row)],
                fill: Some(theme.base),
                border: Some(if ui.focused() == Some(search()) {
                    theme.accent
                } else {
                    theme.chip
                }),
                radius: 4.0,
                pad: [8.0, 0.0],
                role: Some(Role::SearchInput),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(search()) {
            node.set_label("Search options");
        }
        let query: Vec<String> = options
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if options.query != before {
            options.picked = None;
            ui.scroll_to(list(), list().child("top"));
        }
        let sections: Vec<&Section> = SECTIONS
            .iter()
            .filter(|section| {
                section.groups.iter().any(|group| {
                    group
                        .rows
                        .iter()
                        .any(|row| shown(&query, section, group, row))
                })
            })
            .collect();
        ui.open(
            "body",
            Spec {
                size: [fill(), loose],
                gap: 6.0,
                ..Spec::default()
            },
        );
        let index = ui.open(
            "index",
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
        if let Some(node) = ui.access(index) {
            node.set_orientation(accesskit::Orientation::Vertical);
        }
        // The section in the top quarter of the list, or the last once the list is scrolled to
        // its end.
        let current = options.picked.or_else(|| {
            let [_, top, _, bottom] = ui.rect(list())?;
            let starts = |id| ui.rect(id).map(|rect: [f32; 4]| rect[1]);
            let end = starts(list().child("top")).is_some_and(|start| start < top - 1.0)
                && starts(list().child("end")).is_some_and(|end| end <= bottom + 1.0);
            sections
                .iter()
                .rfind(|section| {
                    end || starts(section_id(section))
                        .is_some_and(|start| start <= top + (bottom - top) / 4.0)
                })
                .or(sections.first())
                .map(|section| section.name)
        });
        for section in &sections {
            let marked = current == Some(section.name);
            let tab = ui.open(
                section.name,
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fill(), px(row)],
                    text: Some(section.name),
                    bold: marked,
                    fill: marked.then(|| theme.hover()),
                    hover_fill: Some(theme.hover()),
                    radius: 4.0,
                    pad: [8.0, 0.0],
                    role: Some(Role::Tab),
                    ..Spec::default()
                },
            );
            if let Some(node) = ui.access(tab) {
                node.set_selected(marked);
            }
            ui.close();
            if ui.signal(tab).clicked {
                options.picked = Some(section.name);
                ui.scroll_to(list(), section_id(section));
            }
        }
        ui.close();
        ui.open_as(
            list(),
            Spec {
                flags: Flags::SCROLL | Flags::CLIP,
                axis: Axis::Y,
                size: [fill(), loose],
                pad: [10.0, 0.0],
                ..Spec::default()
            },
        );
        if !ui.signal(list()).events.is_empty() {
            options.picked = None;
        }
        ui.leaf(
            "top",
            Spec {
                size: [fill(), px(0.0)],
                ..Spec::default()
            },
        );
        if sections.is_empty() {
            ui.leaf(
                "none",
                Spec {
                    size: [fill(), px(row * 2.0)],
                    text: Some("No options match"),
                    color: Some(theme.text_dim),
                    center: true,
                    ..Spec::default()
                },
            );
        }
        for section in &sections {
            self.ui.open_as(
                section_id(section),
                Spec {
                    axis: Axis::Y,
                    size: [fill(), ui::children()],
                    pad: [0.0, 4.0],
                    gap: 4.0,
                    ..Spec::default()
                },
            );
            title(
                &mut self.ui,
                &theme,
                section.name,
                lit(&query, section.name),
            );
            for group in section.groups {
                let rows: Vec<&Row> = group
                    .rows
                    .iter()
                    .filter(|row| shown(&query, section, group, row))
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                if !group.heading.is_empty() {
                    heading(
                        &mut self.ui,
                        &theme,
                        group.heading,
                        lit(&query, group.heading),
                    );
                }
                for row in rows {
                    let highlight = !matches!(row.control, Control::Block { .. })
                        && (lit(&query, row.label) || lit(&query, row.keywords));
                    self.ui.open(
                        row.label,
                        Spec {
                            size: [fill(), ui::children()],
                            fill: highlight.then(|| ui::mix(theme.popup, theme.accent, 0.18)),
                            radius: 4.0,
                            pad: [8.0, 0.0],
                            gap: 8.0,
                            ..Spec::default()
                        },
                    );
                    match row.control {
                        Control::Check(choice) => {
                            let checked = choice(&mut options);
                            if ui::check_box(&mut self.ui, "check", row.label, *checked).clicked {
                                *checked = !*checked;
                            }
                        }
                        Control::Field(build) => {
                            self.ui.leaf(
                                "label",
                                Spec {
                                    size: [px(LABEL), px(row_height(&theme))],
                                    text: Some(row.label),
                                    ..Spec::default()
                                },
                            );
                            build(self, &mut options);
                        }
                        Control::Block { build, .. } => {
                            let titled = titled(&query, section, group, row);
                            build(self, &mut options, if titled { &[] } else { &query });
                        }
                    }
                    self.ui.close();
                }
            }
            self.ui.close();
        }
        let ui = &mut self.ui;
        ui.leaf(
            "end",
            Spec {
                size: [fill(), px(6.0)],
                ..Spec::default()
            },
        );
        ui.close();
        ui.close();
        ui.open(
            "buttons",
            Spec {
                size: [fill(), ui::children()],
                pad: [10.0, 4.0],
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
        let [cancel, ok] = ui::dialog_buttons(ui, "OK", true);
        let ok = ok || entered;
        ui.close();
        ui.close();
        let name = options.user_name.trim();
        if ok && !name.is_empty() {
            self.author = name.to_owned();
            self.color_scheme = options.color_scheme;
            self.light_pages = !options.pages_match;
            self.hide_spelling = options.hide_spelling;
            self.view.editor.markdown = crate::commands::markdown(options.markdown);
            self.pen_pressure = options.pen_pressure;
            self.view.editor.default_font = options.default_font;
            self.page_tabs_left = options.page_tabs_left;
            self.navigation_bar_right = !options.navigation_bar_left;
            self.passwords = crate::settings::Passwords {
                lock_after: options.lock_idle.then_some(options.lock_minutes),
                lock_on_leave: options.lock_on_leave,
            };
            self.notebook_theme = options.notebook_theme;
            self.updates.set_automatic(options.automatic_updates);
            let relay = options.relay.trim();
            self.live_options = crate::settings::Live {
                presence: options.presence,
                picture: options.picture,
                relay: (!relay.is_empty()).then(|| relay.to_owned()),
            };
            #[cfg(feature = "live")]
            crate::live::configure(&self.author, &self.live_options);
            self.show_spelling();
            self.follow_color_scheme();
            self.install_keymap(options.keyboard.keymap);
            self.save_settings();
        } else if !cancel {
            self.options = Some(options);
            return;
        }
        self.ui.close_popup(id());
    }
}

#[cfg(feature = "live")]
fn relay(state: &mut State, options: &mut Options) {
    let ui = &mut state.ui;
    let theme = ui.theme.clone();
    let field = id().child("relay");
    ui::text_field(
        ui,
        field,
        &mut options.relay,
        crate::live::DEFAULT_RELAY,
        Spec {
            size: [fill(), px(row_height(&theme))],
            fill: Some(theme.base),
            border: Some(theme.chip),
            radius: 4.0,
            pad: [6.0, 0.0],
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(field) {
        node.set_label("Live Share relay");
    }
}

fn section_id(section: &Section) -> Id {
    list().child(section.name)
}

fn row_height(theme: &Theme) -> f32 {
    theme.font_size * 2.0
}

/// A section's name over its groups, tinted where search found it.
fn title(ui: &mut Ui, theme: &Theme, text: &str, lit: bool) {
    ui.leaf(
        "title",
        Spec {
            size: [fill(), px(theme.font_size * 2.2)],
            text: Some(text),
            font_size: Some(theme.font_size * 1.25),
            bold: true,
            color: lit.then_some(theme.accent),
            role: Some(Role::Heading),
            ..Spec::default()
        },
    );
}

/// A group's heading on a band, as OneNote heads the groups of its Options pages.
fn heading(ui: &mut Ui, theme: &Theme, text: &str, lit: bool) {
    let band = ui::mix(theme.panel, theme.chip, 0.4);
    ui.leaf(
        text,
        Spec {
            size: [fill(), px(theme.font_size * 1.8)],
            text: Some(text),
            bold: true,
            fill: Some(if lit {
                ui::mix(band, theme.accent, 0.3)
            } else {
                band
            }),
            radius: 4.0,
            pad: [8.0, 0.0],
            role: Some(Role::Heading),
            ..Spec::default()
        },
    );
}

fn appearance(state: &mut State, options: &mut Options) {
    let ui = &mut state.ui;
    let combo = ui.id("combo");
    let current = SCHEMES
        .iter()
        .find(|(scheme, _)| *scheme == options.color_scheme)
        .map_or("", |(_, name)| name);
    ui::shell::combo(ui, "combo", "Appearance", current, 140.0, schemes(), true);
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
}

fn notebook_theme(state: &mut State, options: &mut Options) {
    let ui = &mut state.ui;
    let combo = ui.id("combo");
    let themes = notebook::sidecar::themes::built_in();
    let choices: Vec<(Option<&str>, &str)> = std::iter::once((None, "No Theme"))
        .chain(
            themes
                .iter()
                .map(|theme| (Some(theme.id.as_str()), theme.name.as_str())),
        )
        .collect();
    let chosen = options.notebook_theme.as_deref();
    let current = choices
        .iter()
        .find(|(id, _)| *id == chosen)
        .map_or("", |(_, name)| name);
    let title = "Default for new notebooks";
    ui::shell::combo(ui, "combo", title, current, 140.0, notebook_themes(), true);
    let items: Vec<Item> = choices
        .iter()
        .map(|(id, name)| Item {
            text: name,
            checked: Some(*id == chosen),
            current: *id == chosen,
            ..Item::default()
        })
        .collect();
    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
    if let Some(index) = ui::popup::menu(ui, notebook_themes(), anchor, &items, None) {
        options.notebook_theme = choices[index].0.map(Into::into);
    }
}

fn lock_after(state: &mut State, options: &mut Options) {
    let ui = &mut state.ui;
    let combo = ui.id("combo");
    let current = crate::protection::AFTER
        .iter()
        .find(|(minutes, _)| *minutes == options.lock_minutes)
        .map_or("", |(_, name)| name);
    ui::shell::combo(
        ui,
        "combo",
        "Amount of time",
        current,
        140.0,
        lock_times(),
        true,
    );
    let items = crate::protection::AFTER.map(|(minutes, name)| Item {
        text: name,
        checked: Some(minutes == options.lock_minutes),
        current: minutes == options.lock_minutes,
        ..Item::default()
    });
    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
    if let Some(index) = ui::popup::menu(ui, lock_times(), anchor, &items, None) {
        options.lock_minutes = crate::protection::AFTER[index].0;
        options.lock_idle = true;
    }
}

fn user_name_field(state: &mut State, options: &mut Options) {
    let ui = &mut state.ui;
    let theme = &ui.theme;
    let width = Extent {
        size: Size::Pixels(260.0),
        strictness: 0.0,
    };
    let spec = Spec {
        size: [width, px(row_height(theme))],
        fill: Some(theme.base),
        border: Some(theme.accent),
        radius: 4.0,
        pad: [6.0, 0.0],
        ..Spec::default()
    };
    ui::text_field(ui, user_name(), &mut options.user_name, "", spec);
    if let Some(node) = ui.access(user_name()) {
        node.set_label("User name");
    }
}

fn font_face(state: &mut State, options: &mut Options) {
    let State { ui, fonts, .. } = state;
    let font = &mut options.default_font;
    let combo = ui.id("combo");
    ui::shell::combo(ui, "combo", "Font", &font.face, 200.0, self::fonts(), true);
    let items: Vec<_> = fonts
        .iter()
        .map(|name| Item {
            text: name,
            font: Some(name),
            current: *name == font.face,
            ..Item::default()
        })
        .collect();
    let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
    if let Some(index) = ui::popup::menu(ui, self::fonts(), anchor, &items, Some("Font")) {
        font.face = fonts[index].clone();
    }
}

fn font_size(state: &mut State, options: &mut Options) {
    let ui = &mut state.ui;
    let font = &mut options.default_font;
    let combo = ui.id("combo");
    let size = format!("{}", font.size);
    ui::shell::combo(ui, "combo", "Size", &size, 60.0, sizes(), true);
    let labels = SIZES.map(|size| format!("{size}"));
    let items: Vec<_> = labels
        .iter()
        .map(|label| Item {
            text: label,
            checked: Some(*label == size),
            current: *label == size,
            ..Item::default()
        })
        .collect();
    let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
    if let Some(index) = ui::popup::menu(ui, sizes(), anchor, &items, None) {
        font.size = SIZES[index];
    }
}

fn font_color(state: &mut State, options: &mut Options) {
    let ui = &mut state.ui;
    let font = &mut options.default_font;
    let combo = ui.id("combo");
    let shown = crate::FONT_COLORS
        .iter()
        .find(|(color, _)| Some(*color) == font.color)
        .map_or("Automatic", |(_, name)| name);
    ui::shell::combo(ui, "combo", "Font color", shown, 200.0, font_colors(), true);
    let swatches: Vec<_> = crate::FONT_COLORS
        .iter()
        .map(|&(color, name)| (canvas::gpu::colorref(color), name))
        .collect();
    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
    if let Some(chosen) = ui::popup::colors(ui, font_colors(), anchor, "Automatic", &swatches, 10) {
        font.color = chosen.and_then(|chosen| {
            let at = swatches.iter().position(|(swatch, _)| *swatch == chosen)?;
            Some(crate::FONT_COLORS[at].0)
        });
    }
}

/// A path shown dimmed and cut to the row.
fn path(ui: &mut Ui, path: &std::path::Path) {
    let spec = Spec {
        flags: Flags::CLIP,
        size: [fill(), px(row_height(&ui.theme))],
        text: Some(&path.to_string_lossy()),
        color: Some(ui.theme.text_dim),
        ..Spec::default()
    };
    ui.leaf("path", spec);
}

fn cache(state: &mut State, _: &mut Options) {
    path(&mut state.ui, &state.cache);
    let label = if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Open Folder"
    };
    if ui::button(&mut state.ui, "reveal", label).clicked {
        platform::reveal(&state.cache);
    }
}

fn version(state: &mut State, _: &mut Options) {
    let ui = &mut state.ui;
    let spec = Spec {
        size: [fill(), px(row_height(&ui.theme))],
        text: Some(&update::describe_running()),
        ..Spec::default()
    };
    ui.leaf("version", spec);
}

/// Where Install put Snowbound and Uninstall, or Install where it isn't in the app menu.
#[cfg(target_os = "linux")]
fn installed(state: &mut State, _: &mut Options) {
    let ui = &mut state.ui;
    if crate::desktop::uninstallable() {
        path(ui, &crate::desktop::binary().unwrap_or_default());
        if ui::button(ui, "uninstall", "Uninstall").clicked {
            crate::desktop::uninstall(&state.proxy);
        }
    } else if crate::desktop::installable() {
        path(ui, std::path::Path::new("Not in the app menu"));
        if ui::button(ui, "install", "Install").clicked {
            crate::desktop::install();
        }
    } else {
        path(ui, std::path::Path::new("By your system's package manager"));
    }
}

#[cfg(test)]
mod tests {
    use super::found;

    #[test]
    fn search_words_match_the_starts_of_words() {
        let query = |text: &str| vec![text.to_owned()];
        assert!(found(&query("pen"), &["Use pen pressure sensitivity"]));
        assert!(found(&query("pen"), &["Pen"]));
        assert!(!found(&query("pen"), &["Open Notebook"]));
        assert!(found(&query("⌘b"), &["⌘B"]));
        assert!(found(&query("storage"), &["Sync & Storage"]));
        assert!(
            !found(&["pen".into(), "zzz".into()], &["Pen"]),
            "every word"
        );
    }
}
