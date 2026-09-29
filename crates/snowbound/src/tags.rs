//! OneNote 2010's Customize Tags dialog and its New Tag and Modify Tag dialog, which edit
//! the user's tag list: the tags the toolbar, the menus and Ctrl+1 to Ctrl+9 apply.

use crate::{State, commands, platform};
use canvas::editor::NoteTag;
use canvas::gpu::{colorref, tag_sources};
use canvas::outline::{TagIcon, symbol_name};
use draw::edit::Platform;
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

/// Font Color's swatches, Office's forty in their grid, as COLORREFs.
const FONT_COLORS: [u32; 40] = [
    0x000000, 0x003399, 0x003333, 0x003300, 0x663300, 0x800000, 0x993333, 0x333333, //
    0x000080, 0x0066ff, 0x008080, 0x008000, 0x808000, 0xff0000, 0x996666, 0x808080, //
    0x0000ff, 0x0099ff, 0x00cc99, 0x669933, 0xcccc33, 0xff6633, 0x800080, 0x969696, //
    0xff00ff, 0x00ccff, 0x00ffff, 0x00ff00, 0xffff00, 0xffcc00, 0x663399, 0xc0c0c0, //
    0xcc99ff, 0x99ccff, 0x99ffff, 0xccffcc, 0xffffcc, 0xffcc99, 0xff99cc, 0xffffff,
];

/// Highlight Color's swatches as COLORREFs, which differ from the highlighter's in Sky Blue
/// and Dark Green.
const HIGHLIGHTS: [u32; 15] = [
    0x00ffff, 0x00ff00, 0xffcc00, 0xff00ff, 0xff0000, //
    0x0000ff, 0x800000, 0x808000, 0x003300, 0x800080, //
    0x000080, 0x008080, 0x808080, 0xc0c0c0, 0x000000,
];

/// The Customize Tags dialog's list while it is open.
pub struct TagList {
    tags: Vec<NoteTag>,
    selected: Option<usize>,
    /// The New Tag or Modify Tag dialog's fields, and the place Modify Tag replaces.
    editor: Option<(NoteTag, Option<usize>)>,
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

/// A tag's artwork before its name: its symbol, or none.
fn artwork(tag: &NoteTag) -> Option<&'static [&'static str]> {
    TagIcon::of(tag.shape, false).map(tag_sources)
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

fn icon(ui: &mut Ui, part: &str, art: Option<&'static [&'static str]>) {
    ui.leaf(
        part,
        Spec {
            size: [px(SIDE + 4.0), px(ROW)],
            icon: art,
            color: Some([1.0; 4]),
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
            ..Spec::default()
        },
    );
    false
}

/// A square button showing `art` that takes no clicks while not `enabled`, named by a
/// tooltip.
fn tool(ui: &mut Ui, part: &str, art: &'static [&'static str], tip: &str, enabled: bool) -> bool {
    let text = ui.theme.text;
    if !enabled {
        ui::shell::unavailable(ui, part, art, text, false);
        return false;
    }
    let clicked = ui::shell::tool_button(ui, part, art, text, false).clicked;
    ui::popup::tooltip(ui, tip, "", None);
    clicked
}

/// A button showing `art` in its own colours, with `bar` under it, and a menu arrow; opens
/// popup `menu` and returns where it opens.
fn picker(
    ui: &mut Ui,
    part: &str,
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
            ..Spec::default()
        },
    );
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

/// The trailing row of a dialog: its buttons at the right.
fn buttons(ui: &mut Ui) {
    ui.open(
        "buttons",
        Spec {
            size: [fill(), children()],
            pad: [0.0, 8.0],
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
            ..Spec::default()
        },
    );
    ui.leaf(
        "title",
        Spec {
            size: [fill(), px(ROW + 4.0)],
            text: Some(title),
            bold: true,
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
                    ..Spec::default()
                },
            );
            icon(ui, "icon", artwork(tag));
            let text = match commands::tag_chord(place) {
                Some(chord) => format!("{} ({})", tag.label, chord.label(Platform::CURRENT)),
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
        buttons(ui);
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
            };
            list.editor = Some((tag, None));
        } else if let Some(place) = modify {
            list.editor = Some((list.tags[place].clone(), Some(place)));
        }
        if (new || modify.is_some()) && !ui.popup_open(editor_id()) {
            ui.open_popup(editor_id());
            ui.set_focus(Some(name_field()));
        }
        if let Some((tag, place)) = tag_editor(ui, &mut list.editor) {
            match place {
                Some(place) => list.tags[place] = tag,
                None => {
                    list.tags.insert(0, tag);
                    list.selected = Some(0);
                }
            }
        }
        ui.close();
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

/// Builds the New Tag or Modify Tag dialog while it is open over Customize Tags. Returns the
/// tag and the place it replaces once OK keeps it.
fn tag_editor(
    ui: &mut Ui,
    editor: &mut Option<(NoteTag, Option<usize>)>,
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
    let tint = if tag.shape == 0 { theme.text } else { [1.0; 4] };
    let anchor = picker(ui, "picker", Some(symbol), None, popup("symbol"), tint);
    if let Some(shape) = gallery(ui, anchor) {
        tag.shape = shape;
    }
    ui.close();
    column(ui, "color", "Font Color:");
    let bar = tag.color.map_or(theme.text, colorref);
    let anchor = picker(
        ui,
        "picker",
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
    icon(ui, "icon", artwork(tag));
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
    buttons(ui);
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

/// Symbol's gallery below `anchor` while it is open. Returns the symbol picked, 0 for None.
fn gallery(ui: &mut Ui, anchor: Anchor) -> Option<u16> {
    let id = popup("symbol");
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
            ..Spec::default()
        },
    );
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
            let none = shape == 0 && row == 0;
            let spec = if none {
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(CELL * 3.0), px(CELL)],
                    text: Some("None"),
                    hover_fill: Some(theme.hover()),
                    radius: 3.0,
                    center: true,
                    ..Spec::default()
                }
            } else if shape == 0 {
                Spec {
                    size: [px(CELL), px(CELL)],
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
                    ..Spec::default()
                }
            };
            if ui.leaf(("cell", column), spec).clicked && (none || shape != 0) {
                picked = Some(shape);
            }
            if let Some(name) = symbol_name(shape) {
                ui::popup::tooltip(ui, name, "", None);
            }
        }
        ui.close();
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
    swatches: &[u32],
    columns: usize,
) -> Option<Option<u32>> {
    let colors: Vec<_> = swatches.iter().map(|color| colorref(*color)).collect();
    let chosen = ui::popup::colors(ui, popup(name), anchor, none, &colors, columns)?;
    Some(chosen.and_then(|chosen| {
        swatches
            .iter()
            .zip(&colors)
            .find(|(_, color)| **color == chosen)
            .map(|(stored, _)| *stored)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gallery offers each symbol OneNote 2010's does once: every one MS-ONE lists but
    /// the follow-up flags it keeps for Outlook tasks.
    /// OneNote 2010's gallery, cell by cell: its page "Cells" tags a paragraph with a tag
    /// named for each cell's row and column, its symbol picked from that cell
    /// (`corpus/custom-tags/native`).
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
