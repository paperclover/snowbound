//! OneNote 2010's Notebook Recycle Bin: the notebook's `OneNote_RecycleBin` group shown as a
//! row of its own tabs, Deleted Pages and each deleted section, read-only beneath a bar
//! saying how to restore them. Pages go back to a section and sections to a folder, and Empty
//! Recycle Bin deletes what the bin holds for good.

use crate::{Command, Library, Loaded, State, filetime, library, menus, notify, read_session};
use onestore::{
    ExGuid,
    op::{Edit, Op, SectionOp},
};
use std::{error::Error, sync::Arc};
use ui::{Axis, Spec, Ui, children, fill, fit};

/// The bin's folder, a section group of the notebook's.
pub const BIN: &str = "OneNote_RecycleBin";

/// What the bin's commands ask of the section it shows.
pub enum Request {
    /// Puts page `space` at the end of the section at catalog `path`, or with `copy` a copy
    /// of it, as OneNote's Move or Copy takes a page out of the bin.
    Restore {
        space: ExGuid,
        path: String,
        copy: bool,
    },
    /// Deletes pages for good.
    Purge(Vec<ExGuid>),
}

/// Whether the section or group at catalog `path` lies in its notebook's recycle bin.
pub fn binned(path: &str) -> bool {
    library::recycle_bin(&menus::folder(path))
}

impl State {
    /// Whether the open section is one of the recycle bin's.
    pub(crate) fn in_recycle_bin(&self) -> bool {
        (self.session.as_ref()).is_some_and(|session| binned(&session.tabs[session.tab].path))
    }

    /// Notebook Recycle Bin: shows `library`'s bin at its first section holding pages, or
    /// while it shows, goes back to the notebook.
    pub(crate) fn toggle_recycle_bin(&mut self, library: Arc<Library>) {
        if self.in_recycle_bin() {
            if let Some(path) = library.first_section() {
                self.commands.push(Command::OpenSection(library, path));
            }
            return;
        }
        let paths: Vec<String> = library.tabs(BIN).into_iter().map(|tab| tab.path).collect();
        let proxy = self.proxy.clone();
        self.load(move || {
            for path in paths {
                let section = library.open(&path, notify(proxy.clone()))?;
                if section.pages()?.is_empty() {
                    section.close()?;
                    continue;
                }
                let (session, page) = read_session(section, library, path, None)?;
                return Ok(Loaded::Section(Box::new(session), page));
            }
            Err("The Recycle Bin is empty.".into())
        });
    }

    /// Empty Recycle Bin on `library`, once confirmed as OneNote asks.
    pub(crate) fn empty_recycle_bin(&mut self, library: Arc<Library>) {
        crate::platform::confirm(
            "Are you sure you want to empty the Recycle Bin for this notebook?",
            "Its pages and sections are deleted for good.",
            "Cancel",
            "Empty Recycle Bin",
            self.reply(|state, ()| {
                let change = crate::manage::Structure::EmptyRecycleBin;
                state.commands.push(Command::Structure(library, change));
                Ok(())
            }),
        );
    }

    /// Does `request` to the bin's open section, a purge once confirmed.
    pub(crate) fn recycle(&mut self, request: Request) -> Result<(), Box<dyn Error>> {
        let Request::Purge(spaces) = &request else {
            return self.recycle_now(request);
        };
        let spaces = spaces.clone();
        crate::platform::confirm(
            "Are you sure you want to delete this page for good?",
            "It can't be restored.",
            "Cancel",
            "Delete",
            self.reply(move |state, ()| {
                // The pages asked about, unless their section closed meanwhile.
                let open = state.session.as_ref().is_some_and(|session| {
                    (spaces.iter())
                        .all(|space| session.pages.iter().any(|(page, ..)| page == space))
                });
                if open {
                    state.recycle_now(request)
                } else {
                    Ok(())
                }
            }),
        );
        Ok(())
    }

