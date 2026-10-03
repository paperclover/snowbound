//! OneNote 2010's search: the box beside the section tabs searching the open notebooks, its
//! results dropping down from it, and Find on This Page. The index is kept on a thread of
//! its own, from the section files and the open section's replica.

use crate::pane::Pane;
use crate::{Command, Library, State, Theme, art, commands, lap, name, page};
use accesskit::Role;
use canvas::search::{Entry, Found, Index, PageMatch, Query, page_matches, paragraph_match};
use notebook::Replica;
use onestore::ExGuid;
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime},
};
use ui::{Anchor, Axis, Flags, Id, Spec, Ui, children, fill, fit, px};
use web_time::Instant;
use winit::keyboard::NamedKey;

/// Where a search looks, as OneNote's scope menu offers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Scope {
    Section,
    Group,
    Notebook,
    #[default]
    All,
}

impl Scope {
    pub(crate) const ALL: [Scope; 4] = [Scope::Section, Scope::Group, Scope::Notebook, Scope::All];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Scope::Section => "This Section",
            Scope::Group => "This Section Group",
            Scope::Notebook => "This Notebook",
            Scope::All => "All Notebooks",
        }
    }
}

/// The results list's rows, the headings above their groups, and its width.
const RESULT: f32 = 42.0;
const HEADING: f32 = 24.0;
const RESULTS: f32 = 460.0;
/// Room the section name takes beside a result's title.
const PLACE: f32 = 150.0;
/// How long typing pauses before the index reads the pages it changed.
const SETTLE: Duration = Duration::from_millis(400);

pub(crate) fn results() -> Id {
    Id::ROOT.child("results")
}

fn scope_menu() -> Id {
    Id::ROOT.child("scope")
}

/// The field of the box, and of the results over it.
pub(crate) fn field() -> Id {
    Id::ROOT.child("search-field")
}

fn find_field() -> Id {
    Id::ROOT.child("find-field")
}

/// Whether `focus` is one of the box's fields, which take typed text.
pub fn takes_text(focus: Option<Id>) -> bool {
    [
        Some(field()),
        Some(find_field()),
        Some(crate::pane::field()),
    ]
    .contains(&focus)
}

/// Work for the index thread.
enum Job {
    /// Brings the index to these notebooks: sections not read yet are read, and those no
    /// longer listed dropped. Of those read before, a notebook's background sync names the
    /// changed ones, by key, in `changed`; a section on its own is read again when its file
    /// changed. The open section, by key, is read through its replica.
    Notebooks {
        libraries: Vec<Arc<Library>>,
        open: Option<(String, Weak<Replica>)>,
        changed: Vec<String>,
    },
    /// Reads pages of the open section again, with its page list.
    Pages {
        key: String,
        replica: Weak<Replica>,
        spaces: Vec<ExGuid>,
    },
}

/// The search box's state and the index behind it.
pub struct Search {
    pub query: String,
    pub scope: Scope,
    /// Scope searches start in, as "Set This Scope as Default" leaves it.
    pub default: Scope,
    /// Find on This Page is open, at the match selected.
    pub(crate) finding: bool,
    current: Option<usize>,
    /// The page a result opened, whose first match, or paragraph, is selected once it shows.
    reveal: Option<(ExGuid, Option<ExGuid>)>,
    pub(crate) index: Arc<Mutex<Index>>,
    /// Counts the index's changes, so results are made again when it changes.
    version: Arc<AtomicU64>,
    /// Jobs sent the index and not yet answered.
    pub(crate) pending: Arc<AtomicUsize>,
    jobs: mpsc::Sender<Job>,
    /// The index's own state, which the browser, having no thread for it, keeps here to run
    /// the jobs sent after each frame.
    #[cfg(target_arch = "wasm32")]
    indexing: Arc<Mutex<Indexing>>,
    pub(crate) found: Vec<Found>,
    found_for: Option<(Query, Scope, u64, String)>,
    pub(crate) selected: Option<u64>,
    /// The notebooks and open section the index was last brought to, by identity.
    synced: Vec<usize>,
    /// The results drop down, until chosen from or dismissed.
    pub(crate) open: bool,
    /// Where the scope menu opens.
    anchor: Option<Anchor>,
    /// The find field, or the Search Results pane's, takes the keys once built.
    pub(crate) claim: bool,
    pub(crate) pane: Option<Pane>,
}

impl Search {
    pub fn new(default: Scope, redraw: std::task::Waker) -> Self {
        let (jobs, receiver) = mpsc::channel();
        let index = Arc::new(Mutex::new(Index::default()));
        let version = Arc::new(AtomicU64::new(0));
        let pending = Arc::new(AtomicUsize::new(0));
        let indexing = Indexing {
            jobs: receiver,
            shared: (
                Arc::clone(&index),
                Arc::clone(&version),
                Arc::clone(&pending),
            ),
            redraw,
            stamps: HashMap::new(),
        };
        #[cfg(target_arch = "wasm32")]
        let indexing = Arc::new(Mutex::new(indexing));
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::Builder::new()
            .name("search-index".into())
            .spawn(move || indexing.run())
            .expect("the index thread starts");
        Self {
            query: String::new(),
            scope: default,
            default,
            finding: false,
            current: None,
            reveal: None,
            index,
            version,
            pending,
            jobs,
            #[cfg(target_arch = "wasm32")]
            indexing,
            found: Vec::new(),
            found_for: None,
            selected: None,
            synced: Vec::new(),
            open: false,
            anchor: None,
            claim: false,
            pane: None,
        }
    }

