//! Notebooks behind the C surface: a folder or lone section from Files, or a folder on an
//! SMB share through the embedded client. A section opens through its replica in the app's
//! cache, which stores each edit at once and publishes it in the background.

use crate::{Result, owned, report, string};
use canvas::search::{Entry, Index, Query, Tagged};
use notebook::{
    Remote, Replica,
    discover::{Folder, Reason, SectionState},
    session::{self, Background, Event, Known, Notebook, SyncStatus},
    smb::{Client, Credentials},
};
use onestore::{
    CommitError, CommitState, ExGuid, PageCreation, PageEdit, Stamp, Transaction,
    op::{Edit, Op, SectionOp},
    page::Page,
};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::{CString, c_char, c_void},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

/// How long an SMB request may take before the share counts as unreachable.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The largest section file read whole.
const LIMIT: usize = 256 * 1024 * 1024;
/// The tab colour OneNote gives a section that stores none, as the desktop shows it.
const SECTION_COLOR: u32 = 0x00e4_a88a;

/// Called on a background thread when a notebook's sync status may have changed.
static SYNC_WAKE: OnceLock<extern "C" fn()> = OnceLock::new();

/// Has the host call `wake` whenever a notebook's closed sections report; set once, before
/// any library opens.
#[unsafe(no_mangle)]
pub extern "C" fn sb_set_sync_wake(wake: extern "C" fn()) {
    let _ = SYNC_WAKE.set(wake);
}

fn sync_woken() {
    if let Some(wake) = SYNC_WAKE.get() {
        wake();
    }
}

/// Runs `body(context)` within the host's coordinated reading or writing of `path`, as
/// `NSFileCoordinator` has other processes (file providers) wait for it, or not at all when
/// it cannot coordinate.
pub type Coordinator = extern "C" fn(
    path: *const c_char,
    write: bool,
    body: extern "C" fn(*mut c_void),
    context: *mut c_void,
);

static COORDINATOR: OnceLock<Coordinator> = OnceLock::new();

/// Coordinates every read and write of a local notebook's files through `coordinator` from
/// now on; set once, before any library opens.
#[unsafe(no_mangle)]
pub extern "C" fn sb_set_coordinator(coordinator: Coordinator) {
    let _ = COORDINATOR.set(coordinator);
}

/// `work` within the host's coordinated access to `path`, or directly when the host
/// coordinates nothing.
pub(crate) fn coordinated<F: FnOnce() -> T, T>(path: &Path, write: bool, work: F) -> io::Result<T> {
    let Some(coordinator) = COORDINATOR.get() else {
        return Ok(work());
    };
    extern "C" fn run<F: FnOnce() -> T, T>(context: *mut c_void) {
        // SAFETY: `context` is the pair below, alive for the host's call.
        let (work, result) = unsafe { &mut *context.cast::<(Option<F>, Option<T>)>() };
        *result = work.take().map(|work| work());
    }
    let mut pair = (Some(work), None);
    let path = CString::new(path.as_os_str().as_encoded_bytes())?;
    coordinator(path.as_ptr(), write, run::<F, T>, (&raw mut pair).cast());
    pair.1
        .ok_or_else(|| io::Error::other("Another app is using the notebook's file; try again."))
}

/// Calls `found(context, id, device)` for each conflict version the host's file provider keeps
/// beside the file at `path`, as iCloud Drive keeps another device's commit that lost:
/// `id` is a file the version's contents read from, `device` who saved it or null.
pub type Versions = extern "C" fn(
    path: *const c_char,
    found: extern "C" fn(*mut c_void, *const c_char, *const c_char),
    context: *mut c_void,
);

/// Marks version `id` of the file at `path` resolved and removes it, first keeping a copy
/// beside the file when `keep`; false when it could not.
pub type Retire = extern "C" fn(path: *const c_char, id: *const c_char, keep: bool) -> bool;

static VERSIONS: OnceLock<(Versions, Retire)> = OnceLock::new();

/// Merges the conflict versions the host lists into their files from now on, then has the
/// host retire them; set once, before any library opens.
#[unsafe(no_mangle)]
pub extern "C" fn sb_set_versions(versions: Versions, retire: Retire) {
    let _ = VERSIONS.set((versions, retire));
}

/// A local section file read and published under the host's file coordination.
struct Coordinated(PathBuf);

impl Remote for Coordinated {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        coordinated(&self.0, false, || onestore::read_file(&self.0))?
    }

    /// The header and length, as the file remote's uncoordinated change detector reads them.
    fn stamp(&mut self) -> io::Result<Stamp> {
        coordinated(&self.0, false, || {
            use std::io::Read;
            let mut file = std::fs::File::open(&self.0)?;
            let mut header = [0; 1024];
            file.read_exact(&mut header)?;
            Ok(Stamp {
                header,
                length: file.metadata()?.len(),
            })
        })?
    }

    fn publish(&mut self, transaction: &Transaction) -> std::result::Result<(), CommitError> {
        coordinated(&self.0, true, || transaction.commit_file(&self.0)).map_err(uncommitted)?
    }

    fn confirm(&mut self, base: &Stamp) -> std::result::Result<(), CommitError> {
        coordinated(&self.0, true, || onestore::confirm_file(&self.0, base)).map_err(uncommitted)?
    }

    fn versions(&mut self) -> io::Result<Vec<notebook::Version>> {
        let Some((versions, _)) = VERSIONS.get() else {
            return Ok(Vec::new());
        };
        extern "C" fn found(context: *mut c_void, id: *const c_char, device: *const c_char) {
            // SAFETY: `context` is the list below, alive for the host's call.
            let listed = unsafe { &mut *context.cast::<Vec<notebook::Version>>() };
            listed.push(notebook::Version {
                id: string(id),
                device: (!device.is_null()).then(|| string(device)),
            });
        }
        let mut listed: Vec<notebook::Version> = Vec::new();
        let path = CString::new(self.0.as_os_str().as_encoded_bytes())?;
        versions(path.as_ptr(), found, (&raw mut listed).cast());
        Ok(listed)
    }

    /// A version's contents never change: they read without coordination.
    fn version(&mut self, id: &str) -> io::Result<Vec<u8>> {
        std::fs::read(id)
    }

    fn retire(&mut self, id: &str, keep: bool) -> io::Result<()> {
        let Some((_, retire)) = VERSIONS.get() else {
            return Ok(());
        };
        let path = CString::new(self.0.as_os_str().as_encoded_bytes())?;
        let id = CString::new(id)?;
        if retire(path.as_ptr(), id.as_ptr(), keep) {
            Ok(())
        } else {
            Err(io::Error::other("The version could not be retired"))
        }
    }
}

fn uncommitted(error: io::Error) -> CommitError {
    CommitError {
        state: CommitState::NotCommitted,
        error,
    }
}

