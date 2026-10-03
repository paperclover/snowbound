//! Changes to notebooks, sections, section groups and pages, as OneNote 2010 makes them:
//! structure through `notebook::session::Notebook`, pages as ops on the open section.

use crate::{Command, Library, Loaded, State, notify, platform, read_session, undo::Change};
use canvas::template::Template;
use notebook::session::Notebook;
use onestore::{
    ExGuid, PageCreation, PageEdit,
    op::{Edit, Op, PageOp, SectionOp},
    page::{Image, Page, PageObject},
};
use std::{error::Error, path::Path, sync::Arc};

/// A change to a notebook's sections and groups, by catalog path.
pub enum Structure {
    NewSection {
        folder: String,
    },
    NewGroup {
        folder: String,
    },
    Rename {
        path: String,
        name: String,
    },
    /// Moves a section or group to the recycle bin.
    Delete {
        path: String,
    },
    /// Deletes for good what the recycle bin holds.
    EmptyRecycleBin,
    /// Moves a section or group into another folder.
    Move {
        path: String,
        folder: String,
    },
    /// Orders a folder's sections and groups.
    Reorder {
        folder: String,
        paths: Vec<String>,
    },
    /// Moves a section or group back into a folder, which then takes the order `paths`, as
    /// Undo takes back a move.
    Return {
        path: String,
        folder: String,
        paths: Vec<String>,
    },
    /// Colours a section's tab (COLORREF); none is OneNote's None.
    Color {
        path: String,
        color: Option<u32>,
    },
    /// Notebook Properties: the display name on this computer, and a colour picked for the
    /// notebook.
    Properties {
        name: String,
        color: Option<u32>,
    },
    /// Sets, changes or removes a section's password: `key` opens it as it stands.
    Password {
        path: String,
        key: Option<onestore::protected::Key>,
        password: Option<zeroize::Zeroizing<String>>,
    },
}

/// Whether the notebook at `location` is a folder at the top of the app's iCloud Drive folder,
/// which the sidebar lists as long as it is there.
pub fn in_icloud_folder(location: &str) -> bool {
    crate::icloud::folder().is_some_and(|root| Path::new(location).parent() == Some(&root))
}

