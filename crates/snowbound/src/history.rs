//! Page versions as OneNote 2010 shows them (Share, Page Versions): a page's earlier states
//! listed under it by date and author, each read-only under a yellow bar whose menu restores,
//! deletes or copies it. `corpus/page-versions` records OneNote's own.

use super::*;
use onestore::PageVersion;

/// Which versions a Delete All Versions deletes.
#[derive(Clone, Copy, PartialEq)]
pub enum Scope {
    Section,
    /// The section group holding the open section, with the groups inside it.
    Group,
    Notebook,
}

impl Session {
    /// A page's versions, newest first.
    pub(crate) fn page_versions(&self, page: ExGuid) -> &[PageVersion] {
        self.history
            .iter()
            .find(|(listed, _)| *listed == page)
            .map_or(&[], |(_, versions)| versions)
    }

    /// Whether the open page takes no edits: a conflict page, a version, or a page of the
    /// recycle bin.
    pub(crate) fn read_only(&self) -> bool {
        self.version.is_some()
            || self.conflict(self.space).is_some()
            || crate::recycle::binned(&self.tabs[self.tab].path)
    }

    /// Version `version` of page `space` as the editor shows it: read-only, what it changed
    /// since the version before it banded as OneNote bands it.
    pub(crate) fn version_reader(
        &self,
        space: ExGuid,
        version: ExGuid,
    ) -> impl FnOnce() -> Result<Page, Box<dyn Error>> + Send + 'static {
        let replica = Arc::clone(self.section.replica());
        let versions = self.page_versions(space);
        let older = versions
            .iter()
            .position(|listed| listed.context == version)
            .and_then(|at| versions.get(at + 1))
            .map(|older| older.context);
        move || {
            let page = replica.version(space, version)?;
            let older = older
                .map(|older| replica.version(space, older))
                .transpose()?;
            Ok(canvas::conflict::changes(page, older.as_ref()))
        }
    }

    pub(crate) fn refresh_history(&mut self) -> Result<(), Box<dyn Error>> {
        self.history = self.section.versions()?;
        if self
            .shown_history
            .is_some_and(|page| self.page_versions(page).is_empty())
        {
            self.shown_history = None;
        }
        Ok(())
    }

    /// Whether the open section lies in a section group; a section opened on its own has a
    /// file path, not a catalog one.
    pub(crate) fn grouped(&self) -> bool {
        self.library.catalog().is_some() && self.tabs[self.tab].path.contains('/')
    }
}

/// A version's row in the page list: its date and who last changed it.
pub fn label(version: &PageVersion) -> String {
    match (version.modified, &version.author) {
        (Some(modified), Some(author)) => format!("{} {author}", platform::short_date(modified)),
        (Some(modified), None) => platform::short_date(modified),
        (None, author) => author.clone().unwrap_or_default(),
    }
}

/// The bar's menu over an open version, in OneNote 2010's words, and what its choice asks.
/// Copy Page To opens `copy`, listing the folder's sections.
pub fn menu(
    ui: &mut Ui,
    anchor: ui::Anchor,
    [page, version]: [ExGuid; 2],
    grouped: bool,
    sections: &[&str],
) -> Option<Command> {
    let menu = Id::ROOT.child("conflict-menu");
    let copy = Id::ROOT.child("conflict-copy");
    let item = |text| ui::popup::Item {
        text,
        ..Default::default()
    };
    let drawn = |text, icon| ui::popup::Item {
        icon: Some(icon),
        ..item(text)
    };
    let items = [
        drawn("Restore Version", art::UNDO),
        drawn("Delete Version", art::DELETE),
        ui::popup::Item {
            disabled: sections.is_empty(),
            ..drawn("Copy Page To", art::COPY)
        },
        ui::popup::Item {
            separated: true,
            ..drawn("Delete All Versions in Section", art::DELETE)
        },
        ui::popup::Item {
            disabled: !grouped,
            ..drawn("Delete All Versions in Section Group", art::DELETE)
        },
        drawn("Delete All Versions in Notebook", art::DELETE),
        ui::popup::Item {
            separated: true,
            ..drawn("Hide Page Versions", art::PAGE_VERSIONS)
        },
    ];
    let chosen = match ui::popup::menu(ui, menu, anchor, &items, None) {
        Some(0) => Some(Command::RestoreVersion { page, version }),
        Some(1) => Some(Command::DeletePageVersion { page, version }),
        Some(2) => {
            ui.open_popup(copy);
            None
        }
        Some(3) => Some(Command::DeleteAllVersions(Scope::Section)),
        Some(4) => Some(Command::DeleteAllVersions(Scope::Group)),
        Some(5) => Some(Command::DeleteAllVersions(Scope::Notebook)),
        Some(_) => Some(Command::History { page, show: false }),
        None => None,
    };
    let targets: Vec<_> = sections.iter().map(|name| item(name)).collect();
    let tab = ui::popup::menu(ui, copy, anchor, &targets, None);
    chosen.or(tab.map(|tab| Command::CopyVersion { version, tab }))
}