/// A server and share with the account that opens it, kept to reconnect.
pub(crate) struct Server {
    pub(crate) address: String,
    pub(crate) share: String,
    pub(crate) user: String,
    pub(crate) password: String,
    pub(crate) domain: String,
}

impl Server {
    pub(crate) fn connect(&self) -> io::Result<Client> {
        Client::connect(
            &self.address,
            &self.share,
            Credentials {
                username: &self.user,
                password: &self.password,
                domain: &self.domain,
            },
            TIMEOUT,
        )
    }
}

/// A connected share, for browsing to a notebook folder.
pub struct Share(Client);

enum Place {
    Folder(PathBuf),
    /// A section file opened on its own.
    File(PathBuf),
    /// A notebook folder at `root` on a share; no client while the server cannot be reached.
    Share {
        server: Arc<Server>,
        client: Mutex<Option<Arc<Client>>>,
        root: String,
    },
}

/// A share notebook's sections as last listed, so they open from their replicas while the
/// server cannot be reached.
#[derive(serde::Serialize, serde::Deserialize)]
struct Listed {
    tabs: Vec<Tab>,
    /// Each readable section's file identity in hex, which names its replica, by catalog path.
    files: BTreeMap<String, String>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every readable section's file identity in `folder` and its groups, by catalog path, as
/// `Notebook::replicas` lists a local notebook's. `Client::read` answers a protected section
/// as it does one mid-write (WouldBlock), so syncing one would show it in use for good.
pub(crate) fn files(folder: &Folder, files: &mut BTreeMap<String, String>) {
    for section in &folder.sections {
        if matches!(section.state, SectionState::Readable { .. }) {
            files.insert(section.path.clone(), hex(&section.file_id));
        }
    }
    for group in &folder.groups {
        self::files(group, files);
    }
}

/// When a section's file was last checked for changes, and how it was then.
struct Checked {
    at: Instant,
    stamp: Vec<u8>,
}

pub struct Library {
    /// `None` for a lone section.
    notebook: Mutex<Option<Notebook>>,
    place: Place,
    cache: PathBuf,
    index: Mutex<(HashMap<String, Checked>, Index)>,
    /// Syncs the sections no session holds; none for a lone section.
    background: Option<Background>,
    /// The sections open for editing, by catalog path, whose sessions sync them.
    open: Mutex<Vec<(String, Weak<Shared>)>>,
    /// Work Offline, which sections opened later follow too.
    offline: AtomicBool,
    /// How long edits wait for a pause in typing before they publish (`Section::set_pause`):
    /// a file provider such as iCloud Drive uploads each publication, and makes one concurrent
    /// with another device's a conflict version.
    pause: Duration,
}

/// The pause edits to a file a provider keeps elsewhere wait for.
const PAUSE: Duration = Duration::from_secs(3);

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Tab {
    pub(crate) name: String,
    /// The catalog path `sb_section_open` takes.
    pub(crate) path: String,
    /// The section group holding it, `/`-separated; empty at the notebook's top.
    pub(crate) group: String,
    /// The tab colour in sRGB.
    pub(crate) color: [u8; 3],
    /// Password-protected or unreadable sections list but do not open.
    pub(crate) readable: bool,
    /// Not on this device yet, as iCloud Drive keeps it elsewhere; the host downloads it.
    #[serde(default)]
    pub(crate) downloading: bool,
    /// Why the file could not be read, where retrying or repair may help.
    #[serde(default)]
    pub(crate) problem: Option<String>,
}

fn rgb(colorref: u32) -> [u8; 3] {
    let [red, green, blue, _] = colorref.to_le_bytes();
    [red, green, blue]
}

fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A folder's sections, its groups' after them, leaving out the recycle bin OneNote keeps
/// deleted sections and pages in.
fn tabs(folder: &Folder, tabs: &mut Vec<Tab>) {
    for section in &folder.sections {
        let (name, color, readable) = match &section.state {
            SectionState::Readable { name, color, .. } => (name.clone(), *color, true),
            SectionState::Locked | SectionState::Unreadable(_) => (None, None, false),
        };
        tabs.push(Tab {
            name: name.unwrap_or_else(|| stem(&section.path)),
            path: section.path.clone(),
            group: folder.path.clone(),
            color: rgb(color.unwrap_or(SECTION_COLOR)),
            readable,
            downloading: false,
            problem: None,
        });
    }
    for entry in folder.unavailable.iter().filter(|entry| !entry.group) {
        tabs.push(Tab {
            name: stem(&entry.path),
            path: entry.path.clone(),
            group: folder.path.clone(),
            color: rgb(SECTION_COLOR),
            readable: false,
            downloading: entry.reason == Reason::Evicted,
            problem: match entry.reason {
                Reason::InUse => Some("Section in use".into()),
                Reason::Unreadable => Some("Can’t read this section".into()),
                _ => None,
            },
        });
    }
    for group in &folder.groups {
        if !group.path.ends_with("OneNote_RecycleBin") {
            self::tabs(group, tabs);
        }
    }
}

impl Library {
    /// The notebook folder or lone section file at `path`. A folder `local` to this device
    /// reports every change to `touched`; any other, as a file provider keeps it, gets offline
    /// copies of its sections and is checked every few seconds.
    pub(crate) fn open(path: &Path, cache: &Path, local: bool) -> Result<Self> {
        let (notebook, place, background) = if path.is_file() {
            (None, Place::File(path.to_owned()), None)
        } else {
            let mut notebook = Notebook::open(path, cache)?;
            let background = notebook.background_with(
                local,
                !local,
                |file| Coordinated(file.to_owned()),
                sync_woken,
            )?;
            background.watch(notebook.replicas());
            (
                Some(notebook),
                Place::Folder(path.to_owned()),
                Some(background),
            )
        };
        Ok(Self {
            notebook: Mutex::new(notebook),
            place,
            cache: cache.to_owned(),
            index: Mutex::default(),
            background,
            open: Mutex::default(),
            offline: AtomicBool::new(false),
            pause: if local { Duration::ZERO } else { PAUSE },
        })
    }

    /// The notebook folder `root` on `server`; while the server cannot be reached, its sections
    /// as last listed, if it was listed before.
    pub(crate) fn server(server: Server, root: &str, cache: &Path) -> Result<Self> {
        let server = Arc::new(server);
        let connect = Arc::clone(&server);
        let background = Background::smb(root, LIMIT, move || connect.connect(), sync_woken)?;
        let library = Self {
            notebook: Mutex::default(),
            place: Place::Share {
                server,
                client: Mutex::default(),
                root: root.to_owned(),
            },
            cache: cache.to_owned(),
            index: Mutex::default(),
            background: Some(background),
            open: Mutex::default(),
            offline: AtomicBool::new(false),
            pause: Duration::ZERO,
        };
        let reached = library.with_notebook(false, |_| Ok(()));
        match library.listed() {
            Some(listed) => library.watch(&listed.files),
            None => reached?,
        }
        Ok(library)
    }

