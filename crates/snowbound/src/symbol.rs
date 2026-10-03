//! Insert Symbol, as OneNote 2010's: a gallery of the symbols inserted lately, and More
//! Symbols, which finds any character by subset, name or code.

use crate::{Command, State, art, commands::Choice};
use std::sync::OnceLock;
use ui::{Anchor, Axis, Flags, Id, Rows, Spec, Ui, children, fill, fit, popup::Item, px};
use winit::keyboard::NamedKey;

const TITLE: &str = "Symbol";
/// Symbols the gallery holds and the settings keep.
pub const GALLERY: usize = 20;
/// The dialog's grid: 16 columns of cells, as OneNote's, with a scrollbar beside them.
const COLUMNS: usize = 16;
const ROWS: usize = 6;
const CELL: f32 = 30.0;
/// The grid's width, with room for its scrollbar.
const GRID: f32 = COLUMNS as f32 * CELL + 12.0;

/// OneNote 2010's gallery before any symbol is inserted (Windows 7 lab, 2026-10-01).
const DEFAULTS: [char; GALLERY] = [
    '€', '£', '¥', '©', '®', '™', '±', '≠', '≤', '≥', '÷', '×', '∞', 'µ', 'α', 'β', 'π', 'Ω', '∑',
    '☺',
];

/// More Symbols while it is open.
pub struct Symbols {
    search: String,
    /// The search, upper case, and the characters it found; none while it is empty.
    found: Option<(String, Vec<char>)>,
    selected: char,
    /// What the Character code field holds, a hex code point.
    code: String,
    /// Cancel reads Close once a symbol is inserted, as in OneNote.
    inserted: bool,
}

impl Symbols {
    fn select(&mut self, symbol: char) {
        self.selected = symbol;
        self.code = code(symbol);
    }
}

/// `symbol`'s code point in hex, as the Character code field shows it.
fn code(symbol: char) -> String {
    format!("{:04X}", u32::from(symbol))
}

fn id() -> Id {
    Id::ROOT.child("symbols")
}

fn search_field() -> Id {
    id().child("search")
}

fn code_field() -> Id {
    id().child("code")
}

fn grid() -> Id {
    id().child("grid")
}

fn subsets() -> Id {
    id().child("subsets")
}

/// The gallery's symbols: those inserted lately, then OneNote's defaults.
pub fn recent(inserted: &[char]) -> Vec<char> {
    let mut shown = inserted.to_vec();
    shown.extend(DEFAULTS.iter().filter(|symbol| !inserted.contains(symbol)));
    shown.truncate(GALLERY);
    shown
}

/// Builds popup `id` below `anchor` while it is open as the toolbar's Symbol gallery:
/// `symbols`, five to a row, then More Symbols. Returns the symbol chosen, or `None` for
/// More Symbols.
pub fn gallery(ui: &mut Ui, id: Id, anchor: Anchor, symbols: &[char]) -> Option<Option<char>> {
    const SIZE: f32 = 32.0;
    let groups = [
        ui::popup::Group {
            heading: "",
            cells: symbols.len(),
            columns: 5,
            size: [SIZE; 2],
        },
        ui::popup::Group {
            heading: "",
            cells: 1,
            columns: 1,
            size: [5.0 * SIZE, 28.0],
        },
    ];
    let chosen = ui::popup::gallery(ui, id, anchor, &groups, &[], |ui, index| {
        match symbols.get(index) {
            Some(&symbol) => glyph(ui, symbol, None),
            None => {
                ui.leaf(
                    "more",
                    Spec {
                        size: [fill(), fill()],
                        text: Some("More Symbols"),
                        icon: Some(art::SYMBOL),
                        gap: 6.0,
                        ..Spec::default()
                    },
                );
            }
        }
    })?;
    Some(symbols.get(chosen).copied())
}

/// A cell's `symbol`, centred, named for assistive technology by its Unicode name.
fn glyph(ui: &mut Ui, symbol: char, color: Option<[f32; 4]>) {
    let text = symbol.to_string();
    ui.leaf(
        "glyph",
        Spec {
            size: [fill(), fill()],
            text: Some(&text),
            font_size: Some(18.0),
            color,
            center: true,
            ..Spec::default()
        },
    );
    let cell = ui.current();
    if let Some(node) = ui.access(cell) {
        node.set_label(name(symbol));
    }
}

/// `symbol`'s Unicode name as OneNote shows it, "EURO SIGN" as "Euro Sign"; empty where it
/// has none.
fn name(symbol: char) -> String {
    let Some(name) = unicode_names2::name(symbol) else {
        return String::new();
    };
    // Codes, as in "CJK UNIFIED IDEOGRAPH-4E00", stay as they are.
    let part = |part: &str| match part.split_at_checked(1) {
        Some((first, rest)) if !part.bytes().any(|byte| byte.is_ascii_digit()) => {
            first.to_owned() + &rest.to_ascii_lowercase()
        }
        _ => part.to_owned(),
    };
    let word = |word: &str| word.split('-').map(part).collect::<Vec<_>>().join("-");
    let name = name.to_string();
    name.split(' ').map(word).collect::<Vec<_>>().join(" ")
}

