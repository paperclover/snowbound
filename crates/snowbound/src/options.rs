//! OneNote's Options dialog as one scrolling list of sections (`resources/settings.md`):
//! indexed on the left, filtered by the search field above, each choice kept when OK is
//! chosen.

use crate::{
    State, UserEvent,
    settings::{Backend, ColorScheme},
    update,
};
use accesskit::Role;
use canvas::editor::DefaultFont;
use ui::{Anchor, Axis, Extent, Flags, Id, Size, Spec, Theme, Ui, fill, popup::Item, px};
use winit::keyboard::NamedKey;

const WIDTH: f32 = 700.0;
const NAV: f32 = 150.0;
const SCHEMES: [(ColorScheme, &str); 3] = [
    (ColorScheme::System, "System"),
    (ColorScheme::Light, "Light"),
    (ColorScheme::Dark, "Dark"),
];
const SIDES: [&str; 2] = ["Left", "Right"];
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
    /// A line under the control for what the label can't say; empty for none.
    hint: &'static str,
    /// Other words search finds the row by.
    keywords: &'static str,
    control: Control,
}

impl Row {
    const fn new(label: &'static str, keywords: &'static str, control: Control) -> Self {
        Self {
            label,
            hint: "",
            keywords,
            control,
        }
    }

    const fn hint(self, hint: &'static str) -> Self {
        Self { hint, ..self }
    }
}

/// Every control stands in one column, after the labels' column.
enum Control {
    /// A check box labelled by the row, bound to a choice.
    Check(fn(&mut Options) -> &mut bool),
    /// The row's label in the labels' column, then what the function builds, given the
    /// label to name its control by.
    Field(fn(&mut State, &mut Options, &str)),
    /// What `build` makes across the row, unlabelled, from the search's words; `finds` tells
    /// whether they find anything in it. Words found in the row's own texts give it none.
    Block {
        build: fn(&mut State, &mut Options, &[String]),
        finds: fn(&[String]) -> bool,
    },
}

