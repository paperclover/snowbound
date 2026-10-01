//! Unread changes, as OneNote 2010 shows them: a page another author changed since it was
//! last viewed is bold in the page list, and so are its section's tab and row and the groups
//! and notebook holding it, until the page is viewed and left or marked as read. OneNote keeps
//! this, and whether a notebook shows it, per user and machine in its cache, never in the
//! notebook's files; this keeps it in the cache folder's `read.json`.

use crate::{Library, State, UserEvent, menus, recycle};
use onestore::ExGuid;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Seconds from 1970 to 1980, where OneNote's Time32 starts.
const TIME32: u64 = 315_532_800;

/// What has been read, by notebook location.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Reads {
    notebooks: BTreeMap<String, Kept>,
    #[serde(skip)]
    file: PathBuf,
    /// The page shown: its notebook, section and space, and whether Mark as Unread left it
    /// unread, which leaving it then keeps.
    #[serde(skip)]
    viewing: Option<(String, [u8; 16], ExGuid, bool)>,
    /// The notebooks whose sections were last taken in, by `Arc` address.
    #[serde(skip)]
    taken: Vec<usize>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Kept {
    /// Show Unread Changes in This Notebook turned off.
    #[serde(default)]
    hidden: bool,
    /// By section file identity, in hexadecimal.
    sections: BTreeMap<String, Section>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Section {
    /// Time32 up to which changes to the section's pages are known.
    read: u32,
    unread: BTreeSet<ExGuid>,
}

fn hex(identity: [u8; 16]) -> String {
    identity.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now() -> u32 {
    let unix = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    u32::try_from(unix.saturating_sub(TIME32)).unwrap_or(u32::MAX)
}

impl Reads {
    /// What the cache folder `cache` keeps.
    pub fn load(cache: &Path) -> Self {
        let file = cache.join("read.json");
        let reads: Self = (notebook::fs::read(&file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { file, ..reads }
    }

    fn save(&self) {
        let partial = self.file.with_extension("partial");
        let saved = serde_json::to_vec(self)
            .map_err(std::io::Error::from)
            .and_then(|bytes| notebook::fs::write(&partial, bytes))
            .and_then(|()| notebook::fs::rename(&partial, &self.file));
        if let Err(error) = saved {
            eprintln!("Keeping what was read failed: {error}");
        }
    }

    fn section(&self, notebook: &str, section: [u8; 16]) -> Option<&Section> {
        let kept = self.notebooks.get(notebook).filter(|kept| !kept.hidden)?;
        kept.sections.get(&hex(section))
    }

    fn section_mut(&mut self, notebook: &str, section: [u8; 16]) -> &mut Section {
        let kept = self.notebooks.entry(notebook.to_owned()).or_default();
        (kept.sections.entry(hex(section))).or_insert_with(|| Section {
            read: now(),
            unread: BTreeSet::new(),
        })
    }

    /// Whether Show Unread Changes is on for the notebook at `notebook`.
    pub fn shown(&self, notebook: &str) -> bool {
        self.notebooks.get(notebook).is_none_or(|kept| !kept.hidden)
    }

    /// The unread pages of a section.
    pub fn pages(&self, notebook: &str, section: [u8; 16]) -> Option<&BTreeSet<ExGuid>> {
        self.section(notebook, section)
            .map(|section| &section.unread)
    }

    /// Takes in the pages of a section another author changed, by space.
    fn changed(
        &mut self,
        notebook: &str,
        section: [u8; 16],
        pages: impl IntoIterator<Item = ExGuid>,
    ) {
        self.section_mut(notebook, section).unread.extend(pages);
        self.save();
    }

    /// Takes in a section as its file stands, its pages with their `LastModifiedTime`: those
    /// changed since it was last known are unread.
    fn arrived(
        &mut self,
        notebook: &str,
        section: [u8; 16],
        pages: impl IntoIterator<Item = (ExGuid, Option<u32>)>,
    ) {
        let now = now();
        let kept = self.section_mut(notebook, section);
        let read = kept.read;
        let mut newest = read.max(now);
        for (space, modified) in pages {
            if modified.is_some_and(|modified| modified > read) {
                kept.unread.insert(space);
            }
            newest = newest.max(modified.unwrap_or(0));
        }
        kept.read = newest;
        self.save();
    }
}

impl State {
    /// The open section's unread pages.
    pub(crate) fn unread_pages(&self) -> HashSet<ExGuid> {
        let Some(session) = &self.session else {
            return HashSet::new();
        };
        let section = session
            .library
            .section_identity(&session.tabs[session.tab].path);
        (section.and_then(|section| self.reads.pages(&session.library.location, section)))
            .map_or_else(HashSet::new, |pages| pages.iter().copied().collect())
    }

    /// The sections of `library` holding unread pages, but the recycle bin's.
    fn unread_sections<'a>(&'a self, library: &'a Library) -> impl Iterator<Item = &'a str> + 'a {
        (menus::folders(library).into_iter())
            .flat_map(|folder| &folder.sections)
            .filter(|section| {
                (self.reads.pages(&library.location, section.file_id))
                    .is_some_and(|pages| !pages.is_empty())
            })
            .map(|section| section.path.as_str())
    }

    /// The notebooks, groups and sections holding unread pages, by `Library::key`.
    pub(crate) fn unread_keys(&self) -> HashSet<String> {
        let mut keys = HashSet::new();
        for library in &self.notebooks {
            for mut path in self.unread_sections(library) {
                keys.insert(library.key(path));
                while let Some((folder, _)) = path.rsplit_once('/') {
                    keys.insert(library.key(folder));
                    path = folder;
                }
                keys.insert(library.key(""));
            }
        }
        keys
    }

    /// Whether `library` has an unread page anywhere.
    pub(crate) fn unread_notebook(&self, library: &Library) -> bool {
        self.unread_sections(library).next().is_some()
    }

    /// Follows what is read, each frame: a notebook's sections are known from when it opens,
    /// and the page shown turns read once left, its section known up to then.
    pub(crate) fn follow_reading(&mut self) {
        let taken: Vec<usize> = self
            .notebooks
            .iter()
            .map(|library| Arc::as_ptr(library) as usize)
            .collect();
        if taken != self.reads.taken {
            let mut added = false;
            for library in &self.notebooks {
                for folder in menus::folders(library) {
                    for section in &folder.sections {
                        let kept = self
                            .reads
                            .notebooks
                            .entry(library.location.clone())
                            .or_default();
                        if !kept.sections.contains_key(&hex(section.file_id)) {
                            self.reads.section_mut(&library.location, section.file_id);
                            added = true;
                        }
                    }
                }
            }
            if added {
                self.reads.save();
            }
            self.reads.taken = taken;
        }
        let viewing = self.session.as_ref().and_then(|session| {
            let path = &session.tabs[session.tab].path;
            let section = session.library.section_identity(path)?;
            (session.version.is_none() && !recycle::binned(path))
                .then(|| (session.library.location.clone(), section, session.space))
        });
        let same = |(notebook, section, space, _): &(String, [u8; 16], ExGuid, bool)| {
            viewing.as_ref() == Some(&(notebook.clone(), *section, *space))
        };
        if self.reads.viewing.as_ref().is_some_and(same) {
            return;
        }
        if let Some((notebook, section, space, kept)) = self.reads.viewing.take() {
            let left = self.reads.section_mut(&notebook, section);
            let removed = !kept && left.unread.remove(&space);
            let moved = viewing.as_ref().is_none_or(|(_, now, _)| *now != section);
            if moved {
                left.read = left.read.max(now());
            }
            if removed || moved {
                self.reads.save();
            }
        }
        self.reads.viewing =
            viewing.map(|(notebook, section, space)| (notebook, section, space, false));
    }

    /// Pages of the open section another author changed, as its sync reports them.
    pub(crate) fn changed_elsewhere(&mut self, spaces: &[ExGuid]) {
        if let Some(session) = &self.session
            && let Some(section) = session
                .library
                .section_identity(&session.tabs[session.tab].path)
        {
            self.reads
                .changed(&session.library.location, section, spaces.iter().copied());
        }
    }

    /// Sections of `library` at catalog `paths` whose files another client changed: their
    /// pages are read on a thread of their own, those changed since unread.
    pub(crate) fn sections_changed(&self, library: &Arc<Library>, paths: Vec<String>) {
        let (library, proxy) = (Arc::clone(library), self.proxy.clone());
        crate::spawn(move || {
            let Ok(Some(notebook)) = &library.notebook else {
                return;
            };
            let read: Vec<_> = (paths.iter())
                .filter(|path| !recycle::binned(path))
                .filter_map(|path| {
                    let section = library.section_identity(path)?;
                    let image = notebook.read_section(path).ok()?;
                    let pages = notebook::session::stored_pages(&image).ok()?;
                    Some((section, pages))
                })
                .collect();
            let location = library.location.clone();
            let _ = proxy.send_event(UserEvent::Then(Box::new(move |state: &mut State| {
                for (section, pages) in read {
                    let pages = pages.iter().map(|page| (page.space, page.modified));
                    state.reads.arrived(&location, section, pages);
                }
                Ok(())
            })));
        });
    }

    /// Mark as Read, or as Unread, on the page shown, as OneNote 2010's Ctrl+Q does.
    pub(crate) fn toggle_read(&mut self) {
        let Some((notebook, section, space, kept)) = self.reads.viewing.as_mut() else {
            return;
        };
        let pages = &mut self.reads.notebooks.entry(notebook.clone()).or_default();
        let unread = &mut (pages.sections.entry(hex(*section)).or_default()).unread;
        *kept = unread.insert(*space);
        if !*kept {
            unread.remove(space);
        }
        self.reads.save();
    }

    /// Whether the page shown is unread.
    pub(crate) fn page_unread(&self) -> bool {
        self.reads
            .viewing
            .as_ref()
            .is_some_and(|(notebook, section, space, _)| {
                self.reads
                    .pages(notebook, *section)
                    .is_some_and(|pages| pages.contains(space))
            })
    }

    /// Mark Notebook as Read.
    pub(crate) fn mark_notebook_read(&mut self, library: &Library) {
        if let Some(kept) = self.reads.notebooks.get_mut(&library.location) {
            kept.sections
                .values_mut()
                .for_each(|section| section.unread.clear());
            if let Some((.., kept)) = &mut self.reads.viewing {
                *kept = false;
            }
            self.reads.save();
        }
    }

    /// Show Unread Changes in This Notebook, on or off.
    pub(crate) fn toggle_unread_shown(&mut self, library: &Library) {
        let kept = self
            .reads
            .notebooks
            .entry(library.location.clone())
            .or_default();
        kept.hidden = !kept.hidden;
        self.reads.save();
    }

    /// Next Unread: the next unread page of the open section, else the first of the next
    /// section, in the notebook's order, holding one.
    pub(crate) fn next_unread(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let unread = self.unread_pages();
        let at = session
            .pages
            .iter()
            .position(|(space, ..)| *space == session.space);
        let after = (session.pages.iter().skip(at.map_or(0, |at| at + 1)))
            .find(|(space, ..)| unread.contains(space));
        if let Some((space, ..)) = after {
            self.commands.push(crate::Command::OpenPage(*space));
            return;
        }
        let library = Arc::clone(&session.library);
        let here = session.tabs[session.tab].path.clone();
        let paths: Vec<String> = (menus::folders(&library).into_iter())
            .flat_map(|folder| library.tabs(&folder.path))
            .map(|tab| tab.path)
            .collect();
        let start = paths
            .iter()
            .position(|path| *path == here)
            .map_or(0, |at| at + 1);
        let next = (paths[start..].iter().chain(&paths[..start])).find_map(|path| {
            let section = library.section_identity(path)?;
            let first = self.reads.pages(&library.location, section)?.first()?;
            Some((path.clone(), *first))
        });
        if let Some((path, space)) = next {
            self.last_pages.insert(library.key(&path), space);
            self.commands
                .push(crate::Command::OpenSection(library, path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(n: u32, modified: Option<u32>) -> (ExGuid, Option<u32>) {
        (
            ExGuid {
                guid: [n as u8; 16],
                n,
            },
            modified,
        )
    }

    /// A section first known has nothing unread; pages another client changed after are,
    /// and stay so across a reload, until read; a notebook not showing them shows none.
    #[test]
    fn pages_changed_elsewhere_stay_unread_until_read() {
        let folder = std::env::temp_dir().join(format!("snowbound-unread-{}", std::process::id()));
        notebook::fs::create_dir_all(&folder).unwrap();
        let (notebook, section) = ("/notebook", [3; 16]);
        let mut reads = Reads::load(&folder);
        let known = reads.section_mut(notebook, section).read;
        reads.arrived(
            notebook,
            section,
            [page(1, Some(known - 10)), page(2, Some(known + 5))],
        );
        let unread = |reads: &Reads| -> Vec<u32> {
            (reads.pages(notebook, section).into_iter().flatten())
                .map(|space| space.n)
                .collect()
        };
        assert_eq!(unread(&reads), [2]);
        reads.changed(
            notebook,
            section,
            [ExGuid {
                guid: [4; 16],
                n: 4,
            }],
        );
        let mut reads = Reads::load(&folder);
        assert_eq!(unread(&reads), [2, 4]);
        // Known up to the newest change: the same page arriving again is no news.
        reads.section_mut(notebook, section).unread.clear();
        reads.arrived(notebook, section, [page(2, Some(known + 5))]);
        assert!(unread(&reads).is_empty());
        reads.changed(
            notebook,
            section,
            [ExGuid {
                guid: [4; 16],
                n: 4,
            }],
        );
        reads.notebooks.get_mut(notebook).unwrap().hidden = true;
        assert!(reads.pages(notebook, section).is_none() && !reads.shown(notebook));
        notebook::fs::remove_dir_all(&folder).unwrap();
    }
}