    /// Has the background sync a share notebook's sections, `files` as `Listed` keeps them.
    fn watch(&self, files: &BTreeMap<String, String>) {
        let (Some(background), Some(replicas)) = (&self.background, self.share_replicas()) else {
            return;
        };
        background.watch(
            files
                .iter()
                .map(|(path, identity)| Known {
                    path: path.clone(),
                    replica: Some(replicas.join(format!("{identity}.sqlite"))),
                    found: None,
                    image: None,
                })
                .collect(),
        );
    }

    /// The cache folder of a share notebook's replicas.
    fn share_replicas(&self) -> Option<PathBuf> {
        let Place::Share { server, root, .. } = &self.place else {
            return None;
        };
        let location = notebook::location::smb(&server.address, &server.share, root);
        Some(notebook::location::folder(&self.cache, &location))
    }

    /// Where a share notebook's last listing is kept.
    fn listing(&self) -> Option<PathBuf> {
        let Place::Share { server, root, .. } = &self.place else {
            return None;
        };
        let name: String = format!("{}/{}/{root}", server.address, server.share)
            .chars()
            .map(|char| if char.is_alphanumeric() { char } else { '_' })
            .collect();
        Some(self.cache.join("smb").join(format!("{name}.json")))
    }

    fn listed(&self) -> Option<Listed> {
        let bytes = std::fs::read(self.listing()?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn notebook(&self) -> std::sync::MutexGuard<'_, Option<Notebook>> {
        self.notebook
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn client(&self) -> Option<Arc<Client>> {
        match &self.place {
            Place::Share { client, .. } => client
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
            _ => None,
        }
    }

    /// Runs `change`, which reads or with `write` changes the notebook's files, on the
    /// notebook: a local folder under the host's file coordination; on a share not reached
    /// yet or whose connection dropped, connecting again and retrying once.
    fn with_notebook<T>(
        &self,
        write: bool,
        mut change: impl FnMut(&mut Notebook) -> Result<T>,
    ) -> Result<T> {
        let mut notebook = self.notebook();
        let Place::Share {
            server,
            client,
            root,
        } = &self.place
        else {
            let notebook = notebook.as_mut().ok_or("A lone section has no notebook")?;
            return match &self.place {
                Place::Folder(root) => coordinated(root, write, || change(notebook))?,
                _ => change(notebook),
            };
        };
        if let Some(current) = notebook.as_mut()
            && let Ok(result) = change(current)
        {
            return Ok(result);
        }
        let fresh = Arc::new(server.connect()?);
        *client.lock().unwrap_or_else(|error| error.into_inner()) = Some(Arc::clone(&fresh));
        let reopened = notebook.insert(Notebook::open_smb(fresh, root, &self.cache)?);
        change(reopened)
    }

    /// The notebook's sections, read again.
    pub(crate) fn tabs(&self) -> Result<Vec<Tab>> {
        if let Place::File(file) = &self.place {
            let path = file.to_string_lossy().into_owned();
            return Ok(vec![Tab {
                name: stem(&path),
                path,
                group: String::new(),
                color: rgb(SECTION_COLOR),
                readable: true,
                downloading: false,
                problem: None,
            }]);
        }
        let listed = self.with_notebook(false, |notebook| {
            notebook.refresh()?;
            let mut listed = Listed {
                tabs: Vec::new(),
                files: BTreeMap::new(),
            };
            tabs(notebook.catalog(), &mut listed.tabs);
            files(notebook.catalog(), &mut listed.files);
            if let (Place::Folder(_), Some(background)) = (&self.place, &self.background) {
                background.watch(notebook.replicas());
            }
            Ok(listed)
        });
        let Some(listing) = self.listing() else {
            return Ok(listed?.tabs);
        };
        match listed {
            Ok(listed) => {
                std::fs::create_dir_all(listing.parent().ok_or("No cache")?)?;
                std::fs::write(&listing, serde_json::to_vec(&listed)?)?;
                self.watch(&listed.files);
                Ok(listed.tabs)
            }
            Err(error) => Ok(self.listed().ok_or(error)?.tabs),
        }
    }

    /// The section at catalog `path`, through a replica in the cache, following Work
    /// Offline. The background may hold the replica for a step, which takes a network round
    /// trip, so a busy replica is tried again for a while.
    pub(crate) fn section(
        &self,
        path: &str,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> Result<session::Section> {
        let deadline = Instant::now() + TIMEOUT * 3;
        let notify = Arc::new(notify);
        let section = loop {
            let notify = Arc::clone(&notify);
            match self.section_once(path, move || notify()) {
                Err(error)
                    if Instant::now() < deadline
                        && error
                            .downcast_ref::<notebook::Error>()
                            .is_some_and(notebook::Error::busy) =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                opened => break opened?,
            }
        };
        section.set_offline(self.offline.load(Ordering::Relaxed));
        section.set_pause(self.pause);
        if let Some(background) = &self.background {
            background.hold(path, &section);
        }
        Ok(section)
    }

    fn section_once(
        &self,
        path: &str,
        notify: impl Fn() + Send + 'static,
    ) -> Result<session::Section> {
        match &self.place {
            Place::File(file) => Ok(session::Section::open_with(
                file,
                &self.cache,
                |file| Ok(Coordinated(file.to_owned())),
                notify,
            )?),
            Place::Folder(_) => Ok(self
                .notebook()
                .as_ref()
                .ok_or("A lone section has no notebook")?
                .section_with(path, |file| Ok(Coordinated(file.to_owned())), notify)?),
            Place::Share { server, root, .. } => {
                let file = if root.is_empty() {
                    path.to_owned()
                } else {
                    format!("{root}/{path}")
                };
                let identity = self
                    .listed()
                    .and_then(|mut listed| listed.files.remove(path))
                    .ok_or("The notebook hasn’t listed this section")?;
                let replicas = self.share_replicas().ok_or("Not a share")?;
                let cache = replicas.join(format!("{identity}.sqlite"));
                std::fs::create_dir_all(&replicas)?;
                let replica = Replica::open_or_create(&cache, || {
                    let client = self.client().ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::NotConnected,
                            "The server can’t be reached, and this section hasn’t been opened here before.",
                        )
                    })?;
                    Ok(client.read_storage(&file, LIMIT)?)
                })?;
                let server = Arc::clone(server);
                Ok(session::Section::resume_smb(
                    file,
                    replica,
                    LIMIT,
                    move || server.connect(),
                    notify,
                )?)
            }
        }
    }
}

/// What saving has come to, as the page shows it.
#[repr(u8)]
#[derive(Clone, Copy)]
pub(crate) enum Status {
    Saved = 1,
    Saving = 2,
    /// The file cannot be reached; edits wait in the cache.
    Offline = 3,
    /// An edit was refused or the replica failed.
    NotSaving = 4,
}

/// An open section as its page views share it.
pub(crate) struct Shared {
    pub section: session::Section,
    /// Who the edits name, as OneNote names the Office user.
    pub author: String,
    pub status: AtomicU8,
}

impl Shared {
    pub fn apply(&self, edit: Edit) -> Result<()> {
        self.section.apply(&self.author, edit)?;
        self.status.store(Status::Saving as u8, Ordering::Relaxed);
        Ok(())
    }