    /// Asks the index to read `spaces` of the open section again, as edits or another
    /// writer changed them, and its page list.
    pub fn changed(&self, key: String, replica: &Arc<Replica>, spaces: Vec<ExGuid>) {
        self.send(Job::Pages {
            key,
            replica: Arc::downgrade(replica),
            spaces,
        });
    }

    fn send(&self, job: Job) {
        self.pending.fetch_add(1, Ordering::Relaxed);
        let _ = self.jobs.send(job);
        #[cfg(target_arch = "wasm32")]
        {
            let indexing = Arc::clone(&self.indexing);
            crate::spawn(move || {
                if let Ok(mut indexing) = indexing.lock() {
                    while let Ok(first) = indexing.jobs.try_recv() {
                        indexing.answer(first);
                    }
                }
            });
        }
    }

    /// Pages of section `key` among the results, which the page list marks.
    pub fn found_in(&self, key: &str) -> HashSet<ExGuid> {
        if self.finding || self.query.trim().is_empty() {
            return HashSet::new();
        }
        self.found
            .iter()
            .filter(|found| found.section == key)
            .map(|found| found.space)
            .collect()
    }

    /// The query whose matches the page shows.
    fn shown(&self) -> Query {
        Query::new(&self.query)
    }
}

/// When a file last changed, as far as its length and modification time tell.
type Stamp = Option<(u64, SystemTime)>;

fn stamp(file: &Path) -> Stamp {
    let metadata = notebook::fs::metadata(file).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

/// Now in Time32, seconds since 1980, as page modification times are kept.
pub(crate) fn now() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs().saturating_sub(315_532_800))
}

/// The index thread's jobs and what it keeps between them.
struct Indexing {
    jobs: mpsc::Receiver<Job>,
    shared: (Arc<Mutex<Index>>, Arc<AtomicU64>, Arc<AtomicUsize>),
    redraw: std::task::Waker,
    stamps: HashMap<String, Stamp>,
}

impl Indexing {
    /// The index thread: answers jobs, gathering those typing sends a keystroke apart.
    #[cfg(not(target_arch = "wasm32"))]
    fn run(mut self) {
        while let Ok(first) = self.jobs.recv() {
            // Typing sends a job a keystroke; a pause gathers them into one read.
            if matches!(first, Job::Pages { .. }) {
                std::thread::sleep(SETTLE);
            }
            self.answer(first);
        }
    }

    /// Answers `first` and the jobs waiting behind it, the newest notebooks first, then page
    /// changes, and wakes the window once the index changed.
    fn answer(&mut self, first: Job) {
        let (index, version, pending) = &self.shared;
        let (jobs, stamps) = (&self.jobs, &mut self.stamps);
        let mut notebooks = None;
        let mut changed = HashSet::new();
        let mut pages: HashMap<String, (Weak<Replica>, HashSet<ExGuid>)> = HashMap::new();
        let mut answered = 0;
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            answered += 1;
            match job {
                Job::Notebooks {
                    libraries,
                    open,
                    changed: keys,
                } => {
                    notebooks = Some((libraries, open));
                    changed.extend(keys);
                }
                Job::Pages {
                    key,
                    replica,
                    spaces,
                } => pages
                    .entry(key)
                    .or_insert_with(|| (replica, HashSet::new()))
                    .1
                    .extend(spaces),
            }
        }
        let start = Instant::now();
        if let Some((libraries, open)) = notebooks {
            sync(index, version, stamps, &libraries, open.as_ref(), &changed);
            lap("index notebooks", start);
        }
        for (key, (replica, spaces)) in pages {
            if let Err(error) = reread(index, &key, &replica, &spaces) {
                eprintln!("Cannot index the open section: {error}");
            }
            version.fetch_add(1, Ordering::Relaxed);
        }
        lap("index", start);
        pending.fetch_sub(answered, Ordering::Relaxed);
        self.redraw.wake_by_ref();
    }
}

/// The readable sections of `library` outside its recycle bin, by catalog path.
fn sections(library: &Library) -> Vec<String> {
    match &library.notebook {
        Ok(Some(notebook)) => {
            let enter =
                |group: &notebook::discover::Folder| !crate::library::recycle_bin(&group.path);
            crate::library::folders(notebook.catalog(), enter)
                .into_iter()
                .flat_map(|folder| &folder.sections)
                .filter(|section| match section.state {
                    notebook::discover::SectionState::Readable { .. } => true,
                    // OneNote searches a protected section only while it is unlocked.
                    notebook::discover::SectionState::Locked => {
                        library.unlocked(&section.path).is_some()
                    }
                    _ => false,
                })
                .map(|section| section.path.clone())
                .collect()
        }
        Ok(None) => vec![library.location.clone()],
        Err(_) => Vec::new(),
    }
}

