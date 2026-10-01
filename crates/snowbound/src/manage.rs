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

/// New iCloud Notebook while it is open: the name typed, and why the last was refused.
pub struct Naming {
    name: String,
    problem: Option<String>,
}

fn naming() -> ui::Id {
    ui::Id::ROOT.child("new icloud notebook")
}

fn naming_field() -> ui::Id {
    naming().child("name")
}

/// Whether the notebook at `location` is a folder at the top of the app's iCloud Drive folder,
/// which the sidebar lists as long as it is there.
pub fn in_icloud_folder(location: &str) -> bool {
    crate::icloud::folder().is_some_and(|root| Path::new(location).parent() == Some(&root))
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
    /// A page OneNote 2010 would create now: titled in the Default font's face, with its date
    /// and time.
    pub(crate) fn dated_page(
        &self,
        before: Option<ExGuid>,
    ) -> Result<PageCreation, Box<dyn Error>> {
        let creation = PageCreation::new(before, Some(""), &self.author)?.titled_in(
            &self.view.editor.default_font.face,
            self.view.editor.default_font.color,
        )?;
        let [date, time] = platform::date_text(creation.created());
        Ok(creation.dated(&date, &time)?)
    }

    /// Adds a page at the end of the open section, or a subpage of page `under`, and opens
    /// it with its title ready for typing, as OneNote's New Page and New Subpage do.
    pub(crate) fn new_page(&mut self, under: Option<ExGuid>) -> Result<(), Box<dyn Error>> {
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
        let creation = self.dated_page(None)?;
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

    /// Gives the open page `choice`'s background: a template's art in place of any it
    /// had, or a page colour instead of art.
    pub(crate) fn apply_template(
        &mut self,
        choice: crate::templates::Choice,
    ) -> Result<(), Box<dyn Error>> {
        use crate::templates::Choice;
        let template = match choice {
            Choice::Template(name) => {
                Some(canvas::template::find(name).ok_or("That template is not available")?)
            }
            _ => None,
        };
        self.with_art(template, move |state, art| {
            state.persist()?;
            let session = state.session.as_ref().ok_or("No section is open")?;
            let ops = template_ops(&session.section.page(session.space)?, choice, art)?;
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
        std::thread::spawn(move || {
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
    /// panel starts in iCloud Drive, as Notes keeps notes there.
    pub(crate) fn new_notebook(&mut self) -> Result<(), Box<dyn Error>> {
        let icloud = crate::icloud::folder().or_else(crate::icloud::drive);
        let Some(root) =
            platform::pick_new("New Notebook", "My Notebook", "Create", icloud.as_deref())
        else {
            return Ok(());
        };
        self.create_notebook(root)
    }

    /// New iCloud Notebook: asks for a name, the first free of "iCloud Notebook", "iCloud
    /// Notebook 2"…, for a notebook at the top of the app's iCloud Drive folder.
    pub(crate) fn new_icloud_notebook(&mut self) {
        let taken: Vec<String> = self
            .notebooks
            .iter()
            .map(|library| library.name.clone())
            .collect();
        self.new_icloud = Some(Naming {
            name: unused(&taken, "iCloud Notebook", false),
            problem: None,
        });
        self.ui.open_popup(naming());
        self.ui.focus_all(naming_field());
    }

    /// Builds New iCloud Notebook while it is open.
    pub(crate) fn new_icloud_dialog(&mut self) {
        use ui::{Anchor, Axis, Spec, children, fill, px};
        let Some(dialog) = &mut self.new_icloud else {
            return;
        };
        if !self.ui.popup_open(naming()) {
            self.new_icloud = None;
            return;
        }
        let ui = &mut self.ui;
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        let entered =
            ui::popup::navigation(ui, &[naming_field()], &[winit::keyboard::NamedKey::Enter])
                .contains(&winit::keyboard::NamedKey::Enter);
        ui.open_as(
            naming(),
            Spec {
                axis: Axis::Y,
                size: [px(320.0), children()],
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
        if let Some(node) = ui.access(naming()) {
            node.set_label("New iCloud Notebook");
        }
        ui.leaf(
            "title",
            Spec {
                size: [fill(), px(row)],
                text: Some("New iCloud Notebook"),
                bold: true,
                role: Some(accesskit::Role::Heading),
                ..Spec::default()
            },
        );
        ui::text_field(
            ui,
            naming_field(),
            &mut dialog.name,
            "",
            Spec {
                size: [fill(), px(row)],
                fill: Some(theme.base),
                border: Some(theme.accent),
                radius: 4.0,
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
        crate::name(ui, naming_field(), "Name");
        if let Some(problem) = &dialog.problem {
            ui.leaf(
                "problem",
                Spec {
                    size: [fill(), ui::fit()],
                    text: Some(problem),
                    overflow: ui::Overflow::Wrap,
                    pad: [0.0, 4.0],
                    role: Some(accesskit::Role::Status),
                    ..Spec::default()
                },
            );
        }
        crate::buttons(ui);
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let create = ui::button(ui, "create", "Create").clicked || entered;
        ui.close();
        ui.close();
        if cancel {
            self.ui.close_popup(naming());
            self.ui.set_focus(Some(crate::page()));
            self.new_icloud = None;
        } else if create {
            let name = dialog.name.trim().to_owned();
            let folder = crate::icloud::folder();
            dialog.problem = match &folder {
                None => Some("Turn on iCloud Drive in System Settings.".into()),
                Some(_) if name.is_empty() => Some("Enter a name.".into()),
                Some(_) if name.starts_with('.') || name.contains(['/', ':']) => {
                    Some("Use a name without “/”, “:” or a leading “.”.".into())
                }
                Some(folder) if folder.join(&name).exists() => Some(format!(
                    "“{name}” is already in iCloud Drive. Choose another name."
                )),
                Some(_) => None,
            };
            if let (None, Some(folder)) = (&dialog.problem, folder) {
                self.ui.close_popup(naming());
                self.new_icloud = None;
                if let Err(error) = self.create_notebook(folder.join(name)) {
                    platform::alert("Couldn't create the notebook", &error.to_string());
                }
            }
        }
    }

    /// Lists the folders at the top of the app's iCloud Drive folder in the sidebar, each a
    /// notebook, as they come and go, and brings down what iCloud Drive keeps elsewhere of
    /// every notebook in it.
    pub(crate) fn list_icloud(&mut self) {
        let proxy = self.proxy.clone();
        // Listing a folder iCloud Drive has not brought down yet waits for it.
        std::thread::spawn(move || {
            let listed = crate::icloud::folder().and_then(|root| {
                let mut listed: Vec<String> = std::fs::read_dir(&root)
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
        let gone: Vec<Arc<Library>> = self
            .notebooks
            .iter()
            .filter(|library| {
                in_icloud_folder(&library.location) && !listed.contains(&library.location)
            })
            .cloned()
            .collect();
        for library in gone {
            self.close_notebook(&library);
        }
        for location in listed {
            if self.notebooks.iter().any(|open| open.location == location)
                || !self.icloud_reading.insert(location.clone())
            {
                continue;
            }
            let (cache, proxy) = (self.cache.clone(), self.proxy.clone());
            std::thread::spawn(move || {
                let library = Arc::new(Library::notebook(&location, &cache));
                let _ = proxy.send_event(crate::UserEvent::Then(Box::new(move |state| {
                    state.icloud_reading.remove(&location);
                    if state.notebooks.iter().any(|open| open.location == location) {
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
        std::thread::spawn(move || {
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
    fn create_notebook(&mut self, root: std::path::PathBuf) -> Result<(), Box<dyn Error>> {
        let page = self.dated_page(None)?;
        let (cache, notify) = (self.cache.clone(), notify(self.proxy.clone()));
        self.load(move || {
            let location = std::path::absolute(&root)?.to_string_lossy().into_owned();
            let notebook = Notebook::create(&location, &cache, Notebook::NEW_COLOR, &page)?;
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
            let exported = std::fs::create_dir_all(&folder)
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
            self.templates = crate::templates::View::Strip;
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

    /// Changes `library`'s sections and groups on a thread of its own, then shows the
    /// section the change leaves open: a new or moved one, or the one shown before.
    pub(crate) fn restructure(&mut self, library: Arc<Library>, change: Structure) {
        let page = match change {
            Structure::NewSection { .. } => match self.dated_page(None) {
                Ok(page) => Some(page),
                Err(error) => {
                    return platform::alert("Couldn't add the section", &error.to_string());
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

/// The ops giving `page` `choice`'s background: its template art or a page colour, in
/// place of the background it had.
pub fn template_ops(
    page: &Page,
    choice: crate::templates::Choice,
    art: Vec<Image>,
) -> Result<Vec<PageOp>, Box<dyn Error>> {
    use crate::templates::Choice;
    let mut ops = match choice {
        Choice::Template(_) | Choice::Color(_) => art_ops(page, art),
        Choice::More | Choice::Dismiss | Choice::Colors => return Ok(Vec::new()),
    };
    match choice {
        Choice::Template(name) => {
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
        }
        Choice::Color(index) => {
            ops.push(PageOp::Color(Some(canvas::template::PAGE_COLORS[index].1)))
        }
        Choice::More | Choice::Dismiss | Choice::Colors => {}
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
    use crate::templates::Choice;
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
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
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
        let _ = std::fs::remove_dir_all(&temporary);
        std::fs::create_dir_all(&temporary).unwrap();
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
        let background = |space: ExGuid, choice: Choice| {
            let art = match choice {
                Choice::Template(name) => canvas::template::find(name).unwrap().pictures().unwrap(),
                _ => Vec::new(),
            };
            template_ops(&page(&file, space), choice, art)
                .unwrap()
                .into_iter()
                .map(|op| Op::Page { space, op })
                .collect::<Vec<_>>()
        };
        edit(&file, background(ivy, Choice::Template("Ivy")));
        edit(
            &file,
            background(meeting, Choice::Template("Informal Meeting Notes")),
        );
        let teal_index = canvas::template::PAGE_COLORS
            .iter()
            .position(|(name, _)| *name == "Teal")
            .unwrap();
        edit(&file, background(teal, Choice::Color(teal_index)));
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
        assert_eq!(
            shown.color,
            Some(canvas::template::PAGE_COLORS[teal_index].1)
        );
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
        std::fs::remove_dir_all(&temporary).unwrap();
    }

    /// Deleting a group's last section shows the notebook's other one; deleting that too
    /// leaves the notebook showing no section, where it failed before.
    #[test]
    fn deleting_every_section_leaves_the_notebook_showing_none() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temporary);
        std::fs::create_dir_all(&temporary).unwrap();
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
        std::fs::remove_dir_all(&temporary).unwrap();
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
        let _ = std::fs::remove_dir_all(&temporary);
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
        std::fs::remove_dir_all(&temporary).unwrap();
    }
}