    /// The page in `space` as a view shows it: a conflict page highlighted and read-only.
    pub fn page(&self, space: ExGuid) -> Result<(Page, bool)> {
        let page = self.section.page(space)?;
        let conflict = self
            .section
            .conflicts()?
            .into_iter()
            .flat_map(|(_, versions)| versions)
            .find(|version| version.space == space);
        Ok(match conflict {
            Some(version) => (canvas::conflict::highlighted(page, &version.objects), true),
            None => (page, false),
        })
    }
}

/// A section open for editing, with the notebook it belongs to.
pub struct Section {
    pub(crate) shared: Arc<Shared>,
    pub(crate) library: Arc<Library>,
}

#[derive(serde::Serialize)]
pub(crate) struct Row {
    pub(crate) id: String,
    pub(crate) title: String,
    /// 1 at the top, 2 for a subpage and so on.
    pub(crate) level: u32,
    /// Conflict pages: versions with changes a merge could not take.
    pub(crate) versions: Vec<Version>,
}

#[derive(serde::Serialize)]
pub(crate) struct Version {
    pub(crate) id: String,
    /// Whose version it is.
    pub(crate) user: String,
    /// When the merge made it, in seconds since 1970.
    pub(crate) created: Option<i64>,
}

fn unix(filetime: u64) -> i64 {
    (filetime / 10_000_000) as i64 - 11_644_473_600
}

/// FILETIME now: when an edit happened, which its modification times record.
pub(crate) fn filetime() -> u64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(unix.subsec_nanos() / 100)
}

/// A page OneNote 2010 would create now, titled `date` and `time` as the host formats
/// them: the long date and the short time; titled in the Default font's face.
fn dated(author: &str, date: &str, time: &str) -> Result<PageCreation> {
    let font = crate::options().1;
    Ok(PageCreation::new(None, Some(""), author)?
        .titled_in(&font.face, font.color)?
        .dated(date, time)?)
}

impl Section {
    /// The notebook's themes; none for a lone section.
    pub(crate) fn themes(&self) -> notebook::sidecar::themes::Themes {
        let notebook = self
            .library
            .notebook
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        notebook
            .as_ref()
            .and_then(|notebook| crate::report(notebook.themes().map_err(Into::into)))
            .unwrap_or_default()
    }

    /// Merges `change` into the notebook's themes.
    pub(crate) fn save_themes(&self, change: notebook::sidecar::themes::Themes) -> Result<()> {
        let notebook = self
            .library
            .notebook
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        notebook
            .as_ref()
            .ok_or("A section on its own keeps no themes")?
            .save_themes(change)?;
        Ok(())
    }

    /// The theme scopes of page `id`: the page, its section and the notebook, where known.
    pub(crate) fn scopes(&self, id: &str) -> Result<[Option<notebook::sidecar::themes::Scope>; 3]> {
        use notebook::sidecar::themes::Scope;
        let (page, _) = self.shared.page(id.parse()?)?;
        Ok([
            page.identity.map(Scope::page),
            self.shared.section.identity().ok().map(Scope::section),
            Some(Scope::Notebook),
        ])
    }

    /// The theme page `identity` of this section wears, if any scope names one.
    pub(crate) fn theme(
        &self,
        identity: Option<[u8; 16]>,
    ) -> Option<notebook::sidecar::themes::Theme> {
        let section = self.shared.section.identity().ok();
        self.themes().effective(section, identity)
    }