fn sync(
    index: &Mutex<Index>,
    version: &AtomicU64,
    stamps: &mut HashMap<String, Stamp>,
    libraries: &[Arc<Library>],
    open: Option<&(String, Weak<Replica>)>,
    changed: &HashSet<String>,
) {
    let mut listed = HashSet::new();
    for library in libraries {
        for path in sections(library) {
            let key = library.key(&path);
            listed.insert(key.clone());
            let notebook = library.notebook.as_ref().ok().and_then(Option::as_ref);
            let file = match notebook {
                Some(_) => Path::new(&library.location).join(&path),
                None => PathBuf::from(&library.location),
            };
            // A notebook's background sync names its changed sections, so none is checked here.
            let reported = library.background.is_some();
            let stamp = if reported { None } else { stamp(&file) };
            let fresh = match stamps.get(&key) {
                None => false,
                Some(_) if reported => !changed.contains(&key),
                // A file with no stamp, as on a share not mounted, is read once.
                Some(known) => known == &stamp || stamp.is_none(),
            };
            if fresh {
                continue;
            }
            let replica = open
                .filter(|(open, _)| *open == key)
                .and_then(|(_, replica)| replica.upgrade());
            let read = || -> Result<Vec<Entry>, Box<dyn Error>> {
                let bytes = match notebook {
                    Some(notebook) => notebook.read_section(&path)?,
                    None => notebook::fs::read_file(&file)?,
                };
                let stored = match library.unlocked(&path) {
                    Some(key) => notebook::session::stored_pages_unlocked(&bytes, &key)?,
                    None => notebook::session::stored_pages(&bytes)?,
                };
                let modified = |space: ExGuid| {
                    stored
                        .iter()
                        .find(|page| page.space == space)
                        .and_then(|page| page.modified)
                        .map_or_else(now, u64::from)
                };
                Ok(match &replica {
                    // The open section shows its edits before they reach the file.
                    Some(replica) => replica
                        .pages()?
                        .into_iter()
                        .map(|(space, ..)| {
                            Ok(Entry::new(
                                &key,
                                space,
                                &replica.page(space)?,
                                modified(space),
                            ))
                        })
                        .collect::<Result<_, notebook::Error>>()?,
                    None => stored
                        .iter()
                        .map(|page| Entry::new(&key, page.space, &page.page, modified(page.space)))
                        .collect(),
                })
            };
            match read() {
                Ok(entries) => {
                    let mut index = index.lock().unwrap_or_else(|error| error.into_inner());
                    index.retain(|entry| entry.section != key);
                    for entry in entries {
                        index.set(entry);
                    }
                    drop(index);
                    stamps.insert(key, stamp);
                    version.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => eprintln!("Cannot index {path}: {error}"),
            }
        }
    }
    stamps.retain(|key, _| listed.contains(key));
    index
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|entry| listed.contains(&entry.section));
    version.fetch_add(1, Ordering::Relaxed);
}

/// Reads `spaces` of the open section `key` again, and pages its list gained, dropping
/// those it lost.
fn reread(
    index: &Mutex<Index>,
    key: &str,
    replica: &Weak<Replica>,
    spaces: &HashSet<ExGuid>,
) -> Result<(), Box<dyn Error>> {
    let Some(replica) = replica.upgrade() else {
        return Ok(());
    };
    let listed: Vec<ExGuid> = replica
        .pages()?
        .into_iter()
        .map(|(space, ..)| space)
        .collect();
    let unread: Vec<ExGuid> = {
        let index = index.lock().unwrap_or_else(|error| error.into_inner());
        listed
            .iter()
            .copied()
            .filter(|space| spaces.contains(space) || index.get(key, *space).is_none())
            .collect()
    };
    let entries = unread
        .into_iter()
        .map(|space| Ok(Entry::new(key, space, &replica.page(space)?, now())))
        .collect::<Result<Vec<_>, notebook::Error>>()?;
    let mut index = index.lock().unwrap_or_else(|error| error.into_inner());
    index.retain(|entry| entry.section != key || listed.contains(&entry.space));
    for entry in entries {
        index.set(entry);
    }
    Ok(())
}

