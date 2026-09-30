//! What the app keeps between launches: the open notebooks, the sidebar, recent fonts and
//! the Options dialog's choices, as JSON in the platform's settings directory.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Open notebooks in the sidebar's order, as `Location::key` names them.
    pub notebooks: Vec<String>,
    /// The notebook shown last.
    pub current: Option<String>,
    /// Whether the notebook sidebar is expanded.
    pub sidebar: bool,
    /// Fonts picked from the font box, latest first.
    pub recent_fonts: Vec<String>,
    pub toolbar: Toolbar,
    /// The name edits are stored under, OneNote's User name; the account's full name until
    /// first saved.
    pub user_name: Option<String>,
    pub color_scheme: ColorScheme,
    /// Keeps pages white in a dark appearance: Pages Match UI Theme off.
    pub light_pages: bool,
    /// Leaves misspelled words unmarked, as OneNote's Hide Spelling Errors.
    pub hide_spelling: bool,
    /// Where searches look first, as "Set This Scope as Default" chose.
    pub search_scope: crate::search::Scope,
    /// The tag list Customize Tags edits; none keeps OneNote's.
    pub tags: Option<Vec<canvas::editor::NoteTag>>,
    /// Checks for updates only when Check for Updates… asks.
    pub manual_updates: bool,
    /// Draws a tablet pen's strokes at its width, as OneNote 2010 with "Use pen pressure
    /// sensitivity" off.
    pub ignore_pen_pressure: bool,
    /// Places and moves things where they are dropped, as OneNote's Snap To Grid off.
    pub ignore_grid: bool,
    /// Options' Default font: new text's font and size, and new titles' font.
    pub default_font: DefaultFont,
    /// OneNote's "Page tabs appear on the left".
    pub page_tabs_left: bool,
    /// OneNote's "Navigation bar appears on the left" turned off: the notebooks on the right.
    pub navigation_bar_right: bool,
    /// Servers Open Notebook from Server signed in to, latest first, as addresses without a
    /// password.
    pub servers: Vec<String>,
}

/// The font and size OneNote's Default font gives new text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultFont {
    pub face: String,
    pub size: f32,
}

impl Default for DefaultFont {
    /// OneNote 2010's: Calibri 11.
    fn default() -> Self {
        Self {
            face: "Calibri".into(),
            size: 11.0,
        }
    }
}

/// What the toolbar's buttons apply from their menus' last picks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Toolbar {
    /// COLORREFs the highlighter and the font colour button apply; `None` removes the
    /// highlight, or makes the colour automatic.
    pub highlight: Option<u32>,
    pub font_color: Option<u32>,
    /// Places in OneNote's bullet and numbering libraries picked lately, latest first.
    pub bullets: Vec<usize>,
    pub numbering: Vec<usize>,
    /// The place in the pen gallery Pen draws with: the accent pen first.
    pub pen: usize,
}

impl Default for Toolbar {
    /// Office's yellow highlighter and red font colour.
    fn default() -> Self {
        Self {
            highlight: Some(0x00ffff),
            font_color: Some(0x0000ff),
            bullets: Vec::new(),
            numbering: Vec::new(),
            pen: 0,
        }
    }
}

/// The interface's colours: the system's appearance, or one chosen in Options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorScheme {
    #[default]
    System,
    Light,
    Dark,
}

impl ColorScheme {
    /// The appearance the window is held to; none follows the system.
    pub fn theme(self) -> Option<winit::window::Theme> {
        match self {
            Self::System => None,
            Self::Light => Some(winit::window::Theme::Light),
            Self::Dark => Some(winit::window::Theme::Dark),
        }
    }
}