    /// The section at catalog `path` of `library`, its edits naming `author`.
    pub(crate) fn open(
        library: Arc<Library>,
        path: &str,
        author: String,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            section: library.section(path, notify)?,
            author,
            status: AtomicU8::new(0),
        });
        let mut open = library
            .open
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        open.push((path.to_owned(), Arc::downgrade(&shared)));
        drop(open);
        Ok(Self { shared, library })
    }

    pub(crate) fn rows(&self) -> Result<Vec<Row>> {
        let section = &self.shared.section;
        let conflicts = section.conflicts()?;
        Ok(section
            .pages()?
            .into_iter()
            .map(|(space, title, level)| Row {
                id: space.to_string(),
                // Titles keep OneNote's line breaks, which a one-line list shows as spaces.
                title: title.replace(|char: char| char.is_control(), " "),
                level,
                versions: conflicts
                    .iter()
                    .find(|(page, _)| *page == space)
                    .map(|(_, versions)| {
                        versions
                            .iter()
                            .map(|version| Version {
                                id: version.space.to_string(),
                                user: version.user.clone(),
                                created: version.created.map(unix),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect())
    }

    /// Adds a page at the end, or a subpage after `parent` and its subpages, as OneNote's
    /// New Page and New Subpage do; returns its space.
    pub(crate) fn new_page(
        &self,
        parent: Option<ExGuid>,
        date: &str,
        time: &str,
    ) -> Result<ExGuid> {
        let creation = dated(&self.shared.author, date, time)?;
        let space = creation.space();
        let mut ops = vec![Op::Section(SectionOp::Create(creation))];
        if let Some(parent) = parent {
            let pages = self.shared.section.pages()?;
            let at = pages
                .iter()
                .position(|(space, ..)| *space == parent)
                .ok_or("The page is not listed")?;
            let level = pages[at].2;
            let next = pages[at + 1..]
                .iter()
                .find(|(.., below)| *below <= level)
                .map(|(space, ..)| *space);
            ops.push(Op::Section(SectionOp::Pages(vec![PageEdit::move_to(
                space,
                next,
                (level + 1).min(3),
            )?])));
        }
        self.shared.apply(Edit {
            at: filetime(),
            ops,
        })?;
        Ok(space)
    }

    /// Deletes a page as OneNote 2010 does, keeping a copy in the notebook's recycle bin;
    /// a conflict page goes for good. A section left empty gets a new page.
    pub(crate) fn delete_page(&self, space: ExGuid, date: &str, time: &str) -> Result<()> {
        let section = &self.shared.section;
        let listed = section.pages()?;
        let mut ops = vec![Op::Section(SectionOp::Delete(vec![space]))];
        if listed.iter().any(|(page, ..)| *page == space) {
            if self.library.notebook().is_some() {
                let page = section.page(space)?;
                let author = &self.shared.author;
                self.library.with_notebook(true, |notebook| {
                    Ok(notebook.recycle_pages(std::slice::from_ref(&page), author)?)
                })?;
            }
            if listed.len() == 1 {
                ops.push(Op::Section(SectionOp::Create(dated(
                    &self.shared.author,
                    date,
                    time,
                )?)));
            }
        }
        self.shared.apply(Edit {
            at: filetime(),
            ops,
        })
    }

    /// Applies the events since the last poll to the shared status.
    pub(crate) fn poll(&self) -> u32 {
        let mut flags = 0;
        for event in self.shared.section.events() {
            let status = match event {
                Event::Changed(_) => {
                    flags |= LISTED | CHANGED;
                    continue;
                }
                Event::Rejected { error, .. } => {
                    eprintln!("snowbound: saving failed: {error}");
                    flags |= REJECTED;
                    Status::NotSaving
                }
                Event::Attempt {
                    status: notebook::EditStatus::Published { .. },
                    ..
                } => Status::Saved,
                Event::Attempt { .. } => Status::Saving,
                Event::Unreachable(error) => {
                    eprintln!("snowbound: the section cannot be reached: {error}");
                    Status::Offline
                }
                Event::Failed(error) => {
                    eprintln!("snowbound: synchronization stopped: {error}");
                    Status::NotSaving
                }
            };
            self.shared.status.store(status as u8, Ordering::Relaxed);
        }
        flags
    }

    /// Waits until every edit is in the cache, then publishes them within `limit`: true when
    /// nothing waits to be published.
    pub(crate) fn flush(&self, limit: Duration) -> bool {
        let section = &self.shared.section;
        if report(section.written().map_err(Into::into)).is_none() {
            return false;
        }
        let deadline = Instant::now() + limit;
        loop {
            match section.pending() {
                Ok(pending) if pending.is_empty() => return true,
                Ok(_) if Instant::now() < deadline => {
                    section.wake();
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => return false,
            }
        }
    }
}

/// `sb_section_poll`: the page list or conflict pages changed.
const LISTED: u32 = 1;
/// A change made elsewhere reached the section; open pages reload.
const CHANGED: u32 = 2;
/// The section refused an edit; the page shown returns to what is stored.
const REJECTED: u32 = 4;

#[derive(serde::Serialize)]
pub(crate) struct Found {
    /// The section's catalog path.
    pub(crate) section: String,
    pub(crate) page: String,
    pub(crate) title: String,
    /// The line around the first match.
    pub(crate) snippet: String,
}

impl Library {
    /// Pages of the notebook holding every word of `query` in their title or text, as the
    /// desktop searches.
    pub(crate) fn search(&self, open: Option<(&str, &Section)>, query: &str) -> Result<Vec<Found>> {
        self.indexed(open, |index| {
            index
                .search(&Query::new(query), |_| true)
                .into_iter()
                .map(|found| Found {
                    section: found.section,
                    page: found.space.to_string(),
                    title: found.title.replace(|char: char| char.is_control(), " "),
                    snippet: found.snippet,
                })
                .collect()
        })
    }

    /// The notebook's tagged paragraphs in page order, as OneNote's Tags Summary lists
    /// them: once for each of their tags.
    pub(crate) fn tagged(&self, open: Option<(&str, &Section)>) -> Result<Vec<Tag>> {
        self.indexed(open, |index| {
            index
                .tagged(|_| true)
                .into_iter()
                .map(|tagged: Tagged| Tag {
                    section: tagged.section,
                    page: tagged.space.to_string(),
                    title: tagged.title.replace(|char: char| char.is_control(), " "),
                    paragraph: tagged.paragraph.to_string(),
                    name: tagged.name,
                    shape: tagged.shape,
                    checked: tagged.checked,
                    text: tagged.text,
                })
                .collect()
        })
    }

    /// `read` over the notebook's pages: the open section as its edits leave it, others as
    /// stored, read again when their file changed.
    fn indexed<T>(
        &self,
        open: Option<(&str, &Section)>,
        read: impl FnOnce(&Index) -> T,
    ) -> Result<T> {
        let tabs: Vec<String> = self
            .tabs()?
            .into_iter()
            .filter(|tab| tab.readable)
            .map(|tab| tab.path)
            .collect();
        let mut indexed = self.index.lock().unwrap_or_else(|error| error.into_inner());
        let (checked, index) = &mut *indexed;
        for path in &tabs {
            match open.filter(|(open, _)| open == path) {
                Some((_, section)) => {
                    let section = &section.shared.section;
                    let mut entries = Vec::new();
                    for (space, ..) in section.pages()? {
                        let modified = index.get(path, space).map_or(0, |entry| entry.modified);
                        entries.push(Entry::new(path, space, &section.page(space)?, modified));
                    }
                    index.retain(|entry| entry.section != *path);
                    entries.into_iter().for_each(|entry| index.set(entry));
                    checked.remove(path);
                }
                None => {
                    report(self.reindex(path, checked, index));
                }
            }
        }
        index.retain(|entry| tabs.contains(&entry.section));
        Ok(read(index))
    }

    /// Each section's sync status in the notebook's order: an open section's from its
    /// session, the others' from the background.
    pub(crate) fn sync_status(&self) -> Vec<Synced> {
        let mut sections = self
            .background
            .as_ref()
            .map(Background::status)
            .unwrap_or_default();
        for (path, shared) in self.open_sections() {
            let Some(status) = report(shared.section.sync_status().map_err(Into::into)) else {
                continue;
            };
            match sections.iter_mut().find(|(listed, _)| *listed == path) {
                Some((_, listed)) => *listed = status,
                None => sections.push((path, status)),
            }
        }
        sections
            .into_iter()
            .map(|(path, status): (String, SyncStatus)| Synced {
                state: status.state() as u8,
                synced: status.synced.map(unix),
                queued: status.queued,
                error: status.error.map(|error| error.to_string()),
                path,
            })
            .collect()
    }

    /// Publishes every edit the notebook's replicas hold within `limit`, then lets go of the
    /// notebook as OneNote closes one: nothing syncs from then on, and each replica holding
    /// nothing unpublished is deleted once no session holds it. An error leaves it syncing.
    pub(crate) fn close(&self, limit: Duration) -> Result<()> {
        const WAITING: &str = "Some changes haven’t been saved to the notebook yet.";
        let deadline = Instant::now() + limit;
        let open = self.open_sections();
        for (_, shared) in &open {
            while !shared.section.pending()?.is_empty() {
                if Instant::now() >= deadline {
                    return Err(WAITING.into());
                }
                shared.section.wake();
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let Some(background) = &self.background else {
            return Ok(());
        };
        let before: HashMap<String, Option<u64>> = background
            .status()
            .into_iter()
            .map(|(path, status)| (path, status.synced))
            .collect();
        // Sections without a replica hold nothing to publish.
        let replicas: Vec<&String> = {
            let notebook = self.notebook();
            before
                .keys()
                .filter(|path| !open.iter().any(|(held, _)| held == *path))
                .filter(|path| {
                    notebook
                        .as_ref()
                        .and_then(|notebook| notebook.replica_path(path).ok())
                        .is_some_and(|replica| replica.exists())
                })
                .collect()
        };
        loop {
            // Working offline, the background checks only when woken.
            background.wake();
            let status = background.status();
            let mut waiting = replicas.iter().filter_map(|path| {
                match status.iter().find(|(listed, _)| listed == *path) {
                    Some((_, status))
                        if status.queued == 0
                            && status.error.is_none()
                            && status.synced > before[*path] =>
                    {
                        None
                    }
                    Some((_, status)) => Some(status.error.as_ref().map(ToString::to_string)),
                    None => Some(None),
                }
            });
            let Some(error) = waiting.next() else { break };
            if Instant::now() >= deadline {
                return Err(error.unwrap_or_else(|| WAITING.to_owned()).into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        background.discard();
        Ok(())
    }

    fn open_sections(&self) -> Vec<(String, Arc<Shared>)> {
        let mut open = self.open.lock().unwrap_or_else(|error| error.into_inner());
        open.retain(|(_, shared)| shared.strong_count() > 0);
        open.iter()
            .filter_map(|(path, shared)| Some((path.clone(), shared.upgrade()?)))
            .collect()
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Synced {
    pub(crate) path: String,
    /// `SyncState` by its place in the enum, best first.
    pub(crate) state: u8,
    /// When the section file was last reached, in seconds since 1970.
    pub(crate) synced: Option<i64>,
    /// Changes the file does not hold yet.
    pub(crate) queued: u64,
    pub(crate) error: Option<String>,
}

/// A tagged paragraph, from `sb_library_tagged`.
#[derive(serde::Serialize)]
pub(crate) struct Tag {
    pub(crate) section: String,
    pub(crate) page: String,
    pub(crate) title: String,
    pub(crate) paragraph: String,
    pub(crate) name: String,
    pub(crate) shape: u16,
    pub(crate) checked: bool,
    pub(crate) text: String,
}

impl Library {
    /// Reads the section at catalog `path` into `index` again when its file changed, and
    /// checks at most every 30 seconds.
    fn reindex(
        &self,
        path: &str,
        checked: &mut HashMap<String, Checked>,
        index: &mut Index,
    ) -> Result<()> {
        if checked
            .get(path)
            .is_some_and(|known| known.at.elapsed() < Duration::from_secs(30))
        {
            return Ok(());
        }
        let (stamp, read): (Vec<u8>, Reader) = match &self.place {
            Place::Share { root, .. } => {
                let client = self.client().ok_or("Not a share")?;
                let file = if root.is_empty() {
                    path.to_owned()
                } else {
                    format!("{root}/{path}")
                };
                let stamp = client.stamp(&file)?;
                let mut key = stamp.header.to_vec();
                key.extend(stamp.length.to_le_bytes());
                (
                    key,
                    Box::new(move || Ok(client.read_storage(&file, LIMIT)?)),
                )
            }
            Place::Folder(root) => local(root.join(path))?,
            Place::File(file) => local(file.clone())?,
        };
        if checked.get(path).is_none_or(|known| known.stamp != stamp) {
            let pages = session::stored_pages(&read()?)?;
            index.retain(|entry| entry.section != path);
            for stored in pages {
                let modified = stored.modified.map_or(0, u64::from);
                index.set(Entry::new(path, stored.space, &stored.page, modified));
            }
        }
        checked.insert(
            path.to_owned(),
            Checked {
                at: Instant::now(),
                stamp,
            },
        );
        Ok(())
    }
}

type Reader = Box<dyn Fn() -> Result<Vec<u8>>>;

/// A local file's change key, its length and modification time, with its reader.
fn local(file: PathBuf) -> Result<(Vec<u8>, Reader)> {
    let metadata = std::fs::metadata(&file)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let mut key = metadata.len().to_le_bytes().to_vec();
    key.extend(modified.as_nanos().to_le_bytes());
    Ok((
        key,
        Box::new(move || Ok(coordinated(&file, false, || onestore::read_file(&file))??)),
    ))
}

/// Writes `error` for the host to show, freed with `sb_string_free`.
fn failed(error: Box<dyn std::error::Error>, out: *mut *mut c_char) {
    eprintln!("snowbound: {error}");
    if !out.is_null() {
        // SAFETY: the caller passes null or a place for one string.
        unsafe { out.write(owned(error.to_string())) };
    }
}

fn boxed<T>(result: Result<T>, error: *mut *mut c_char) -> *mut T {
    match result {
        Ok(value) => Box::into_raw(Box::new(value)),
        Err(cause) => {
            failed(cause, error);
            std::ptr::null_mut()
        }
    }
}

pub(crate) fn json(value: Result<impl serde::Serialize>) -> *mut c_char {
    report(value.and_then(|value| Ok(serde_json::to_string(&value)?)))
        .map_or(std::ptr::null_mut(), owned)
}

pub(crate) fn optional(text: *const c_char) -> Option<String> {
    (!text.is_null()).then(|| string(text))
}

fn space(text: *const c_char) -> Result<ExGuid> {
    Ok(string(text).parse()?)
}

/// The notebook folder or lone section file at `path`, its section replicas kept in the
/// directory `cache`; null with `error` set if it cannot be read. A folder `local` to this
/// device has the host report its changes to `sb_library_touched`; any other is checked
/// every few seconds and kept in offline copies.
///
/// # Safety
/// `path` and `cache` are NUL-terminated UTF-8; `error` is null or a place for a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_library_open(
    path: *const c_char,
    cache: *const c_char,
    local: bool,
    error: *mut *mut c_char,
) -> *const Library {
    match Library::open(Path::new(&string(path)), Path::new(&string(cache)), local) {
        Ok(library) => Arc::into_raw(Arc::new(library)),
        Err(cause) => {
            failed(cause, error);
            std::ptr::null()
        }
    }
}

/// The notebook folder `root`, `/`-separated from the top of `share` on the server at
/// `address` (`host` or `host:port`), opened as `user` or as a guest when `user` is empty.
/// While the server cannot be reached, a notebook listed before opens with its sections as
/// last listed. Blocks for up to ten seconds.
///
/// # Safety
/// Every string is NUL-terminated UTF-8; `error` is null or a place for a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_library_server(
    address: *const c_char,
    share: *const c_char,
    user: *const c_char,
    password: *const c_char,
    domain: *const c_char,
    root: *const c_char,
    cache: *const c_char,
    error: *mut *mut c_char,
) -> *const Library {
    let server = server(address, share, user, password, domain);
    match Library::server(server, &string(root), Path::new(&string(cache))) {
        Ok(library) => Arc::into_raw(Arc::new(library)),
        Err(cause) => {
            failed(cause, error);
            std::ptr::null()
        }
    }
}

/// Creates the notebook folder `path` holding one section with one page titled with `date`
/// and `time`, as OneNote's New Notebook does; `sb_library_open` opens it. False, with
/// `error` set, when it cannot be made.
///
/// # Safety
/// Every string is NUL-terminated UTF-8; `error` is null or a place for a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_notebook_create(
    path: *const c_char,
    cache: *const c_char,
    author: *const c_char,
    date: *const c_char,
    time: *const c_char,
    error: *mut *mut c_char,
) -> bool {
    let created = dated(&string(author), &string(date), &string(time)).and_then(|page| {
        Notebook::create(string(path), string(cache), Notebook::NEW_COLOR, &page)?;
        Ok(())
    });
    created.map_err(|cause| failed(cause, error)).is_ok()
}

/// Moves the replicas of the notebook folder or lone section file the app has just moved
/// from `from` to `to`, so that its edits waiting there still publish. False, with `error`
/// set, when they cannot be moved.
///
/// # Safety
/// Every string is NUL-terminated UTF-8; `error` is null or a place for a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_notebook_moved(
    cache: *const c_char,
    from: *const c_char,
    to: *const c_char,
    error: *mut *mut c_char,
) -> bool {
    let moved = || -> Result<()> {
        let from = notebook::location::local(Path::new(&string(from)))?;
        let to = notebook::location::local(Path::new(&string(to)))?;
        Ok(notebook::location::moved(
            Path::new(&string(cache)),
            &from,
            &to,
        )?)
    };
    moved().map_err(|cause| failed(cause, error)).is_ok()
}

/// The notebook's sections read again, as JSON: each with `name`, `path`, `group`,
/// `color` as sRGB bytes, `readable`, `downloading` and `problem`, in the notebook's order; null
/// if it cannot be read.
#[unsafe(no_mangle)]
pub extern "C" fn sb_library_sections(library: &Library) -> *mut c_char {
    json(library.tabs())
}

/// Creates a section named `name` in the group at catalog path `folder` ("" for the
/// notebook's top) holding one page titled with `date` and `time`, as OneNote creates one;
/// returns its catalog path, or null.
///
/// # Safety
/// Every string is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_library_new_section(
    library: &Library,
    folder: *const c_char,
    name: *const c_char,
    author: *const c_char,
    date: *const c_char,
    time: *const c_char,
) -> *mut c_char {
    let (folder, name) = (string(folder), string(name));
    let page = dated(&string(author), &string(date), &string(time));
    report(page.and_then(|page| {
        library.with_notebook(true, |notebook| {
            Ok(notebook.create_section(&folder, &name, &page)?)
        })
    }))
    .map_or(std::ptr::null_mut(), owned)
}

/// Pages of the notebook whose title or text holds `query`, as JSON: each with `section`
/// (a catalog path), `page`, `title` and `snippet`. `open` is the section being edited, at
/// catalog path `path`, or null. Reads the notebook's sections, so call it off the main
/// thread.
///
/// # Safety
/// `query` and a non-null `path` are NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_library_search(
    library: &Library,
    open: Option<&Section>,
    path: *const c_char,
    query: *const c_char,
) -> *mut c_char {
    let path = optional(path).unwrap_or_default();
    json(library.search(open.map(|section| (path.as_str(), section)), &string(query)))
}

/// The notebook's tagged paragraphs in page order as JSON, once for each of their tags:
/// each with `section`, `page`, `title`, `paragraph`, `name`, `shape` (the tag's symbol, 0
/// for a highlighting tag), `checked` and `text`. `open` and `path` as `sb_library_search`
/// takes them; reads the notebook's sections, so call it off the main thread.
///
/// # Safety
/// A non-null `path` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_library_tagged(
    library: &Library,
    open: Option<&Section>,
    path: *const c_char,
) -> *mut c_char {
    let path = optional(path).unwrap_or_default();
    json(library.tagged(open.map(|section| (path.as_str(), section))))
}

/// Each section's sync status as JSON, in the notebook's order: `path`, `state` (0 up to
/// date, 1 syncing, 2 in use elsewhere, 3 not connected, 4 read-only, 5 failed), `synced`
/// (seconds since 1970, or null before the file was reached), `queued` changes and `error`.
#[unsafe(no_mangle)]
pub extern "C" fn sb_library_sync_status(library: &Library) -> *mut c_char {
    json(Ok(library.sync_status()))
}

/// Works offline, or online again: edits wait in the cache until then or until
/// `sb_library_sync_now`. Sections opened later follow.
#[unsafe(no_mangle)]
pub extern "C" fn sb_library_set_offline(library: &Library, offline: bool) {
    library.offline.store(offline, Ordering::Relaxed);
    if let Some(background) = &library.background {
        background.set_offline(offline);
    }
    for (_, shared) in library.open_sections() {
        shared.section.set_offline(offline);
    }
}

/// Checks the sections at or below `path`, `/`-separated from the notebook's folder, as the
/// host's watch on a local folder reports them changed; `""` names the folder.
///
/// # Safety
/// `path` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_library_touched(library: &Library, path: *const c_char) {
    if let Some(background) = &library.background {
        background.touched(&[string(path)]);
    }
}

/// Syncs every section of the notebook now, working offline included.
#[unsafe(no_mangle)]
pub extern "C" fn sb_library_sync_now(library: &Library) {
    if let Some(background) = &library.background {
        background.wake();
    }
    for (_, shared) in library.open_sections() {
        shared.section.wake();
    }
}

/// Publishes every edit the notebook holds within `seconds`, then lets go of it
/// (`Library::close`) so its folder can be moved or deleted: false, with `error` set, while
/// an edit is still unpublished, and the notebook syncs on.
///
/// # Safety
/// `error` is null or a place for a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_library_close(
    library: &Library,
    seconds: f64,
    error: *mut *mut c_char,
) -> bool {
    library
        .close(Duration::from_secs_f64(seconds))
        .map_err(|cause| failed(cause, error))
        .is_ok()
}

/// # Safety
/// `library` came from `sb_library_open` or `sb_library_server` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_library_free(library: *const Library) {
    drop(unsafe { Arc::from_raw(library) });
}

/// Connects to `share` on the server at `address` (`host` or `host:port`) as `user`, or
/// as a guest when `user` is empty. Blocks for up to ten seconds.
///
/// # Safety
/// Every string is NUL-terminated UTF-8; `error` is null or a place for a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_share_connect(
    address: *const c_char,
    share: *const c_char,
    user: *const c_char,
    password: *const c_char,
    domain: *const c_char,
    error: *mut *mut c_char,
) -> *mut Share {
    let server = server(address, share, user, password, domain);
    boxed(server.connect().map_err(Into::into).map(Share), error)
}

fn server(
    address: *const c_char,
    share: *const c_char,
    user: *const c_char,
    password: *const c_char,
    domain: *const c_char,
) -> Server {
    let mut address = string(address);
    if !address.contains(':') {
        address.push_str(":445");
    }
    Server {
        address,
        share: string(share),
        user: string(user),
        password: string(password),
        domain: string(domain),
    }
}

#[derive(serde::Serialize)]
struct Listing {
    /// The folder holds a notebook's table of contents.
    notebook: bool,
    /// Folders, then section files, by name.
    folders: Vec<String>,
    sections: Vec<String>,
}

/// The folder at `path` on the share, as JSON: `notebook`, `folders` and `sections`.
///
/// # Safety
/// `path` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_share_list(share: &Share, path: *const c_char) -> *mut c_char {
    json(
        share
            .0
            .read_dir(&string(path), 100_000)
            .map_err(Into::into)
            .map(|entries| {
                let extension = |name: &str, wanted: &str| {
                    Path::new(name)
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
                };
                // Hidden and system folders, OneNote's recycle bin among them, stay out.
                let shown = |name: &str| !name.starts_with('.') && name != "OneNote_RecycleBin";
                let mut listing = Listing {
                    notebook: entries
                        .iter()
                        .any(|entry| extension(&entry.name, "onetoc2")),
                    folders: Vec::new(),
                    sections: Vec::new(),
                };
                for entry in entries.into_iter().filter(|entry| shown(&entry.name)) {
                    if entry.attributes & 0x10 != 0 {
                        listing.folders.push(entry.name);
                    } else if extension(&entry.name, "one") {
                        listing.sections.push(entry.name);
                    }
                }
                listing.folders.sort_by_key(|name| name.to_lowercase());
                listing.sections.sort_by_key(|name| name.to_lowercase());
                listing
            }),
    )
}

/// # Safety
/// `share` came from `sb_share_connect` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_share_free(share: *mut Share) {
    drop(unsafe { Box::from_raw(share) });
}