/// A text box's label with `hits`, byte ranges of `text`, marked in OneNote's yellow.
pub(crate) fn marked(
    ui: &mut Ui,
    part: &str,
    text: &str,
    hits: &[std::ops::Range<usize>],
    spec: Spec,
) {
    let height = match spec.size[1].size {
        ui::Size::Pixels(height) => height,
        _ => 0.0,
    };
    let pad = spec.pad[0];
    ui.open(
        part,
        Spec {
            text: Some(text),
            ..spec
        },
    );
    for hit in hits {
        let (Some(before), Some(word)) = (text.get(..hit.start), text.get(hit.clone())) else {
            continue;
        };
        let [x, _] = ui.measure(before);
        let [width, tall] = ui.measure(word);
        let top = (height - tall) / 2.0;
        ui.mark(
            [pad + x - 1.0, top, pad + x + width + 1.0, top + tall],
            [1.0, 0.82, 0.0, 0.45],
            2.0,
        );
    }
    ui.close();
}

/// A section's place as a result names it: its notebook, groups and name.
fn place(notebooks: &[Arc<Library>], key: &str) -> String {
    let (location, path) = key.split_once('\n').unwrap_or((key, ""));
    let notebook = notebooks
        .iter()
        .find(|library| library.location == location)
        .map_or("", |library| library.name.as_str());
    let path = match notebooks
        .iter()
        .find(|library| library.location == location)
    {
        Some(library) if matches!(library.notebook, Ok(None)) => {
            crate::library::section_name(path, &None)
        }
        _ => path.strip_suffix(".one").unwrap_or(path).to_owned(),
    };
    format!("({notebook}/{path})")
}

impl State {
    /// Brings the index to the open notebooks and section when they changed, and when
    /// `always` or sections `changed` names, by key, to the section files changed since.
    pub(crate) fn sync_index(&mut self, always: bool, changed: Vec<String>) {
        let open = self
            .session
            .as_ref()
            .map(|session| (session.key(), Arc::downgrade(session.section.replica())));
        let identity: Vec<usize> = self
            .notebooks
            .iter()
            .map(|library| Arc::as_ptr(library) as usize)
            .chain(open.as_ref().map(|(_, replica)| replica.as_ptr() as usize))
            .collect();
        if !always && changed.is_empty() && identity == self.search.synced {
            return;
        }
        self.search.synced = identity;
        self.search.send(Job::Notebooks {
            libraries: self.notebooks.clone(),
            open,
            changed,
        });
    }

    /// Tells the index an edit changed page `space` of the open section.
    pub(crate) fn edited(&self, spaces: Vec<ExGuid>) {
        if let Some(session) = &self.session {
            self.search
                .changed(session.key(), session.section.replica(), spaces);
        }
    }

    /// Whether `key` names a section the search's scope takes in.
    pub(crate) fn in_scope(&self, scope: Scope, key: &str) -> bool {
        let Some(session) = &self.session else {
            return scope == Scope::All;
        };
        let path = &session.tabs[session.tab].path;
        let notebook = format!("{}\n", session.library.location);
        match scope {
            Scope::Section => *key == session.library.key(path),
            Scope::Group => match path.rsplit_once('/') {
                Some((folder, _)) => key.starts_with(&format!("{notebook}{folder}/")),
                None => key.starts_with(&notebook),
            },
            Scope::Notebook => key.starts_with(&notebook),
            Scope::All => true,
        }
    }

    /// The results for the query in its scope, made again when either or the index changed.
    pub(crate) fn refresh_results(&mut self) -> Result<(), Box<dyn Error>> {
        let query = Query::new(&self.search.query);
        let version = self.search.version.load(Ordering::Relaxed);
        let open = self
            .session
            .as_ref()
            .map(crate::Session::key)
            .unwrap_or_default();
        let wanted = (query.clone(), self.search.scope, version, open);
        if self.search.found_for.as_ref() == Some(&wanted) {
            return Ok(());
        }
        let start = Instant::now();
        let scope = self.search.scope;
        let found = {
            let index = self
                .search
                .index
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            index.search(&query, |key| self.in_scope(scope, key))
        };
        lap("search", start);
        if self
            .search
            .found_for
            .as_ref()
            .is_none_or(|(before, ..)| *before != query)
        {
            // The page shown marks the matches as they are typed.
            self.search.selected = None;
            self.refind(false)?;
        }
        self.search.found = found;
        self.search.found_for = Some(wanted);
        Ok(())
    }

    /// Marks the query's matches on the page; with `select`, selects the match Find on
    /// This Page is at, or the first, or the paragraph, when a result opened the page.
    pub(crate) fn refind(&mut self, select: bool) -> Result<(), Box<dyn Error>> {
        self.view.found = page_matches(&self.view.editor, &self.search.shown());
        if !select {
            return Ok(());
        }
        let space = self.session.as_ref().map(|session| session.space);
        let target = if self.search.finding {
            self.search.current = match self.search.current {
                Some(at) if at < self.view.found.len() => Some(at),
                _ => (!self.view.found.is_empty()).then_some(0),
            };
            self.search
                .current
                .and_then(|at| self.view.found.get(at).copied())
        } else if let Some((_, paragraph)) =
            self.search.reveal.filter(|(page, _)| Some(*page) == space)
        {
            self.search.reveal = None;
            match paragraph {
                Some(paragraph) => paragraph_match(&self.view.editor, paragraph),
                None => self.view.found.first().copied(),
            }
        } else {
            None
        };
        self.select(target)
    }

