//! What the app keeps between launches: the open notebooks, the sidebar and recent fonts,
//! as JSON in the platform's settings directory.

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
    /// Keeps what the next launch restores: the open notebooks, the one shown, the sidebar
    /// and recent fonts. A section opened on its own is not kept.
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
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
