//! Style themes: `themes.json` in `.snowbound` keeps a notebook's own themes and which
//! theme its notebook, sections and pages take. A theme gives OneNote 2010's eleven gallery
//! styles their formatting; a page wears it as its paragraph style objects, so OneNote draws
//! the look and keeps the names (`resources/styles.md`). Writers merge as `tags.json`'s do:
//! themes by id and assignments by scope, the later timestamp winning, then the greater
//! entry, so writers agree whatever order they read in.

use crate::{Result, session::Storage};
use onestore::{
    document::{Format, Kind},
    page::Definition,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

const FILE: &str = ".snowbound/themes.json";

/// OneNote 2010's gallery, in its order: each style's stored name and the name it shows.
pub const STYLES: [(&str, &str); 11] = [
    ("h1", "Heading 1"),
    ("h2", "Heading 2"),
    ("h3", "Heading 3"),
    ("h4", "Heading 4"),
    ("h5", "Heading 5"),
    ("h6", "Heading 6"),
    ("PageTitle", "Page Title"),
    ("cite", "Citation"),
    ("blockquote", "Quote"),
    ("code", "Code"),
    ("p", "Normal"),
];

/// A style's look in a theme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeStyle {
    pub font: String,
    /// Points, in half points.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    /// `#rrggbb`, or none for automatic. Under `accent`, the accent of a section without a
    /// colour, which versions that don't know `accent` take.
    pub color: Option<String>,
    /// Takes the page's section's accent (the "Theme" colour) in place of `color`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accent: bool,
    /// Space above and below the paragraph, in points.
    pub before: f32,
    pub after: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub id: String,
    pub name: String,
    /// By stored style name; a theme names all eleven.
    pub styles: BTreeMap<String, ThemeStyle>,
    /// When it last changed, as a FILETIME; built-in themes have none.
    #[serde(default)]
    pub modified: u64,
    #[serde(default)]
    pub deleted: bool,
}

/// What a theme is assigned to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Notebook,
    /// A section, by its file identity, which survives renames and moves.
    Section(String),
    /// A page, by its notebook-management identity.
    Page(String),
}

impl Scope {
    pub fn section(identity: [u8; 16]) -> Self {
        Self::Section(hex(identity))
    }

    pub fn page(identity: [u8; 16]) -> Self {
        Self::Page(hex(identity))
    }
}

fn hex(identity: [u8; 16]) -> String {
    identity.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub scope: Scope,
    /// None takes the scope's override away.
    pub theme: Option<String>,
    pub assigned: u64,
}

/// A notebook's themes file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Themes {
    #[serde(default)]
    pub themes: Vec<Theme>,
    #[serde(default)]
    pub assignments: Vec<Assignment>,
}

impl Themes {
    /// The built-in themes, then the notebook's own that aren't deleted.
    pub fn all(&self) -> Vec<Theme> {
        let mut all = built_in();
        all.extend(self.themes.iter().filter(|theme| !theme.deleted).cloned());
        all
    }

    /// The theme `id` names, retired built-ins too, which pages may still wear.
    pub fn theme(&self, id: &str) -> Option<Theme> {
        shipped()
            .into_iter()
            .chain(self.themes.iter().filter(|theme| !theme.deleted).cloned())
            .find(|theme| theme.id == id)
    }

    /// The theme `scope` names itself, if one it can find.
    pub fn assigned(&self, scope: &Scope) -> Option<Theme> {
        let assignment = self.assignments.iter().find(|kept| kept.scope == *scope)?;
        self.theme(assignment.theme.as_deref()?)
    }

    /// The theme a page wears: its own, else its section's, else the notebook's.
    pub fn effective(&self, section: Option<[u8; 16]>, page: Option<[u8; 16]>) -> Option<Theme> {
        page.map(Scope::page)
            .into_iter()
            .chain(section.map(Scope::section))
            .chain([Scope::Notebook])
            .find_map(|scope| self.assigned(&scope))
    }
}