/// Called on a background thread with the token `sb_section_open` took when the section
/// has events for `sb_section_poll`.
pub type Wake = extern "C" fn(token: usize);

/// Opens the section at catalog `path` of `library` for editing, its edits naming
/// `author`. Reads the whole section the first time, so call it off the main thread.
///
/// # Safety
/// `path` and `author` are NUL-terminated UTF-8; `error` is null or a place for a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_open(
    library: &Library,
    path: *const c_char,
    author: *const c_char,
    wake: Wake,
    token: usize,
    error: *mut *mut c_char,
) -> *mut Section {
    // SAFETY: the host keeps the library alive while this call runs.
    let library = unsafe {
        Arc::increment_strong_count(library);
        Arc::from_raw(library as *const Library)
    };
    boxed(
        Section::open(library, &string(path), string(author), move || wake(token)),
        error,
    )
}

/// The section's pages in order as JSON: each with `id`, `title`, `level` and `versions`,
/// its conflict pages, each with `id`, `user` and `created` (seconds since 1970, or null).
#[unsafe(no_mangle)]
pub extern "C" fn sb_section_pages(section: &Section) -> *mut c_char {
    json(section.rows())
}

/// Takes the section's events: `LISTED` (1) the page list or conflict pages changed,
/// `CHANGED` (2) a change made elsewhere arrived, so open pages reload; `REJECTED` (4) an
/// edit was refused. Writes the saving status: 1 saved, 2 saving, 3 offline, 4 not saving,
/// or 0 before anything happened.
#[unsafe(no_mangle)]
pub extern "C" fn sb_section_poll(section: &Section, status: &mut u8) -> u32 {
    let flags = section.poll();
    *status = section.shared.status.load(Ordering::Relaxed);
    flags
}