impl State {
    /// Shows or hides a page's versions under it; hiding leaves an open version for its page.
    pub(crate) fn show_history(&mut self, page: ExGuid, show: bool) -> Result<(), Box<dyn Error>> {
        let session = self.session.as_mut().ok_or("No section is open")?;
        session.shown_history = show.then_some(page);
        if !show && session.version.is_some() {
            self.commands.push(Command::OpenPage(page));
        }
        Ok(())
    }

    pub(crate) fn open_version(
        &mut self,
        page: ExGuid,
        version: ExGuid,
    ) -> Result<(), Box<dyn Error>> {
        let session = self.session.as_ref().ok_or("No section is open")?;
        let read = session.version_reader(page, version);
        self.load(move || Ok(Loaded::Version(page, version, read()?)));
        Ok(())
    }

    /// Restore Version: the version becomes the page, and the page as it stood the newest
    /// version.
    pub(crate) fn restore_version(
        &mut self,
        page: ExGuid,
        version: ExGuid,
    ) -> Result<(), Box<dyn Error>> {
        let session = self.session.as_mut().ok_or("No section is open")?;
        session
            .section
            .restore_version(page, version, &self.author)?;
        session.refresh_history()?;
        self.edited(vec![page]);
        self.commands.push(Command::OpenPage(page));
        Ok(())
    }

    pub(crate) fn delete_version(
        &mut self,
        page: ExGuid,
        version: ExGuid,
    ) -> Result<(), Box<dyn Error>> {
        let session = self.session.as_mut().ok_or("No section is open")?;
        session.section.delete_versions(&[(page, vec![version])])?;
        session.refresh_history()?;
        self.commands.push(Command::OpenPage(page));
        Ok(())
    }

    /// Delete All Versions in the section, its group or the notebook, once confirmed.
    pub(crate) fn delete_all_versions(&mut self, scope: Scope) -> Result<(), Box<dyn Error>> {
        let session = self.session.as_ref().ok_or("No section is open")?;
        let tab = &session.tabs[session.tab];
        let (open, section) = (tab.path.clone(), tab.name.clone());
        let folder = open
            .rsplit_once('/')
            .map_or("", |(folder, _)| folder)
            .to_owned();
        let (container, name) = match scope {
            Scope::Section => ("section", section),
            Scope::Group => (
                "section group",
                folder.rsplit('/').next().unwrap_or(&folder).to_owned(),
            ),
            Scope::Notebook => ("notebook", session.library.name.clone()),
        };
        let asked = Arc::clone(&session.library);
        platform::confirm(
            &format!("Do you want to delete all page versions in the {container} \"{name}\"?"),
            "You can't restore these versions afterward.",
            "Cancel",
            "Delete Versions",
            self.reply(move |state, ()| state.clear_versions(scope, &asked, open, folder)),
        );
        Ok(())
    }

    /// Deletes every page version in `scope` around `open`, the section in `library` that
    /// was shown when asked; nothing once another is shown.
    fn clear_versions(
        &mut self,
        scope: Scope,
        library: &Arc<Library>,
        open: String,
        folder: String,
    ) -> Result<(), Box<dyn Error>> {
        let Some(session) = self.session.as_mut().filter(|session| {
            Arc::ptr_eq(&session.library, library) && session.tabs[session.tab].path == open
        }) else {
            return Ok(());
        };
        clear(&session.section)?;
        session.refresh_history()?;
        if session.version.is_some() {
            self.commands.push(Command::OpenPage(session.space));
        }
        if scope == Scope::Section {
            return Ok(());
        }
        // The other sections, each opened, cleared and closed in the background.
        let folder = match scope {
            Scope::Group => folder,
            _ => String::new(),
        };
        let library = Arc::clone(&session.library);
        let proxy = self.proxy.clone();
        crate::spawn(move || {
            for path in sections(&library, &folder) {
                if path == open {
                    continue;
                }
                let cleared = library
                    .open(&path, notify(proxy.clone()))
                    .and_then(|section| {
                        clear(&section)?;
                        Ok(section.close()?)
                    });
                if let Err(error) = cleared {
                    eprintln!("Deleting the versions in {path} failed: {error}");
                }
            }
        });
        Ok(())
    }
}

/// Deletes every page version `section` holds.
fn clear(section: &notebook::session::Section) -> Result<(), notebook::Error> {
    let all: Vec<(ExGuid, Vec<ExGuid>)> = (section.versions()?.into_iter())
        .map(|(page, versions)| {
            (
                page,
                versions.iter().map(|version| version.context).collect(),
            )
        })
        .collect();
    if !all.is_empty() {
        section.delete_versions(&all)?;
    }
    Ok(())
}

/// The readable sections at catalog path `folder` and in the groups inside it, leaving out
/// the recycle bin.
fn sections(library: &Library, folder: &str) -> Vec<String> {
    let Some(catalog) = library.catalog() else {
        return vec![library.location.clone()];
    };
    let root = library::folders(catalog, |_| true)
        .into_iter()
        .find(|candidate| candidate.path == folder);
    root.map_or_else(Vec::new, |root| {
        library::folders(root, |group| !library::recycle_bin(&group.path))
    })
    .into_iter()
    .flat_map(|folder| library.tabs(&folder.path))
    .map(|tab| tab.path)
    .collect()
}
