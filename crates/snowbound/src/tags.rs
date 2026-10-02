//! OneNote 2010's Customize Tags dialog and its New Tag and Modify Tag dialog, which edit
//! the user's tag list: the tags the toolbar, the menus and Ctrl+1 to Ctrl+9 apply.

use crate::{State, commands, platform};
use accesskit::Role;
use canvas::editor::NoteTag;
use canvas::gpu::{colorref, tag_sources};
use canvas::outline::{TagIcon, symbol_name};
use notebook::sidecar::art_name;
use ui::{Anchor, Axis, Flags, Id, Spec, Ui, children, fill, fit, px};
use winit::keyboard::NamedKey;

const WIDTH: f32 = 300.0;
const EDITOR_WIDTH: f32 = 280.0;
const ROW: f32 = 20.0;
/// Rows the list shows at once; the wheel scrolls the rest.
const ROWS: f32 = 19.0;
const SIDE: f32 = 16.0;
const CELL: f32 = 23.0;

/// OneNote 2010's symbol gallery: ten rows of five groups of three, 0 for None, which spans
/// the first group's top row, and for the empty cells at the end.
const GALLERY: [[u16; 15]; 10] = [
    [
        0, 0, 0, 40, 13, 61, 41, 82, 62, 106, 107, 108, 124, 125, 126,
    ],
    [3, 2, 1, 43, 84, 64, 34, 75, 54, 109, 18, 110, 127, 128, 27],
    [6, 5, 4, 36, 77, 56, 46, 87, 67, 14, 15, 17, 129, 130, 131],
    [9, 8, 7, 42, 83, 63, 47, 88, 68, 21, 24, 111, 132, 133, 134],
    [
        12, 11, 10, 39, 81, 60, 38, 79, 58, 112, 22, 23, 135, 136, 137,
    ],
    [
        28, 69, 48, 29, 70, 49, 16, 80, 59, 113, 25, 26, 138, 139, 140,
    ],
    [
        30, 71, 50, 31, 72, 51, 37, 78, 57, 114, 115, 116, 141, 142, 143,
    ],
    [32, 73, 52, 33, 74, 53, 45, 86, 66, 117, 118, 119, 0, 0, 0],
    [94, 95, 96, 35, 76, 55, 100, 101, 102, 120, 19, 20, 0, 0, 0],
    [
        97, 98, 99, 44, 85, 65, 103, 104, 105, 121, 122, 123, 0, 0, 0,
    ],
];

/// Snowbound's own symbols past OneNote's: their names, the symbols OneNote shows in their
/// place, and their art, which a notebook keeps as it keeps a picture chosen for a tag.
const EXTRAS: [(&str, u16, &str); 15] = [
    (
        "Snowflake",
        34,
        include_str!("../assets/tags/snowflake.svg"),
    ),
    ("Rocket", 127, include_str!("../assets/tags/rocket.svg")),
    ("Fire", 140, include_str!("../assets/tags/fire.svg")),
    (
        "Thumbs Up",
        55,
        include_str!("../assets/tags/thumbs-up.svg"),
    ),
    (
        "Thumbs Down",
        113,
        include_str!("../assets/tags/thumbs-down.svg"),
    ),
    ("Laughing", 25, include_str!("../assets/tags/laughing.svg")),
    (
        "Surprised",
        25,
        include_str!("../assets/tags/surprised.svg"),
    ),
    ("Bug", 17, include_str!("../assets/tags/bug.svg")),
    ("Trophy", 26, include_str!("../assets/tags/trophy.svg")),
    ("Gift", 142, include_str!("../assets/tags/gift.svg")),
    ("Coffee", 20, include_str!("../assets/tags/coffee.svg")),
    ("Warning", 17, include_str!("../assets/tags/warning.svg")),
    ("Map Pin", 22, include_str!("../assets/tags/map-pin.svg")),
    ("Camera", 122, include_str!("../assets/tags/camera.svg")),
    ("Sparkles", 13, include_str!("../assets/tags/sparkles.svg")),
];

/// The symbol OneNote shows for a tag drawn with a picture chosen for it, until another is
/// picked.
const PICTURE_FALLBACK: u16 = 36;

/// The names a notebook keeps `EXTRAS`' art under.
fn extra_art() -> &'static [String] {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        EXTRAS
            .iter()
            .map(|(_, _, svg)| art_name(svg.as_bytes(), "svg"))
            .collect()
    })
}

/// Where the pictures chosen for tags are kept, by the names a notebook keeps them under.
fn pictures() -> Option<std::path::PathBuf> {
    Some(platform::settings_dir()?.join("tag-art"))
}

/// Tag art `art`: one of Snowbound's own symbols, or a picture chosen for a tag.
pub(crate) fn art_bytes(art: &str) -> Option<Vec<u8>> {
    match extra_art().iter().position(|name| name == art) {
        Some(extra) => Some(EXTRAS[extra].2.as_bytes().to_vec()),
        None => notebook::fs::read(pictures()?.join(art)).ok(),
    }
}

