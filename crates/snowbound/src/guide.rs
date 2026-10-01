//! The Snowbound Guide, a notebook the app carries (`docs/guide-notebook`). It opens from a
//! copy in the user's documents, so the one inside the app is never edited.

use crate::{State, platform};
use notebook::session::Notebook;
use std::{error::Error, path::Path};

const NAME: &str = "Snowbound Guide";
/// Whether the welcome screen offers the guide; off until Clover has read it through.
pub(crate) const OFFERED: bool = false;

macro_rules! file {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!(
                "../../../docs/guide-notebook/Snowbound Guide/",
                $name
            ))
            .as_slice(),
        )
    };
}

const FILES: [(&str, &[u8]); 6] = [
    file!("Open Notebook.onetoc2"),
    file!("Getting Started.one"),
    file!("Writing & Formatting.one"),
    file!("Tags & Search.one"),
    file!("Drawing & Recording.one"),
    file!("Sharing & Sync.one"),
];

impl State {
    /// Opens the guide in the user's documents, copying it there first unless it is there.
    pub(crate) fn open_guide(&mut self) -> Result<(), Box<dyn Error>> {
        let documents = platform::documents_dir().ok_or("There is no Documents folder")?;
        let folder = notebook::fs::absolute(documents.join(NAME))?;
        if notebook::fs::metadata(&folder).is_err() {
            copy(&folder, &self.cache)?;
        }
        self.open_notebook(folder.to_string_lossy().into_owned(), None);
        Ok(())
    }
}

/// Writes the guide to `folder` under a temporary name, then gives it its own, so a copy cut
/// short never passes for the guide.
fn copy(folder: &Path, cache: &Path) -> Result<(), Box<dyn Error>> {
    let partial = folder.with_file_name(format!("{NAME}.partial"));
    if notebook::fs::metadata(&partial).is_ok() {
        notebook::fs::remove_dir_all(&partial)?;
    }
    notebook::fs::create_dir_all(&partial)?;
    for (name, bytes) in FILES {
        notebook::fs::write(partial.join(name), bytes)?;
    }
    // The snowflake tag's art, kept in the notebook as Customize Tags keeps it.
    Notebook::open(&partial, cache)?.map_tag_art(
        "Snowflake",
        34,
        include_bytes!("../assets/tags/snowflake.svg"),
        "svg",
    )?;
    notebook::fs::rename(&partial, folder)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The carried files are the committed guide's, and the copy reads as that notebook with
    /// its tag art.
    #[test]
    fn the_guide_copies_whole() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide-notebook");
        let mut committed: Vec<String> = notebook::fs::read_dir(root.join(NAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.'))
            .collect();
        committed.sort();
        let mut carried: Vec<String> = FILES.iter().map(|(name, _)| name.to_string()).collect();
        carried.sort();
        assert_eq!(carried, committed);

        let temporary =
            std::env::temp_dir().join(format!("snowbound-guide-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&temporary);
        let folder = temporary.join(NAME);
        copy(&folder, &temporary.join("cache")).unwrap();
        let notebook = Notebook::open(&folder, temporary.join("cache")).unwrap();
        assert_eq!(notebook.catalog().sections.len(), 5);
        let art = notebook.tag_art().unwrap();
        assert_eq!(
            art.iter()
                .map(|tag| (tag.name.as_str(), tag.shape))
                .collect::<Vec<_>>(),
            [("Snowflake", 34)]
        );
        let committed = notebook::fs::read_to_string(root.join(NAME).join(".snowbound/tags.json"));
        assert!(committed.unwrap().contains(&art[0].art));
        notebook::fs::remove_dir_all(&temporary).unwrap();
    }
}