/// Every section, in the list's order, with only the rows that apply here.
const SECTIONS: &[Section] = &[
    Section {
        name: "General",
        groups: &[
            Group {
                heading: "Interface",
                rows: &[
                    Row::new(
                        "Appearance:",
                        "color colour scheme dark light mode system theme",
                        Control::Field(appearance),
                    ),
                    Row::new(
                        "Pages match the appearance",
                        "dark white paper background ui theme",
                        Control::Check(|options| &mut options.pages_match),
                    )
                    .hint("Turned off, pages stay white"),
                ],
            },
            Group {
                heading: "Personalize",
                rows: &[
                    Row::new("User name:", "author", Control::Field(user_name_field))
                        .hint("Shown with your edits in shared notebooks"),
                ],
            },
        ],
    },
    Section {
        name: "Editing",
        groups: &[
            Group {
                heading: "Default font",
                rows: &[
                    Row::new(
                        "Font:",
                        "typeface size points text new",
                        Control::Field(font_face),
                    ),
                    Row::new(
                        "Color:",
                        "font colour text new automatic",
                        Control::Field(font_color),
                    ),
                ],
            },
            Group {
                heading: "Font styles",
                rows: &[Row::new(
                    "New notebooks:",
                    "default style theme headings modern",
                    Control::Field(notebook_theme),
                )
                .hint("The styles a new notebook starts with")],
            },
            Group {
                heading: "Proofing",
                rows: &[Row::new(
                    "Hide spelling errors",
                    "spell check misspelled words",
                    Control::Check(|options| &mut options.hide_spelling),
                )],
            },
            Group {
                heading: "AutoFormat",
                rows: &[Row::new(
                    "Markdown shortcuts",
                    "markdown autoformat heading bullet numbering list to do quote bold \
                     italic strikethrough code typing",
                    Control::Check(|options| &mut options.markdown),
                )
                .hint("Type # for a heading, - for a bullet, **bold** and more")],
            },
            Group {
                heading: "Pen",
                rows: &[Row::new(
                    "Use pen pressure sensitivity",
                    "tablet stylus ink stroke width drawing",
                    Control::Check(|options| &mut options.pen_pressure),
                )],
            },
            Group {
                heading: "Passwords",
                rows: &[
                    Row::new(
                        "Lock when idle for:",
                        "password protect protected lock unlock idle minutes hours timeout \
                         security",
                        Control::Field(lock_after),
                    )
                    .hint("Password-protected sections you haven't worked in"),
                    Row::new(
                        "Lock when I leave a section",
                        "password protect protected lock navigate away switch security",
                        Control::Check(|options| &mut options.lock_on_leave),
                    ),
                ],
            },
        ],
    },
    Section {
        name: "Display",
        groups: &[Group {
            heading: "",
            rows: &[
                Row::new(
                    "Page tabs:",
                    "pages list side left right layout",
                    Control::Field(|state, options, name| {
                        side(state, name, &mut options.page_tabs_left)
                    }),
                ),
                Row::new(
                    "Notebooks:",
                    "navigation bar sidebar side left right layout",
                    Control::Field(|state, options, name| {
                        side(state, name, &mut options.navigation_bar_left)
                    }),
                ),
                Row::new(
                    "Renderer:",
                    "graphics gpu backend drawing metal opengl vulkan direct3d webgpu webgl \
                     canvas",
                    Control::Field(renderer),
                ),
            ],
        }],
    },
    Section {
        name: "Sync & Storage",
        groups: &[
            #[cfg(not(target_arch = "wasm32"))]
            Group {
                heading: "Cache",
                rows: &[Row::new(
                    "Location:",
                    "cache file folder path replica data disk",
                    Control::Field(cache),
                )
                .hint("Where Snowbound keeps a copy of each section it syncs")],
            },
            #[cfg(feature = "live")]
            Group {
                heading: "Live Share",
                rows: &[
                    Row::new(
                        "Show others where I am in shared notebooks",
                        "presence privacy cursor caret avatar collaborate people",
                        Control::Check(|options| &mut options.presence),
                    ),
                    Row::new(
                        "Show my account picture with my name",
                        "presence privacy photo avatar",
                        Control::Check(|options| &mut options.picture),
                    ),
                    Row::new(
                        "Relay:",
                        "live share server internet network presence",
                        Control::Field(relay),
                    ),
                ],
            },
        ],
    },
    Section {
        name: "Updates",
        groups: &[Group {
            heading: "",
            rows: &[
                Row::new("Version:", "about build release", Control::Field(version)),
                #[cfg(not(target_arch = "wasm32"))]
                Row::new(
                    "Check for updates automatically",
                    "update download",
                    Control::Check(|options| &mut options.automatic_updates),
                ),
                #[cfg(target_arch = "wasm32")]
                Row::new(
                    "Update automatically",
                    "update reload newer version",
                    Control::Check(|options| &mut options.automatic_updates),
                )
                .hint("Reloads into new versions while you're idle, your work saved"),
                #[cfg(target_os = "linux")]
                Row::new(
                    "Installed:",
                    "install uninstall app menu remove",
                    Control::Field(installed),
                ),
            ],
        }],
    },
    Section {
        name: "Keyboard",
        groups: &[Group {
            heading: "",
            rows: &[Row::new(
                "Shortcuts",
                "keys chords hotkeys bindings commands menus",
                Control::Block {
                    build: |state, options, query| options.keyboard.build(&mut state.ui, query),
                    finds: crate::keys::searched,
                },
            )],
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
    /// Minutes, as `protection::AFTER` lists them; none never locks.
    lock_after: Option<u32>,
    lock_on_leave: bool,
    presence: bool,
    picture: bool,
    /// Live Share's relay; empty uses Snowbound's.
    relay: String,
    notebook_theme: Option<String>,
    /// Update Now was chosen: OK, then reload into the newer build.
    #[cfg(target_arch = "wasm32")]
    update_now: bool,
    renderer: Backend,
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

fn fonts() -> Id {
    id().child("fonts")
}

fn font_colors() -> Id {
    id().child("font-colors")
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
            lock_after: self.passwords.lock_after,
            lock_on_leave: self.passwords.lock_on_leave,
            presence: self.live_options.presence,
            picture: self.live_options.picture,
            relay: self.live_options.relay.clone().unwrap_or_default(),
            notebook_theme: self.notebook_theme.clone(),
            renderer: self.renderer_choice,
            #[cfg(target_arch = "wasm32")]
            update_now: false,
            keyboard: crate::keys::Keyboard::new(),
        });
        self.ui.open_popup(id());
        self.ui.set_focus(Some(search()));
    }

    /// Builds the Options dialog while it is open. OK, or Enter in the user name, keeps its
    /// choices, and a user name that isn't blank; Cancel, Escape or a press outside leave
    /// them.
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
        let column = labels_column(ui);
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
                    if let Control::Block { build, .. } = row.control {
                        let titled = titled(&query, section, group, row);
                        build(self, &mut options, if titled { &[] } else { &query });
                    } else {
                        self.row(&mut options, row, column);
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
        #[cfg(target_arch = "wasm32")]
        let ok = ok || options.update_now;
        ui.close();
        ui.close();
        if ok {
            let name = options.user_name.trim();
            if !name.is_empty() {
                self.author = name.to_owned();
            }
            self.color_scheme = options.color_scheme;
            self.light_pages = !options.pages_match;
            self.hide_spelling = options.hide_spelling;
            self.view.editor.markdown = crate::commands::markdown(options.markdown);
            self.pen_pressure = options.pen_pressure;
            self.view.editor.default_font = options.default_font;
            self.page_tabs_left = options.page_tabs_left;
            self.navigation_bar_right = !options.navigation_bar_left;
            self.passwords = crate::settings::Passwords {
                lock_after: options.lock_after,
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
            let switch = options.renderer != self.renderer_choice;
            self.renderer_choice = options.renderer;
            self.save_settings();
            if switch {
                let _ = self.proxy.send_event(UserEvent::Renderer(options.renderer));
            }
            #[cfg(target_arch = "wasm32")]
            if options.update_now {
                self.ui.close_popup(id());
                return crate::platform::update_now(self);
            }
        } else if !cancel {
            self.options = Some(options);
            return;
        }
        self.ui.close_popup(id());
    }

    /// A row's label in the labels' column `column` wide, then its control over its hint.
    fn row(&mut self, options: &mut Options, row: &Row, column: f32) {
        let theme = self.ui.theme.clone();
        let height = row_height(&theme);
        let field = matches!(row.control, Control::Field(_));
        self.ui.leaf(
            "label",
            Spec {
                size: [px(column), px(height)],
                text: field.then_some(row.label),
                ..Spec::default()
            },
        );
        self.ui.open(
            "control",
            Spec {
                axis: Axis::Y,
                size: [fill(), ui::children()],
                ..Spec::default()
            },
        );
        match row.control {
            Control::Check(choice) => {
                let checked = choice(options);
                if ui::check_box(&mut self.ui, "check", row.label, *checked).clicked {
                    *checked = !*checked;
                }
            }
            Control::Field(build) => {
                self.ui.open(
                    "field",
                    Spec {
                        size: [fill(), ui::children()],
                        gap: 8.0,
                        ..Spec::default()
                    },
                );
                build(self, options, row.label.trim_end_matches(':'));
                self.ui.close();
            }
            Control::Block { .. } => {}
        }
        if !row.hint.is_empty() {
            self.ui.leaf(
                "hint",
                Spec {
                    size: [fill(), ui::fit()],
                    text: Some(row.hint),
                    font_size: Some((theme.font_size * 0.92).round()),
                    color: Some(theme.text_dim),
                    overflow: ui::Overflow::Wrap,
                    pad: [0.0, 3.0],
                    ..Spec::default()
                },
            );
        }
        self.ui.close();
    }
}

/// How wide the labels' column stands: as the widest field label, so every control lines up.
fn labels_column(ui: &mut Ui) -> f32 {
    let labels = (SECTIONS.iter())
        .flat_map(|section| section.groups)
        .flat_map(|group| group.rows)
        .filter(|row| matches!(row.control, Control::Field(_)));
    let widest = labels.fold(0.0_f32, |widest, row| widest.max(ui.measure(row.label)[0]));
    (widest + 12.0).ceil()
}

#[cfg(feature = "live")]
fn relay(state: &mut State, options: &mut Options, name: &str) {
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
        node.set_label(name);
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

/// A combo named `name` showing `choices[chosen]`, `width` wide, whose menu picks another.
fn dropdown(ui: &mut Ui, name: &str, choices: &[&str], chosen: usize, width: f32) -> Option<usize> {
    let menu = id().child(name);
    let combo = ui.id("combo");
    let current = choices.get(chosen).copied().unwrap_or_default();
    ui::shell::combo(ui, "combo", name, current, width, menu, true);
    let items: Vec<Item> = (choices.iter().enumerate())
        .map(|(index, text)| Item {
            text,
            checked: Some(index == chosen),
            current: index == chosen,
            ..Item::default()
        })
        .collect();
    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
    ui::popup::menu(ui, menu, anchor, &items, None)
}

fn appearance(state: &mut State, options: &mut Options, name: &str) {
    let chosen = (SCHEMES.iter())
        .position(|(scheme, _)| *scheme == options.color_scheme)
        .unwrap_or_default();
    let choices = SCHEMES.map(|(_, label)| label);
    if let Some(index) = ui::segmented(&mut state.ui, "schemes", name, &choices, chosen) {
        options.color_scheme = SCHEMES[index].0;
    }
}

/// Left or Right, for a pane on the left where `left`.
fn side(state: &mut State, name: &str, left: &mut bool) {
    if let Some(index) = ui::segmented(&mut state.ui, "side", name, &SIDES, usize::from(!*left)) {
        *left = index == 0;
    }
}

fn notebook_theme(state: &mut State, options: &mut Options, name: &str) {
    let themes = notebook::sidecar::themes::built_in();
    let ids: Vec<Option<&str>> = std::iter::once(None)
        .chain(themes.iter().map(|theme| Some(theme.id.as_str())))
        .collect();
    let labels: Vec<&str> = std::iter::once("No Theme")
        .chain(themes.iter().map(|theme| theme.name.as_str()))
        .collect();
    let chosen = (ids.iter())
        .position(|id| *id == options.notebook_theme.as_deref())
        .unwrap_or_default();
    if let Some(index) = dropdown(&mut state.ui, name, &labels, chosen, 160.0) {
        options.notebook_theme = ids[index].map(Into::into);
    }
}

/// The platform's backends, the one the default starts with marked, those that didn't start
/// this run disabled; then the backend and adapter drawing now.
fn renderer(state: &mut State, options: &mut Options, name: &str) {
    let State {
        ui,
        unavailable,
        drawing,
        ..
    } = state;
    let failed = |backend: Backend| {
        let (_, reason) = unavailable.iter().find(|(failed, _)| *failed == backend)?;
        // A long reason is the system's, kept in the log.
        Some(if reason.chars().count() <= 28 {
            reason.as_str()
        } else {
            "Didn't start"
        })
    };
    let automatic = (Backend::PLATFORM.iter().copied()).find(|backend| failed(*backend).is_none());
    let label = |backend: Backend| {
        if Some(backend) == automatic {
            format!("{} (Default)", backend.label())
        } else {
            backend.label().to_owned()
        }
    };
    let shown = match options.renderer {
        Backend::Default => automatic,
        chosen => Some(chosen),
    };
    let labels: Vec<String> = Backend::PLATFORM.iter().copied().map(label).collect();
    let current = shown.map(label).unwrap_or_default();
    let combo = ui.id("combo");
    let menu = id().child(name);
    ui::shell::combo(ui, "combo", name, &current, 160.0, menu, true);
    let items: Vec<Item> = (Backend::PLATFORM.iter().zip(&labels))
        .map(|(backend, text)| Item {
            text,
            checked: Some(Some(*backend) == shown),
            current: Some(*backend) == shown,
            disabled: failed(*backend).is_some(),
            badge: failed(*backend),
            ..Item::default()
        })
        .collect();
    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
    if let Some(index) = ui::popup::menu(ui, menu, anchor, &items, None) {
        let picked = Backend::PLATFORM[index];
        options.renderer = if Some(picked) == automatic {
            Backend::Default
        } else {
            picked
        };
    }
    let (backend, adapter) = &*drawing;
    let used = match adapter.as_str() {
        "" => backend.label().to_owned(),
        adapter if adapter == backend.label() => adapter.to_owned(),
        adapter => format!("{} · {adapter}", backend.label()),
    };
    let spec = Spec {
        flags: Flags::CLIP,
        size: [fill(), px(row_height(&ui.theme))],
        text: Some(&used),
        color: Some(ui.theme.text_dim),
        ..Spec::default()
    };
    ui.leaf("adapter", spec);
}

/// Never, or one of OneNote's times, `protection::AFTER`.
fn lock_after(state: &mut State, options: &mut Options, name: &str) {
    let after = crate::protection::AFTER;
    let labels: Vec<&str> = std::iter::once("Never")
        .chain(after.iter().map(|(_, label)| *label))
        .collect();
    let chosen = options
        .lock_after
        .and_then(|minutes| after.iter().position(|(each, _)| *each == minutes))
        .map_or(0, |index| index + 1);
    if let Some(index) = dropdown(&mut state.ui, name, &labels, chosen, 160.0) {
        options.lock_after = index.checked_sub(1).map(|index| after[index].0);
    }
}

fn user_name_field(state: &mut State, options: &mut Options, name: &str) {
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
        node.set_label(name);
    }
}

/// The face, from the font menu, and its size.
fn font_face(state: &mut State, options: &mut Options, name: &str) {
    let State { ui, fonts, .. } = state;
    let font = &mut options.default_font;
    let combo = ui.id("combo");
    ui::shell::combo(ui, "combo", name, &font.face, 200.0, self::fonts(), true);
    let items: Vec<_> = fonts
        .iter()
        .map(|face| Item {
            text: face,
            font: Some(face),
            current: *face == font.face,
            ..Item::default()
        })
        .collect();
    let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
    if let Some(index) = ui::popup::menu(ui, self::fonts(), anchor, &items, Some(name)) {
        font.face = fonts[index].clone();
    }
    let labels = SIZES.map(|size| format!("{size}"));
    let shown = format!("{}", font.size);
    let mut choices: Vec<&str> = labels.iter().map(String::as_str).collect();
    // A size typed in the toolbar's box may be none of OneNote's.
    let chosen = SIZES.iter().position(|size| *size == font.size);
    let chosen = chosen.unwrap_or_else(|| {
        choices.push(&shown);
        choices.len() - 1
    });
    ui.open(
        "size",
        Spec {
            size: [ui::children(), ui::children()],
            ..Spec::default()
        },
    );
    if let Some(index) = dropdown(ui, "Size", &choices, chosen, 64.0) {
        font.size = SIZES.get(index).copied().unwrap_or(font.size);
    }
    ui.close();
}

fn font_color(state: &mut State, options: &mut Options, name: &str) {
    let ui = &mut state.ui;
    let font = &mut options.default_font;
    let combo = ui.id("combo");
    let shown = crate::FONT_COLORS
        .iter()
        .find(|(color, _)| Some(*color) == font.color)
        .map_or("Automatic", |(_, name)| name);
    ui::shell::combo(ui, "combo", name, shown, 200.0, font_colors(), true);
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

/// Text shown dimmed and cut to the row, as a path.
#[cfg(not(target_arch = "wasm32"))]
fn dim(ui: &mut Ui, text: &str) {
    let spec = Spec {
        flags: Flags::CLIP,
        size: [fill(), px(row_height(&ui.theme))],
        text: Some(text),
        color: Some(ui.theme.text_dim),
        ..Spec::default()
    };
    ui.leaf("dim", spec);
}

#[cfg(not(target_arch = "wasm32"))]
fn cache(state: &mut State, _: &mut Options, _: &str) {
    dim(&mut state.ui, &state.cache.to_string_lossy());
    let label = if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Open Folder"
    };
    if ui::button(&mut state.ui, "reveal", label).clicked {
        crate::platform::reveal(&state.cache);
    }
}

/// The build running, and in the browser Update Now once a newer one is fetched.
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
fn version(state: &mut State, options: &mut Options, _: &str) {
    let ui = &mut state.ui;
    let spec = Spec {
        size: [fill(), px(row_height(&ui.theme))],
        text: Some(&update::describe_running()),
        ..Spec::default()
    };
    ui.leaf("version", spec);
    #[cfg(target_arch = "wasm32")]
    if crate::platform::update_ready() && ui::button(ui, "update", "Update Now").clicked {
        options.update_now = true;
    }
}

/// Where Install put Snowbound and Uninstall, or Install where it isn't in the app menu.
#[cfg(target_os = "linux")]
fn installed(state: &mut State, _: &mut Options, _: &str) {
    let ui = &mut state.ui;
    if crate::desktop::uninstallable() {
        dim(
            ui,
            &crate::desktop::binary()
                .unwrap_or_default()
                .to_string_lossy(),
        );
        if ui::button(ui, "uninstall", "Uninstall").clicked {
            crate::desktop::uninstall(&state.proxy);
        }
    } else if crate::desktop::installable() {
        dim(ui, "Not in the app menu");
        if ui::button(ui, "install", "Install").clicked {
            crate::desktop::install();
        }
    } else {
        dim(ui, "By your system's package manager");
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