/// Adds a page, or with `parent` a subpage beneath that page, titled with `date` and
/// `time`; returns its id, or null.
///
/// # Safety
/// `date` and `time` are NUL-terminated UTF-8; `parent` too unless null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_new_page(
    section: &Section,
    parent: *const c_char,
    date: *const c_char,
    time: *const c_char,
) -> *mut c_char {
    let page = optional(parent)
        .map(|parent| Ok::<_, Box<dyn std::error::Error>>(parent.parse()?))
        .transpose()
        .and_then(|parent| section.new_page(parent, &string(date), &string(time)));
    report(page).map_or(std::ptr::null_mut(), |space| owned(space.to_string()))
}

/// The notebook's themes for page `id` as JSON: `themes`, each with `id` and `name`, then
/// the id each of `page`, `section` and `notebook` names itself, or null.
///
/// # Safety
/// `id` is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_themes(section: &Section, id: *const c_char) -> *mut c_char {
    use notebook::sidecar::themes::Scope;
    #[derive(serde::Serialize)]
    struct Listed {
        id: String,
        name: String,
    }
    #[derive(serde::Serialize)]
    struct Shown {
        themes: Vec<Listed>,
        page: Option<String>,
        section: Option<String>,
        notebook: Option<String>,
    }
    let shown = (|| {
        let scopes = section.scopes(&string(id))?;
        let themes = section.themes();
        let named = |scope: &Option<Scope>| themes.assigned(scope.as_ref()?).map(|theme| theme.id);
        Ok(Shown {
            themes: themes
                .all()
                .into_iter()
                .map(|theme| Listed {
                    id: theme.id,
                    name: theme.name,
                })
                .collect(),
            page: named(&scopes[0]),
            section: named(&scopes[1]),
            notebook: named(&scopes[2]),
        })
    })();
    json(shown)
}