/// Font Color's swatches, Office's forty in their grid, as COLORREFs with their names.
const FONT_COLORS: [(u32, &str); 40] = [
    (0x000000, "Black"),
    (0x003399, "Brown"),
    (0x003333, "Olive Green"),
    (0x003300, "Dark Green"),
    (0x663300, "Dark Teal"),
    (0x800000, "Dark Blue"),
    (0x993333, "Indigo"),
    (0x333333, "Gray-80%"),
    (0x000080, "Dark Red"),
    (0x0066ff, "Orange"),
    (0x008080, "Dark Yellow"),
    (0x008000, "Green"),
    (0x808000, "Teal"),
    (0xff0000, "Blue"),
    (0x996666, "Blue-Gray"),
    (0x808080, "Gray-50%"),
    (0x0000ff, "Red"),
    (0x0099ff, "Light Orange"),
    (0x00cc99, "Lime"),
    (0x669933, "Sea Green"),
    (0xcccc33, "Aqua"),
    (0xff6633, "Light Blue"),
    (0x800080, "Violet"),
    (0x969696, "Gray-40%"),
    (0xff00ff, "Pink"),
    (0x00ccff, "Gold"),
    (0x00ffff, "Yellow"),
    (0x00ff00, "Bright Green"),
    (0xffff00, "Turquoise"),
    (0xffcc00, "Sky Blue"),
    (0x663399, "Plum"),
    (0xc0c0c0, "Gray-25%"),
    (0xcc99ff, "Rose"),
    (0x99ccff, "Tan"),
    (0x99ffff, "Light Yellow"),
    (0xccffcc, "Light Green"),
    (0xffffcc, "Light Turquoise"),
    (0xffcc99, "Pale Blue"),
    (0xff99cc, "Lavender"),
    (0xffffff, "White"),
];

/// Highlight Color's swatches as COLORREFs with their names, which differ from the
/// highlighter's in Sky Blue and Dark Green.
const HIGHLIGHTS: [(u32, &str); 15] = [
    (0x00ffff, "Yellow"),
    (0x00ff00, "Bright Green"),
    (0xffcc00, "Sky Blue"),
    (0xff00ff, "Pink"),
    (0xff0000, "Blue"),
    (0x0000ff, "Red"),
    (0x800000, "Dark Blue"),
    (0x808000, "Teal"),
    (0x003300, "Dark Green"),
    (0x800080, "Violet"),
    (0x000080, "Dark Red"),
    (0x008080, "Dark Yellow"),
    (0x808080, "Gray-50%"),
    (0xc0c0c0, "Gray-25%"),
    (0x000000, "Black"),
];

/// The Customize Tags dialog's list while it is open.
pub struct TagList {
    tags: Vec<NoteTag>,
    selected: Option<usize>,
    /// The New Tag or Modify Tag dialog's fields, and the place Modify Tag replaces.
    editor: Option<(NoteTag, Option<usize>)>,
}

/// What the Symbol gallery picked.
enum Pick {
    Symbol(u16),
    /// One of `EXTRAS`.
    Extra(usize),
    /// Custom Image…, which asks for a picture.
    Picture,
}

fn id() -> Id {
    Id::ROOT.child("customize-tags")
}

fn editor_id() -> Id {
    id().child("tag")
}

fn name_field() -> Id {
    editor_id().child("name")
}

fn popup(name: &str) -> Id {
    editor_id().child(("popup", name))
}

/// A tag's artwork: its own art, else its symbol, or none.
pub(crate) fn artwork(tag: &NoteTag) -> Option<&'static [&'static str]> {
    tag.art
        .as_deref()
        .and_then(|art| canvas::gpu::art_sources(art, || art_bytes(art)))
        .map(|[plain, _]| plain)
        .or_else(|| TagIcon::of(tag.shape, false).map(tag_sources))
}

/// The tag's name in its colours, as the list and Preview show it.
fn label(ui: &mut Ui, part: &str, text: &str, tag: &NoteTag) {
    ui.leaf(
        part,
        Spec {
            size: [fit(), px(ROW - 2.0)],
            text: Some(text),
            color: Some(tag.color.map_or(ui.theme.text, colorref)),
            fill: tag.highlight.map(colorref),
            pad: [2.0, 0.0],
            ..Spec::default()
        },
    );
}

fn icon(ui: &mut Ui, part: &str, art: Option<&'static [&'static str]>, tint: [f32; 4]) {
    ui.leaf(
        part,
        Spec {
            size: [px(SIDE + 4.0), px(ROW)],
            icon: art,
            color: Some(tint),
            center: true,
            ..Spec::default()
        },
    );
}