    fn select(&mut self, target: Option<PageMatch>) -> Result<(), Box<dyn Error>> {
        let Some((outline, selection)) = target else {
            return Ok(());
        };
        self.view.editor.focus_outline(outline)?;
        self.view.editor.select(selection)?;
        let response = self.view.focus_text()?;
        self.respond(response);
        Ok(())
    }

    /// Opens the results under the box, as Ctrl+E does.
    pub(crate) fn start_search(&mut self) {
        self.search.finding = false;
        if !self.ui.popup_open(results()) {
            self.sync_index(true, Vec::new());
            self.ui.open_popup(results());
            self.search.open = true;
        }
    }

    /// Turns the box into Find on This Page, as Ctrl+F does; from the page it starts empty.
    pub(crate) fn start_find(&mut self, keep: bool) -> Result<(), Box<dyn Error>> {
        if !keep {
            self.search.query.clear();
        }
        self.search.open = false;
        self.ui.close_popup(results());
        self.search.finding = true;
        self.search.claim = true;
        self.search.current = None;
        self.refind(true)
    }

    /// Ends the search or find: the box empties, the marks go and the page takes the keys,
    /// keeping any match selected. The Search Results pane keeps the search while open.
    pub(crate) fn end_search(&mut self) {
        self.search.open = false;
        self.search.finding = false;
        self.search.current = None;
        self.ui.close_popup(results());
        self.ui.set_focus(Some(page()));
        if !matches!(self.search.pane, Some(Pane::Search { .. })) {
            self.search.query.clear();
            self.search.scope = self.search.default;
            self.view.found.clear();
        }
    }

    /// Opens a result's page, whose first match is selected once it shows.
    fn open_result(&mut self, index: usize) -> Result<(), Box<dyn Error>> {
        match self.search.found.get(index) {
            Some(found) => self.reveal(found.section.clone(), found.space, None),
            None => Ok(()),
        }
    }

    /// Opens page `space` of `section`, whose first match, or `paragraph`, is selected once
    /// it shows.
    pub(crate) fn reveal(
        &mut self,
        section: String,
        space: ExGuid,
        paragraph: Option<ExGuid>,
    ) -> Result<(), Box<dyn Error>> {
        let (location, path) = section.split_once('\n').unwrap_or_default();
        self.search.reveal = Some((space, paragraph));
        let open = self
            .session
            .as_ref()
            .map(|session| (session.key(), session.space));
        match open {
            Some((key, shown)) if key == section && shown == space => self.refind(true)?,
            Some((key, _)) if key == section => self.commands.push(Command::OpenPage(space)),
            _ => {
                let library = self
                    .notebooks
                    .iter()
                    .find(|library| library.location == location)
                    .ok_or("The notebook is no longer open")?;
                self.commands
                    .push(Command::OpenSection(Arc::clone(library), path.to_owned()));
                self.last_pages.insert(section, space);
            }
        }
        Ok(())
    }

    /// The search box in the tab row: OneNote's search while idle or searching, Find on
    /// This Page while finding.
    pub(crate) fn search_box(&mut self, theme: &Theme) -> Result<(), Box<dyn Error>> {
        let focused = [Some(field()), Some(find_field())].contains(&self.ui.focused());
        let searching = self.ui.popup_open(results());
        let spec = |focused: bool| Spec {
            size: [fill(), px(ui::shell::TOOL)],
            fill: Some(theme.base),
            border: Some(if focused { theme.accent } else { theme.chip }),
            radius: 4.0,
            pad: [4.0, 0.0],
            gap: 2.0,
            ..Spec::default()
        };
        let box_id = self.ui.open("search", spec(focused || searching));
        if self.search.finding {
            self.find_bar(theme)?;
        } else if searching {
            // The results' own field lies over the box.
            self.ui.leaf(
                "covered",
                Spec {
                    size: [fill(), px(ui::shell::TOOL)],
                    ..Spec::default()
                },
            );
        } else {
            let placeholder = format!(
                "Search {} ({})",
                self.search.scope.name(),
                commands::shortcut(commands::Id::Search)
            );
            let signal = ui::text_field(
                &mut self.ui,
                field(),
                &mut self.search.query,
                &placeholder,
                Spec {
                    size: [fill(), px(ui::shell::TOOL)],
                    pad: [4.0, 0.0],
                    role: Some(Role::SearchInput),
                    ..Spec::default()
                },
            );
            name(&mut self.ui, field(), "Search");
            if signal.pressed && !searching {
                self.start_search();
            }
            self.scope_button(theme);
        }
        self.ui.close();
        // Results opened this frame build from the next, as the box's field was built.
        if searching || !self.ui.popup_open(results()) {
            self.results_popup(theme, box_id)?;
        }
        self.scope_popup();
        Ok(())
    }