/// Gives theme `theme` (null for none of its own) to page `id` (`scope` 0), its section (1)
/// or its notebook (2); the page wears it when next opened. Writes to the notebook, so call
/// it off the main thread.
///
/// # Safety
/// Every string is NUL-terminated UTF-8; `theme` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_set_theme(
    section: &Section,
    id: *const c_char,
    scope: u8,
    theme: *const c_char,
) -> bool {
    use notebook::sidecar::themes::{Assignment, Themes};
    let set = (|| {
        let scope = section
            .scopes(&string(id))?
            .into_iter()
            .nth(usize::from(scope))
            .flatten()
            .ok_or("No such scope")?;
        section.save_themes(Themes {
            assignments: vec![Assignment {
                scope,
                theme: optional(theme),
                assigned: filetime(),
            }],
            ..Default::default()
        })
    })();
    report(set).is_some()
}

/// Deletes page `id` to the notebook's recycle bin, or a conflict page for good; a section
/// left without pages gets one titled `date` and `time`. Writes to the notebook, so call it
/// off the main thread.
///
/// # Safety
/// Every string is NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_delete_page(
    section: &Section,
    id: *const c_char,
    date: *const c_char,
    time: *const c_char,
) -> bool {
    report(space(id).and_then(|space| section.delete_page(space, &string(date), &string(time))))
        .is_some()
}

/// Waits for every edit to be stored in the cache, then up to `seconds` for them to be
/// published; true when nothing waits. For the host's background time.
#[unsafe(no_mangle)]
pub extern "C" fn sb_section_flush(section: &Section, seconds: f64) -> bool {
    section.flush(Duration::from_secs_f64(seconds))
}

/// Asks the section to look for changes made elsewhere now, as when the app returns.
#[unsafe(no_mangle)]
pub extern "C" fn sb_section_wake(section: &Section) {
    section.shared.section.wake();
}

/// # Safety
/// `section` came from `sb_section_open` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_section_free(section: *mut Section) {
    drop(unsafe { Box::from_raw(section) });
}