/// A code point typed in hex, perhaps after "U+" or "0x".
fn code_point(text: &str) -> Option<char> {
    let text = text.trim();
    let digits = ["U+", "u+", "0x", "0X"]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .unwrap_or(text);
    if digits.is_empty() || digits.len() > 6 {
        return None;
    }
    char::from_u32(u32::from_str_radix(digits, 16).ok()?)
}

/// Every named character of the subsets that the interface's fonts draw, in code point order.
fn characters(ui: &mut Ui) -> &'static [char] {
    static CHARACTERS: OnceLock<Vec<char>> = OnceLock::new();
    CHARACTERS.get_or_init(|| {
        BLOCKS
            .iter()
            .flat_map(|&(start, end, _)| start..=end)
            .filter_map(char::from_u32)
            .filter(|symbol| {
                unicode_names2::name(*symbol).is_some() && ui.shows(symbol.encode_utf8(&mut [0; 4]))
            })
            .collect()
    })
}

/// The characters `search` finds: the one it is, or names a code point of with "U+", then
/// those whose names have words starting with each of its words.
fn find(ui: &mut Ui, search: &str) -> Vec<char> {
    let mut found = Vec::new();
    let mut letters = search.chars();
    if let (Some(symbol), None) = (letters.next(), letters.next()) {
        found.push(symbol);
    }
    if search.starts_with(['U', 'u']) {
        found.extend(code_point(search));
    }
    let typed = found.len();
    let words: Vec<String> = search.split_whitespace().map(str::to_uppercase).collect();
    for &symbol in characters(ui) {
        let Some(name) = unicode_names2::name(symbol) else {
            continue;
        };
        let name = name.to_string();
        let named = name.split([' ', '-']);
        if words
            .iter()
            .all(|word| named.clone().any(|part| part.starts_with(word.as_str())))
            && !found[..typed].contains(&symbol)
        {
            found.push(symbol);
        }
    }
    found
}

/// The subset holding `symbol`, as an index into `BLOCKS`.
fn subset(symbol: char) -> Option<usize> {
    let point = u32::from(symbol);
    BLOCKS
        .iter()
        .position(|&(start, end, _)| (start..=end).contains(&point))
}

/// The grid's rows of `COLUMNS` characters.
struct Grid<'a>(&'a [char]);

impl Rows for Grid<'_> {
    fn count(&self) -> usize {
        self.0.len().div_ceil(COLUMNS)
    }

    fn key(&self, index: usize) -> u64 {
        index as u64
    }

    fn find(&self, key: u64) -> Option<usize> {
        (key < self.count() as u64).then_some(key as usize)
    }

    // Cells take the clicks, not rows.
    fn selectable(&self, _: usize) -> bool {
        false
    }
}

/// What the dialog's cells did this frame.
#[derive(Default)]
struct Picked {
    symbol: Option<char>,
    /// A double click, which inserts it.
    inserted: bool,
}

/// A grid cell showing `symbol`, filled while `selected`.
fn cell(ui: &mut Ui, symbol: char, selected: bool, picked: &mut Picked) {
    let theme = ui.theme.clone();
    let signal = ui.open(
        ("cell", u32::from(symbol)),
        Spec {
            flags: Flags::CLICKABLE,
            axis: Axis::Y,
            size: [px(CELL), px(CELL)],
            fill: selected.then_some(theme.accent),
            hover_fill: (!selected).then(|| theme.hover()),
            border: Some(theme.chip),
            role: Some(accesskit::Role::GridCell),
            ..Spec::default()
        },
    );
    let signal = ui.signal(signal);
    if signal.pressed {
        picked.symbol = Some(symbol);
        picked.inserted |= signal.unit != draw::edit::SelectionUnit::Grapheme;
    }
    glyph(ui, symbol, selected.then_some([1.0; 4]));
    if selected && let Some(node) = ui.access(ui.current()) {
        node.set_selected(true);
    }
    ui.close();
}

impl State {
    /// Opens More Symbols on the symbol inserted last, its search taking typing.
    pub(crate) fn open_symbols(&mut self) {
        let last = recent(&self.toolbar.symbols)[0];
        self.symbols = Some(Symbols {
            search: String::new(),
            found: None,
            selected: last,
            code: code(last),
            inserted: false,
        });
        self.ui.open_popup(id());
        self.ui.set_focus(Some(search_field()));
    }