    fn scope_button(&mut self, theme: &Theme) {
        let button = self.ui.id("scope");
        if ui::shell::tool_button(&mut self.ui, "scope", art::SEARCH, theme.text_dim, None).pressed
        {
            self.search.anchor = Some(Anchor::Below(button));
            self.ui.open_popup(scope_menu());
        }
        let open = self.ui.popup_open(scope_menu());
        if let Some(node) = self.ui.access(button) {
            node.set_label("Search In");
            node.set_has_popup(accesskit::HasPopup::Menu);
            node.set_expanded(open);
        }
    }

    /// OneNote's "Search In:" menu.
    fn scope_popup(&mut self) {
        let current = self.search.scope;
        let find = commands::shortcut(commands::Id::Find);
        let mut items = vec![
            ui::popup::Item {
                text: "Search In:",
                heading: true,
                ..Default::default()
            },
            ui::popup::Item {
                text: commands::command(commands::Id::Find).title,
                shortcut: &find,
                ..Default::default()
            },
        ];
        items.extend(
            Scope::ALL
                .iter()
                .enumerate()
                .map(|(index, scope)| ui::popup::Item {
                    text: scope.name(),
                    checked: Some(*scope == current),
                    separated: index == 0,
                    ..Default::default()
                }),
        );
        items.push(ui::popup::Item {
            text: "Set This Scope as Default",
            separated: true,
            disabled: current == self.search.default,
            ..Default::default()
        });
        let anchor = self.search.anchor.unwrap_or(Anchor::Point([0.0, 0.0]));
        match ui::popup::menu(&mut self.ui, scope_menu(), anchor, &items, None) {
            Some(1) => {
                if let Err(error) = self.start_find(true) {
                    eprintln!("{error}");
                }
            }
            Some(index @ 2..=5) => {
                self.search.scope = Scope::ALL[index - 2];
                self.start_search();
            }
            Some(6) => {
                self.search.default = current;
                self.save_settings();
            }
            _ => {}
        }
    }