/// `into` with `other`'s entries merged by the rule the module describes.
pub fn merge(into: &mut Themes, other: Themes) {
    fn later<T: Serialize>(new: &T, new_at: u64, kept: &T, kept_at: u64) -> bool {
        let json = |value: &T| serde_json::to_string(value).unwrap_or_default();
        (new_at, json(new)) > (kept_at, json(kept))
    }
    for theme in other.themes {
        match into.themes.iter_mut().find(|kept| kept.id == theme.id) {
            Some(kept) => {
                if later(&theme, theme.modified, kept, kept.modified) {
                    *kept = theme;
                }
            }
            None => into.themes.push(theme),
        }
    }
    for assignment in other.assignments {
        match into
            .assignments
            .iter_mut()
            .find(|kept| kept.scope == assignment.scope)
        {
            Some(kept) => {
                if later(&assignment, assignment.assigned, kept, kept.assigned) {
                    *kept = assignment;
                }
            }
            None => into.assignments.push(assignment),
        }
    }
}

/// The themes file: empty without one, and without entries it holds unreadably.
pub(crate) fn read(storage: &dyn Storage) -> Result<Themes> {
    let bytes = match storage.read_file(FILE, super::LIMIT) {
        Err(crate::Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Themes::default());
        }
        bytes => bytes?,
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    let entries = |key: &str| -> Vec<serde_json::Value> {
        value
            .get(key)
            .and_then(|list| list.as_array().cloned())
            .unwrap_or_default()
    };
    let mut themes = Themes::default();
    merge(
        &mut themes,
        Themes {
            themes: entries("themes")
                .into_iter()
                .filter_map(|entry| serde_json::from_value(entry).ok())
                .filter(|theme: &Theme| !is_built_in(&theme.id))
                .collect(),
            assignments: entries("assignments")
                .into_iter()
                .filter_map(|entry| serde_json::from_value(entry).ok())
                .collect(),
        },
    );
    Ok(themes)
}

/// Merges `change` into the notebook's themes file; returns the file as it then stands.
pub(crate) fn write(storage: &dyn Storage, change: Themes) -> Result<Themes> {
    if change.themes.iter().any(|theme| is_built_in(&theme.id)) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
    }
    match storage.create_directory(super::FOLDER) {
        Err(crate::Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {}
        created => created?,
    }
    storage.hide(super::FOLDER)?;
    for _ in 0..3 {
        let mut merged = read(storage)?;
        merge(&mut merged, change.clone());
        let written = super::temporary(FILE);
        let json = serde_json::to_vec_pretty(&merged).map_err(io::Error::from)?;
        storage.create(&written, &json)?;
        storage.replace(&written, FILE)?;
        let kept = read(storage)?;
        let mut held = kept.clone();
        merge(&mut held, change.clone());
        if held == kept {
            return Ok(kept);
        }
    }
    Err(io::Error::from(io::ErrorKind::ResourceBusy).into())
}

/// Whether `id` names a theme Snowbound ships, or once offered.
pub fn is_built_in(id: &str) -> bool {
    shipped().iter().any(|theme| theme.id == id)
}