    /// Does `request` to the bin's open section on a thread of its own, then shows the page
    /// after the ones gone, or the notebook once the section holds none.
    fn recycle_now(&mut self, request: Request) -> Result<(), Box<dyn Error>> {
        let gone = match &request {
            Request::Restore { copy: true, .. } => Vec::new(),
            Request::Restore { space, .. } => vec![*space],
            Request::Purge(spaces) => spaces.clone(),
        };
        let session = self.session.as_ref().ok_or("No section is open")?;
        let replica = Arc::clone(session.section.replica());
        let library = Arc::clone(&session.library);
        let shown = session.space;
        let at = (session.pages.iter()).position(|(space, ..)| gone.contains(space));
        let kept = |space: &ExGuid| !gone.contains(space);
        let after = at.and_then(|at| {
            let spaces = session.pages.iter().map(|(space, ..)| *space);
            (spaces.clone().skip(at).find(kept)).or_else(|| spaces.take(at).rev().find(kept))
        });
        let (author, proxy) = (self.author.clone(), self.proxy.clone());
        self.load(move || {
            carry_out(&library, &replica, request, &author)?;
            if let Some(show) = if gone.contains(&shown) {
                after
            } else {
                Some(shown)
            } {
                return Ok(Loaded::Page(show, replica.page(show)?));
            }
            // A section without pages shows no tab: the notebook shows instead.
            let path = library
                .first_section()
                .ok_or("The notebook has no sections")?;
            let section = library.open(&path, notify(proxy))?;
            let (session, page) = read_session(section, library, path, None)?;
            Ok(Loaded::Section(Box::new(session), page))
        });
        Ok(())
    }