    /// The dropdown of results, over the box it drops from.
    fn results_popup(&mut self, theme: &Theme, over: Id) -> Result<(), Box<dyn Error>> {
        if !self.ui.popup_open(results()) {
            // Closed by Esc or a press elsewhere: the search ends.
            if std::mem::take(&mut self.search.open) {
                self.end_search();
            }
            return Ok(());
        }
        self.refresh_results()?;
        let keys = ui::popup::navigation(
            &mut self.ui,
            &[field(), results()],
            &[
                NamedKey::ArrowUp,
                NamedKey::ArrowDown,
                NamedKey::PageUp,
                NamedKey::PageDown,
                NamedKey::Enter,
            ],
        );
        if self.ui.focused() == Some(results()) {
            self.ui.set_focus(Some(field()));
        }
        self.ui.open_as(
            results(),
            Spec {
                axis: Axis::Y,
                size: [px(RESULTS), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 6.0,
                pad: [4.0, 4.0],
                gap: 4.0,
                anchor: Some(Anchor::Over(over)),
                role: Some(Role::Dialog),
                ..Spec::default()
            },
        );
        name(&mut self.ui, results(), "Search");
        self.ui.open(
            "box",
            Spec {
                // Takes the toolbar's box's place at once, widening with the dropdown.
                flags: Flags::STILL,
                size: [fill(), px(ui::shell::TOOL)],
                fill: Some(theme.base),
                border: Some(theme.accent),
                radius: 4.0,
                pad: [4.0, 0.0],
                gap: 2.0,
                ..Spec::default()
            },
        );
        let placeholder = format!(
            "Search {} ({})",
            self.search.scope.name(),
            commands::shortcut(commands::Id::Search)
        );
        ui::text_field(
            &mut self.ui,
            field(),
            &mut self.search.query,
            &placeholder,
            Spec {
                size: [fill(), px(ui::shell::TOOL)],
                pad: [4.0, 0.0],
                role: Some(Role::SearchInput),
                ..Spec::default()
            },
        );
        name(&mut self.ui, field(), "Search");
        self.scope_button(theme);
        self.ui.close();
        let query = self.search.query.trim().to_owned();
        let found = &self.search.found;
        let titled = found.iter().take_while(|found| found.in_title).count();
        // OneNote's status line: whether the search is done, and where it looked.
        let status = if query.is_empty() {
            "Search In:"
        } else if self.search.pending.load(Ordering::Relaxed) > 0 {
            "Searching:"
        } else if found.is_empty() {
            "No matches:"
        } else {
            "Finished:"
        };
        let scope = if query.is_empty() || !found.is_empty() {
            format!("{} (change)", self.search.scope.name())
        } else {
            self.search.scope.name().to_owned()
        };
        self.ui.open(
            "status",
            Spec {
                size: [fill(), px(22.0)],
                pad: [6.0, 0.0],
                gap: 4.0,
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "state",
            Spec {
                size: [fit(), px(22.0)],
                text: Some(status),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        );
        let change = self.ui.id("scope");
        if self
            .ui
            .leaf(
                "scope",
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fit(), px(22.0)],
                    text: Some(&scope),
                    color: Some(theme.accent),
                    role: Some(Role::Button),
                    ..Spec::default()
                },
            )
            .clicked
        {
            self.search.anchor = Some(Anchor::Below(change));
            self.ui.open_popup(scope_menu());
        }
        self.ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "find",
            Spec {
                size: [fit(), px(22.0)],
                text: Some(&format!(
                    "Find on page: {}",
                    commands::shortcut(commands::Id::Find)
                )),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        );
        self.ui.close();
        let mut chosen = None;
        if !found.is_empty() {
            let rows = Results {
                titled,
                count: found.len(),
            };
            let content = found.len() as f32 * RESULT + ui::Rows::space_before(&rows, found.len());
            // Down to the window's foot from the box, as last laid out.
            let bottom = self.ui.laid_out(over).map_or(0.0, |rect| rect[3]);
            let view = content.min((self.ui.size()[1] - bottom - 80.0).max(RESULT));
            let before = self.search.selected;
            let places: Vec<String> = found
                .iter()
                .map(|found| place(&self.notebooks, &found.section))
                .collect();
            let headings = [
                format!("Title contains: {query} ({titled})"),
                format!("Body contains: {query} ({})", found.len() - titled),
            ];
            let clicked = ui::list(
                &mut self.ui,
                results().child("rows"),
                Spec {
                    size: [fill(), px(view)],
                    fill: Some(theme.popup),
                    role: Some(Role::ListBox),
                    ..Spec::default()
                },
                ui::List {
                    rows: &rows,
                    row: RESULT,
                    keys: &keys,
                    hover_selects: true,
                },
                &mut self.search.selected,
                |ui, row| {
                    if let Some(node) = ui.access(results().child("rows").child(row.key)) {
                        node.set_role(Role::ListBoxOption);
                        node.set_selected(row.selected);
                    }
                    let found = &found[row.index];
                    if row.index == 0 || row.index == titled {
                        let heading = if found.in_title {
                            &headings[0]
                        } else {
                            &headings[1]
                        };
                        ui.leaf(
                            "heading",
                            Spec {
                                flags: Flags::FLOAT,
                                size: [fill(), px(HEADING)],
                                position: [0.0, -HEADING],
                                text: Some(heading),
                                bold: true,
                                color: Some(theme.text),
                                pad: [6.0, 0.0],
                                ..Spec::default()
                            },
                        );
                    }
                    result_row(ui, theme, found, &places[row.index], row.selected);
                },
            );
            if let Some(key) = self.search.selected
                && let Some(node) = self.ui.access(field())
            {
                node.set_active_descendant(results().child("rows").child(key).node());
            }
            // Arrows show the page they select, as OneNote previews it.
            if before != self.search.selected
                && keys.iter().any(|key| *key != NamedKey::Enter)
                && let Some(index) = self.search.selected.map(|key| key as usize)
            {
                self.open_result(index)?;
            }
            let entered = keys.contains(&NamedKey::Enter);
            chosen = clicked.or_else(|| {
                self.search
                    .selected
                    .map(|key| key as usize)
                    .filter(|_| entered)
                    .or_else(|| entered.then_some(0))
            });
        }
        let link = format!(
            "Open Search Results Pane ({})",
            commands::shortcut(commands::Id::SearchResults)
        );
        let pane = self
            .ui
            .leaf(
                "pane",
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fit(), px(22.0)],
                    icon: Some(art::SEARCH),
                    text: Some(&link),
                    color: Some(theme.accent),
                    pad: [6.0, 0.0],
                    role: Some(Role::Link),
                    ..Spec::default()
                },
            )
            .clicked;
        self.ui.close();
        if pane {
            self.open_search_pane();
        } else if let Some(index) = chosen {
            self.open_result(index)?;
            self.search.open = false;
            self.ui.close_popup(results());
            self.ui.set_focus(Some(page()));
        }
        Ok(())
    }

    /// Find on This Page in the box: the match count with previous and next, then the field.
    fn find_bar(&mut self, theme: &Theme) -> Result<(), Box<dyn Error>> {
        let keys = ui::popup::navigation(
            &mut self.ui,
            &[find_field()],
            &[NamedKey::Enter, NamedKey::F3, NamedKey::Escape],
        );
        let before = self.search.query.clone();
        let count = self.view.found.len();
        let label = if self.search.query.trim().is_empty() {
            "Find on page".to_owned()
        } else if count == 0 {
            format!(
                "No matches. Try {}",
                commands::shortcut(commands::Id::Search)
            )
        } else {
            format!(
                "Match {} of {count}",
                self.search.current.map_or(0, |at| at + 1)
            )
        };
        self.ui.leaf(
            "count",
            Spec {
                // It shortens once the query has given up all it may.
                size: [
                    ui::Extent {
                        size: ui::Size::Text,
                        strictness: 0.5,
                    },
                    px(ui::shell::TOOL),
                ],
                overflow: ui::Overflow::Ellipsis,
                inset: [0.0, 3.0, 0.0, 3.0],
                text: Some(&label),
                fill: Some(draw::srgb(0xff, 0xe0, 0x5a)),
                color: Some(draw::srgb(0x20, 0x20, 0x20)),
                radius: 3.0,
                pad: [5.0, 0.0],
                ..Spec::default()
            },
        );
        let mut step = None;
        if count > 0 {
            if ui::shell::tool_button(&mut self.ui, "previous", art::CHEVRON_UP, theme.text, None)
                .clicked
            {
                step = Some(false);
            }
            let button = self.ui.id("previous");
            name(&mut self.ui, button, "Previous Match");
            if ui::shell::tool_button(&mut self.ui, "next", ui::shell::CHEVRON, theme.text, None)
                .clicked
            {
                step = Some(true);
            }
            let button = self.ui.id("next");
            name(&mut self.ui, button, "Next Match");
        }
        ui::text_field(
            &mut self.ui,
            find_field(),
            &mut self.search.query,
            "",
            Spec {
                // Wider than the box ever is, so it takes all there is, yet keeps 72 for a
                // few words of the query.
                size: [
                    ui::Extent {
                        size: ui::Size::Pixels(1000.0),
                        strictness: 72.0 / 1000.0,
                    },
                    px(ui::shell::TOOL),
                ],
                pad: [4.0, 0.0],
                role: Some(Role::SearchInput),
                ..Spec::default()
            },
        );
        name(&mut self.ui, find_field(), "Find on Page");
        if std::mem::take(&mut self.search.claim) {
            self.ui.set_focus(Some(find_field()));
        }
        let close =
            ui::shell::tool_button(&mut self.ui, "close", art::CLOSE, theme.text_dim, None).clicked;
        let button = self.ui.id("close");
        name(&mut self.ui, button, "Close");
        if close {
            self.end_search();
            return Ok(());
        }
        let shift = self.view.modifiers().shift;
        for key in keys {
            match key {
                NamedKey::Escape => {
                    self.end_search();
                    return Ok(());
                }
                _ => step = Some(!shift),
            }
        }
        if self.search.query != before {
            self.search.current = None;
            self.refind(true)?;
        } else if let Some(forward) = step
            && count > 0
        {
            let at = self.search.current.unwrap_or(0);
            let next = if forward {
                (at + 1) % count
            } else {
                (at + count - 1) % count
            };
            self.search.current = Some(next);
            self.select(self.view.found.get(next).copied())?;
        }
        Ok(())
    }
}