/// The accent of a section coloured `section` (a COLORREF; none for OneNote's None), as a
/// COLORREF: its hue at a fixed saturation and lightness, grey staying grey. Pages store it,
/// so it never changes: versions resolving it apart would restyle each other's pages.
pub fn accent(section: Option<u32>) -> u32 {
    // OneNote's blue for a section without a colour, as Snowbound's tabs show one.
    let [red, green, blue, _] = section.unwrap_or(0x00E4_A88A).to_le_bytes();
    let [red, green, blue] = [red, green, blue].map(|byte| f32::from(byte) / 255.0);
    let (max, min) = (red.max(green).max(blue), red.min(green).min(blue));
    let range = max - min;
    let sector = if range == 0.0 {
        0.0
    } else if max == red {
        (green - blue) / range
    } else if max == green {
        (blue - red) / range + 2.0
    } else {
        (red - green) / range + 4.0
    };
    let hue = (sector * 60.0).rem_euclid(360.0);
    let (saturation, lightness) = (if range == 0.0 { 0.0 } else { 0.60 }, 0.45);
    let chroma = (1.0 - (2.0 * lightness - 1.0f32).abs()) * saturation;
    let x = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let [r, g, b] = match hue {
        h if h < 60.0 => [chroma, x, 0.0],
        h if h < 120.0 => [x, chroma, 0.0],
        h if h < 180.0 => [0.0, chroma, x],
        h if h < 240.0 => [0.0, x, chroma],
        h if h < 300.0 => [x, 0.0, chroma],
        _ => [chroma, 0.0, x],
    };
    let byte = |value: f32| {
        ((value + lightness - chroma / 2.0) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u32
    };
    byte(r) | byte(g) << 8 | byte(b) << 16
}

/// A COLORREF as `#rrggbb`, as themes keep colours.
pub fn color_hex(colorref: u32) -> String {
    let [red, green, blue, _] = colorref.to_le_bytes();
    format!("#{red:02X}{green:02X}{blue:02X}")
}

impl ThemeStyle {
    /// The style's colour as a COLORREF in a section coloured `section`; none for automatic.
    pub fn colorref(&self, section: Option<u32>) -> Option<u32> {
        if self.accent {
            return Some(accent(section));
        }
        let rgb = u32::from_str_radix(self.color.as_deref()?.strip_prefix('#')?, 16).ok()?;
        Some((rgb >> 16) | (rgb & 0xff00) | ((rgb & 0xff) << 16))
    }
}

/// Stored style `name`'s paragraph style as `style` gives it in a section coloured `section`:
/// every character flag set, as OneNote 2010 writes its own, headings followed by Normal.
pub fn definition(name: &str, style: &ThemeStyle, section: Option<u32>) -> Definition {
    let color = style.colorref(section).unwrap_or(0xff00_0000);
    // Paragraph spacing is stored in half inches.
    let stored = |points: f32| points / 36.0 * 36.0;
    Definition {
        kind: Kind::Style {
            name: Some(name.into()),
            next: matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6").then(|| "p".into()),
        },
        format: Format {
            bold: Some(style.bold),
            italic: Some(style.italic),
            underline: Some(false),
            strike: Some(false),
            superscript: Some(false),
            subscript: Some(false),
            font: Some(style.font.clone()),
            font_size: Some((style.size * 2.0).round() / 2.0),
            color: Some(color),
            highlight: Some(0xff00_0000),
            space_before: Some(stored(style.before)),
            space_after: Some(stored(style.after)),
            line_spacing: Some(0.0),
            ..Format::default()
        },
    }
}

impl Theme {
    /// The paragraph style definitions the theme gives in a section coloured `section`, by
    /// stored name.
    pub fn sheet(&self, section: Option<u32>) -> BTreeMap<String, Definition> {
        self.styles
            .iter()
            .map(|(name, style)| (name.clone(), definition(name, style, section)))
            .collect()
    }
}

/// A style in `font` at `size`, `bold` and `italic`, in `color`, with spacing.
fn style(
    font: &str,
    size: f32,
    [bold, italic]: [bool; 2],
    color: Option<&str>,
    [before, after]: [f32; 2],
) -> ThemeStyle {
    ThemeStyle {
        font: font.into(),
        size,
        bold,
        italic,
        color: color.map(Into::into),
        accent: false,
        before,
        after,
    }
}

const PLAIN: [bool; 2] = [false, false];
const BOLD: [bool; 2] = [true, false];
const ITALIC: [bool; 2] = [false, true];
const BOTH: [bool; 2] = [true, true];
const NONE: [f32; 2] = [0.0; 2];

/// The theme new notebooks take unless the user chooses another.
pub const DEFAULT: &str = "modern-2";

/// Built-ins a page wears still but no longer offered: a changed look ships under a new id.
const RETIRED: [&str; 1] = ["modern"];

/// The themes Snowbound offers, OneNote 2010's first.
pub fn built_in() -> Vec<Theme> {
    let mut offered = shipped();
    offered.retain(|theme| !RETIRED.contains(&theme.id.as_str()));
    offered
}

/// Every theme Snowbound ever shipped. Once shipped a built-in's look never changes: two
/// versions disagreeing would restyle a page back and forth as each opens it.
fn shipped() -> Vec<Theme> {
    let theme = |id: &str, name: &str, styles: [ThemeStyle; 11]| Theme {
        id: id.into(),
        name: name.into(),
        styles: STYLES
            .iter()
            .zip(styles)
            .map(|((stored, _), style)| ((*stored).into(), style))
            .collect(),
        modified: 0,
        deleted: false,
    };
    let blue = Some("#366092");
    let tinted = |style: ThemeStyle| ThemeStyle {
        color: Some(color_hex(accent(None))),
        accent: true,
        ..style
    };
    vec![
        // OneNote 2010's own (lab, 2026-09-30).
        theme(
            "onenote",
            "OneNote",
            [
                style("Calibri", 16.0, BOLD, Some("#17365D"), NONE),
                style("Calibri", 13.0, BOLD, blue, NONE),
                style("Calibri", 11.0, BOLD, blue, NONE),
                style("Calibri", 11.0, BOTH, blue, NONE),
                style("Calibri", 11.0, PLAIN, blue, NONE),
                style("Calibri", 11.0, ITALIC, blue, NONE),
                style("Calibri", 17.0, PLAIN, None, NONE),
                style("Calibri", 9.0, PLAIN, Some("#595959"), NONE),
                style("Calibri", 11.0, ITALIC, Some("#595959"), NONE),
                style("Courier New", 11.0, PLAIN, None, NONE),
                style("Calibri", 11.0, PLAIN, None, NONE),
            ],
        ),
        theme(
            "manuscript",
            "Manuscript",
            [
                style("Georgia", 20.0, PLAIN, Some("#3B2F2A"), [12.0, 4.0]),
                style("Georgia", 16.0, BOLD, Some("#3B2F2A"), [10.0, 2.0]),
                style("Georgia", 13.0, BOLD, Some("#3B2F2A"), [8.0, 2.0]),
                style("Georgia", 12.0, BOTH, Some("#3B2F2A"), [6.0, 0.0]),
                style("Georgia", 12.0, PLAIN, Some("#3B2F2A"), [6.0, 0.0]),
                style("Georgia", 12.0, ITALIC, Some("#3B2F2A"), [6.0, 0.0]),
                style("Georgia", 26.0, PLAIN, Some("#1F1A17"), NONE),
                style("Georgia", 9.0, PLAIN, Some("#7A6E66"), NONE),
                style("Georgia", 12.0, ITALIC, Some("#6B5E55"), [0.0, 4.0]),
                style("Courier New", 10.5, PLAIN, Some("#2B2B2B"), NONE),
                style("Georgia", 12.0, PLAIN, Some("#2B2B2B"), [0.0, 4.0]),
            ],
        ),
        theme(
            "editorial",
            "Editorial",
            [
                style("Georgia", 18.0, BOLD, Some("#9A3B1F"), [12.0, 2.0]),
                style("Georgia", 14.0, BOLD, Some("#202020"), [10.0, 2.0]),
                style("Georgia", 12.0, BOLD, Some("#202020"), [8.0, 0.0]),
                style("Georgia", 11.0, BOTH, Some("#202020"), [6.0, 0.0]),
                style("Georgia", 11.0, PLAIN, Some("#9A3B1F"), [6.0, 0.0]),
                style("Georgia", 11.0, ITALIC, Some("#9A3B1F"), [6.0, 0.0]),
                style("Georgia", 24.0, PLAIN, Some("#202020"), NONE),
                style("Calibri", 9.0, PLAIN, Some("#707070"), NONE),
                style("Georgia", 12.0, ITALIC, Some("#555555"), [0.0, 3.0]),
                style("Courier New", 10.0, PLAIN, Some("#262626"), NONE),
                style("Calibri", 11.5, PLAIN, Some("#262626"), [0.0, 3.0]),
            ],
        ),
        // OneNote's page title in Arial; its accent the section's.
        theme(
            "modern-2",
            "Modern",
            [
                tinted(style("Arial", 16.0, BOLD, None, [10.0, 2.0])),
                style("Arial", 13.0, BOLD, Some("#1B2631"), [8.0, 2.0]),
                style("Arial", 11.0, BOLD, Some("#1B2631"), [6.0, 0.0]),
                style("Arial", 10.5, BOTH, Some("#1B2631"), [6.0, 0.0]),
                tinted(style("Arial", 10.5, BOLD, None, [6.0, 0.0])),
                tinted(style("Arial", 10.5, ITALIC, None, [6.0, 0.0])),
                style("Arial", 17.0, PLAIN, None, NONE),
                style("Arial", 8.0, PLAIN, Some("#7B868C"), NONE),
                style("Arial", 10.5, ITALIC, Some("#5F6B73"), [0.0, 3.0]),
                style("Courier New", 10.0, PLAIN, Some("#1B2631"), NONE),
                style("Arial", 10.5, PLAIN, Some("#2E2E2E"), [0.0, 3.0]),
            ],
        ),
        theme(
            "modern",
            "Modern (original)",
            [
                style("Arial", 16.0, BOLD, Some("#0E6E6E"), [10.0, 2.0]),
                style("Arial", 13.0, BOLD, Some("#1B2631"), [8.0, 2.0]),
                style("Arial", 11.0, BOLD, Some("#1B2631"), [6.0, 0.0]),
                style("Arial", 10.5, BOTH, Some("#1B2631"), [6.0, 0.0]),
                style("Arial", 10.5, BOLD, Some("#0E6E6E"), [6.0, 0.0]),
                style("Arial", 10.5, ITALIC, Some("#0E6E6E"), [6.0, 0.0]),
                style("Arial", 22.0, BOLD, Some("#1B2631"), NONE),
                style("Arial", 8.0, PLAIN, Some("#7B868C"), NONE),
                style("Arial", 10.5, ITALIC, Some("#5F6B73"), [0.0, 3.0]),
                style("Courier New", 10.0, PLAIN, Some("#1B2631"), NONE),
                style("Arial", 10.5, PLAIN, Some("#2E2E2E"), [0.0, 3.0]),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OneNote 2010's theme is what OneNote writes, as Snowbound reads it back, so a page
    /// OneNote styled has nothing to restyle under it.
    #[test]
    fn the_onenote_theme_is_what_onenote_writes() {
        let source = include_bytes!("../../../../corpus/styles/onenote/Styles.one");
        let store = onestore::Store::parse(source).unwrap();
        let index = onestore::RevisionIndex::parse(&store).unwrap();
        let document = onestore::document::Document::parse(&index).unwrap();
        let (space, _) = document.pages().unwrap()[0];
        let page = onestore::page::Page::from_space(&document, space).unwrap();
        let sheet = built_in()[0].sheet(None);
        for (name, definition) in &sheet {
            assert!(
                page.definitions.values().any(|stored| stored == definition),
                "{name}"
            );
        }
        // Only the heading pasted in from a page styled through COM differs.
        let ops = onestore::op::restyle(&page, &sheet).unwrap();
        assert_eq!(ops.len(), 1);
        let onestore::op::PageOp::Restyle { style, into, .. } = &ops[0] else {
            unreachable!()
        };
        assert_eq!(
            page.definitions[style].format.font.as_deref(),
            Some("Georgia")
        );
        assert_eq!(page.definitions[into], sheet["h1"]);
    }

    /// OneNote 2010 on a page Snowbound dressed in Manuscript: Ctrl+Alt+2 and Enter add its
    /// own `h2` and `p` beside the theme's (`corpus/styles/onenote-edit`). Snowbound reads
    /// them under those names, and restyling heals both: one edit, every style the theme's.
    #[test]
    fn styles_onenote_adds_to_a_themed_page_are_healed() {
        let source = include_bytes!("../../../../corpus/styles/onenote-edit/Manuscript.one");
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, source.to_vec()).unwrap();
        let (space, _, _) = section.pages().unwrap()[0];
        let page = section.page(space).unwrap();
        let sheet = built_in()
            .into_iter()
            .find(|theme| theme.id == "manuscript")
            .unwrap()
            .sheet(None);
        let calibri: Vec<&str> = page
            .definitions
            .values()
            .filter_map(|stored| match &stored.kind {
                Kind::Style {
                    name: Some(name), ..
                } if stored.format.font.as_deref() == Some("Calibri") => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(calibri, ["h2", "p"]);
        let ops = onestore::op::restyle(&page, &sheet).unwrap();
        assert_eq!(ops.len(), 2);
        let edit = onestore::op::Edit {
            at: 134_000_000_000_000_000,
            ops: ops
                .into_iter()
                .map(|op| onestore::op::Op::Page { space, op })
                .collect(),
        };
        section.apply("Author", &edit).unwrap();
        section.seal().unwrap();
        let healed = onestore::Section::open(&arena, section.image())
            .unwrap()
            .page(space)
            .unwrap();
        for stored in healed.definitions.values() {
            if let Kind::Style {
                name: Some(name), ..
            } = &stored.kind
            {
                assert_eq!(Some(stored), sheet.get(name), "{name}");
            }
        }
        assert!(onestore::op::restyle(&healed, &sheet).unwrap().is_empty());
    }

    /// The "Theme" colour is the section's hue at the accent's shade, OneNote's blue for a
    /// section without a colour, and grey for a grey section.
    #[test]
    fn a_theme_coloured_style_takes_its_section_s_accent() {
        assert_eq!(color_hex(accent(None)), "#2E5CB8");
        assert_eq!(color_hex(accent(Some(0x0000_00FF))), "#B82E2E");
        assert_eq!(color_hex(accent(Some(0x0080_8080))), "#737373");
        let style = ThemeStyle {
            accent: true,
            ..style("Arial", 16.0, BOLD, Some("#2E5CB8"), NONE)
        };
        let red = definition("h1", &style, Some(0x0000_00FF));
        assert_eq!(red.format.color, Some(0x002E_2EB8));
        // Versions that don't know the accent read the colour beside it.
        let json = serde_json::to_value(&style).unwrap();
        assert_eq!(json["color"], "#2E5CB8");
        assert_eq!(json["accent"], true);
    }

    /// Modern's title is OneNote's in Arial. The Modern first shipped, its title bold, is
    /// no longer offered, but pages given it keep it.
    #[test]
    fn modern_keeps_onenote_s_page_title_and_the_first_modern_stays() {
        let [onenote, modern] = ["onenote", "modern-2"]
            .map(|id| built_in().into_iter().find(|theme| theme.id == id).unwrap());
        let title = |theme: &Theme| definition("PageTitle", &theme.styles["PageTitle"], None);
        let mut expected = title(&onenote);
        expected.format.font = Some("Arial".into());
        assert_eq!(title(&modern), expected);
        assert!(built_in().iter().all(|theme| theme.id != "modern"));
        let themes = Themes {
            assignments: vec![Assignment {
                scope: Scope::Notebook,
                theme: Some("modern".into()),
                assigned: 1,
            }],
            ..Themes::default()
        };
        assert!(themes.effective(None, None).unwrap().styles["PageTitle"].bold);
        assert!(themes.all().iter().all(|theme| theme.id != "modern"));
    }

    #[test]
    fn every_built_in_theme_names_every_gallery_style() {
        for theme in shipped() {
            let sheet = theme.sheet(None);
            for (name, _) in STYLES {
                assert!(sheet.contains_key(name), "{} {name}", theme.id);
            }
        }
    }

    /// A page takes its own theme, else its section's, else the notebook's; a scope naming a
    /// deleted theme, or none, falls through.
    #[test]
    fn a_page_takes_the_nearest_theme() {
        let (section, page) = ([1; 16], [2; 16]);
        let assign = |scope, theme: Option<&str>, assigned| Assignment {
            scope,
            theme: theme.map(Into::into),
            assigned,
        };
        let mut custom = built_in().remove(1);
        custom.id = "essay".into();
        custom.deleted = true;
        let mut themes = Themes {
            themes: vec![custom],
            assignments: vec![assign(Scope::Notebook, Some("modern"), 1)],
        };
        let effective = |themes: &Themes| themes.effective(Some(section), Some(page)).unwrap().id;
        assert_eq!(effective(&themes), "modern");
        merge(
            &mut themes,
            Themes {
                assignments: vec![
                    assign(Scope::section(section), Some("editorial"), 2),
                    assign(Scope::page(page), Some("essay"), 2),
                ],
                ..Default::default()
            },
        );
        assert_eq!(effective(&themes), "editorial");
        themes.themes[0].deleted = false;
        assert_eq!(effective(&themes), "essay");
        merge(
            &mut themes,
            Themes {
                assignments: vec![assign(Scope::page(page), None, 3)],
                ..Default::default()
            },
        );
        assert_eq!(effective(&themes), "editorial");
        assert_eq!(themes.effective(None, None).unwrap().id, "modern");
    }

    /// Writers merging in either order agree: the later entry wins, then the greater.
    #[test]
    fn merges_agree_in_either_order() {
        let theme = |name: &str, modified| {
            let mut theme = built_in().remove(2);
            theme.id = "mine".into();
            theme.name = name.into();
            theme.modified = modified;
            theme
        };
        let a = Themes {
            themes: vec![theme("A", 5)],
            assignments: vec![Assignment {
                scope: Scope::Notebook,
                theme: Some("mine".into()),
                assigned: 7,
            }],
        };
        let b = Themes {
            themes: vec![theme("B", 5)],
            assignments: vec![Assignment {
                scope: Scope::Notebook,
                theme: None,
                assigned: 9,
            }],
        };
        let mut ab = a.clone();
        merge(&mut ab, b.clone());
        let mut ba = b;
        merge(&mut ba, a);
        assert_eq!(ab, ba);
        assert_eq!(ab.themes[0].name, "B");
        assert_eq!(ab.assignments[0].theme, None);
    }
}