/// How long a notebook folder a listing of the iCloud folder missed must stay missing to count
/// as gone.
const VANISH: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether the folder at `path` is still not there `after` it was missed: not found, rather
/// than unreadable for a moment, as iCloud Drive's coordinated writes can leave it.
fn vanished(path: &Path, after: std::time::Duration) -> bool {
    std::thread::sleep(after);
    notebook::fs::metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

/// `current`, the path of a section, after `from` moved to `to`: a section moved itself,
/// or one inside a moved group.
fn follow(current: &str, from: &str, to: &str) -> String {
    match current.strip_prefix(from) {
        Some("") => to.to_owned(),
        Some(rest) if rest.starts_with('/') => format!("{to}{rest}"),
        _ => current.to_owned(),
    }
}

/// The section `library` shows after a change: `open` where it still lists it, else its
/// first, or none once it has no sections.
fn shown(library: &Library, open: Option<String>) -> Option<String> {
    open.filter(|path| library.contains(path))
        .or_else(|| library.first_section())
}

/// The first of `base`, `base 2`, `base 3`… that `taken` does not hold, as OneNote names
/// new sections ("New Section 1", …) and groups.
fn unused(taken: &[String], base: &str, numbered: bool) -> String {
    (1..)
        .map(|n| match (numbered, n) {
            (false, 1) => base.to_owned(),
            _ => format!("{base} {n}"),
        })
        .find(|name| !taken.iter().any(|taken| taken.eq_ignore_ascii_case(name)))
        .expect("Some number is free")
}

impl State {
    /// A page OneNote 2010 would create now, titled `title` in the Default font's face, with
    /// its date and time.
    pub(crate) fn dated_page(
        &self,
        before: Option<ExGuid>,
        title: &str,
    ) -> Result<PageCreation, Box<dyn Error>> {
        let creation = PageCreation::new(before, Some(title), &self.author)?.titled_in(
            &self.view.editor.default_font.face,
            self.view.editor.default_font.color,
        )?;
        let [date, time] = platform::date_text(creation.created());
        Ok(creation.dated(&date, &time)?)
    }

    /// Adds a page titled `title` at the end of the open section, or a subpage of page
    /// `under`, and opens it with its title ready for typing, as OneNote's New Page and New
    /// Subpage do.
    pub(crate) fn new_page(
        &mut self,
        under: Option<ExGuid>,
        title: &str,
    ) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_ref().ok_or("No section is open")?;
        // A subpage follows its page and the subpages already under it.
        let (before, level) = if let Some(under) = under {
            let at = session
                .pages
                .iter()
                .position(|(space, ..)| *space == under)
                .ok_or("That page is not listed")?;
            let level = session.pages[at].2;
            let next = session.pages[at + 1..]
                .iter()
                .find(|(.., below)| *below <= level)
                .map(|(space, ..)| *space);
            (next, (level + 1).min(3))
        } else {
            (None, 1)
        };
        let creation = self.dated_page(None, title)?;
        let space = creation.space();
        let mut ops = vec![Op::Section(SectionOp::Create(creation))];
        if under.is_some() {
            ops.push(Op::Section(SectionOp::Pages(vec![PageEdit::move_to(
                space, before, level,
            )?])));
        }
        let session = self.session.as_mut().ok_or("No section is open")?;
        session.section.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops,
            },
        )?;
        let shown = session.space;
        self.made(Change::Delete {
            pages: vec![space],
            show: Some(shown),
            created: true,
        });
        self.title_focus = Some(space);
        self.commands.push(Command::OpenPage(space));
        Ok(())
    }

    /// Gives the open page template `name`'s art in place of any it had.
    pub(crate) fn apply_template(&mut self, name: &'static str) -> Result<(), Box<dyn Error>> {
        let template = canvas::template::find(name).ok_or("That template is not available")?;
        self.with_art(Some(template), move |state, art| {
            state.persist()?;
            let session = state.session.as_ref().ok_or("No section is open")?;
            let ops = template_ops(&session.section.page(session.space)?, name, art)?;
            state.edit_page(ops)
        })
    }

    /// Calls `then` with `template`'s pictures, or none without one. Rasterizing and encoding
    /// them takes seconds on a slow machine, so they are made on a thread of their own, and
    /// `then` runs on this one after if the page they were chosen for is still open.
    pub(crate) fn with_art(
        &mut self,
        template: Option<&'static Template>,
        then: impl FnOnce(&mut State, Vec<Image>) -> Result<(), Box<dyn Error>> + Send + 'static,
    ) -> Result<(), Box<dyn Error>> {
        let Some(template) = template else {
            return then(self, Vec::new());
        };
        let space = self.session.as_ref().ok_or("No section is open")?.space;
        let proxy = self.proxy.clone();
        crate::spawn(move || {
            let art = template.pictures().map_err(|error| error.to_string());
            let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                if state
                    .session
                    .as_ref()
                    .is_some_and(|open| open.space == space)
                {
                    then(state, art?)
                } else {
                    Ok(())
                }
            })));
        });
        Ok(())
    }

    /// View, Page Color and Rule Lines: gives the open page `color`, a COLORREF,
    /// `rule_lines` and, when given, `art` in place of the art it had, as one undo step.
    pub(crate) fn paper_page(
        &mut self,
        color: Option<u32>,
        rule_lines: Option<onestore::page::RuleLines>,
        art: Option<Vec<Image>>,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.view.set_paper(color, rule_lines, art)?;
        self.respond(response);
        Ok(())
    }

    /// Applies `ops` to the open page as one edit and shows the result.
    pub(crate) fn edit_page(&mut self, ops: Vec<PageOp>) -> Result<(), Box<dyn Error>> {
        if ops.is_empty() {
            return Ok(());
        }
        let session = self.session.as_ref().ok_or("No section is open")?;
        let space = session.space;
        session.section.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops: ops.into_iter().map(|op| Op::Page { space, op }).collect(),
            },
        )?;
        self.edited(vec![space]);
        self.refresh()
    }

    /// Asks where to create a notebook and creates it, as File, New does in OneNote; the
    /// panel starts in Snowbound's iCloud Drive folder, whose notebooks every Mac lists.
    pub(crate) fn new_notebook(&mut self) {
        let icloud = crate::icloud::folder().or_else(crate::icloud::drive);
        let reply = self.reply(|state, root| state.create_notebook(root));
        platform::pick_new(
            "New Notebook",
            "My Notebook",
            "Create",
            icloud.as_deref(),
            reply,
        );
    }

    /// Lists the folders at the top of the app's iCloud Drive folder in the sidebar, each a
    /// notebook, as they come and go, and brings down what iCloud Drive keeps elsewhere of
    /// every notebook in it.
    pub(crate) fn list_icloud(&mut self) {
        let proxy = self.proxy.clone();
        // Listing a folder iCloud Drive has not brought down yet waits for it.
        crate::spawn(move || {
            let listed = crate::icloud::folder().and_then(|root| {
                let mut listed: Vec<String> = notebook::fs::read_dir(&root)
                    .ok()?
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                    .map(|entry| entry.path().to_string_lossy().into_owned())
                    .collect();
                listed.sort();
                Some(listed)
            });
            let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                if let Some(listed) = listed {
                    state.listed_icloud(listed);
                }
                for library in state.notebooks.clone() {
                    if library.in_icloud() {
                        state.fetch(library, None);
                    }
                }
                Ok(())
            })));
        });
    }

    /// Takes `listed`, the notebook folders at the top of the app's iCloud Drive folder, in
    /// place of those the sidebar lists from it.
    fn listed_icloud(&mut self, listed: Vec<String>) {
        let missed: Vec<String> = (self.notebooks.iter())
            .map(|library| &library.location)
            .filter(|location| {
                in_icloud_folder(location)
                    && !listed.contains(location)
                    && !self.folder_renaming(location)
                    && !self.trashing.contains(*location)
            })
            .cloned()
            .collect();
        for location in missed {
            self.confirm_gone(location);
        }
        for location in listed {
            if self.notebooks.iter().any(|open| open.location == location)
                || self.folder_renaming(&location)
                || self.trashing.contains(&location)
                || !self.icloud_reading.insert(location.clone())
            {
                continue;
            }
            let (cache, proxy) = (self.cache.clone(), self.proxy.clone());
            crate::spawn(move || {
                let gone = notebook::fs::metadata(&location).is_err();
                let library = Arc::new(Library::notebook(&location, &cache));
                let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                    state.icloud_reading.remove(&location);
                    if gone
                        || state.notebooks.iter().any(|open| open.location == location)
                        || state.folder_renaming(&location)
                        || state.trashing.contains(&location)
                        || notebook::fs::metadata(&location).is_err()
                    {
                        return Ok(());
                    }
                    state.notebooks.push(Arc::clone(&library));
                    state.save_settings();
                    state.show_arrived(&library);
                    state.fetch(library, None);
                    Ok(())
                })));
            });
        }
    }

    /// Closes the notebook at `location`, which a listing of the iCloud folder missed, once
    /// its folder is still not there `VANISH` later: iCloud Drive's own writes and downloads
    /// can hide a folder from a listing for a moment. The notebook shown stays while edits
    /// wait in it.
    fn confirm_gone(&mut self, location: String) {
        if !self.vanishing.insert(location.clone()) {
            return;
        }
        let proxy = self.proxy.clone();
        crate::spawn(move || {
            let gone = vanished(Path::new(&location), VANISH);
            let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                state.vanishing.remove(&location);
                let waiting = (state.session.as_ref())
                    .filter(|session| session.library.location == location)
                    .is_some_and(|session| {
                        session
                            .section
                            .pending()
                            .is_ok_and(|pending| !pending.is_empty())
                    });
                let listed = (state.notebooks.iter()).find(|open| open.location == location);
                if let Some(library) = listed.cloned().filter(|_| gone && !waiting) {
                    state.close_notebook(&library);
                }
                Ok(())
            })));
        });
    }

    /// Whether a folder rename is taking the notebook from or to `location`.
    pub(crate) fn folder_renaming(&self, location: &str) -> bool {
        self.folder_rename
            .as_ref()
            .is_some_and(|(from, to)| from == location || to == location)
    }

    /// Shows `library`, a notebook that just arrived or whose sections did, where no other
    /// notebook is shown: at its first section, or without one while none is here yet.
    pub(crate) fn show_arrived(&mut self, library: &Arc<Library>) {
        if self.session.is_some()
            || self
                .sectionless
                .as_ref()
                .is_some_and(|shown| shown.location != library.location)
        {
            return;
        }
        let first = library.first_section();
        self.sectionless = Some(Arc::clone(library));
        self.title();
        if let Some(path) = first {
            self.commands
                .push(Command::OpenSection(Arc::clone(library), path));
        }
    }

    /// Brings down the files of `library`, a notebook in iCloud Drive, that iCloud Drive keeps
    /// elsewhere, `missing` of them when last asked, and reads the notebook again as they
    /// arrive, until all have.
    pub(crate) fn fetch(&mut self, library: Arc<Library>, missing: Option<usize>) {
        let Some(root) = library.folder().map(Path::to_owned) else {
            return;
        };
        if missing.is_none() && !self.icloud_reading.insert(library.location.clone()) {
            return;
        }
        let proxy = self.proxy.clone();
        crate::spawn(move || {
            if missing.is_some() {
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            let now = crate::icloud::download(&root);
            let arrived = missing.map_or(now == 0 && library.downloading(), |before| now < before);
            let read = arrived
                .then(|| {
                    library
                        .reopen()
                        .map(|notebook| Arc::new(library.with(notebook)))
                })
                .and_then(|read| {
                    read.inspect_err(|error| eprintln!("{}: {error}", library.location))
                        .ok()
                });
            let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                let Some(listed) = state
                    .notebooks
                    .iter()
                    .find(|open| open.location == library.location)
                    .cloned()
                else {
                    state.icloud_reading.remove(&library.location);
                    return Ok(());
                };
                let current = match read {
                    Some(read) => {
                        let path = state
                            .session
                            .as_ref()
                            .filter(|session| session.library.location == read.location)
                            .map(|session| session.tabs[session.tab].path.clone())
                            .filter(|path| read.contains(path));
                        state.adopt(Arc::clone(&read), path.as_deref())?;
                        read
                    }
                    None => listed,
                };
                if now == 0 {
                    state.icloud_reading.remove(&current.location);
                } else {
                    state.fetch(current, Some(now));
                }
                Ok(())
            })));
        });
    }

    /// Creates a notebook in the new folder `root` and opens it.
    pub(crate) fn create_notebook(
        &mut self,
        root: std::path::PathBuf,
    ) -> Result<(), Box<dyn Error>> {
        let page = self.dated_page(None, "")?;
        let (cache, notify) = (self.cache.clone(), notify(self.proxy.clone()));
        let theme = self.notebook_theme.clone();
        self.load(move || {
            let location = notebook::fs::absolute(&root)?
                .to_string_lossy()
                .into_owned();
            let notebook = Notebook::create(&location, &cache, Notebook::NEW_COLOR, &page)?;
            if theme.is_some() {
                notebook.save_themes(notebook::sidecar::themes::Themes {
                    assignments: vec![notebook::sidecar::themes::Assignment {
                        scope: notebook::sidecar::themes::Scope::Notebook,
                        theme,
                        assigned: crate::filetime(),
                    }],
                    ..Default::default()
                })?;
            }
            let library = Arc::new(Library::created(&location, notebook, &cache));
            let path = library
                .first_section()
                .ok_or("The new notebook has no section")?;
            let section = library.open(&path, notify)?;
            let (session, page) = read_session(section, library, path, None)?;
            Ok(Loaded::Section(Box::new(session), page))
        });
        Ok(())
    }

    /// Closes the notebooks in iCloud Drive once its account signs out or changes: their
    /// folders go with the account. Replicas holding edits stay, the open section's also
    /// exported as a recovery archive; signing in again and reopening publishes them.
    pub(crate) fn icloud_account_changed(&mut self) {
        let closing: Vec<Arc<Library>> = self
            .notebooks
            .iter()
            .filter(|library| library.in_icloud())
            .cloned()
            .collect();
        if let Some(session) = &self.session
            && session.library.in_icloud()
            && session
                .section
                .pending()
                .is_ok_and(|pending| !pending.is_empty())
            && let Some(folder) = platform::settings_dir().map(|folder| folder.join("Recovery"))
        {
            let archive = folder.join(format!(
                "{}-{}.sqlite",
                session.library.name,
                crate::filetime()
            ));
            let exported = notebook::fs::create_dir_all(&folder)
                .map_err(notebook::Error::from)
                .and_then(|()| session.section.export_recovery(&archive));
            if let Err(error) = exported {
                eprintln!("{}: {error}", archive.display());
            }
        }
        for library in closing {
            self.close_notebook(&library);
        }
        crate::icloud::look_up(crate::icloud_listed(self.proxy.clone()));
    }

    /// Closes `library`: its files stay, its offline copies go unless edits wait in them,
    /// and the sidebar and the next launch leave it out.
    pub(crate) fn close_notebook(&mut self, library: &Arc<Library>) {
        self.notebooks.retain(|open| !Arc::ptr_eq(open, library));
        self.undo.close(&library.location);
        if let Some(background) = &library.background {
            background.discard();
        }
        let shown = |shown: &Arc<Library>| shown.location == library.location;
        if self.sectionless.as_ref().is_some_and(shown)
            || self
                .session
                .as_ref()
                .is_some_and(|session| shown(&session.library))
        {
            self.persist().unwrap_or_else(|error| eprintln!("{error}"));
            self.session = None;
            self.sectionless = None;
            match self
                .notebooks
                .iter()
                .find_map(|open| Some((Arc::clone(open), open.first_section()?)))
            {
                Some((open, path)) => self.commands.push(Command::OpenSection(open, path)),
                None => {
                    self.sectionless = self.notebooks.first().cloned();
                    self.title();
                }
            }
        }
        self.save_settings();
    }

    /// Asks whether to move `library`'s folder to the Trash, telling of changes not yet in its
    /// files, which go with it, then does.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn delete_notebook(&mut self, library: Arc<Library>) {
        // What was typed last counts too.
        self.persist().unwrap_or_else(|error| eprintln!("{error}"));
        let open = (self.session.as_ref())
            .filter(|session| Arc::ptr_eq(&session.library, &library))
            .and_then(|session| session.section.pending().ok())
            .map_or(0, |pending| pending.len() as u64);
        let waiting = (library.background.iter())
            .flat_map(|background| background.status())
            .map(|(_, sync)| sync.queued)
            .sum::<u64>()
            .max(open);
        let mut detail = if in_icloud_folder(&library.location) {
            "It moves to the Trash on every device signed in to this iCloud account.".to_owned()
        } else {
            format!("It moves to the {}.", platform::TRASH)
        };
        if waiting > 0 {
            let changes = match waiting {
                1 => "1 change".to_owned(),
                count => format!("{count} changes"),
            };
            detail += &format!(" {changes} not yet saved to its files will be lost.");
        }
        crate::confirm(
            &format!("Delete “{}”?", library.name),
            &detail,
            "Cancel",
            "Delete",
            self.reply(move |state, ()| {
                state.trash_notebook(library);
                Ok(())
            }),
        );
    }

    /// Closes `library` and moves its folder to the Trash on a thread of its own, forgetting
    /// what this computer and the next launch keep of it. Refused, it opens again.
    #[cfg(not(target_arch = "wasm32"))]
    fn trash_notebook(&mut self, library: Arc<Library>) {
        let location = library.location.clone();
        if let Some(session) =
            (self.session).take_if(|session| Arc::ptr_eq(&session.library, &library))
        {
            // Its replica goes with it, so nothing may hold it.
            let _ = self.view.editor.take_ops();
            if let Err(error) = session.section.close() {
                eprintln!("{location}: {error}");
            }
            self.sectionless = Some(Arc::clone(&library));
        }
        self.trashing.insert(location.clone());
        self.close_notebook(&library);
        self.trail.forget(&location);
        self.reads.forget(&location);
        let ours = crate::library::key(&location, "");
        self.folded.retain(|key| !key.starts_with(&ours));
        self.last_pages.retain(|key, _| !key.starts_with(&ours));
        self.save_settings();
        let proxy = self.proxy.clone();
        crate::spawn(move || {
            let trashed = library.trash();
            let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                state.trashing.remove(&location);
                if let Err(problem) = trashed {
                    platform::alert("Couldn’t delete the notebook", &problem);
                    state.open_notebook(location, None);
                }
                Ok(())
            })));
        });
    }

    /// Renames `library`'s folder to `name` on a thread of its own, the notebook closed
    /// meanwhile, then opens it from there in its place, at the section it showed, with `color`
    /// given it. What this computer keeps by the notebook's location follows it. Refused, the
    /// notebook opens again as it was.
    pub(crate) fn rename_notebook(
        &mut self,
        library: Arc<Library>,
        name: String,
        color: Option<u32>,
    ) {
        let Some(to) = library.renamed_location(&name) else {
            return;
        };
        let ours = |shown: &Arc<Library>| Arc::ptr_eq(shown, &library);
        let shown = self
            .session
            .as_ref()
            .filter(|session| ours(&session.library))
            .map(|session| session.tabs[session.tab].path.clone());
        if shown.is_some() {
            let closed = self.persist().and_then(|()| match self.session.take() {
                Some(session) => Ok(session.section.close()?),
                None => Ok(()),
            });
            if let Err(error) = closed {
                return platform::alert(
                    "Couldn't rename the folder",
                    &crate::plain(&*error, "section"),
                );
            }
        }
        let showing = shown.is_some() || self.sectionless.as_ref().is_some_and(ours);
        if showing {
            self.sectionless = Some(Arc::clone(&library));
            self.title();
        }
        self.folder_rename = Some((library.location.clone(), to.clone()));
        let (cache, proxy) = (self.cache.clone(), self.proxy.clone());
        crate::spawn(move || {
            let renamed = library.rename_folder(&name);
            // A failure after the folder moved, as its replicas followed, leaves it there.
            let moved = library
                .folder()
                .is_some_and(|folder| notebook::fs::metadata(folder).is_err());
            let location = match &renamed {
                Ok(to) => to,
                Err(_) if moved => &to,
                Err(_) => &library.location,
            };
            let mut reopened = Library::notebook(location, &cache);
            let mut problem = renamed.err();
            if let (None, Some(color)) = (&problem, color) {
                let colored = reopened.reopen().and_then(|mut notebook| {
                    notebook.set_color(color)?;
                    Ok(reopened.with(notebook))
                });
                match colored {
                    Ok(colored) => reopened = colored,
                    Err(error) => problem = Some(crate::plain(&*error, "notebook")),
                }
            }
            let reopened = Arc::new(reopened);
            let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                state.renamed_notebook(&library, reopened, showing.then_some(shown));
                if let Some(problem) = problem {
                    platform::alert("Couldn't rename the folder", &problem);
                }
                Ok(())
            })));
        });
    }

    /// Lists `reopened` in the place of `old`, the notebook `rename_notebook` closed, moving
    /// what this computer keeps by its location, and shows it again where `shown`: at the
    /// section it showed, or its first.
    fn renamed_notebook(
        &mut self,
        old: &Arc<Library>,
        reopened: Arc<Library>,
        shown: Option<Option<String>>,
    ) {
        self.folder_rename = None;
        let (from, to) = (old.location.clone(), reopened.location.clone());
        match (self.notebooks.iter_mut()).find(|open| Arc::ptr_eq(open, old)) {
            Some(open) => *open = Arc::clone(&reopened),
            None => self.notebooks.push(Arc::clone(&reopened)),
        }
        // iCloud Drive's listing may have found the renamed folder first.
        let mut listed = false;
        self.notebooks
            .retain(|open| open.location != to || !std::mem::replace(&mut listed, true));
        if from != to {
            self.undo.close(&from);
            self.trail.moved(&from, &to);
            self.reads.moved(&from, &to);
            let (from, to) = (crate::library::key(&from, ""), crate::library::key(&to, ""));
            let rekey = |key: &String| match key.strip_prefix(&from) {
                Some(rest) => format!("{to}{rest}"),
                None => key.clone(),
            };
            self.folded = self.folded.iter().map(rekey).collect();
            self.last_pages = (self.last_pages.iter())
                .map(|(key, space)| (rekey(key), *space))
                .collect();
        }
        if let Some(shown) = shown {
            self.sectionless = Some(Arc::clone(&reopened));
            self.title();
            if let Some(path) = shown
                .filter(|path| reopened.contains(path))
                .or_else(|| reopened.first_section())
            {
                self.commands.push(Command::OpenSection(reopened, path));
            }
        }
        self.save_settings();
    }

    /// Changes `library`'s sections and groups on a thread of its own, then shows the
    /// section the change leaves open: a new or moved one, or the one shown before.
    pub(crate) fn restructure(&mut self, library: Arc<Library>, change: Structure) {
        let page = match change {
            Structure::NewSection { .. } => match self.dated_page(None, "") {
                Ok(page) => Some(page),
                Err(error) => {
                    return platform::alert(
                        "Couldn't add the section",
                        &crate::plain(&*error, "section"),
                    );
                }
            },
            _ => None,
        };
        let current = self
            .session
            .as_ref()
            .filter(|session| Arc::ptr_eq(&session.library, &library))
            .map(|session| session.tabs[session.tab].path.clone());
        let notify = notify(self.proxy.clone());
        self.load(move || {
            // A section kept open would hold a file the change moves.
            library.close_kept();
            let mut notebook = library.reopen()?;
            let names = |notebook: &Notebook, folder: &str| -> Vec<String> {
                let found = crate::library::folders(notebook.catalog(), |_| true)
                    .into_iter()
                    .find(|candidate| candidate.path == folder);
                found.map_or_else(Vec::new, |folder| {
                    (folder.sections.iter().map(|section| &section.path))
                        .chain(folder.groups.iter().map(|group| &group.path))
                        .map(|path| crate::library::entry_name(path).to_owned())
                        .collect()
                })
            };
            // A notebook not shown stays so through a change of its colours or name.
            let stays = matches!(
                change,
                Structure::Color { .. } | Structure::Properties { .. }
            );
            // The open section where the change leaves it, and the section to show.
            let (followed, created) = match change {
                Structure::NewSection { folder } => {
                    let name = unused(&names(&notebook, &folder), "New Section", true);
                    let page = page.ok_or("The new section has no page")?;
                    (
                        current,
                        Some(notebook.create_section(&folder, &name, &page)?),
                    )
                }
                Structure::NewGroup { folder } => {
                    let name = unused(&names(&notebook, &folder), "New Section Group", false);
                    notebook.create_group(&folder, &name)?;
                    (current, None)
                }
                Structure::Rename { path, name } => {
                    let renamed = notebook.rename(&path, &name)?;
                    (
                        current.map(|current| follow(&current, &path, &renamed)),
                        None,
                    )
                }
                Structure::Delete { path } => {
                    notebook.delete(&path)?;
                    (current, None)
                }
                // A section of the bin open goes with it.
                Structure::EmptyRecycleBin => {
                    notebook.empty_recycle_bin()?;
                    (current.filter(|path| !crate::recycle::binned(path)), None)
                }
                Structure::Move { path, folder } => {
                    let moved = notebook.move_entry(&path, &folder)?;
                    (current.map(|current| follow(&current, &path, &moved)), None)
                }
                Structure::Reorder { folder, paths } => {
                    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
                    notebook.reorder(&folder, &paths)?;
                    (current, None)
                }
                Structure::Return {
                    path,
                    folder,
                    paths,
                } => {
                    let moved = notebook.move_entry(&path, &folder)?;
                    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
                    notebook.reorder(&folder, &paths)?;
                    (current.map(|current| follow(&current, &path, &moved)), None)
                }
                Structure::Color { path, color } => {
                    notebook.set_section_color(&path, color)?;
                    (current, None)
                }
                Structure::Password {
                    path,
                    key,
                    password,
                } => {
                    let key = notebook.set_password(
                        &path,
                        key.as_ref(),
                        password.as_deref().map(String::as_str),
                    )?;
                    let library = Arc::new(library.with(notebook));
                    if let Some(key) = key {
                        library.keep_key(&path, key);
                    }
                    let section = library.open(&path, notify)?;
                    let (session, page) = read_session(section, library, path, None)?;
                    return Ok(Loaded::Section(Box::new(session), page));
                }
                Structure::Properties { name, color } => {
                    if name != library.name {
                        library.set_display_name(&name)?;
                    }
                    if let Some(color) = color {
                        notebook.set_color(color)?;
                    }
                    (current, None)
                }
            };
            let fresh = created.is_some();
            let open = created.or(followed.clone());
            let library = Arc::new(library.with(notebook));
            if stays && followed.is_none() {
                return Ok(Loaded::Library(library, None));
            }
            let Some(path) = shown(&library, open) else {
                return Ok(Loaded::Library(library, None));
            };
            // The open section, perhaps renamed or moved, stays open: its replica is this
            // thread's to reopen only where no section holds it.
            if followed.as_deref() == Some(path.as_str()) {
                return Ok(Loaded::Library(library, Some(path)));
            }
            let section = library.open(&path, notify)?;
            let (session, page) = read_session(section, library, path, None)?;
            Ok(if fresh {
                Loaded::Created(Box::new(session), page)
            } else {
                Loaded::Section(Box::new(session), page)
            })
        });
    }

    /// Deletes pages of the open section as OneNote 2010 does: each goes to the notebook's
    /// recycle bin, then leaves the section. A section left without pages gains a new one.
    pub(crate) fn delete_pages(&mut self, spaces: Vec<ExGuid>) -> Result<(), Box<dyn Error>> {
        self.change_pages(Change::Delete {
            pages: spaces,
            show: None,
            created: false,
        })
    }

    /// Moves page `space` of the open section to the end of the section at catalog `path`,
    /// as OneNote moves a page dropped on a section's tab: the page keeps its identity,
    /// title, date and content there and leaves this section.
    pub(crate) fn move_page(&mut self, space: ExGuid, path: String) -> Result<(), Box<dyn Error>> {
        let session = self.session.as_ref().ok_or("No section is open")?;
        let target = session
            .library
            .section_identity(&path)
            .ok_or("That section is not in the notebook")?;
        self.change_pages(Change::Send {
            page: space,
            target,
        })
    }

    /// Moves or indents pages of the open section, in order.
    pub(crate) fn edit_pages(&mut self, edits: Vec<PageEdit>) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_mut().ok_or("No section is open")?;
        let order = session
            .pages
            .iter()
            .map(|(space, _, level)| (*space, *level))
            .collect();
        let moved = edits.iter().map(PageEdit::space).collect();
        session.section.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops: vec![Op::Section(SectionOp::Pages(edits))],
            },
        )?;
        session.pages = session.section.pages()?;
        self.made(Change::Arrange { order, moved });
        Ok(())
    }
}