    /// Builds More Symbols while it is open: a search by name, a subset to jump to, the grid
    /// of characters, the recently used ones, and the selection's name and code. Insert,
    /// Enter or a double click inserts the selection and leaves the dialog open, as OneNote's.
    pub(crate) fn symbols_dialog(&mut self) {
        let Some(dialog) = &mut self.symbols else {
            return;
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.symbols = None;
            return;
        }
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        let mut moves = ui::popup::navigation(
            ui,
            &[grid()],
            &[
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                NamedKey::Home,
                NamedKey::End,
            ],
        );
        moves.extend(ui::popup::navigation(
            ui,
            &[grid(), search_field()],
            &[
                NamedKey::ArrowUp,
                NamedKey::ArrowDown,
                NamedKey::PageUp,
                NamedKey::PageDown,
            ],
        ));
        let entered = !ui::popup::navigation(
            ui,
            &[grid(), search_field(), code_field()],
            &[NamedKey::Enter],
        )
        .is_empty();

        let search = dialog.search.trim().to_owned();
        if search.is_empty() {
            dialog.found = None;
        } else if dialog
            .found
            .as_ref()
            .is_none_or(|(found, _)| *found != search)
        {
            let found = find(ui, &search);
            if let Some(&first) = found.first() {
                dialog.select(first);
            }
            dialog.found = Some((search, found));
        }
        let cells = dialog
            .found
            .as_ref()
            .map_or(characters(ui), |(_, found)| found.as_slice());
        let mut at = cells.iter().position(|symbol| *symbol == dialog.selected);
        if let Some(last) = cells.len().checked_sub(1) {
            for key in moves {
                let from = at.unwrap_or(0);
                let page = COLUMNS * ROWS;
                let to = match key {
                    NamedKey::ArrowLeft => from.saturating_sub(1),
                    NamedKey::ArrowRight => from + 1,
                    NamedKey::ArrowUp => from.saturating_sub(COLUMNS),
                    NamedKey::ArrowDown => from + COLUMNS,
                    NamedKey::PageUp => from.saturating_sub(page),
                    NamedKey::PageDown => from + page,
                    NamedKey::Home => 0,
                    _ => last,
                };
                at = Some(to.min(last));
            }
        }
        if let Some(at) = at
            && cells[at] != dialog.selected
        {
            dialog.selected = cells[at];
            dialog.code = code(cells[at]);
        }

        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(GRID + 32.0), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [16.0, 12.0],
                gap: 6.0,
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
        let field = Spec {
            size: [fill(), px(row)],
            fill: Some(theme.base),
            border: Some(theme.accent),
            radius: 4.0,
            pad: [6.0, 0.0],
            ..Spec::default()
        };
        let label = |ui: &mut Ui, text: &str| {
            ui.leaf(
                text,
                Spec {
                    size: [fit(), px(row)],
                    text: Some(text),
                    ..Spec::default()
                },
            );
        };
        ui.open(
            "find",
            Spec {
                size: [fill(), children()],
                gap: 8.0,
                ..Spec::default()
            },
        );
        ui::text_field(
            ui,
            search_field(),
            &mut dialog.search,
            "Search by name",
            field.clone(),
        );
        if let Some(node) = ui.access(search_field()) {
            node.set_label("Search by name");
        }
        label(ui, "Subset:");
        let current = subset(dialog.selected);
        let named = current.map_or("", |index| BLOCKS[index].2);
        let combo = ui.id("subset");
        ui::shell::combo(ui, "subset", "Subset", named, 220.0, subsets(), true);
        ui.close();

        let mut picked = Picked::default();
        let selected = dialog.selected;
        let mut row_selected = at.map(|at| (at / COLUMNS) as u64);
        ui::list(
            ui,
            grid(),
            Spec {
                axis: Axis::Y,
                size: [px(GRID), px(ROWS as f32 * CELL)],
                border: Some(theme.chip),
                role: Some(accesskit::Role::Grid),
                ..Spec::default()
            },
            ui::List {
                rows: &Grid(cells),
                row: CELL,
                keys: &[],
                hover_selects: false,
            },
            &mut row_selected,
            |ui, row| {
                for &symbol in cells.iter().skip(row.index * COLUMNS).take(COLUMNS) {
                    cell(ui, symbol, symbol == selected, &mut picked);
                }
            },
        );
        if let Some(node) = ui.access(grid()) {
            node.set_label("Characters");
        }
        if cells.is_empty() {
            ui.leaf(
                "none",
                Spec {
                    size: [fill(), px(row)],
                    text: Some("No characters have that name."),
                    color: Some(theme.text_dim),
                    ..Spec::default()
                },
            );
        }

        label(ui, "Recently used symbols:");
        ui.open(
            "recent",
            Spec {
                size: [children(), px(CELL)],
                role: Some(accesskit::Role::Grid),
                ..Spec::default()
            },
        );
        for symbol in recent(&self.toolbar.symbols).into_iter().take(COLUMNS) {
            cell(ui, symbol, symbol == selected, &mut picked);
        }
        ui.close();
        if let Some(node) = ui.access(ui.id("recent")) {
            node.set_label("Recently used symbols");
        }

        ui.open(
            "code",
            Spec {
                size: [fill(), children()],
                gap: 8.0,
                ..Spec::default()
            },
        );
        ui.leaf(
            "name",
            Spec {
                flags: Flags::CLIP,
                size: [fill(), px(row)],
                text: Some(&name(selected)),
                ..Spec::default()
            },
        );
        label(ui, "Character code:");
        ui::text_field(
            ui,
            code_field(),
            &mut dialog.code,
            "",
            Spec {
                size: [px(80.0), px(row)],
                ..field
            },
        );
        if let Some(node) = ui.access(code_field()) {
            node.set_label("Character code, Unicode hex");
        }
        label(ui, "Unicode (hex)");
        ui.close();

        crate::buttons(ui);
        let insert = ui::button(ui, "insert", "Insert").clicked || entered;
        let close = if dialog.inserted { "Close" } else { "Cancel" };
        let closed = ui::button(ui, "cancel", close).clicked;
        ui.close();
        ui.close();

        let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
        if ui.popup_open(subsets()) {
            // Each subset that shows a character, and its first.
            let all = characters(ui);
            let shown: Vec<(usize, char)> = BLOCKS
                .iter()
                .enumerate()
                .filter_map(|(index, &(start, end, _))| {
                    let first = all.partition_point(|symbol| u32::from(*symbol) < start);
                    let symbol = *all.get(first)?;
                    (u32::from(symbol) <= end).then_some((index, symbol))
                })
                .collect();
            let items: Vec<Item> = shown
                .iter()
                .map(|&(index, _)| Item {
                    text: BLOCKS[index].2,
                    current: Some(index) == current,
                    ..Item::default()
                })
                .collect();
            if let Some(chosen) = ui::popup::menu(ui, subsets(), anchor, &items, Some("Subset")) {
                dialog.search.clear();
                dialog.found = None;
                dialog.select(shown[chosen].1);
            }
        }

        if let Some(symbol) = picked.symbol {
            dialog.select(symbol);
            ui.set_focus(Some(grid()));
        }
        if let Some(symbol) = code_point(&dialog.code)
            && symbol != dialog.selected
        {
            dialog.selected = symbol;
        }
        if insert || picked.inserted {
            dialog.inserted = true;
            let symbol = dialog.selected;
            self.commands
                .push(Command::Choose(Choice::Symbol(Some(symbol))));
        } else if closed {
            self.ui.close_popup(id());
            self.symbols = None;
        }
    }
}