/// The results list's rows: those whose titles hold every word, then the others, each
/// group under its heading.
struct Results {
    titled: usize,
    count: usize,
}

impl ui::Rows for Results {
    fn count(&self) -> usize {
        self.count
    }

    fn key(&self, index: usize) -> u64 {
        index as u64
    }

    fn find(&self, key: u64) -> Option<usize> {
        ((key as usize) < self.count).then_some(key as usize)
    }

    fn space_before(&self, index: usize) -> f32 {
        let headings = usize::from(self.count > 0)
            + usize::from(0 < self.titled && self.titled < self.count && index >= self.titled);
        headings as f32 * HEADING
    }
}

/// A result: its page's title and section, and the snippet around its first match.
fn result_row(ui: &mut Ui, theme: &Theme, found: &Found, place: &str, selected: bool) {
    // On the highlight every line takes the text's colour, as menus do.
    let dim = if selected { theme.text } else { theme.text_dim };
    ui.open(
        "result",
        Spec {
            size: [fill(), fill()],
            fill: selected.then(|| theme.hover()),
            radius: 4.0,
            pad: [6.0, 3.0],
            gap: 6.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "icon",
        Spec {
            size: [px(16.0), px(18.0)],
            icon: Some(art::PAGE),
            color: Some(dim),
            ..Spec::default()
        },
    );
    ui.open(
        "lines",
        Spec {
            axis: Axis::Y,
            size: [fill(), fill()],
            ..Spec::default()
        },
    );
    ui.open(
        "top",
        Spec {
            size: [fill(), px(18.0)],
            gap: 8.0,
            ..Spec::default()
        },
    );
    let title = if found.title.trim().is_empty() {
        "Untitled page"
    } else {
        found.title.as_str()
    };
    marked(
        ui,
        "title",
        title,
        &found.title_hits,
        Spec {
            size: [fill(), px(18.0)],
            color: Some(theme.text),
            ..Spec::default()
        },
    );
    ui.leaf(
        "place",
        Spec {
            size: [px(PLACE), px(18.0)],
            text: Some(place),
            color: Some(dim),
            ..Spec::default()
        },
    );
    ui.close();
    marked(
        ui,
        "snippet",
        &found.snippet,
        &found.snippet_hits,
        Spec {
            size: [fill(), px(18.0)],
            color: Some(dim),
            ..Spec::default()
        },
    );
    ui.close();
    ui.close();
}