/// The ops giving `page` template `name`'s art, `art`, in place of the background it had,
/// and no page colour.
pub fn template_ops(
    page: &Page,
    name: &str,
    art: Vec<Image>,
) -> Result<Vec<PageOp>, Box<dyn Error>> {
    let mut ops = art_ops(page, art);
    if page.color.is_some() {
        ops.push(PageOp::Color(None));
    }
    // The one template with content worth keeping.
    if name == "Informal Meeting Notes" {
        ops.extend(onestore::op::lower_page(
            page,
            &crate::meeting::content(page)?,
        )?);
    }
    Ok(ops)
}

/// The ops giving `page` `art`, a template's pictures, in place of the background pictures
/// it had.
fn art_ops(page: &Page, art: Vec<Image>) -> Vec<PageOp> {
    let mut ops: Vec<PageOp> = page
        .objects
        .iter()
        .filter(|object| matches!(object, PageObject::Image(image) if image.background))
        .map(|object| PageOp::Delete {
            object: object.id(),
        })
        .collect();
    // Art lies under everything else on the page, first of its children (the title is no
    // child).
    let under = page
        .objects
        .iter()
        .find(|object| match object {
            PageObject::Title(_) => false,
            PageObject::Image(image) => !image.background,
            _ => true,
        })
        .map(PageObject::id);
    ops.extend(art.into_iter().map(|image| PageOp::Add {
        object: PageObject::Image(image),
        before: under,
    }));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const AUTHOR: &str = "Rust Author";

    fn dated() -> PageCreation {
        let creation = PageCreation::new(None, Some(""), AUTHOR).unwrap();
        let [date, time] = platform::date_text(creation.created());
        creation.dated(&date, &time).unwrap()
    }

    /// Applies `ops` to the section file at `file` as one edit, as the section thread does,
    /// and publishes it.
    fn edit(file: &Path, ops: Vec<Op>) {
        let arena = onestore::Arena::default();
        let mut section =
            onestore::Section::open(&arena, onestore::read_file(file).unwrap()).unwrap();
        section
            .apply(
                AUTHOR,
                &Edit {
                    at: crate::filetime(),
                    ops,
                },
            )
            .unwrap();
        if let Some(transaction) = section.seal().unwrap() {
            transaction.commit_file(file).unwrap();
        }
    }

    fn pages(file: &Path) -> Vec<(ExGuid, String, u32)> {
        let arena = onestore::Arena::default();
        onestore::Section::open(&arena, onestore::read_file(file).unwrap())
            .unwrap()
            .pages()
            .unwrap()
    }

    fn page(file: &Path, space: ExGuid) -> Page {
        let arena = onestore::Arena::default();
        onestore::Section::open(&arena, onestore::read_file(file).unwrap())
            .unwrap()
            .page(space)
            .unwrap()
    }

    fn copy(from: &Path, to: &Path) {
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

    /// A notebook made and changed the way the app makes and changes one: created with a
    /// dated first page; sections and groups added, renamed, moved, reordered and deleted to
    /// the recycle bin; pages titled, given a template's art or a colour, made subpages and
    /// deleted to the recycle bin. `SNOWBOUND_MANAGEMENT_EXPORT` names a new directory that
    /// receives it for a cold reopen in OneNote 2010.
    #[test]
    fn a_notebook_is_managed_as_onenote_manages_one() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-manage-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&temporary);
        notebook::fs::create_dir_all(&temporary).unwrap();
        let root = temporary.join("Managed");
        let cache = temporary.join("cache");
        let mut notebook = Notebook::create(&root, &cache, Notebook::NEW_COLOR, &dated()).unwrap();
        for (folder, name) in [("", "New Section 2"), ("", "Binned")] {
            notebook.create_section(folder, name, &dated()).unwrap();
        }
        for (group, section) in [("Kept", "Inner"), ("Doomed", "Gone")] {
            notebook.create_group("", group).unwrap();
            notebook.create_section(group, section, &dated()).unwrap();
        }

        // Pages of the first section, each titled.
        let file = root.join("New Section 1.one");
        let first = pages(&file)[0].0;
        let title = |space: ExGuid, text: &str| {
            let page = page(&file, space);
            let title = page
                .objects
                .iter()
                .find_map(|object| match object {
                    PageObject::Title(title) => title.outlines[0].paragraphs[0].text(),
                    _ => None,
                })
                .unwrap()
                .id;
            Op::Page {
                space,
                op: PageOp::Text {
                    text: title,
                    range: 0..0,
                    with: text.to_owned(),
                },
            }
        };
        edit(&file, vec![title(first, "Garden plan")]);
        let mut spaces = Vec::new();
        for text in [
            "Ivy page",
            "Teal page",
            "Subpage",
            "Deleted page",
            "Meeting page",
        ] {
            let creation = dated();
            let space = creation.space();
            edit(&file, vec![Op::Section(SectionOp::Create(creation))]);
            edit(&file, vec![title(space, text)]);
            spaces.push(space);
        }
        let [ivy, teal, subpage, deleted, meeting] = spaces[..] else {
            unreachable!()
        };
        let background = |space: ExGuid, name: &str| {
            let art = canvas::template::find(name).unwrap().pictures().unwrap();
            template_ops(&page(&file, space), name, art)
                .unwrap()
                .into_iter()
                .map(|op| Op::Page { space, op })
                .collect::<Vec<_>>()
        };
        edit(&file, background(ivy, "Ivy"));
        edit(&file, background(meeting, "Informal Meeting Notes"));
        let (_, teal_color) = canvas::template::PAGE_COLORS
            .iter()
            .find(|(name, _)| *name == "Teal")
            .unwrap();
        edit(
            &file,
            vec![Op::Page {
                space: teal,
                op: PageOp::Color(Some(*teal_color)),
            }],
        );
        edit(
            &file,
            vec![Op::Section(SectionOp::Pages(vec![
                PageEdit::set_level(subpage, 2).unwrap(),
            ]))],
        );
        let recycled = page(&file, deleted);
        notebook
            .recycle_pages(std::slice::from_ref(&recycled), AUTHOR)
            .unwrap();
        edit(&file, vec![Op::Section(SectionOp::Delete(vec![deleted]))]);

        // A page moved to another section, as a drop on its tab moves it.
        let moved_space = {
            let creation = dated();
            let space = creation.space();
            edit(&file, vec![Op::Section(SectionOp::Create(creation))]);
            edit(&file, vec![title(space, "Moved page")]);
            space
        };
        let moving = page(&file, moved_space);
        let other = root.join("New Section 2.one");
        edit(
            &other,
            vec![notebook::session::moved(&moving, AUTHOR).unwrap()],
        );
        edit(
            &file,
            vec![Op::Section(SectionOp::Delete(vec![moved_space]))],
        );
        let arrived = pages(&other).last().unwrap().0;
        let arrived = page(&other, arrived);
        assert_eq!(
            (
                arrived.title.as_str(),
                arrived.identity,
                arrived.date_text()
            ),
            ("Moved page", moving.identity, moving.date_text())
        );

        // Structure.
        assert_eq!(
            notebook.rename("New Section 2.one", "Kitchen").unwrap(),
            "Kitchen.one"
        );
        assert_eq!(
            notebook.move_entry("Kitchen.one", "Kept").unwrap(),
            "Kept/Kitchen.one"
        );
        assert_eq!(notebook.rename("Kept", "Archive").unwrap(), "Archive");
        notebook.delete("Doomed").unwrap();
        notebook.delete("Binned.one").unwrap();
        notebook
            .reorder("", &["Archive", "New Section 1.one"])
            .unwrap();

        let catalog = notebook.catalog();
        let names = |folder: &notebook::discover::Folder| {
            folder
                .sections
                .iter()
                .map(|section| section.path.clone())
                .chain(folder.groups.iter().map(|group| group.path.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(catalog),
            ["New Section 1.one", "Archive", "OneNote_RecycleBin"]
        );
        assert_eq!(
            names(&catalog.groups[0]),
            ["Archive/Inner.one", "Archive/Kitchen.one"]
        );
        let mut binned = names(&catalog.groups[1]);
        binned.sort();
        assert_eq!(
            binned,
            [
                "OneNote_RecycleBin/Binned.one",
                "OneNote_RecycleBin/Gone.one",
                "OneNote_RecycleBin/OneNote_DeletedPages.one",
            ]
        );
        assert!(!root.join("Doomed").exists());

        let listed = pages(&file);
        assert_eq!(
            listed
                .iter()
                .map(|(_, title, level)| (title.as_str(), *level))
                .collect::<Vec<_>>(),
            [
                ("Garden plan", 1),
                ("Ivy page", 1),
                ("Teal page", 1),
                ("Subpage", 2),
                ("Meeting page", 1)
            ]
        );
        let shown = page(&file, teal);
        assert_eq!(shown.color, Some(*teal_color));
        assert!(shown.date_text().is_some());
        assert!(
            page(&file, ivy)
                .objects
                .iter()
                .any(|object| matches!(object, PageObject::Image(image) if image.background))
        );
        let written = page(&file, meeting);
        let outlines: Vec<_> = written
            .objects
            .iter()
            .filter_map(|object| match object {
                PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .collect();
        assert_eq!(outlines.len(), 4);
        let lines: Vec<&str> = outlines
            .iter()
            .flat_map(|outline| &outline.paragraphs)
            .filter_map(|paragraph| paragraph.text())
            .map(|text| text.text.text())
            .collect();
        assert!(lines.contains(&"Meeting Details") && lines.contains(&"Attendees:"));
        assert!(
            outlines
                .iter()
                .flat_map(|outline| &outline.paragraphs)
                .filter_map(|paragraph| paragraph.text())
                .any(|text| !text.tags.is_empty())
        );
        let bin = root.join("OneNote_RecycleBin/OneNote_DeletedPages.one");
        let [(space, title, 1)] = &pages(&bin)[..] else {
            panic!("{:?}", pages(&bin))
        };
        assert_eq!(title, "Deleted page");
        let kept = page(&bin, *space);
        assert_eq!(
            (kept.identity, kept.created),
            (recycled.identity, recycled.created)
        );
        assert_eq!(kept.date_text(), recycled.date_text());

        if let Some(directory) = std::env::var_os("SNOWBOUND_MANAGEMENT_EXPORT") {
            copy(&root, Path::new(&directory));
        }
        notebook::fs::remove_dir_all(&temporary).unwrap();
    }

    /// Deleting a group's last section shows the notebook's other one; deleting that too
    /// leaves the notebook showing no section, where it failed before.
    #[test]
    fn deleting_every_section_leaves_the_notebook_showing_none() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-empty-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&temporary);
        notebook::fs::create_dir_all(&temporary).unwrap();
        let root = temporary.join("Emptied");
        let location = root.to_str().unwrap();
        let cache = temporary.join("cache");
        let mut notebook =
            Notebook::create(location, &cache, Notebook::NEW_COLOR, &dated()).unwrap();
        notebook.create_group("", "Group").unwrap();
        notebook.create_section("Group", "Inner", &dated()).unwrap();

        notebook.delete("Group/Inner.one").unwrap();
        let library = Library::created(location, notebook, &cache);
        assert_eq!(
            shown(&library, Some("Group/Inner.one".into())).as_deref(),
            Some("New Section 1.one")
        );

        let mut notebook = library.reopen().unwrap();
        notebook.delete("New Section 1.one").unwrap();
        let library = library.with(notebook);
        assert_eq!(shown(&library, Some("New Section 1.one".into())), None);
        assert!(library.tabs("").is_empty());
        notebook::fs::remove_dir_all(&temporary).unwrap();
    }

    #[test]
    fn moved_sections_are_followed_and_new_names_take_the_next_number() {
        assert_eq!(follow("Group/Inner.one", "Group", "Kept"), "Kept/Inner.one");
        assert_eq!(follow("Group.one", "Group", "Kept"), "Group.one");
        assert_eq!(follow("A.one", "A.one", "B/A.one"), "B/A.one");
        let taken = ["New Section 1".to_owned(), "new section 2".to_owned()];
        assert_eq!(unused(&taken, "New Section", true), "New Section 3");
        assert_eq!(unused(&[], "New Section Group", false), "New Section Group");
        assert_eq!(
            unused(
                &["New Section Group".to_owned()],
                "New Section Group",
                false
            ),
            "New Section Group 2"
        );
    }

    /// Backgrounds the Page Color menu gives OneNote's ruled pages
    /// (`corpus/rule-lines/native`) through the editor, each step one edit: art on a page
    /// with rule lines, keeping them; art taken away, undone, redone and replaced; a colour
    /// outside OneNote's menu; and art on a coloured page. `SNOWBOUND_BACKGROUND_EXPORT`
    /// names a new directory that receives the notebook for a cold reopen in OneNote 2010.
    #[test]
    fn backgrounds_go_on_and_off_pages_that_have_content() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-background-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&temporary);
        copy(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../corpus/rule-lines/native/notebook"
            )),
            &temporary,
        );
        let file = temporary.join("Rules.one");
        let titled = |title: &str| {
            pages(&file)
                .into_iter()
                .find(|(_, name, _)| name == title)
                .unwrap()
                .0
        };
        enum Step {
            Art(Option<&'static str>),
            Color(u32),
            Undo,
            Redo,
        }
        use Step::*;
        let mut engine = canvas::layout::TextEngine::default();
        let mut run = |title: &str, steps: &[Step]| {
            let space = titled(title);
            let mut editor =
                canvas::editor::CanvasEditor::from_page(page(&file, space), &mut engine).unwrap();
            for step in steps {
                let (color, lines) = (editor.page_color(), editor.rule_lines());
                let changed = match step {
                    Art(name) => {
                        let art = name.map_or(Ok(Vec::new()), |name| {
                            canvas::template::find(name).unwrap().pictures()
                        });
                        editor.set_paper(color, lines, Some(art.unwrap()))
                    }
                    Color(color) => editor.set_paper(Some(*color), lines, None),
                    Undo => editor.undo(&mut engine).unwrap(),
                    Redo => editor.redo(&mut engine).unwrap(),
                };
                assert!(changed);
                let ops = editor.take_ops().unwrap();
                assert!(!ops.is_empty());
                edit(
                    &file,
                    ops.into_iter().map(|op| Op::Page { space, op }).collect(),
                );
            }
            page(&file, space)
        };
        let lemon = canvas::template::PAGE_COLORS[2];
        assert_eq!(lemon.0, "Lemon");
        let pages = [
            run("Standard", &[Art(Some("Ivy"))]),
            run("Wide", &[Art(Some("Ivy")), Art(None)]),
            // A warm grey no menu offers.
            run("SmallGrid", &[Color(0x00c8d8e8)]),
            run("College", &[Art(Some("Ivy")), Undo]),
            run("MediumGrid", &[Art(Some("Ivy")), Undo, Redo]),
            run("LargeGrid", &[Art(Some("Ivy")), Art(Some("Tulips")), Undo]),
            run("VeryLargeGrid", &[Color(lemon.1), Art(Some("Sparks"))]),
        ];
        let art = |page: &Page| -> Vec<f32> {
            page.objects
                .iter()
                .filter_map(|object| match object {
                    PageObject::Image(image) if image.background => image.layout.max_width,
                    _ => None,
                })
                .collect()
        };
        let [standard, wide, grid, college, medium, large, very_large] = &pages;
        let [ivy, sparks] = ["Ivy", "Sparks"]
            .map(|name| canvas::template::find(name).unwrap().art[0].size.unwrap()[0]);
        assert_eq!(art(standard), [ivy]);
        assert!(pages.iter().all(|page| page.rule_lines.is_some()));
        assert!(art(wide).is_empty() && art(college).is_empty());
        assert_eq!(grid.color, Some(0x00c8d8e8));
        assert_eq!(art(medium), [ivy]);
        assert_eq!(art(large), [ivy]);
        assert_eq!(art(very_large), [sparks]);
        assert_eq!(very_large.color, Some(lemon.1));
        if let Some(directory) = std::env::var_os("SNOWBOUND_BACKGROUND_EXPORT") {
            copy(&temporary, Path::new(&directory));
        }
        notebook::fs::remove_dir_all(&temporary).unwrap();
    }

    /// A notebook folder that leaves the iCloud folder for a moment, as iCloud Drive's
    /// writes can take it, is not gone; one that stays away is.
    #[test]
    fn a_folder_missing_for_a_moment_is_not_gone() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-vanish-{}", std::process::id()));
        let (folder, away) = (temporary.join("Cloudy"), temporary.join(".Cloudy-away"));
        notebook::fs::create_dir_all(&folder).unwrap();
        notebook::fs::rename(&folder, &away).unwrap();
        let back = std::thread::spawn({
            let (folder, away) = (folder.clone(), away.clone());
            move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                notebook::fs::rename(away, folder).unwrap();
            }
        });
        assert!(!super::vanished(
            &folder,
            std::time::Duration::from_millis(600)
        ));
        back.join().unwrap();
        notebook::fs::remove_dir_all(&folder).unwrap();
        assert!(super::vanished(&folder, std::time::Duration::ZERO));
        notebook::fs::remove_dir_all(&temporary).unwrap();
    }
}