    /// The bin's tab row leads with a way back to the notebook and where it is, as
    /// OneNote's "Recycle Bin for" does.
    pub(crate) fn recycle_heading(&mut self, theme: &ui::Theme) {
        let Some(session) = self.session.as_ref().filter(|_| self.in_recycle_bin()) else {
            return;
        };
        let library = Arc::clone(&session.library);
        let heading = format!("Recycle Bin for \u{201c}{}\u{201d}:", library.name);
        let back = ui::shell::tool_button(
            &mut self.ui,
            "recycle back",
            crate::art::BACK,
            theme.text,
            None,
        );
        if let Some(node) = self.ui.access(self.ui.id("recycle back")) {
            node.set_label("Back to the Notebook");
        }
        self.ui.leaf(
            "recycle heading",
            Spec {
                size: [fit(), fill()],
                text: Some(&heading),
                color: Some(theme.text),
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
        if back.clicked {
            self.toggle_recycle_bin(library);
        }
    }
}

/// Does `request` to the bin's section `replica` holds, in `library`, as `author`: a page
/// restored goes to its section as one op, keeping its identity, a copy under a new one, and
/// what leaves the bin goes from it as another.
fn carry_out(
    library: &Library,
    replica: &notebook::Replica,
    request: Request,
    author: &str,
) -> Result<(), Box<dyn Error>> {
    let gone = match request {
        Request::Restore { space, path, copy } => {
            let page = replica.page(space)?;
            let section = library.open(&path, || {})?;
            if copy {
                section.import_page(&page, author)?;
            } else {
                let ops = vec![notebook::session::moved(&page, author)?];
                section.replica().apply(
                    author,
                    Edit {
                        at: filetime(),
                        ops,
                    },
                )?;
            }
            section.close()?;
            (!copy).then_some(vec![space]).unwrap_or_default()
        }
        Request::Purge(spaces) => spaces,
    };
    if !gone.is_empty() {
        let ops = vec![Op::Section(SectionOp::Delete(gone))];
        replica.apply(
            author,
            Edit {
                at: filetime(),
                ops,
            },
        )?;
    }
    Ok(())
}

/// The bar above a page of the recycle bin, in OneNote 2010's words but for the menu item
/// that restores.
pub fn bar(ui: &mut Ui) {
    ui.open(
        "recycle bar",
        Spec {
            axis: Axis::Y,
            size: [fill(), children()],
            fill: Some(draw::srgb(0xff, 0xee, 0xc2)),
            pad: [12.0, 6.0],
            role: Some(accesskit::Role::Banner),
            ..Spec::default()
        },
    );
    for (part, text) in [
        (
            "restore",
            "To restore a page or section, right-click it and choose Restore To.",
        ),
        ("purge", "Content here is deleted after 60 days."),
    ] {
        ui.leaf(
            part,
            Spec {
                size: [fill(), fit()],
                text: Some(text),
                overflow: ui::Overflow::Wrap,
                color: Some(draw::srgb(0x20, 0x20, 0x20)),
                ..Spec::default()
            },
        );
    }
    ui.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use notebook::session::{Notebook, stored_pages};
    use onestore::PageCreation;

    const AUTHOR: &str = "Author";
    const FIRST: &str = "New Section 1.one";

    /// Waits until a section's edits are in its file.
    fn published(library: &Library, path: &str) {
        let section = library.open(path, || {}).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while section.sync_status().unwrap().queued > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "{path} never published"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        section.close().unwrap();
    }

    fn titles(root: &std::path::Path, path: &str) -> Vec<(String, Option<[u8; 16]>)> {
        let pages = stored_pages(&notebook::fs::read(root.join(path)).unwrap()).unwrap();
        (pages.into_iter())
            .map(|page| (page.page.title, page.page.identity))
            .collect()
    }

    /// Pages leave the recycle bin for a section, keeping their identity, or as copies under
    /// new ones, and deleted for good, each as an op; a section leaves for the notebook's
    /// folder, and Empty Recycle Bin takes the rest. `SNOWBOUND_RECYCLE_EXPORT` names a new
    /// directory receiving the notebook `before` and `after`, for cold reopens in OneNote 2010
    /// (`corpus/recycle-bin-view`).
    #[test]
    fn pages_and_sections_leave_the_recycle_bin_as_onenote_restores_them() {
        let folder = std::env::temp_dir().join(format!("snowbound-recycle-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&folder);
        notebook::fs::create_dir_all(&folder).unwrap();
        let (root, cache) = (folder.join("Recycled"), folder.join("cache"));
        let page = |title: &str| PageCreation::new(None, Some(title), AUTHOR).unwrap();
        let mut notebook =
            Notebook::create(&root, &cache, Notebook::NEW_COLOR, &page("Kept")).unwrap();
        for (name, title) in [
            ("Back", "Restored page"),
            ("Copied", "Copied page"),
            ("Purged", "Purged page"),
        ] {
            notebook.create_section("", name, &page(title)).unwrap();
        }
        notebook
            .create_section("", "Restored", &page("In a restored section"))
            .unwrap();
        notebook
            .create_section("", "Emptied", &page("In an emptied section"))
            .unwrap();
        for name in ["Back", "Copied", "Purged"] {
            let image = notebook.read_section(&format!("{name}.one")).unwrap();
            let pages = stored_pages(&image).unwrap();
            notebook
                .recycle_pages(&[pages[0].page.clone()], AUTHOR)
                .unwrap();
        }
        for name in ["Back", "Copied", "Purged", "Restored", "Emptied"] {
            notebook.delete(&format!("{name}.one")).unwrap();
        }
        let export = std::env::var_os("SNOWBOUND_RECYCLE_EXPORT").map(std::path::PathBuf::from);
        if let Some(directory) = &export {
            copy(&root, &directory.join("before"));
        }
        let library = Library::created(root.to_str().unwrap(), notebook, &cache);
        let deleted = format!("{BIN}/OneNote_DeletedPages.one");
        let binned = titles(&root, &deleted);
        let bin = library.open(&deleted, || {}).unwrap();
        let space = |title: &str| {
            let pages = bin.replica().pages().unwrap();
            pages
                .into_iter()
                .find(|(_, listed, _)| listed == title)
                .unwrap()
                .0
        };
        let (back, copied, purged) = (
            space("Restored page"),
            space("Copied page"),
            space("Purged page"),
        );
        let restore = |space, copy| Request::Restore {
            space,
            path: FIRST.into(),
            copy,
        };
        carry_out(&library, bin.replica(), restore(back, false), AUTHOR).unwrap();
        carry_out(&library, bin.replica(), restore(copied, true), AUTHOR).unwrap();
        carry_out(
            &library,
            bin.replica(),
            Request::Purge(vec![purged]),
            AUTHOR,
        )
        .unwrap();
        bin.close().unwrap();
        published(&library, FIRST);
        published(&library, &deleted);
        let identity = |title: &str| binned.iter().find(|(listed, _)| listed == title).unwrap().1;
        let first = titles(&root, FIRST);
        let listed: Vec<&str> = first.iter().map(|(title, _)| title.as_str()).collect();
        assert_eq!(listed, ["Kept", "Restored page", "Copied page"]);
        assert_eq!(
            first[1].1,
            identity("Restored page"),
            "a restored page keeps its identity"
        );
        assert_ne!(
            first[2].1,
            identity("Copied page"),
            "a copy takes a new one"
        );
        let left: Vec<String> = titles(&root, &deleted)
            .into_iter()
            .map(|(title, _)| title)
            .collect();
        assert_eq!(left, ["Copied page"]);

        let mut notebook = library.reopen().unwrap();
        notebook
            .move_entry(&format!("{BIN}/Restored.one"), "")
            .unwrap();
        notebook.empty_recycle_bin().unwrap();
        let sections: Vec<&str> = (notebook.catalog().sections.iter())
            .map(|section| section.path.as_str())
            .collect();
        assert_eq!(sections, [FIRST, "Restored.one"]);
        assert!(titles(&root, &deleted).is_empty());
        assert!(!root.join(BIN).join("Emptied.one").exists());
        if let Some(directory) = &export {
            copy(&root, &directory.join("after"));
        }
        notebook::fs::remove_dir_all(&folder).unwrap();
    }

    fn copy(from: &std::path::Path, to: &std::path::Path) {
        notebook::fs::create_dir_all(to).unwrap();
        for entry in notebook::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                notebook::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
}