/// Unicode 17's blocks, but those of ideographs, Hangul syllables, surrogates, private use,
/// variation selectors and tags, which a symbol is not picked from.
const BLOCKS: &[(u32, u32, &str)] = &[
    (0x0000, 0x007F, "Basic Latin"),
    (0x0080, 0x00FF, "Latin-1 Supplement"),
    (0x0100, 0x017F, "Latin Extended-A"),
    (0x0180, 0x024F, "Latin Extended-B"),
    (0x0250, 0x02AF, "IPA Extensions"),
    (0x02B0, 0x02FF, "Spacing Modifier Letters"),
    (0x0300, 0x036F, "Combining Diacritical Marks"),
    (0x0370, 0x03FF, "Greek and Coptic"),
    (0x0400, 0x04FF, "Cyrillic"),
    (0x0500, 0x052F, "Cyrillic Supplement"),
    (0x0530, 0x058F, "Armenian"),
    (0x0590, 0x05FF, "Hebrew"),
    (0x0600, 0x06FF, "Arabic"),
    (0x0700, 0x074F, "Syriac"),
    (0x0750, 0x077F, "Arabic Supplement"),
    (0x0780, 0x07BF, "Thaana"),
    (0x07C0, 0x07FF, "NKo"),
    (0x0800, 0x083F, "Samaritan"),
    (0x0840, 0x085F, "Mandaic"),
    (0x0860, 0x086F, "Syriac Supplement"),
    (0x0870, 0x089F, "Arabic Extended-B"),
    (0x08A0, 0x08FF, "Arabic Extended-A"),
    (0x0900, 0x097F, "Devanagari"),
    (0x0980, 0x09FF, "Bengali"),
    (0x0A00, 0x0A7F, "Gurmukhi"),
    (0x0A80, 0x0AFF, "Gujarati"),
    (0x0B00, 0x0B7F, "Oriya"),
    (0x0B80, 0x0BFF, "Tamil"),
    (0x0C00, 0x0C7F, "Telugu"),
    (0x0C80, 0x0CFF, "Kannada"),
    (0x0D00, 0x0D7F, "Malayalam"),
    (0x0D80, 0x0DFF, "Sinhala"),
    (0x0E00, 0x0E7F, "Thai"),
    (0x0E80, 0x0EFF, "Lao"),
    (0x0F00, 0x0FFF, "Tibetan"),
    (0x1000, 0x109F, "Myanmar"),
    (0x10A0, 0x10FF, "Georgian"),
    (0x1100, 0x11FF, "Hangul Jamo"),
    (0x1200, 0x137F, "Ethiopic"),
    (0x1380, 0x139F, "Ethiopic Supplement"),
    (0x13A0, 0x13FF, "Cherokee"),
    (0x1400, 0x167F, "Unified Canadian Aboriginal Syllabics"),
    (0x1680, 0x169F, "Ogham"),
    (0x16A0, 0x16FF, "Runic"),
    (0x1700, 0x171F, "Tagalog"),
    (0x1720, 0x173F, "Hanunoo"),
    (0x1740, 0x175F, "Buhid"),
    (0x1760, 0x177F, "Tagbanwa"),
    (0x1780, 0x17FF, "Khmer"),
    (0x1800, 0x18AF, "Mongolian"),
    (
        0x18B0,
        0x18FF,
        "Unified Canadian Aboriginal Syllabics Extended",
    ),
    (0x1900, 0x194F, "Limbu"),
    (0x1950, 0x197F, "Tai Le"),
    (0x1980, 0x19DF, "New Tai Lue"),
    (0x19E0, 0x19FF, "Khmer Symbols"),
    (0x1A00, 0x1A1F, "Buginese"),
    (0x1A20, 0x1AAF, "Tai Tham"),
    (0x1AB0, 0x1AFF, "Combining Diacritical Marks Extended"),
    (0x1B00, 0x1B7F, "Balinese"),
    (0x1B80, 0x1BBF, "Sundanese"),
    (0x1BC0, 0x1BFF, "Batak"),
    (0x1C00, 0x1C4F, "Lepcha"),
    (0x1C50, 0x1C7F, "Ol Chiki"),
    (0x1C80, 0x1C8F, "Cyrillic Extended-C"),
    (0x1C90, 0x1CBF, "Georgian Extended"),
    (0x1CC0, 0x1CCF, "Sundanese Supplement"),
    (0x1CD0, 0x1CFF, "Vedic Extensions"),
    (0x1D00, 0x1D7F, "Phonetic Extensions"),
    (0x1D80, 0x1DBF, "Phonetic Extensions Supplement"),
    (0x1DC0, 0x1DFF, "Combining Diacritical Marks Supplement"),
    (0x1E00, 0x1EFF, "Latin Extended Additional"),
    (0x1F00, 0x1FFF, "Greek Extended"),
    (0x2000, 0x206F, "General Punctuation"),
    (0x2070, 0x209F, "Superscripts and Subscripts"),
    (0x20A0, 0x20CF, "Currency Symbols"),
    (0x20D0, 0x20FF, "Combining Diacritical Marks for Symbols"),
    (0x2100, 0x214F, "Letterlike Symbols"),
    (0x2150, 0x218F, "Number Forms"),
    (0x2190, 0x21FF, "Arrows"),
    (0x2200, 0x22FF, "Mathematical Operators"),
    (0x2300, 0x23FF, "Miscellaneous Technical"),
    (0x2400, 0x243F, "Control Pictures"),
    (0x2440, 0x245F, "Optical Character Recognition"),
    (0x2460, 0x24FF, "Enclosed Alphanumerics"),
    (0x2500, 0x257F, "Box Drawing"),
    (0x2580, 0x259F, "Block Elements"),
    (0x25A0, 0x25FF, "Geometric Shapes"),
    (0x2600, 0x26FF, "Miscellaneous Symbols"),
    (0x2700, 0x27BF, "Dingbats"),
    (0x27C0, 0x27EF, "Miscellaneous Mathematical Symbols-A"),
    (0x27F0, 0x27FF, "Supplemental Arrows-A"),
    (0x2800, 0x28FF, "Braille Patterns"),
    (0x2900, 0x297F, "Supplemental Arrows-B"),
    (0x2980, 0x29FF, "Miscellaneous Mathematical Symbols-B"),
    (0x2A00, 0x2AFF, "Supplemental Mathematical Operators"),
    (0x2B00, 0x2BFF, "Miscellaneous Symbols and Arrows"),
    (0x2C00, 0x2C5F, "Glagolitic"),
    (0x2C60, 0x2C7F, "Latin Extended-C"),
    (0x2C80, 0x2CFF, "Coptic"),
    (0x2D00, 0x2D2F, "Georgian Supplement"),
    (0x2D30, 0x2D7F, "Tifinagh"),
    (0x2D80, 0x2DDF, "Ethiopic Extended"),
    (0x2DE0, 0x2DFF, "Cyrillic Extended-A"),
    (0x2E00, 0x2E7F, "Supplemental Punctuation"),
    (0x2E80, 0x2EFF, "CJK Radicals Supplement"),
    (0x2F00, 0x2FDF, "Kangxi Radicals"),
    (0x2FF0, 0x2FFF, "Ideographic Description Characters"),
    (0x3000, 0x303F, "CJK Symbols and Punctuation"),
    (0x3040, 0x309F, "Hiragana"),
    (0x30A0, 0x30FF, "Katakana"),
    (0x3100, 0x312F, "Bopomofo"),
    (0x3130, 0x318F, "Hangul Compatibility Jamo"),
    (0x3190, 0x319F, "Kanbun"),
    (0x31A0, 0x31BF, "Bopomofo Extended"),
    (0x31C0, 0x31EF, "CJK Strokes"),
    (0x31F0, 0x31FF, "Katakana Phonetic Extensions"),
    (0x3200, 0x32FF, "Enclosed CJK Letters and Months"),
    (0x3300, 0x33FF, "CJK Compatibility"),
    (0x4DC0, 0x4DFF, "Yijing Hexagram Symbols"),
    (0xA000, 0xA48F, "Yi Syllables"),
    (0xA490, 0xA4CF, "Yi Radicals"),
    (0xA4D0, 0xA4FF, "Lisu"),
    (0xA500, 0xA63F, "Vai"),
    (0xA640, 0xA69F, "Cyrillic Extended-B"),
    (0xA6A0, 0xA6FF, "Bamum"),
    (0xA700, 0xA71F, "Modifier Tone Letters"),
    (0xA720, 0xA7FF, "Latin Extended-D"),
    (0xA800, 0xA82F, "Syloti Nagri"),
    (0xA830, 0xA83F, "Common Indic Number Forms"),
    (0xA840, 0xA87F, "Phags-pa"),
    (0xA880, 0xA8DF, "Saurashtra"),
    (0xA8E0, 0xA8FF, "Devanagari Extended"),
    (0xA900, 0xA92F, "Kayah Li"),
    (0xA930, 0xA95F, "Rejang"),
    (0xA960, 0xA97F, "Hangul Jamo Extended-A"),
    (0xA980, 0xA9DF, "Javanese"),
    (0xA9E0, 0xA9FF, "Myanmar Extended-B"),
    (0xAA00, 0xAA5F, "Cham"),
    (0xAA60, 0xAA7F, "Myanmar Extended-A"),
    (0xAA80, 0xAADF, "Tai Viet"),
    (0xAAE0, 0xAAFF, "Meetei Mayek Extensions"),
    (0xAB00, 0xAB2F, "Ethiopic Extended-A"),
    (0xAB30, 0xAB6F, "Latin Extended-E"),
    (0xAB70, 0xABBF, "Cherokee Supplement"),
    (0xABC0, 0xABFF, "Meetei Mayek"),
    (0xD7B0, 0xD7FF, "Hangul Jamo Extended-B"),
    (0xFB00, 0xFB4F, "Alphabetic Presentation Forms"),
    (0xFB50, 0xFDFF, "Arabic Presentation Forms-A"),
    (0xFE10, 0xFE1F, "Vertical Forms"),
    (0xFE20, 0xFE2F, "Combining Half Marks"),
    (0xFE30, 0xFE4F, "CJK Compatibility Forms"),
    (0xFE50, 0xFE6F, "Small Form Variants"),
    (0xFE70, 0xFEFF, "Arabic Presentation Forms-B"),
    (0xFF00, 0xFFEF, "Halfwidth and Fullwidth Forms"),
    (0xFFF0, 0xFFFF, "Specials"),
    (0x10000, 0x1007F, "Linear B Syllabary"),
    (0x10080, 0x100FF, "Linear B Ideograms"),
    (0x10100, 0x1013F, "Aegean Numbers"),
    (0x10140, 0x1018F, "Ancient Greek Numbers"),
    (0x10190, 0x101CF, "Ancient Symbols"),
    (0x101D0, 0x101FF, "Phaistos Disc"),
    (0x10280, 0x1029F, "Lycian"),
    (0x102A0, 0x102DF, "Carian"),
    (0x102E0, 0x102FF, "Coptic Epact Numbers"),
    (0x10300, 0x1032F, "Old Italic"),
    (0x10330, 0x1034F, "Gothic"),
    (0x10350, 0x1037F, "Old Permic"),
    (0x10380, 0x1039F, "Ugaritic"),
    (0x103A0, 0x103DF, "Old Persian"),
    (0x10400, 0x1044F, "Deseret"),
    (0x10450, 0x1047F, "Shavian"),
    (0x10480, 0x104AF, "Osmanya"),
    (0x104B0, 0x104FF, "Osage"),
    (0x10500, 0x1052F, "Elbasan"),
    (0x10530, 0x1056F, "Caucasian Albanian"),
    (0x10570, 0x105BF, "Vithkuqi"),
    (0x105C0, 0x105FF, "Todhri"),
    (0x10600, 0x1077F, "Linear A"),
    (0x10780, 0x107BF, "Latin Extended-F"),
    (0x10800, 0x1083F, "Cypriot Syllabary"),
    (0x10840, 0x1085F, "Imperial Aramaic"),
    (0x10860, 0x1087F, "Palmyrene"),
    (0x10880, 0x108AF, "Nabataean"),
    (0x108E0, 0x108FF, "Hatran"),
    (0x10900, 0x1091F, "Phoenician"),
    (0x10920, 0x1093F, "Lydian"),
    (0x10940, 0x1095F, "Sidetic"),
    (0x10980, 0x1099F, "Meroitic Hieroglyphs"),
    (0x109A0, 0x109FF, "Meroitic Cursive"),
    (0x10A00, 0x10A5F, "Kharoshthi"),
    (0x10A60, 0x10A7F, "Old South Arabian"),
    (0x10A80, 0x10A9F, "Old North Arabian"),
    (0x10AC0, 0x10AFF, "Manichaean"),
    (0x10B00, 0x10B3F, "Avestan"),
    (0x10B40, 0x10B5F, "Inscriptional Parthian"),
    (0x10B60, 0x10B7F, "Inscriptional Pahlavi"),
    (0x10B80, 0x10BAF, "Psalter Pahlavi"),
    (0x10C00, 0x10C4F, "Old Turkic"),
    (0x10C80, 0x10CFF, "Old Hungarian"),
    (0x10D00, 0x10D3F, "Hanifi Rohingya"),
    (0x10D40, 0x10D8F, "Garay"),
    (0x10E60, 0x10E7F, "Rumi Numeral Symbols"),
    (0x10E80, 0x10EBF, "Yezidi"),
    (0x10EC0, 0x10EFF, "Arabic Extended-C"),
    (0x10F00, 0x10F2F, "Old Sogdian"),
    (0x10F30, 0x10F6F, "Sogdian"),
    (0x10F70, 0x10FAF, "Old Uyghur"),
    (0x10FB0, 0x10FDF, "Chorasmian"),
    (0x10FE0, 0x10FFF, "Elymaic"),
    (0x11000, 0x1107F, "Brahmi"),
    (0x11080, 0x110CF, "Kaithi"),
    (0x110D0, 0x110FF, "Sora Sompeng"),
    (0x11100, 0x1114F, "Chakma"),
    (0x11150, 0x1117F, "Mahajani"),
    (0x11180, 0x111DF, "Sharada"),
    (0x111E0, 0x111FF, "Sinhala Archaic Numbers"),
    (0x11200, 0x1124F, "Khojki"),
    (0x11280, 0x112AF, "Multani"),
    (0x112B0, 0x112FF, "Khudawadi"),
    (0x11300, 0x1137F, "Grantha"),
    (0x11380, 0x113FF, "Tulu-Tigalari"),
    (0x11400, 0x1147F, "Newa"),
    (0x11480, 0x114DF, "Tirhuta"),
    (0x11580, 0x115FF, "Siddham"),
    (0x11600, 0x1165F, "Modi"),
    (0x11660, 0x1167F, "Mongolian Supplement"),
    (0x11680, 0x116CF, "Takri"),
    (0x116D0, 0x116FF, "Myanmar Extended-C"),
    (0x11700, 0x1174F, "Ahom"),
    (0x11800, 0x1184F, "Dogra"),
    (0x118A0, 0x118FF, "Warang Citi"),
    (0x11900, 0x1195F, "Dives Akuru"),
    (0x119A0, 0x119FF, "Nandinagari"),
    (0x11A00, 0x11A4F, "Zanabazar Square"),
    (0x11A50, 0x11AAF, "Soyombo"),
    (
        0x11AB0,
        0x11ABF,
        "Unified Canadian Aboriginal Syllabics Extended-A",
    ),
    (0x11AC0, 0x11AFF, "Pau Cin Hau"),
    (0x11B00, 0x11B5F, "Devanagari Extended-A"),
    (0x11B60, 0x11B7F, "Sharada Supplement"),
    (0x11BC0, 0x11BFF, "Sunuwar"),
    (0x11C00, 0x11C6F, "Bhaiksuki"),
    (0x11C70, 0x11CBF, "Marchen"),
    (0x11D00, 0x11D5F, "Masaram Gondi"),
    (0x11D60, 0x11DAF, "Gunjala Gondi"),
    (0x11DB0, 0x11DEF, "Tolong Siki"),
    (0x11EE0, 0x11EFF, "Makasar"),
    (0x11F00, 0x11F5F, "Kawi"),
    (0x11FB0, 0x11FBF, "Lisu Supplement"),
    (0x11FC0, 0x11FFF, "Tamil Supplement"),
    (0x12000, 0x123FF, "Cuneiform"),
    (0x12400, 0x1247F, "Cuneiform Numbers and Punctuation"),
    (0x12480, 0x1254F, "Early Dynastic Cuneiform"),
    (0x12F90, 0x12FFF, "Cypro-Minoan"),
    (0x13000, 0x1342F, "Egyptian Hieroglyphs"),
    (0x13430, 0x1345F, "Egyptian Hieroglyph Format Controls"),
    (0x13460, 0x143FF, "Egyptian Hieroglyphs Extended-A"),
    (0x14400, 0x1467F, "Anatolian Hieroglyphs"),
    (0x16100, 0x1613F, "Gurung Khema"),
    (0x16800, 0x16A3F, "Bamum Supplement"),
    (0x16A40, 0x16A6F, "Mro"),
    (0x16A70, 0x16ACF, "Tangsa"),
    (0x16AD0, 0x16AFF, "Bassa Vah"),
    (0x16B00, 0x16B8F, "Pahawh Hmong"),
    (0x16D40, 0x16D7F, "Kirat Rai"),
    (0x16E40, 0x16E9F, "Medefaidrin"),
    (0x16EA0, 0x16EDF, "Beria Erfe"),
    (0x16F00, 0x16F9F, "Miao"),
    (0x16FE0, 0x16FFF, "Ideographic Symbols and Punctuation"),
    (0x1AFF0, 0x1AFFF, "Kana Extended-B"),
    (0x1B000, 0x1B0FF, "Kana Supplement"),
    (0x1B100, 0x1B12F, "Kana Extended-A"),
    (0x1B130, 0x1B16F, "Small Kana Extension"),
    (0x1B170, 0x1B2FF, "Nushu"),
    (0x1BC00, 0x1BC9F, "Duployan"),
    (0x1BCA0, 0x1BCAF, "Shorthand Format Controls"),
    (0x1CC00, 0x1CEBF, "Symbols for Legacy Computing Supplement"),
    (0x1CEC0, 0x1CEFF, "Miscellaneous Symbols Supplement"),
    (0x1CF00, 0x1CFCF, "Znamenny Musical Notation"),
    (0x1D000, 0x1D0FF, "Byzantine Musical Symbols"),
    (0x1D100, 0x1D1FF, "Musical Symbols"),
    (0x1D200, 0x1D24F, "Ancient Greek Musical Notation"),
    (0x1D2C0, 0x1D2DF, "Kaktovik Numerals"),
    (0x1D2E0, 0x1D2FF, "Mayan Numerals"),
    (0x1D300, 0x1D35F, "Tai Xuan Jing Symbols"),
    (0x1D360, 0x1D37F, "Counting Rod Numerals"),
    (0x1D400, 0x1D7FF, "Mathematical Alphanumeric Symbols"),
    (0x1D800, 0x1DAAF, "Sutton SignWriting"),
    (0x1DF00, 0x1DFFF, "Latin Extended-G"),
    (0x1E000, 0x1E02F, "Glagolitic Supplement"),
    (0x1E030, 0x1E08F, "Cyrillic Extended-D"),
    (0x1E100, 0x1E14F, "Nyiakeng Puachue Hmong"),
    (0x1E290, 0x1E2BF, "Toto"),
    (0x1E2C0, 0x1E2FF, "Wancho"),
    (0x1E4D0, 0x1E4FF, "Nag Mundari"),
    (0x1E5D0, 0x1E5FF, "Ol Onal"),
    (0x1E6C0, 0x1E6FF, "Tai Yo"),
    (0x1E7E0, 0x1E7FF, "Ethiopic Extended-B"),
    (0x1E800, 0x1E8DF, "Mende Kikakui"),
    (0x1E900, 0x1E95F, "Adlam"),
    (0x1EC70, 0x1ECBF, "Indic Siyaq Numbers"),
    (0x1ED00, 0x1ED4F, "Ottoman Siyaq Numbers"),
    (0x1EE00, 0x1EEFF, "Arabic Mathematical Alphabetic Symbols"),
    (0x1F000, 0x1F02F, "Mahjong Tiles"),
    (0x1F030, 0x1F09F, "Domino Tiles"),
    (0x1F0A0, 0x1F0FF, "Playing Cards"),
    (0x1F100, 0x1F1FF, "Enclosed Alphanumeric Supplement"),
    (0x1F200, 0x1F2FF, "Enclosed Ideographic Supplement"),
    (0x1F300, 0x1F5FF, "Miscellaneous Symbols and Pictographs"),
    (0x1F600, 0x1F64F, "Emoticons"),
    (0x1F650, 0x1F67F, "Ornamental Dingbats"),
    (0x1F680, 0x1F6FF, "Transport and Map Symbols"),
    (0x1F700, 0x1F77F, "Alchemical Symbols"),
    (0x1F780, 0x1F7FF, "Geometric Shapes Extended"),
    (0x1F800, 0x1F8FF, "Supplemental Arrows-C"),
    (0x1F900, 0x1F9FF, "Supplemental Symbols and Pictographs"),
    (0x1FA00, 0x1FA6F, "Chess Symbols"),
    (0x1FA70, 0x1FAFF, "Symbols and Pictographs Extended-A"),
    (0x1FB00, 0x1FBFF, "Symbols for Legacy Computing"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gallery_follows_inserts_with_onenotes_defaults() {
        assert_eq!(recent(&[]), DEFAULTS);
        let shown = recent(&['⅔', '±']);
        assert_eq!(shown[..4], ['⅔', '±', '€', '£']);
        assert_eq!(shown.len(), GALLERY);
        assert_eq!(shown.iter().filter(|symbol| **symbol == '±').count(), 1);
    }

    #[test]
    fn names_and_codes_read_as_onenote_shows_them() {
        assert_eq!(name('€'), "Euro Sign");
        assert_eq!(name('⅔'), "Vulgar Fraction Two Thirds");
        assert_eq!(name('\u{4e00}'), "Cjk Unified Ideograph-4E00");
        assert_eq!(name('\n'), "");
        assert_eq!(code('€'), "20AC");
        for typed in ["20ac", "U+20AC", "0x20AC"] {
            assert_eq!(code_point(typed), Some('€'));
        }
        assert_eq!(code_point("D800"), None);
        assert_eq!(code_point("1234567"), None);
    }

    #[test]
    fn subsets_are_ordered_and_whole_rows() {
        for pair in BLOCKS.windows(2) {
            assert!(pair[0].1 < pair[1].0);
        }
        for &(start, end, _) in BLOCKS {
            assert_eq!((start % 16, end % 16), (0, 15));
        }
        assert_eq!(
            subset('€').map(|index| BLOCKS[index].2),
            Some("Currency Symbols")
        );
        assert_eq!(subset('\u{e000}'), None);
    }
}