/// A text button that takes no clicks while not `enabled`.
fn button(ui: &mut Ui, part: &str, text: &str, enabled: bool) -> bool {
    if enabled {
        return ui::button(ui, part, text).clicked;
    }
    let theme = ui.theme.clone();
    ui.leaf(
        part,
        Spec {
            size: [fit(), px(theme.font_size * 2.0)],
            text: Some(text),
            color: Some(theme.text_dim),
            fill: Some(theme.chip),
            radius: 4.0,
            pad: [theme.font_size * 0.75, 0.0],
            center: true,
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(ui.id(part)) {
        node.set_disabled();
    }
    false
}

/// A square button showing `art` that takes no clicks while not `enabled`, named by a
/// tooltip.
fn tool(ui: &mut Ui, part: &str, art: &'static [&'static str], tip: &str, enabled: bool) -> bool {
    let text = ui.theme.text;
    if !enabled {
        ui::shell::unavailable(ui, part, art, text, false);
        ui::popup::tooltip(ui, tip, "", None);
        return false;
    }
    let clicked = ui::shell::tool_button(ui, part, art, text, None).clicked;
    ui::popup::tooltip(ui, tip, "", None);
    clicked
}

/// A button `name`d, showing `art` in its own colours, with `bar` under it, and a menu
/// arrow; opens popup `menu` and returns where it opens.
fn picker(
    ui: &mut Ui,
    part: &str,
    name: &str,
    art: Option<&'static [&'static str]>,
    bar: Option<[f32; 4]>,
    menu: Id,
    tint: [f32; 4],
) -> Anchor {
    let theme = ui.theme.clone();
    let open = ui.popup_open(menu);
    let button = ui.open(
        part,
        Spec {
            flags: Flags::CLICKABLE,
            size: [children(), px(ui::shell::TOOL)],
            fill: open.then(|| theme.hover()),
            hover_fill: Some(theme.hover()),
            border: Some(theme.chip),
            radius: 4.0,
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(button) {
        node.set_label(name);
        node.set_has_popup(accesskit::HasPopup::Menu);
        node.set_expanded(open);
    }
    ui.open(
        "icon",
        Spec {
            size: [px(ui::shell::TOOL), px(ui::shell::TOOL)],
            icon: art,
            color: Some(tint),
            center: true,
            ..Spec::default()
        },
    );
    if let Some(color) = bar {
        let top = (ui::shell::TOOL - 16.0) / 2.0;
        ui.mark([top, top + 13.0, top + 16.0, top + 16.0], color, 0.0);
    }
    ui.close();
    ui.leaf(
        "arrow",
        Spec {
            size: [px(12.0), px(ui::shell::TOOL)],
            icon: Some(ui::shell::CHEVRON),
            color: Some(theme.text_dim),
            center: true,
            ..Spec::default()
        },
    );
    ui.close();
    if ui.signal(button).pressed {
        ui.open_popup(menu);
    }
    Anchor::Below(ui.rect(button).unwrap_or_default())
}

/// The frame of a dialog titled `title`, centred in the window.
fn dialog(ui: &mut Ui, id: Id, title: &str, width: f32) {
    let theme = ui.theme.clone();
    ui.open_as(
        id,
        Spec {
            axis: Axis::Y,
            size: [px(width), children()],
            fill: Some(theme.popup),
            border: Some(theme.chip),
            shadow: Some(theme.shadow),
            radius: 8.0,
            pad: [14.0, 10.0],
            gap: 6.0,
            anchor: Some(Anchor::Dialog),
            role: Some(Role::Dialog),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(id) {
        node.set_label(title);
    }
    ui.leaf(
        "title",
        Spec {
            size: [fill(), px(ROW + 4.0)],
            text: Some(title),
            bold: true,
            role: Some(Role::Heading),
            ..Spec::default()
        },
    );
}

impl State {
    /// Customize Tags opens on the tag list with its first tag selected, as OneNote's does.
    pub(crate) fn open_customize_tags(&mut self) {
        self.tag_list = Some(TagList {
            tags: self.tags.clone(),
            selected: (!self.tags.is_empty()).then_some(0),
            editor: None,
        });
        self.ui.open_popup(id());
    }

    /// Builds the Customize Tags dialog while it is open. OK keeps a changed list, which the
    /// toolbar, the menus and the chords then apply; Cancel, Escape or a press outside leave
    /// it.
    pub(crate) fn customize_tags(&mut self) {
        let Some(list) = &mut self.tag_list else {
            return;
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.tag_list = None;
            return;
        }
        let theme = ui.theme.clone();
        dialog(ui, id(), "Customize Tags", WIDTH);
        ui.leaf(
            "all",
            Spec {
                size: [fill(), px(ROW)],
                text: Some("All Tags:"),
                ..Spec::default()
            },
        );
        ui.open(
            "body",
            Spec {
                size: [fill(), children()],
                gap: 6.0,
                ..Spec::default()
            },
        );
        ui.open(
            "list",
            Spec {
                flags: Flags::SCROLL | Flags::CLIP,
                axis: Axis::Y,
                size: [fill(), px(ROW * ROWS + 4.0)],
                fill: Some(theme.base),
                border: Some(theme.chip),
                radius: 4.0,
                pad: [2.0, 2.0],
                role: Some(Role::List),
                ..Spec::default()
            },
        );
        let mut modify = None;
        for (place, tag) in list.tags.iter().enumerate() {
            let selected = list.selected == Some(place);
            let row = ui.open(
                ("tag", place),
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fill(), px(ROW)],
                    fill: selected.then_some(theme.selection),
                    hover_fill: Some(theme.hover()),
                    radius: 3.0,
                    role: Some(Role::ListItem),
                    ..Spec::default()
                },
            );
            if let Some(node) = ui.access(row) {
                node.set_selected(selected);
            }
            // OneNote's list marks a tag without a symbol with its "None" symbol art.
            match artwork(tag) {
                Some(art) => icon(ui, "icon", Some(art), [1.0; 4]),
                None => icon(ui, "icon", Some(crate::art::FONT_COLOR), theme.text),
            }
            let text = match commands::tag_chord(place) {
                Some(chord) => format!("{} ({})", tag.label, chord.label(commands::shown())),
                None => tag.label.clone(),
            };
            label(ui, "label", &text, tag);
            ui.close();
            let signal = ui.signal(row);
            if signal.pressed {
                list.selected = Some(place);
            }
            if signal.pressed && signal.unit == draw::edit::SelectionUnit::Word {
                modify = Some(place);
            }
        }
        ui.close();
        ui.open(
            "order",
            Spec {
                axis: Axis::Y,
                size: [children(), children()],
                gap: 4.0,
                ..Spec::default()
            },
        );
        let last = list.tags.len().saturating_sub(1);
        let up = tool(
            ui,
            "up",
            crate::art::CHEVRON_UP,
            "Move Tag Up",
            list.selected.is_some_and(|place| place > 0),
        );
        let down = tool(
            ui,
            "down",
            ui::shell::CHEVRON,
            "Move Tag Down",
            list.selected.is_some_and(|place| place < last),
        );
        let remove = tool(
            ui,
            "remove",
            crate::art::CLOSE,
            "Remove",
            list.selected.is_some(),
        );
        ui.close();
        ui.close();
        ui.open(
            "edit",
            Spec {
                size: [fill(), children()],
                gap: 8.0,
                ..Spec::default()
            },
        );
        let new = button(
            ui,
            "new",
            "New Tag…",
            list.tags.len() < commands::TAGS && list.editor.is_none(),
        );
        modify = modify.or_else(|| {
            button(ui, "modify", "Modify Tag…", list.selected.is_some())
                .then_some(list.selected)
                .flatten()
        });
        ui.close();
        crate::buttons(ui);
        let ok = button(ui, "ok", "OK", list.tags != self.tags);
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        ui.close();
        if let Some(place) = list.selected {
            if up {
                list.tags.swap(place, place - 1);
                list.selected = Some(place - 1);
            } else if down {
                list.tags.swap(place, place + 1);
                list.selected = Some(place + 1);
            } else if remove {
                list.tags.remove(place);
                list.selected = (!list.tags.is_empty()).then(|| place.min(list.tags.len() - 1));
            }
        }
        if new {
            // A new tag has no symbol, and takes the top of the list.
            let tag = NoteTag {
                label: "Undefined".into(),
                shape: 0,
                color: None,
                highlight: None,
                art: None,
            };
            list.editor = Some((tag, None));
        } else if let Some(place) = modify {
            list.editor = Some((list.tags[place].clone(), Some(place)));
        }
        if (new || modify.is_some()) && !ui.popup_open(editor_id()) {
            ui.open_popup(editor_id());
            ui.set_focus(Some(name_field()));
        }
        let mut picking = false;
        if let Some((tag, place)) = tag_editor(ui, &mut list.editor, &mut picking) {
            match place {
                Some(place) => list.tags[place] = tag,
                None => {
                    list.tags.insert(0, tag);
                    list.selected = Some(0);
                }
            }
        }
        ui.close();
        if picking {
            self.commands.push(crate::Command::TagPicture);
        }
        if ok {
            self.tags = list.tags.clone();
            self.save_settings();
            platform::update_tag_menu(&self.tags);
        } else if !cancel {
            return;
        }
        self.ui.close_popup(id());
        self.ui.set_focus(Some(crate::page()));
        self.tag_list = None;
    }
}

impl State {
    /// Asks for a picture, a PNG or SVG, for the tag New Tag or Modify Tag edits, which then
    /// draws with it; OneNote shows the tag's symbol, or a blue circle where it has none.
    pub(crate) fn pick_tag_picture(&mut self) {
        let reply = self.reply(|state, path| {
            state.use_tag_picture(path);
            Ok(())
        });
        platform::pick_file("Custom Image", &["png", "svg"], reply);
    }

    /// Draws the tag New Tag or Modify Tag edits with the picture at `path`, while it does.
    fn use_tag_picture(&mut self, path: std::path::PathBuf) {
        let Some((tag, _)) = self.tag_list.as_mut().and_then(|list| list.editor.as_mut()) else {
            return;
        };
        let kept = notebook::fs::read(&path)
            .ok()
            .and_then(|bytes| canvas::gpu::import_tag_art(&bytes));
        let Some((bytes, extension)) = kept else {
            return platform::alert("Couldn't use the picture", "Choose a PNG or SVG picture.");
        };
        if bytes.len() > notebook::sidecar::LIMIT {
            return platform::alert("Couldn't use the picture", "Choose an SVG of 1 MB or less.");
        }
        let art = art_name(&bytes, extension);
        let written = pictures()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
            .and_then(|folder| {
                notebook::fs::create_dir_all(&folder)?;
                match notebook::fs::File::create_new(folder.join(&art)) {
                    Ok(mut file) => std::io::Write::write_all(&mut file, &bytes),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                    Err(error) => Err(error),
                }
            });
        if let Err(error) = written {
            return platform::alert("Couldn't keep the picture", &error.to_string());
        }
        tag.art = Some(art);
        if tag.shape == 0 {
            tag.shape = PICTURE_FALLBACK;
        }
    }

    /// Has the open notebook draw the tag at `place` with its art, as Snowbound draws it
    /// wherever the notebook opens.
    pub(crate) fn keep_tag_art(&self, place: usize) {
        let (Some(tag), Some(session)) = (self.tags.get(place), &self.session) else {
            return;
        };
        if let Some(bytes) = tag.art.as_deref().and_then(art_bytes) {
            session.library.map_tag_art(tag, bytes);
        }
    }
}

/// Builds the New Tag or Modify Tag dialog while it is open over Customize Tags. Returns the
/// tag and the place it replaces once OK keeps it; sets `picking` once Custom Image… asks
/// for a picture.
fn tag_editor(
    ui: &mut Ui,
    editor: &mut Option<(NoteTag, Option<usize>)>,
    picking: &mut bool,
) -> Option<(NoteTag, Option<usize>)> {
    if !ui.popup_open(editor_id()) {
        *editor = None;
        return None;
    }
    let (tag, place) = editor.as_mut()?;
    let theme = ui.theme.clone();
    let entered =
        ui::popup::navigation(ui, &[name_field()], &[NamedKey::Enter]).contains(&NamedKey::Enter);
    let title = if place.is_some() {
        "Modify Tag"
    } else {
        "New Tag"
    };
    dialog(ui, editor_id(), title, EDITOR_WIDTH);
    let caption = |ui: &mut Ui, part: &str, text: &str| {
        ui.leaf(
            part,
            Spec {
                size: [fill(), px(ROW)],
                text: Some(text),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        );
    };
    caption(ui, "format", "Format");
    ui.leaf(
        "name-label",
        Spec {
            size: [fill(), px(ROW)],
            text: Some("Display name:"),
            ..Spec::default()
        },
    );
    ui::text_field(
        ui,
        name_field(),
        &mut tag.label,
        "",
        Spec {
            size: [fill(), px(ROW + 4.0)],
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
    ui.open(
        "pickers",
        Spec {
            size: [fill(), children()],
            gap: 10.0,
            ..Spec::default()
        },
    );
    let column = |ui: &mut Ui, part: &str, text: &str| {
        ui.open(
            part,
            Spec {
                axis: Axis::Y,
                size: [children(), children()],
                gap: 2.0,
                ..Spec::default()
            },
        );
        ui.leaf(
            "label",
            Spec {
                size: [fit(), px(ROW)],
                text: Some(text),
                ..Spec::default()
            },
        );
    };
    column(ui, "symbol", "Symbol:");
    let symbol = artwork(tag).unwrap_or(crate::art::FONT_COLOR);
    let tint = if artwork(tag).is_none() {
        theme.text
    } else {
        [1.0; 4]
    };
    let anchor = picker(
        ui,
        "picker",
        "Symbol",
        Some(symbol),
        None,
        popup("symbol"),
        tint,
    );
    match gallery(ui, popup("symbol"), anchor, true) {
        Some(Pick::Symbol(shape)) => {
            tag.shape = shape;
            tag.art = None;
        }
        Some(Pick::Extra(extra)) => {
            tag.shape = EXTRAS[extra].1;
            tag.art = Some(extra_art()[extra].clone());
        }
        Some(Pick::Picture) => *picking = true,
        None => {}
    }
    ui.close();
    column(ui, "color", "Font Color:");
    let bar = tag.color.map_or(theme.text, colorref);
    let anchor = picker(
        ui,
        "picker",
        "Font Color",
        Some(crate::art::FONT_COLOR),
        Some(bar),
        popup("color"),
        theme.text,
    );
    if let Some(color) = swatch(ui, "color", anchor, "Automatic", &FONT_COLORS, 8) {
        tag.color = color;
    }
    ui.close();
    column(ui, "highlight", "Highlight Color:");
    let bar = tag.highlight.map(colorref);
    let anchor = picker(
        ui,
        "picker",
        "Highlight Color",
        Some(crate::art::HIGHLIGHTER),
        bar,
        popup("highlight"),
        theme.text,
    );
    if let Some(color) = swatch(ui, "highlight", anchor, "None", &HIGHLIGHTS, 5) {
        tag.highlight = color;
    }
    ui.close();
    ui.close();
    // A symbol of Snowbound's own names the one OneNote shows in its place.
    if tag.art.is_some() {
        ui.open(
            "fallback-row",
            Spec {
                size: [fill(), children()],
                gap: 10.0,
                ..Spec::default()
            },
        );
        column(ui, "fallback", "In OneNote:");
        let anchor = picker(
            ui,
            "picker",
            "In OneNote",
            TagIcon::of(tag.shape, false).map(tag_sources),
            None,
            popup("fallback"),
            [1.0; 4],
        );
        if let Some(Pick::Symbol(shape)) = gallery(ui, popup("fallback"), anchor, false)
            && shape != 0
        {
            tag.shape = shape;
        }
        ui.close();
        ui.leaf(
            "onenote",
            Spec {
                size: [fill(), fit()],
                text: Some("OneNote shows this symbol in place of the art."),
                color: Some(theme.text_dim),
                overflow: ui::Overflow::Wrap,
                pad: [0.0, ROW],
                ..Spec::default()
            },
        );
        ui.close();
    }
    caption(ui, "preview-label", "Preview");
    ui.open(
        "preview",
        Spec {
            size: [fill(), px(ROW * 2.0)],
            fill: Some(theme.base),
            border: Some(theme.chip),
            radius: 4.0,
            pad: [8.0, ROW / 2.0],
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
    icon(ui, "icon", artwork(tag), [1.0; 4]);
    label(ui, "label", &tag.label, tag);
    ui.leaf(
        "end",
        Spec {
            size: [fill(), px(1.0)],
            ..Spec::default()
        },
    );
    ui.close();
    ui.leaf(
        "note",
        Spec {
            size: [fill(), fit()],
            text: Some("Customizations do not affect notes you have already tagged."),
            color: Some(theme.text_dim),
            overflow: ui::Overflow::Wrap,
            ..Spec::default()
        },
    );
    crate::buttons(ui);
    let named = !tag.label.trim().is_empty();
    let ok = button(ui, "ok", "OK", named) || entered && named;
    let cancel = ui::button(ui, "cancel", "Cancel").clicked;
    ui.close();
    ui.close();
    if ok {
        ui.close_popup(editor_id());
        return editor.take().map(|(mut tag, place)| {
            tag.label = tag.label.trim().to_owned();
            (tag, place)
        });
    }
    if cancel {
        ui.close_popup(editor_id());
        *editor = None;
    }
    None
}

/// Popup `id`, a gallery of OneNote's symbols below `anchor`, while it is open; with
/// `snowbound`, also None, Snowbound's own symbols and Custom Image…. Returns the pick,
/// symbol 0 for None.
fn gallery(ui: &mut Ui, id: Id, anchor: Anchor, snowbound: bool) -> Option<Pick> {
    if !ui.popup_open(id) {
        return None;
    }
    let theme = ui.theme.clone();
    ui.open_as(
        id,
        Spec {
            axis: Axis::Y,
            size: [children(), children()],
            fill: Some(theme.popup),
            border: Some(theme.chip),
            shadow: Some(theme.shadow),
            radius: 6.0,
            pad: [6.0, 6.0],
            anchor: Some(anchor),
            role: Some(Role::Dialog),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(id) {
        node.set_label("Symbol");
    }
    let mut picked = None;
    for (row, shapes) in GALLERY.iter().enumerate() {
        ui.open(
            ("row", row),
            Spec {
                size: [children(), px(CELL)],
                ..Spec::default()
            },
        );
        for (column, &shape) in shapes.iter().enumerate() {
            // A rule between the groups of three.
            let gap = if column % 3 == 0 && column > 0 {
                8.0
            } else {
                0.0
            };
            if gap > 0.0 {
                ui.leaf(
                    ("gap", column),
                    Spec {
                        size: [px(gap), px(CELL)],
                        ..Spec::default()
                    },
                );
            }
            if shape == 0 && row == 0 && column > 0 {
                continue;
            }
            let none = shape == 0 && row == 0 && snowbound;
            let spec = if none {
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(CELL * 3.0), px(CELL)],
                    text: Some("None"),
                    hover_fill: Some(theme.hover()),
                    radius: 3.0,
                    center: true,
                    role: Some(Role::Button),
                    ..Spec::default()
                }
            } else if shape == 0 {
                Spec {
                    size: [px(if row == 0 { CELL * 3.0 } else { CELL }), px(CELL)],
                    ..Spec::default()
                }
            } else {
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(CELL), px(CELL)],
                    icon: TagIcon::of(shape, false).map(tag_sources),
                    color: Some([1.0; 4]),
                    hover_fill: Some(theme.hover()),
                    radius: 3.0,
                    center: true,
                    role: Some(Role::Button),
                    ..Spec::default()
                }
            };
            if ui.leaf(("cell", column), spec).clicked && (none || shape != 0) {
                picked = Some(Pick::Symbol(shape));
            }
            if let Some(name) = symbol_name(shape) {
                ui::popup::tooltip(ui, name, "", None);
            }
        }
        ui.close();
    }
    if snowbound {
        ui.leaf(
            "snowbound",
            Spec {
                size: [fit(), px(ROW + 4.0)],
                text: Some("Snowbound Symbols"),
                color: Some(theme.text_dim),
                role: Some(Role::Heading),
                ..Spec::default()
            },
        );
        ui.open(
            "extras",
            Spec {
                size: [children(), px(CELL)],
                ..Spec::default()
            },
        );
        for (extra, (name, _, _)) in EXTRAS.iter().enumerate() {
            if extra % 3 == 0 && extra > 0 {
                ui.leaf(
                    ("gap", extra),
                    Spec {
                        size: [px(8.0), px(CELL)],
                        ..Spec::default()
                    },
                );
            }
            let art = &extra_art()[extra];
            let icon = canvas::gpu::art_sources(art, || art_bytes(art)).map(|[plain, _]| plain);
            let cell = Spec {
                flags: Flags::CLICKABLE,
                size: [px(CELL), px(CELL)],
                icon,
                color: Some([1.0; 4]),
                hover_fill: Some(theme.hover()),
                radius: 3.0,
                center: true,
                role: Some(Role::Button),
                ..Spec::default()
            };
            if ui.leaf(("extra", extra), cell).clicked {
                picked = Some(Pick::Extra(extra));
            }
            ui::popup::tooltip(ui, name, "", None);
        }
        ui.close();
        ui.leaf(
            "space",
            Spec {
                size: [px(1.0), px(6.0)],
                ..Spec::default()
            },
        );
        if ui::button(ui, "picture", "Custom Image…").clicked {
            picked = Some(Pick::Picture);
        }
    }
    ui.close();
    if picked.is_some() {
        ui.close_popup(id);
    }
    picked
}

/// A colour popup of `swatches` below `anchor` while it is open. Returns the COLORREF
/// picked, or `Some(None)` for the button labelled `none`.
fn swatch(
    ui: &mut Ui,
    name: &str,
    anchor: Anchor,
    none: &str,
    swatches: &[(u32, &str)],
    columns: usize,
) -> Option<Option<u32>> {
    let colors: Vec<_> = swatches
        .iter()
        .map(|&(color, name)| (colorref(color), name))
        .collect();
    let chosen = ui::popup::colors(ui, popup(name), anchor, none, &colors, columns)?;
    Some(chosen.and_then(|chosen| {
        swatches
            .iter()
            .zip(&colors)
            .find(|(_, (color, _))| *color == chosen)
            .map(|((stored, _), _)| *stored)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 48-pixel picture as a PNG: a violet disc ringed in gold, standing for one a user
    /// chose.
    fn picture() -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in 0..48 {
            for x in 0..48 {
                let distance = ((x as f32 - 23.5).powi(2) + (y as f32 - 23.5).powi(2)).sqrt();
                pixels.extend_from_slice(match distance {
                    ..16.0 => &[120, 60, 220, 255],
                    ..22.0 => &[240, 190, 40, 255],
                    _ => &[0, 0, 0, 0],
                });
            }
        }
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 48, 48);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
        encoded
    }

    /// Tags with Snowbound's art keep OneNote's definition, name and fallback symbol, on the
    /// page, and their art in the notebook's `.snowbound` folder, from which the page draws
    /// them (`corpus/custom-art`). `SNOWBOUND_CUSTOM_ART_EXPORT` names a new directory
    /// receiving the notebook for a cold reopen.
    #[test]
    fn tags_with_art_keep_it_beside_the_page() {
        use canvas::{
            document::TextPosition,
            editor::{CanvasEditor, Formatting},
            layout::TextEngine,
        };
        use onestore::{RevisionIndex, Store, document::Document, op, page::Page};
        let page = |bytes: &[u8]| {
            let store = Store::parse(bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            let (space, _) = document.pages().unwrap()[0];
            (space, Page::from_space(&document, space).unwrap())
        };
        let (picture, extension) = canvas::gpu::import_tag_art(&picture()).unwrap();
        let rocket = EXTRAS
            .iter()
            .position(|(name, ..)| *name == "Rocket")
            .unwrap();
        let tag = |label: &str, shape, art: Option<String>| NoteTag {
            label: label.into(),
            shape,
            color: None,
            highlight: None,
            art,
        };
        let list = [
            tag("Launch", 13, Some(art_name(&picture, extension))),
            tag(
                "Ship it",
                EXTRAS[rocket].1,
                Some(extra_art()[rocket].clone()),
            ),
            tag("Done", 3, Some(extra_art()[rocket].clone())),
            NoteTag::defaults()[1].clone(),
        ];
        let lines = [
            "Launch day",
            "Ship the release",
            "Checked off",
            "Plain OneNote tag",
        ];
        let source = onestore::create_section("Custom art.one", "", "Author").unwrap();
        let (space, before) = page(&source);
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::from_page(before, &mut engine).unwrap();
        let body = editor.outlines()[0].id;
        editor.focus_outline(body).unwrap();
        for (paragraph, text) in lines.iter().enumerate() {
            if paragraph > 0 {
                editor.enter(&mut engine, false).unwrap();
            }
            editor.insert(&mut engine, text).unwrap();
        }
        // Tagged once typed, as Enter would continue a check box.
        for (paragraph, tag) in list.iter().take(lines.len()).enumerate() {
            editor
                .select(
                    [TextPosition {
                        paragraph,
                        offset: 0,
                    }; 2]
                        .into(),
                )
                .unwrap();
            let formatting = Formatting::Tag(tag.clone(), paragraph as u16);
            editor.format(&mut engine, formatting).unwrap();
            if paragraph == 2 {
                editor.format(&mut engine, Formatting::Check).unwrap();
            }
        }
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, source.to_vec()).unwrap();
        let ops = editor
            .take_ops()
            .unwrap()
            .into_iter()
            .map(|op| op::Op::Page { space, op })
            .collect();
        let edit = op::Edit {
            at: 134_000_000_000_000_000,
            ops,
        };
        section.apply("Author", &edit).unwrap();
        section.seal().unwrap();
        let written = section.image();
        let temporary =
            std::env::temp_dir().join(format!("snowbound-custom-art-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&temporary);
        let root = temporary.join("Custom Art");
        notebook::fs::create_dir_all(&root).unwrap();
        notebook::fs::write(root.join("Custom art.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        let toc = onestore::create_table_of_contents(
            "Open Notebook.onetoc2",
            &[("Custom art.one", file_id)],
        );
        notebook::fs::write(root.join("Open Notebook.onetoc2"), toc.unwrap()).unwrap();
        let notebook = notebook::session::Notebook::open(&root, temporary.join("cache"));
        let notebook = notebook.unwrap();
        for tag in &list[..3] {
            let art = tag.art.as_deref().unwrap();
            let bytes = art_bytes(art).unwrap_or_else(|| picture.clone());
            let extension = art.rsplit('.').next().unwrap();
            notebook
                .map_tag_art(&tag.label, tag.shape, &bytes, extension)
                .unwrap();
        }
        // The page as OneNote stores it, drawn with the notebook's art.
        let mut art = canvas::gpu::TagArt::default();
        for mapping in notebook.tag_art().unwrap() {
            let bytes = || notebook.tag_art_file(&mapping.art).ok();
            let sources = canvas::gpu::art_sources(&mapping.art, bytes).unwrap();
            art.map(&mapping.name, mapping.shape, &mapping.art, sources);
        }
        let (_, stored) = page(&written);
        let reread = CanvasEditor::from_page(stored, &mut engine).unwrap();
        let outline = reread.outlines().iter().find(|outline| outline.id == body);
        let drawn: Vec<_> = outline
            .unwrap()
            .layouts()
            .flat_map(|(_, paragraph)| &paragraph.tags)
            .map(|tag| (tag.label.clone(), art.sources(tag)))
            .collect();
        let picture_art = canvas::gpu::art_sources(list[0].art.as_ref().unwrap(), || None);
        let rocket_art = canvas::gpu::art_sources(&extra_art()[rocket], || None).unwrap();
        let important = tag_sources(TagIcon::of(13, false).unwrap());
        assert_eq!(
            drawn,
            [
                ("Launch".to_owned(), picture_art.unwrap()[0]),
                ("Ship it".to_owned(), rocket_art[0]),
                ("Done".to_owned(), rocket_art[1]),
                ("Important".to_owned(), important),
            ]
        );
        if let Some(directory) = std::env::var_os("SNOWBOUND_CUSTOM_ART_EXPORT") {
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

    /// The gallery offers each symbol OneNote 2010's does once: every one MS-ONE lists but
    /// the follow-up flags it keeps for Outlook tasks.
    /// OneNote 2010's gallery, cell by cell: its page "Cells" tags a paragraph with a tag
    /// named for each cell's row and column, its symbol picked from that cell
    /// (`corpus/custom-tags/native`).
    /// OneNote 2010 edited the page of `corpus/custom-art` (new text on one tagged paragraph,
    /// another's check box cleared); the notebook's art still draws each tag it mapped.
    #[test]
    fn art_outlasts_onenote_editing_the_page() {
        use canvas::{editor::CanvasEditor, layout::TextEngine};
        use onestore::{RevisionIndex, Store, document::Document, page::Page};
        let corpus =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/custom-art");
        let cache =
            std::env::temp_dir().join(format!("snowbound-custom-art-read-{}", std::process::id()));
        let notebook = notebook::session::Notebook::open(corpus.join("candidate"), &cache).unwrap();
        let mut art = canvas::gpu::TagArt::default();
        for mapping in notebook.tag_art().unwrap() {
            let bytes = || notebook.tag_art_file(&mapping.art).ok();
            let sources = canvas::gpu::art_sources(&mapping.art, bytes).unwrap();
            art.map(&mapping.name, mapping.shape, &mapping.art, sources);
        }
        notebook::fs::remove_dir_all(&cache).unwrap();
        let bytes = notebook::fs::read(corpus.join("cold/notebook/Custom art.one")).unwrap();
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (space, _) = document.pages().unwrap()[0];
        let page = Page::from_space(&document, space).unwrap();
        let editor = CanvasEditor::from_page(page, &mut TextEngine::default()).unwrap();
        let drawn: Vec<_> = editor
            .outlines()
            .iter()
            .flat_map(|outline| outline.layouts())
            .flat_map(|(_, paragraph)| &paragraph.tags)
            .map(|tag| {
                (
                    tag.label.as_str(),
                    art.sources(tag) != tag_sources(tag.icon),
                )
            })
            .collect();
        assert_eq!(
            drawn,
            [
                ("Launch", true),
                ("Ship it", true),
                ("Done", true),
                ("Important", false),
            ]
        );
    }

    #[test]
    fn the_gallery_lays_symbols_out_as_onenotes_does() {
        use onestore::{
            RevisionIndex, Store,
            document::{Document, Kind},
            page::Page,
        };
        let bytes = include_bytes!("../../../corpus/custom-tags/native/shapes.one");
        let store = Store::parse(bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let cells = document
            .pages()
            .unwrap()
            .into_iter()
            .map(|(space, _)| Page::from_space(&document, space).unwrap())
            .find(|page| page.title.starts_with("Cells"))
            .unwrap();
        let picked: std::collections::BTreeMap<String, u16> = cells
            .definitions
            .values()
            .filter_map(|definition| match &definition.kind {
                Kind::TagDefinition {
                    label: Some(label),
                    shape: Some(shape),
                    ..
                } => Some((label.clone(), *shape)),
                _ => None,
            })
            .collect();
        let mut cells = 0;
        for (row, shapes) in GALLERY.iter().enumerate() {
            for (column, shape) in shapes.iter().enumerate() {
                let stored = picked.get(&format!("c{row}-{column}"));
                assert_eq!(
                    stored.copied().unwrap_or(0),
                    *shape,
                    "row {row}, column {column}"
                );
                cells += usize::from(stored.is_some());
            }
        }
        assert_eq!(cells, 138);
    }

    #[test]
    fn the_gallery_offers_every_symbol_once() {
        let mut offered: Vec<u16> = GALLERY
            .iter()
            .flatten()
            .copied()
            .filter(|shape| *shape != 0)
            .collect();
        offered.sort_unstable();
        let expected: Vec<u16> = (1..=143)
            .filter(|shape| !(89..=93).contains(shape))
            .collect();
        assert_eq!(offered, expected);
    }
}