impl Settings {
    /// The settings at `path`; missing or unreadable ones are the defaults, reported.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                eprintln!("Ignoring the settings in {}: {error}", path.display());
                Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                eprintln!("Ignoring the settings in {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Replaces the file at `path` whole, so a crash leaves the old settings or the new.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let partial = path.with_extension("partial");
        std::fs::write(&partial, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(partial, path)
    }
}

impl crate::State {
    /// Keeps what the next launch restores: the open notebooks, the one shown, the sidebar,
    /// recent fonts and the Options dialog's choices. A section opened on its own is not kept.
    pub(crate) fn save_settings(&self) {
        let Some(path) = &self.settings else {
            return;
        };
        let kept =
            |library: &&std::sync::Arc<crate::Library>| !matches!(library.notebook, Ok(None));
        let settings = Settings {
            notebooks: self
                .notebooks
                .iter()
                .filter(kept)
                .map(|library| library.location.clone())
                .collect(),
            current: self
                .session
                .as_ref()
                .map(|session| &session.library)
                .filter(kept)
                .map(|library| library.location.clone()),
            sidebar: self.sidebar,
            recent_fonts: self.recent_fonts.clone(),
            toolbar: self.toolbar.clone(),
            user_name: Some(self.author.clone()),
            color_scheme: self.color_scheme,
            light_pages: self.light_pages,
            hide_spelling: self.hide_spelling,
            search_scope: self.search.default,
            tags: (self.tags != canvas::editor::NoteTag::defaults()).then(|| self.tags.clone()),
            manual_updates: !self.updates.automatic(),
            ignore_pen_pressure: !self.pen_pressure,
            ignore_grid: !self.view.snap_to_grid,
            default_font: self.default_font.clone(),
            page_tabs_left: self.page_tabs_left,
            navigation_bar_right: self.navigation_bar_right,
            servers: self.servers.clone(),
        };
        if let Err(error) = settings.save(path) {
            eprintln!("Cannot save the settings in {}: {error}", path.display());
        }
    }
}

/// What a launch starts from besides its input.
pub struct Launch {
    /// Where the settings are saved; `None` leaves them as they were read.
    pub file: Option<PathBuf>,
    pub saved: Settings,
    /// The directory holding the sections' replicas.
    pub cache: PathBuf,
}

/// Where the settings live unless `--settings` names a file.
pub fn default_path() -> Option<PathBuf> {
    Some(crate::platform::settings_dir()?.join("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_missing_fields() {
        let directory =
            std::env::temp_dir().join(format!("snowbound-settings-{}", std::process::id()));
        let path = directory.join("nested/settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        let settings = Settings {
            notebooks: vec![
                "/notebooks/Personal".into(),
                "smb://server/share/Work".into(),
            ],
            current: Some("/notebooks/Personal".into()),
            sidebar: true,
            recent_fonts: vec!["Georgia".into()],
            toolbar: Toolbar {
                highlight: None,
                font_color: Some(0x00ff00),
                bullets: vec![3, 0],
                numbering: vec![16],
                pen: 6,
            },
            user_name: Some("Snowbound Test".into()),
            color_scheme: ColorScheme::Dark,
            light_pages: true,
            hide_spelling: true,
            search_scope: crate::search::Scope::Notebook,
            tags: Some(vec![canvas::editor::NoteTag {
                label: "Snow check".into(),
                shape: 61,
                color: Some(0x0000_0080),
                highlight: Some(0x00ff_cc00),
                art: Some(format!("{}.png", "ab".repeat(32))),
            }]),
            manual_updates: true,
            ignore_pen_pressure: true,
            ignore_grid: true,
            default_font: DefaultFont {
                face: "Georgia".into(),
                size: 14.0,
            },
            page_tabs_left: true,
            navigation_bar_right: true,
            servers: vec!["smb://clover@nas/Notes".into()],
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
        std::fs::write(&path, br#"{"sidebar": true, "unknown": 1}"#).unwrap();
        assert_eq!(
            Settings::load(&path),
            Settings {
                sidebar: true,
                ..Settings::default()
            }
        );
        // A picked "No color" stays picked; what was never picked keeps Office's default.
        std::fs::write(&path, br#"{"toolbar": {"highlight": null}}"#).unwrap();
        let toolbar = Settings::load(&path).toolbar;
        assert_eq!(toolbar.highlight, None);
        assert_eq!(toolbar.font_color, Toolbar::default().font_color);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
