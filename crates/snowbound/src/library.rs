//! Open notebooks: where each lives and the sections its tabs offer.

use notebook::discover::{Folder, SectionState};
use notebook::session::{Notebook, Section};
use std::{
    error::Error,
    path::{Path, PathBuf},
};

/// A section tab: where the section opens from and how it is labelled.
pub struct Tab {
    /// The notebook catalog path, or the file of a section opened on its own.
    pub path: String,
    pub name: String,
    /// COLORREF.
    pub color: Option<u32>,
}

/// An open notebook as the sidebar lists it, or a section opened on its own.
pub struct Library {
    /// Where it lives, as the settings keep it: the notebook's folder, or the section file.
    pub location: String,
    /// The notebook's folder name, or the section file's.
    pub name: String,
    /// `Err` holds why a notebook listed as open could not be read this time.
    pub notebook: Result<Option<Notebook>, String>,
    cache: PathBuf,
}

impl Library {
    /// The notebook in the folder at `location`, or why it cannot be read.
    pub fn notebook(location: &str, cache: &Path) -> Self {
        Self {
            location: location.to_owned(),
            name: file_name(Path::new(location)),
            notebook: Notebook::open(location, cache)
                .map(Some)
                .map_err(|error| error.to_string()),
            cache: cache.to_owned(),
        }
    }

    /// `notebook`, open at `location`.
    pub fn open_notebook(location: &str, notebook: Notebook, cache: &Path) -> Self {
        Self {
            location: location.to_owned(),
            name: file_name(Path::new(location)),
            notebook: Ok(Some(notebook)),
            cache: cache.to_owned(),
        }
    }

    /// The section file at `file` as a notebook of one tab.
    pub fn section(file: &Path, cache: &Path) -> Self {
        Self {
            location: file.to_string_lossy().into_owned(),
            name: file_name(file.parent().unwrap_or(file)),
            notebook: Ok(None),
            cache: cache.to_owned(),
        }
    }

    /// Opens the section at catalog `path`.
    pub fn open(&self, path: &str, notify: impl Fn() + Send + 'static) -> Result<Section, Box<dyn Error>> {
        Ok(match &self.notebook {
            Ok(Some(notebook)) => notebook.section(path, notify)?,
            Ok(None) => Section::open(path, &self.cache, notify)?,
            Err(error) => return Err(error.clone().into()),
        })
    }

    /// Names the section at catalog `path` across every open notebook, for remembering
    /// its pages.
    pub fn key(&self, path: &str) -> String {
        format!("{}\n{path}", self.location)
    }

    /// The readable sections of the folder at catalog path `folder`, as tabs in its order.
    pub fn tabs(&self, folder: &str) -> Vec<Tab> {
        match &self.notebook {
            Ok(Some(notebook)) => {
                let mut folders = vec![notebook.catalog()];
                while let Some(candidate) = folders.pop() {
                    if candidate.path == folder {
                        return tabs(candidate);
                    }
                    folders.extend(&candidate.groups);
                }
                Vec::new()
            }
            Ok(None) => vec![Tab {
                path: self.location.clone(),
                name: section_name(&self.location, &None),
                color: None,
            }],
            Err(_) => Vec::new(),
        }
    }

    /// The first readable section, searching groups after sections, as OneNote opens a
    /// notebook.
    pub fn first_section(&self) -> Option<String> {
        let Ok(Some(notebook)) = &self.notebook else {
            return self.tabs("").pop().map(|tab| tab.path);
        };
        let mut folders = std::collections::VecDeque::from([notebook.catalog()]);
        while let Some(folder) = folders.pop_front() {
            if let Some(tab) = tabs(folder).into_iter().next() {
                return Some(tab.path);
            }
            folders.extend(folder.groups.iter().filter(|group| !recycle_bin(group)));
        }
        None
    }

    /// Whether the notebook lists a section at catalog path `path`.
    pub fn contains(&self, path: &str) -> bool {
        let Some(catalog) = self.catalog() else {
            return self.location == path;
        };
        let mut folders = vec![catalog];
        while let Some(folder) = folders.pop() {
            if folder.sections.iter().any(|section| section.path == path) {
                return true;
            }
            folders.extend(&folder.groups);
        }
        false
    }

    /// The notebook's folders of sections, for the sidebar.
    pub fn catalog(&self) -> Option<&Folder> {
        match &self.notebook {
            Ok(Some(notebook)) => Some(notebook.catalog()),
            _ => None,
        }
    }
}

/// Whether `folder` is the notebook's recycle bin, which OneNote keeps out of its lists.
pub fn recycle_bin(folder: &Folder) -> bool {
    folder.path.rsplit('/').next() == Some("OneNote_RecycleBin")
}

pub fn file_name(path: &Path) -> String {
    path.canonicalize()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A section's name: its display name, or its file's.
pub fn section_name(path: &str, name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| {
        Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// Section tabs for a notebook's readable top-level sections, in its order.
fn tabs(catalog: &Folder) -> Vec<Tab> {
    catalog
        .sections
        .iter()
        .filter_map(|section| match &section.state {
            SectionState::Readable { name, color } => Some(Tab {
                name: section_name(&section.path, name),
                path: section.path.clone(),
                color: *color,
            }),
            _ => None,
        })
        .collect()
}
