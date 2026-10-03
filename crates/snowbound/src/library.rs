//! Open notebooks: where each lives and the sections its tabs offer.

use canvas::{editor::NoteTag, gpu::TagArt};
use notebook::discover::{Folder, SectionState};
use notebook::session::{Background, Notebook, Section};
use notebook::sidecar::themes::Themes;
#[cfg(not(target_arch = "wasm32"))]
use notebook::smb::{Client, Credentials};
use onestore::protected::Key;
use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use web_time::Instant;

/// OneNote's Work Offline, which like OneNote's holds for every notebook.
static OFFLINE: AtomicBool = AtomicBool::new(false);

pub fn offline() -> bool {
    OFFLINE.load(Ordering::Relaxed)
}

/// Works offline or online again; sections opened from now on follow.
pub fn set_offline(offline: bool) {
    OFFLINE.store(offline, Ordering::Relaxed);
}

/// Wakes the app when a notebook's closed sections report, once set.
static NOTIFY: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

pub fn on_background(notify: impl Fn() + Send + Sync + 'static) {
    let _ = NOTIFY.set(Box::new(notify));
}

pub(crate) fn notify_background() {
    if let Some(notify) = NOTIFY.get() {
        notify();
    }
}

/// What the location of a notebook another computer shares by Live Share starts with.
pub const SHARED: &str = "live://";

/// How long an SMB request may take before the share counts as unreachable.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The largest section file read whole over SMB.
const LIMIT: usize = 256 * 1024 * 1024;

/// A folder on a mounted SMB share: where the embedded client finds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mount {
    pub server: String,
    pub share: String,
    /// The account the mount signed in as, where the mount names one.
    pub user: Option<String>,
    pub domain: String,
    /// The folder within the share, `/`-separated.
    pub root: String,
}

/// Where `path` sits below the mount point `point`, matched on its resolved path since
/// the system reports mount points with symlinks (`/tmp`, `/var`) resolved.
#[cfg(unix)]
pub fn within_mount(path: &std::path::Path, point: &str) -> Option<String> {
    let path = notebook::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    Some(
        path.strip_prefix(point)
            .ok()?
            .to_string_lossy()
            .into_owned(),
    )
}

impl Mount {
    /// The mount of `source` (`//[domain;][user[:…]@]server/share[/folder]`), holding the
    /// folder `within` its mount point; `options` are the mount's (`username=`, `user=`,
    /// `domain=`), where the source names no account.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn parse(source: &str, within: &str, options: &str) -> Option<Self> {
        let source = source.strip_prefix("//")?;
        let (account, rest) = match source.rsplit_once('@') {
            Some((account, rest)) => (Some(account), rest),
            None => (None, source),
        };
        let mut parts = rest.split('/').filter(|part| !part.is_empty());
        let (server, share) = (parts.next()?, parts.next()?);
        let option = |names: &[&str]| {
            options.split(',').find_map(|option| {
                let (name, value) = option.split_once('=')?;
                names.contains(&name).then(|| value.to_owned())
            })
        };
        let (domain, user) = match account {
            Some(account) => {
                let account = account.split(':').next().unwrap_or_default();
                match account.split_once(';') {
                    Some((domain, user)) => (Some(domain.to_owned()), user.to_owned()),
                    None => (None, account.to_owned()),
                }
            }
            None => (None, option(&["username", "user"]).unwrap_or_default()),
        };
        let root: Vec<&str> = parts
            .chain(within.split('/'))
            .filter(|part| !part.is_empty())
            .collect();
        Some(Self {
            server: server.to_owned(),
            share: share.to_owned(),
            // "GUEST" is how macOS names a guest mount.
            user: (!user.is_empty() && !user.eq_ignore_ascii_case("guest")).then_some(user),
            domain: domain
                .or_else(|| option(&["domain", "dom"]))
                .unwrap_or_default(),
            root: root.join("/"),
        })
    }

    /// The folder an address typed or kept names: `smb://[domain;][user@]server[:port]/share/folder`,
    /// `\\server\share\folder`, or either without its scheme; the share may be left to choose.
    pub fn from_address(text: &str) -> Option<Self> {
        let text = text.trim();
        let rest = match text.get(..6) {
            Some(scheme) if scheme.eq_ignore_ascii_case("smb://") => &text[6..],
            _ => text.trim_start_matches(['/', '\\']),
        }
        .replace('\\', "/");
        let (authority, path) = rest.split_once('/').unwrap_or((&rest, ""));
        let (account, server) = match authority.rsplit_once('@') {
            Some((account, server)) => (account, server),
            None => ("", authority),
        };
        if server.is_empty() || server.contains(char::is_whitespace) {
            return None;
        }
        let account = account.split(':').next().unwrap_or_default();
        let (domain, user) = account.split_once(';').unwrap_or(("", account));
        let mut parts = path.split('/').filter(|part| !part.is_empty()).map(decode);
        let user = decode(user);
        Some(Self {
            server: server.to_owned(),
            share: parts.next().unwrap_or_default(),
            user: (!user.is_empty() && !user.eq_ignore_ascii_case("guest")).then_some(user),
            domain: decode(domain),
            root: parts.collect::<Vec<_>>().join("/"),
        })
    }

    /// The address `from_address` reads back, naming the account but never a password.
    pub fn url(&self) -> String {
        let mut url = String::from("smb://");
        if let Some(user) = &self.user {
            if !self.domain.is_empty() {
                url += &format!("{};", encode(self.domain.as_bytes()));
            }
            url += &format!("{}@", encode(user.as_bytes()));
        }
        url += &self.server;
        for part in [&self.share, &self.root] {
            if !part.is_empty() {
                url += &format!("/{}", encode(part.as_bytes()));
            }
        }
        url
    }

    /// The server's name or address without a port.
    pub fn host(&self) -> &str {
        match self.server.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => host,
            _ => &self.server,
        }
    }

    /// Where the embedded client dials: the server, on SMB's port unless it names another;
    /// for a Bonjour service, where it answers now.
    pub fn endpoint(&self) -> String {
        if let Some(instance) = bonjour_instance(&self.server) {
            #[cfg(target_os = "macos")]
            if let Some(endpoint) = crate::platform::bonjour_endpoint(&instance) {
                return endpoint;
            }
            // Samba and macOS name their service after the host, which mDNS answers for.
            return format!("{instance}.local:445");
        }
        if self.host() == self.server {
            format!("{}:445", self.server)
        } else {
            self.server.clone()
        }
    }

    /// The notebook's name: its folder's, or the share's where it fills the share.
    fn name(&self) -> String {
        self.root
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(&self.share)
            .to_owned()
    }
}

/// The Bonjour SMB service `server` names, as the Finder mounts a server it browsed to.
fn bonjour_instance(server: &str) -> Option<String> {
    let instance = server
        .trim_end_matches('.')
        .strip_suffix("._smb._tcp.local")?;
    (!instance.is_empty()).then(|| decode(instance))
}

/// `text` with `%XX` escapes decoded.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match text
            .get(at + 1..at + 3)
            .filter(|_| bytes[at] == b'%')
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            Some(byte) => {
                decoded.push(byte);
                at += 3;
            }
            None => {
                decoded.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// `bytes` escaped for an address, outside RFC 3986's unreserved set and `/`.
pub(crate) fn encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                char::from(byte).to_string()
            }
            byte => format!("%{byte:02X}"),
        })
        .collect()
}

/// An account on an SMB server.
#[derive(Clone)]
pub struct Login {
    pub user: String,
    pub password: String,
    pub domain: String,
}

impl Login {
    /// The guest account a mount without one signed in as.
    pub fn guest(mount: &Mount) -> Self {
        Self {
            user: String::new(),
            password: String::new(),
            domain: mount.domain.clone(),
        }
    }
}

/// A share and the account that reaches it, kept to reconnect.
struct Server {
    mount: Mount,
    login: Login,
}

#[cfg(not(target_arch = "wasm32"))]
impl Server {
    fn connect(&self) -> io::Result<Client> {
        Client::connect(
            &self.mount.endpoint(),
            &self.mount.share,
            Credentials {
                username: &self.login.user,
                password: &self.login.password,
                domain: &self.login.domain,
            },
            TIMEOUT,
        )
    }
}

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
    /// The share a notebook on a mounted SMB share opens through, by the embedded client.
    server: Option<Arc<Server>>,
    /// Why a notebook on a mounted SMB share opened through the mount instead.
    pub notice: Option<String>,
    cache: PathBuf,
    /// Syncs the notebook's sections while no tab shows them, as OneNote syncs every
    /// section of an open notebook.
    pub background: Option<Arc<Background>>,
    /// Reports the changes to a mounted notebook's folder to `background`.
    watch: Option<Arc<crate::watch::Watch>>,
    /// The art the notebook's tags draw with, as its `.snowbound` folder maps them.
    tag_art: Mutex<Arc<TagArt>>,
    /// Sections opened ahead of being shown, or left open after, most recent first, which
    /// `open` hands out before opening any.
    kept: Mutex<crate::prefetch::Recent<String, Section>>,
    /// The notebook's style themes, and when they were read.
    themes: Mutex<Option<(Instant, Arc<Themes>)>>,
    /// The keys of its password-protected sections unlocked this run, kept as it is read again.
    keys: Arc<Keys>,
    /// The share a notebook another computer shares by Live Share is reached through.
    #[cfg(feature = "live")]
    pub joined: Option<Arc<crate::live::Joined>>,
}

/// Keys of password-protected sections, by file identity, with when each section was last
/// worked in; in memory only, each cleared as it goes.
#[derive(Default)]
pub struct Keys(Mutex<std::collections::HashMap<[u8; 16], (Key, Instant)>>);

impl Library {
    /// `location` named `name`, with no notebook read, server, sync or kept sections.
    fn new(location: &str, name: String, cache: &Path) -> Self {
        Self {
            location: location.to_owned(),
            name,
            notebook: Ok(None),
            server: None,
            notice: None,
            cache: cache.to_owned(),
            background: None,
            watch: None,
            tag_art: Default::default(),
            kept: Mutex::new(crate::prefetch::Recent::new(KEPT)),
            themes: Default::default(),
            keys: Default::default(),
            #[cfg(feature = "live")]
            joined: None,
        }
    }

    /// The notebook in the folder at `location`, or why it cannot be read. A folder on a
    /// mounted SMB share opens through Snowbound's own SMB client, signed in with the
    /// account the system keeps for the mount, as OneNote's own client coordinates with
    /// OneNote; without that account it opens through the mount.
    pub fn notebook(location: &str, cache: &Path) -> Self {
        if location.starts_with(SHARED) {
            return Self::joined(location, cache);
        }
        if let Some(mount) = server_address(location) {
            let login = match mount.user {
                Some(_) => crate::platform::smb_login(&mount),
                None => Ok(Login::guest(&mount)),
            };
            return login
                .and_then(|login| Self::on_share(location, mount.clone(), login, cache))
                .unwrap_or_else(|reason| {
                    let name = display_name(cache, location).unwrap_or_else(|| mount.name());
                    Self {
                        notebook: Err(reason),
                        ..Self::new(location, name, cache)
                    }
                });
        }
        #[cfg(target_arch = "wasm32")]
        let mut notice = crate::platform::lock_notice(location);
        #[cfg(not(target_arch = "wasm32"))]
        let mut notice = None;
        if let Some(mount) = crate::platform::smb_mount(Path::new(location)) {
            match crate::platform::smb_login(&mount)
                .and_then(|login| Self::on_share(location, mount, login, cache))
            {
                Ok(library) => return library,
                Err(reason) => {
                    eprintln!("{location}: opening through the mounted share: {reason}");
                    notice = Some(reason);
                }
            }
        }
        let mut notebook = Notebook::open(location, cache);
        let (background, watch) = match &mut notebook {
            Ok(notebook) => local_background(notebook, location),
            Err(_) => (None, None),
        };
        let name = display_name(cache, location).unwrap_or_else(|| file_name(Path::new(location)));
        Self {
            background,
            watch,
            tag_art: Mutex::new(Arc::new(
                notebook.as_ref().map(read_tag_art).unwrap_or_default(),
            )),
            notebook: notebook.map(Some).map_err(|error| {
                eprintln!("{location}: {error}");
                crate::plain(&error, "notebook")
            }),
            notice,
            ..Self::new(location, name, cache)
        }
    }

    /// The notebook another computer shares at `location` by Live Share, as its last listing
    /// has it until that computer answers.
    #[cfg(feature = "live")]
    fn joined(location: &str, cache: &Path) -> Self {
        let mut library = Self::new(location, String::new(), cache);
        match crate::live::Joined::open(location, cache) {
            Ok((joined, notebook)) => {
                let background = Background::hosted(Arc::clone(&joined.guest), notify_background)
                    .inspect_err(|error| eprintln!("Background sync did not start: {error}"))
                    .ok()
                    .map(|background| {
                        background.set_offline(offline());
                        Arc::new(background)
                    });
                let mut notebook = notebook;
                if let Some(background) = &background {
                    background.watch(notebook.replicas());
                    joined.follow(background);
                }
                library.name = display_name(cache, location).unwrap_or_else(|| joined.name.clone());
                library.tag_art = Mutex::new(Arc::new(read_tag_art(&notebook)));
                library.notebook = Ok(Some(notebook));
                library.background = background;
                library.joined = Some(Arc::new(joined));
            }
            Err((name, reason)) => {
                library.name = display_name(cache, location).unwrap_or(name);
                library.notebook = Err(reason);
            }
        }
        library
    }

    #[cfg(not(feature = "live"))]
    fn joined(location: &str, cache: &Path) -> Self {
        Self {
            notebook: Err("This version of Snowbound can’t open shared notebooks.".into()),
            ..Self::new(location, "Shared notebook".into(), cache)
        }
    }

    /// No server is in reach of the browser until a relay carries SMB to it.
    #[cfg(target_arch = "wasm32")]
    pub fn on_share(_: &str, _: Mount, _: Login, _: &Path) -> Result<Self, String> {
        Err(crate::platform::smb::Refusal::Unreachable.to_string())
    }

    /// The notebook at `location`, which `mount` names on its server, opened through the
    /// embedded client signed in as `login`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn on_share(
        location: &str,
        mount: Mount,
        login: Login,
        cache: &Path,
    ) -> Result<Self, String> {
        let server = Arc::new(Server { mount, login });
        let client = server.connect().map_err(|error| {
            crate::server::refusal(&error, &server.mount, server.login.user.is_empty())
        })?;
        let mut notebook = Notebook::open_smb(Arc::new(client), &server.mount.root, cache)
            .map_err(|error| error.to_string())?;
        let connect = Arc::clone(&server);
        let background = Background::smb(
            &server.mount.root,
            LIMIT,
            move || connect.connect(),
            notify_background,
        )
        .map_err(|error| error.to_string())?;
        background.set_offline(offline());
        background.watch(notebook.replicas());
        let name =
            display_name(cache, location).unwrap_or_else(|| match server_address(location) {
                Some(_) => server.mount.name(),
                None => file_name(Path::new(location)),
            });
        Ok(Self {
            tag_art: Mutex::new(Arc::new(read_tag_art(&notebook))),
            notebook: Ok(Some(notebook)),
            server: Some(server),
            background: Some(Arc::new(background)),
            ..Self::new(location, name, cache)
        })
    }

    /// Names the notebook `name` on this computer, leaving its folder as it is, as OneNote
    /// 2010's Notebook Properties does; the notebook read again with `with` shows it.
    pub fn set_display_name(&self, name: &str) -> io::Result<()> {
        edit_display_names(&self.cache, |names| {
            names.insert(self.location.clone(), name.to_owned());
        })
    }

    /// Where the notebook would be with its folder named `name`; none for a section opened on
    /// its own, or a notebook at the top of its share.
    pub fn renamed_location(&self, name: &str) -> Option<String> {
        self.catalog()?;
        if let Some(mut mount) = server_address(&self.location) {
            let (parent, _) = mount.root.rsplit_once('/').unwrap_or(("", &mount.root));
            mount.root = match (parent, mount.root.is_empty()) {
                (_, true) => return None,
                ("", false) => name.to_owned(),
                (parent, false) => format!("{parent}/{name}"),
            };
            return Some(mount.url());
        }
        let parent = Path::new(&self.location).parent()?;
        Some(parent.join(name).to_string_lossy().into_owned())
    }

    /// Renames the notebook's folder to `name` once nothing of this notebook holds its files:
    /// its background stopped and its kept sections closed, as the open section must be too.
    /// The replicas, and edits waiting in them, follow, and the folder's own name takes the
    /// place of a display name. Refused, with nothing changed, while another writer holds a file
    /// of it. Returns the notebook's new location, where it opens again, or why not, as its
    /// reader is told.
    pub fn rename_folder(&self, name: &str) -> Result<String, String> {
        let to = self
            .renamed_location(name)
            .ok_or("This notebook’s folder can’t be renamed.")?;
        if let Some(background) = &self.background {
            background.stop();
        }
        self.close_kept();
        let renamed = self
            .reopen()
            .and_then(|notebook| Ok(notebook.rename_folder(name)?));
        if let Err(error) = renamed {
            return Err(match crate::io_kind(&*error) {
                Some(io::ErrorKind::WouldBlock | io::ErrorKind::ResourceBusy) => {
                    "Another computer is saving to this notebook. Try again in a moment.".into()
                }
                Some(io::ErrorKind::PermissionDenied) => "Its files are open on another \
                    computer, or you can’t rename folders there. Close the notebook in OneNote \
                    on other computers, then try again."
                    .into(),
                Some(io::ErrorKind::AlreadyExists) => {
                    format!("A folder named “{name}” is already there. Choose another name.")
                }
                Some(io::ErrorKind::InvalidInput) => "A folder name can’t contain \\ / : * ? \" \
                    < > | or end with a dot or space."
                    .into(),
                _ => crate::plain(&*error, "folder"),
            });
        }
        // The folder's own name shows now.
        if let Err(error) = edit_display_names(&self.cache, |names| {
            names.remove(&self.location);
        }) {
            eprintln!("{}: {error}", self.location);
        }
        Ok(to)
    }

    /// The art the notebook's tags draw with.
    pub fn tag_art(&self) -> Arc<TagArt> {
        Arc::clone(
            &self
                .tag_art
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// Draws `tag` with its art, `bytes`, in this notebook from now on, and keeps the art in
    /// the notebook's folder on a thread of its own; a section opened on its own keeps none.
    pub fn map_tag_art(self: &Arc<Self>, tag: &NoteTag, bytes: Vec<u8>) {
        let (Some(art), Ok(Some(_))) = (&tag.art, &self.notebook) else {
            return;
        };
        let Some(sources) = canvas::gpu::art_sources(art, || Some(bytes.clone())) else {
            return;
        };
        {
            let mut mapped = self
                .tag_art
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if mapped.art(&tag.label, tag.shape) == Some(art.as_str()) {
                return;
            }
            Arc::make_mut(&mut mapped).map(&tag.label, tag.shape, art, sources);
        }
        let extension = art.rsplit('.').next().unwrap_or_default().to_owned();
        let (library, tag) = (Arc::clone(self), tag.clone());
        crate::spawn(move || {
            let kept = library.reopen().and_then(|notebook| {
                Ok(notebook.map_tag_art(&tag.label, tag.shape, &bytes, &extension)?)
            });
            if let Err(error) = kept {
                eprintln!(
                    "{}: keeping the art of tag {:?} failed: {error}",
                    library.location, tag.label
                );
            }
        });
    }

    /// The notebook's style themes, read again after a while so other machines' changes
    /// reach pages opened later; a section opened on its own has none.
    pub fn themes(&self) -> Arc<Themes> {
        let mut kept = self
            .themes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((read, themes)) = &*kept
            && read.elapsed() < Duration::from_secs(30)
        {
            return Arc::clone(themes);
        }
        let themes = match &self.notebook {
            Ok(Some(notebook)) => notebook.themes().unwrap_or_else(|error| {
                eprintln!("{}: reading its themes failed: {error}", self.location);
                Themes::default()
            }),
            _ => Themes::default(),
        };
        let themes = Arc::new(themes);
        *kept = Some((Instant::now(), Arc::clone(&themes)));
        themes
    }

    /// Takes `change` into the notebook's themes from now on, and keeps it in the notebook's
    /// folder on a thread of its own.
    pub fn save_themes(self: &Arc<Self>, change: Themes) {
        if !matches!(self.notebook, Ok(Some(_))) {
            return;
        }
        let mut themes = (*self.themes()).clone();
        notebook::sidecar::themes::merge(&mut themes, change.clone());
        *self
            .themes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some((Instant::now(), Arc::new(themes)));
        let library = Arc::clone(self);
        crate::spawn(move || {
            let kept = library
                .reopen()
                .and_then(|notebook| Ok(notebook.save_themes(change)?));
            if let Err(error) = kept {
                eprintln!("{}: keeping its themes failed: {error}", library.location);
            }
        });
    }

    /// Deletes for good, on a thread of its own, what the notebook's recycle bin has held
    /// unchanged past OneNote's 60 days, as OneNote 2010 prunes it a while after opening.
    pub fn purge_recycle_bin(self: &Arc<Self>) {
        if !matches!(self.notebook, Ok(Some(_))) {
            return;
        }
        let library = Arc::clone(self);
        crate::spawn(move || {
            let now = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .map_or(0, |unix| unix.as_secs())
                // Time32 counts from 1980.
                .saturating_sub(315_532_800);
            let purged = library.reopen().and_then(|mut notebook| {
                Ok(notebook.purge_recycle_bin(u32::try_from(now).unwrap_or(u32::MAX))?)
            });
            if let Err(error) = purged {
                eprintln!(
                    "{}: emptying the recycle bin failed: {error}",
                    library.location
                );
            }
        });
    }

    /// This notebook read again, the way it was opened, for changing its structure.
    pub fn reopen(&self) -> Result<Notebook, Box<dyn Error>> {
        #[cfg(feature = "live")]
        if let Some(joined) = &self.joined {
            return Ok(Notebook::open_hosted(
                Arc::clone(&joined.guest),
                &self.cache,
            )?);
        }
        Ok(match &self.server {
            #[cfg(not(target_arch = "wasm32"))]
            Some(server) => {
                Notebook::open_smb(Arc::new(server.connect()?), &server.mount.root, &self.cache)?
            }
            #[cfg(target_arch = "wasm32")]
            Some(_) => unreachable!("The browser opens no notebook on a server"),
            None => Notebook::open(&self.location, &self.cache)?,
        })
    }

    /// This notebook as `notebook`, read again after a change.
    pub fn with(&self, mut notebook: Notebook) -> Self {
        if let Some(background) = &self.background {
            background.watch(notebook.replicas());
        }
        // A network volume's watch covers each folder by name, so a new one needs a new watch.
        let watch = match (&self.watch, &self.background) {
            (Some(_), Some(background))
                if !crate::watch::on_this_computer(Path::new(&self.location)) =>
            {
                let reports = OnceLock::from(Arc::downgrade(background));
                watcher(&self.location, Arc::new(reports)).or_else(|| self.watch.clone())
            }
            _ => self.watch.clone(),
        };
        let name = display_name(&self.cache, &self.location).unwrap_or_else(|| self.name.clone());
        Self {
            notebook: Ok(Some(notebook)),
            server: self.server.clone(),
            notice: self.notice.clone(),
            background: self.background.clone(),
            watch,
            tag_art: Mutex::new(self.tag_art()),
            keys: Arc::clone(&self.keys),
            #[cfg(feature = "live")]
            joined: self.joined.clone(),
            ..Self::new(&self.location, name, &self.cache)
        }
    }

    /// A notebook `Notebook::create` just made in the folder at `location`.
    pub fn created(location: &str, mut notebook: Notebook, cache: &Path) -> Self {
        let (background, watch) = local_background(&mut notebook, location);
        Self {
            background,
            watch,
            notebook: Ok(Some(notebook)),
            ..Self::new(location, file_name(Path::new(location)), cache)
        }
    }

    /// The section file at `file` as a notebook of one tab.
    pub fn section(file: &Path, cache: &Path) -> Self {
        Self::new(
            &file.to_string_lossy(),
            file_name(file.parent().unwrap_or(file)),
            cache,
        )
    }

    /// Opens the section at catalog `path`; on a share, through a replica named by its
    /// file identity that publishes over the embedded client.
    pub fn open(
        &self,
        path: &str,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Section, Box<dyn Error>> {
        // The background may hold the replica for a step, which takes a network round trip,
        // or be making it as an offline copy.
        self.open_by(path, notify, Instant::now() + TIMEOUT * 3)
    }

    /// `open`, waiting until `deadline` for whatever holds the section's replica.
    fn open_by(
        &self,
        path: &str,
        notify: impl Fn() + Send + 'static,
        deadline: Instant,
    ) -> Result<Section, Box<dyn Error>> {
        let notify = Arc::new(Mutex::new(notify));
        let notifier = || {
            let notify = Arc::clone(&notify);
            move || {
                if let Ok(notify) = notify.lock() {
                    notify();
                }
            }
        };
        let key = self.unlocked(path);
        self.touch(path);
        if key.is_none() && self.protected(path) {
            return Err("The section is password protected".into());
        }
        let section = loop {
            if let Some(kept) = self
                .kept
                .lock()
                .ok()
                .and_then(|mut kept| kept.take(&path.to_owned()))
            {
                return Ok(kept);
            }
            let opened = match (&self.notebook, &self.server) {
                #[cfg(feature = "live")]
                (Ok(Some(notebook)), _) if let Some(joined) = &self.joined => {
                    let cache = notebook.replica_path(path)?;
                    notebook::fs::create_dir_all(cache.parent().unwrap_or(&self.cache))?;
                    notebook::Replica::open_or_create(&cache, key.as_ref(), || {
                        notebook.read_section(path)
                    })
                    .and_then(|replica| {
                        let guest = Arc::clone(&joined.guest);
                        Section::resume_hosted(path.to_owned(), replica, guest, notifier())
                    })
                }
                #[cfg(target_arch = "wasm32")]
                (Ok(Some(_)), Some(_)) => unreachable!("The browser opens no notebook on a server"),
                #[cfg(not(target_arch = "wasm32"))]
                (Ok(Some(notebook)), Some(server)) => {
                    let file = match server.mount.root.as_str() {
                        "" => path.to_owned(),
                        root => format!("{root}/{path}"),
                    };
                    let cache = notebook.replica_path(path)?;
                    notebook::fs::create_dir_all(cache.parent().unwrap_or(&self.cache))?;
                    let replica = notebook::Replica::open_or_create(&cache, key.as_ref(), || {
                        Ok(server.connect()?.read_storage(&file, LIMIT)?)
                    });
                    replica.and_then(|replica| {
                        let server = Arc::clone(server);
                        let connect = move || server.connect();
                        Section::resume_smb(file, replica, LIMIT, connect, notifier())
                    })
                }
                (Ok(Some(notebook)), None) if self.in_icloud() => {
                    let root = Path::new(&self.location);
                    crate::icloud::section(notebook, root, path, key.as_ref(), notifier())
                }
                (Ok(Some(notebook)), None) => match &key {
                    Some(key) => notebook.section_unlocked(path, key, notifier()),
                    None => notebook.section(path, notifier()),
                },
                (Ok(None), _) if self.in_icloud() => {
                    crate::icloud::lone_section(Path::new(path), &self.cache, notifier())
                }
                (Ok(None), _) => Section::open(path, &self.cache, notifier()),
                (Err(error), _) => return Err(error.clone().into()),
            };
            match opened {
                Err(error)
                    if (error.busy()
                        || matches!(&error, notebook::Error::Io(error)
                            if error.kind() == io::ErrorKind::AlreadyExists))
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                opened => break opened?,
            }
        };
        section.set_offline(offline());
        if let Some(background) = &self.background {
            background.hold(path, &section);
        }
        Ok(section)
    }

    /// Opens the section at catalog `path`, unless it is kept open already, and keeps it
    /// open for `open` to hand out. A section held elsewhere, as the one shown is, is left.
    pub fn prefetch(
        &self,
        path: &str,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Arc<notebook::Replica>, Box<dyn Error>> {
        let section = self.open_by(path, notify, Instant::now())?;
        let replica = Arc::clone(section.replica());
        self.keep(path, section);
        Ok(replica)
    }

    /// Keeps `section`, at catalog `path`, open for `open` to hand out; the least recent
    /// past `KEPT` close.
    pub fn keep(&self, path: &str, section: Section) {
        let gone = match self.kept.lock() {
            Ok(mut kept) => kept.put(path.to_owned(), section),
            Err(_) => vec![section],
        };
        if !gone.is_empty() {
            crate::spawn(move || close(gone));
        }
    }

    /// Works offline, or online again, in the background and in the sections kept open.
    pub fn set_offline(&self, offline: bool) {
        if let Some(background) = &self.background {
            background.set_offline(offline);
        }
        if let Ok(mut kept) = self.kept.lock() {
            kept.values_mut()
                .for_each(|section| section.set_offline(offline));
        }
    }

    /// Closes the sections kept open, before the notebook's files change.
    pub fn close_kept(&self) {
        if let Ok(mut kept) = self.kept.lock() {
            close(kept.trim(0, |_| 1));
        }
    }

    /// Where `file`, a section file of this notebook, sits on this computer: under the
    /// mount it was opened from when the embedded client holds it share-relative.
    pub fn local(&self, file: &Path) -> Option<PathBuf> {
        if file.is_absolute() {
            return Some(file.to_owned());
        }
        let server = self.server.as_ref()?;
        let file = file.strip_prefix(&server.mount.root).ok()?;
        Some(self.folder()?.join(file))
    }

    /// The notebook's folder or section file on this computer; none for a notebook opened
    /// straight from its server.
    pub fn folder(&self) -> Option<&Path> {
        (server_address(&self.location).is_none() && !self.location.starts_with(SHARED))
            .then(|| Path::new(&self.location))
    }

    /// Whether the notebook is on an SMB share, however it is reached.
    pub fn on_smb(&self) -> bool {
        self.server.is_some() || server_address(&self.location).is_some() || self.notice.is_some()
    }

    /// Whether iCloud Drive keeps the notebook, or the section opened on its own.
    pub fn in_icloud(&self) -> bool {
        self.folder().is_some_and(crate::icloud::ubiquitous)
    }

    /// Where the notebook lives, as its reader knows the place: the server and share, iCloud
    /// Drive, a drive or this computer, then each folder down to the notebook's, left out.
    pub fn place(&self) -> Vec<String> {
        #[cfg(feature = "live")]
        if let Some(joined) = &self.joined {
            return vec![format!("{}’s computer", joined.host)];
        }
        let mut place: Vec<String> = match self
            .server
            .as_ref()
            .map(|server| server.mount.clone())
            .or_else(|| server_address(&self.location))
        {
            Some(mount) => {
                let host = bonjour_instance(&mount.server)
                    .unwrap_or_else(|| mount.host().trim_end_matches(".local").to_owned());
                [host, mount.share.clone()]
                    .into_iter()
                    .chain(mount.root.split('/').map(str::to_owned))
                    .filter(|part| !part.is_empty())
                    .collect()
            }
            None => {
                let path = Path::new(&self.location);
                let icloud = self.in_icloud();
                let this = if cfg!(target_os = "macos") {
                    "This Mac"
                } else {
                    "This Computer"
                };
                let tops = [
                    (
                        icloud.then(crate::icloud::folder).flatten(),
                        "iCloud Drive/Snowbound",
                    ),
                    (icloud.then(crate::icloud::drive).flatten(), "iCloud Drive"),
                    (std::env::var_os("HOME").map(PathBuf::from), this),
                    (Some(PathBuf::from("/Volumes")), ""),
                ];
                let (top, rest) = tops
                    .into_iter()
                    .find_map(|(top, name)| Some((name, path.strip_prefix(top?).ok()?)))
                    .unwrap_or((this, path));
                top.split('/')
                    .map(str::to_owned)
                    .chain(rest.iter().map(|part| part.to_string_lossy().into_owned()))
                    .filter(|part| !part.is_empty() && part != "/")
                    .collect()
            }
        };
        place.pop();
        place
    }

    /// Names the section at catalog `path` across every open notebook, for remembering
    /// its pages.
    pub fn key(&self, path: &str) -> String {
        key(&self.location, path)
    }

    /// The readable sections of the folder at catalog path `folder`, as tabs in its order.
    pub fn tabs(&self, folder: &str) -> Vec<Tab> {
        match &self.notebook {
            Ok(Some(notebook)) => folders(notebook.catalog(), |_| true)
                .into_iter()
                .find(|candidate| candidate.path == folder)
                .map_or_else(Vec::new, tabs),
            Ok(None) => vec![Tab {
                path: self.location.clone(),
                name: section_name(&self.location, &None),
                color: None,
            }],
            Err(_) => Vec::new(),
        }
    }

    /// The file identity of the section at catalog `path`.
    pub fn section_identity(&self, path: &str) -> Option<[u8; 16]> {
        folders(self.catalog()?, |_| true)
            .into_iter()
            .flat_map(|folder| &folder.sections)
            .find(|section| section.path == path)
            .map(|section| section.file_id)
    }

    /// The colour of the section at catalog `path`, COLORREF; none for OneNote's None.
    pub fn section_color(&self, path: &str) -> Option<u32> {
        let listed = folders(self.catalog()?, |_| true)
            .into_iter()
            .flat_map(|folder| &folder.sections)
            .find(|section| section.path == path)?;
        match listed.state {
            SectionState::Readable { color, .. } => color,
            _ => None,
        }
    }

    /// The first readable section, searching groups after sections, as OneNote opens a
    /// notebook.
    pub fn first_section(&self) -> Option<String> {
        let Some(catalog) = self.catalog() else {
            return self.tabs("").pop().map(|tab| tab.path);
        };
        folders(catalog, |group| !recycle_bin(&group.path))
            .into_iter()
            .find_map(|folder| tabs(folder).into_iter().next())
            .map(|tab| tab.path)
    }

    /// Whether the notebook lists a section at catalog path `path`.
    pub fn contains(&self, path: &str) -> bool {
        let Some(catalog) = self.catalog() else {
            return self.location == path;
        };
        folders(catalog, |_| true)
            .iter()
            .any(|folder| folder.sections.iter().any(|section| section.path == path))
    }

    /// The notebook's colour, COLORREF, as its table of contents holds it.
    pub fn color(&self) -> Option<u32> {
        self.catalog()?.toc.as_ref()?.color
    }

    /// Whether the notebook lists sections not on this device yet, as iCloud Drive keeps
    /// them elsewhere.
    pub fn downloading(&self) -> bool {
        self.catalog().is_some_and(|catalog| {
            folders(catalog, |_| true).iter().any(|folder| {
                (folder.unavailable.iter())
                    .any(|entry| entry.reason == notebook::discover::Reason::Evicted)
            })
        })
    }

    /// The notebook's folders of sections, for the sidebar.
    pub fn catalog(&self) -> Option<&Folder> {
        match &self.notebook {
            Ok(Some(notebook)) => Some(notebook.catalog()),
            _ => None,
        }
    }

    /// Whether the section at catalog `path` is password protected.
    pub fn protected(&self, path: &str) -> bool {
        self.catalog().is_some_and(|catalog| {
            folders(catalog, |_| true)
                .into_iter()
                .flat_map(|folder| &folder.sections)
                .any(|section| {
                    section.path == path && matches!(section.state, SectionState::Locked)
                })
        })
    }

    /// Whether the section at catalog `path` is password protected and not unlocked.
    pub fn locked(&self, path: &str) -> bool {
        self.protected(path) && self.unlocked(path).is_none()
    }

    /// The key the section at catalog `path` was unlocked with.
    pub fn unlocked(&self, path: &str) -> Option<Key> {
        let identity = self.section_identity(path)?;
        let keys = self.keys.0.lock().ok()?;
        keys.get(&identity).map(|(key, _)| key.clone())
    }

    /// Whether any of its password-protected sections is unlocked, which Lock All would lock.
    pub fn any_unlocked(&self) -> bool {
        self.keys.0.lock().is_ok_and(|keys| !keys.is_empty())
    }

    /// Marks the unlocked section at catalog `path` worked in now.
    pub fn touch(&self, path: &str) {
        if let (Some(identity), Ok(mut keys)) = (self.section_identity(path), self.keys.0.lock())
            && let Some((_, used)) = keys.get_mut(&identity)
        {
            *used = Instant::now();
        }
    }

    /// Unlocks the section at catalog `path` with `password`, as OneNote's Protected Section
    /// dialog does; a wrong password is `PasswordMismatch`.
    pub fn unlock(&self, path: &str, password: &str) -> Result<(), notebook::Error> {
        let Ok(Some(notebook)) = &self.notebook else {
            return Err(io::Error::from(io::ErrorKind::NotFound).into());
        };
        let key = notebook.unlock(path, password)?;
        self.keep_key(path, key);
        Ok(())
    }

    /// Keeps `key` for the section at catalog `path`, as setting its password gives one.
    pub fn keep_key(&self, path: &str, key: Key) {
        if let (Some(identity), Ok(mut keys)) = (self.section_identity(path), self.keys.0.lock()) {
            keys.insert(identity, (key, Instant::now()));
        }
    }

    /// Locks the protected sections unlocked here that `chosen` picks by catalog path and
    /// time since last worked in, closing the kept sessions that hold them; returns the paths
    /// locked.
    pub fn lock(&self, mut chosen: impl FnMut(&str, Duration) -> bool) -> Vec<String> {
        let Some(catalog) = self.catalog() else {
            return Vec::new();
        };
        let mut locked = Vec::new();
        if let Ok(mut keys) = self.keys.0.lock() {
            for section in folders(catalog, |_| true)
                .into_iter()
                .flat_map(|folder| &folder.sections)
            {
                let stale = keys
                    .get(&section.file_id)
                    .is_some_and(|(_, used)| chosen(&section.path, used.elapsed()));
                if stale {
                    keys.remove(&section.file_id);
                    locked.push(section.path.clone());
                }
            }
        }
        if let Ok(mut kept) = self.kept.lock() {
            for path in &locked {
                if let Some(section) = kept.take(path) {
                    crate::spawn(move || close(vec![section]));
                }
            }
        }
        locked
    }
}

/// The share folder a notebook opened from its server's address lives in, as its location
/// keeps it.
pub fn server_address(location: &str) -> Option<Mount> {
    location
        .starts_with("smb://")
        .then(|| Mount::from_address(location))
        .flatten()
}

/// `Library::key` for the notebook at `location`, open or not.
pub fn key(location: &str, path: &str) -> String {
    format!("{location}\n{path}")
}

/// Where this computer keeps the display names notebooks were given, by location: OneNote
/// 2010 keeps them in its local cache, never in the notebook.
fn display_names(cache: &Path) -> PathBuf {
    cache.join("display-names.json")
}

fn read_display_names(file: &Path) -> std::collections::BTreeMap<String, String> {
    notebook::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn edit_display_names(
    cache: &Path,
    edit: impl FnOnce(&mut std::collections::BTreeMap<String, String>),
) -> io::Result<()> {
    let file = display_names(cache);
    let mut names = read_display_names(&file);
    edit(&mut names);
    let partial = file.with_extension("partial");
    notebook::fs::write(&partial, serde_json::to_vec_pretty(&names)?)?;
    notebook::fs::rename(partial, file)
}

fn display_name(cache: &Path, location: &str) -> Option<String> {
    read_display_names(&display_names(cache)).remove(location)
}

/// `folder` and the groups below it, breadth first, going into those `enter` takes.
pub fn folders(folder: &Folder, enter: impl Fn(&Folder) -> bool) -> Vec<&Folder> {
    let mut all = vec![folder];
    let mut at = 0;
    while let Some(folder) = all.get(at) {
        all.extend(folder.groups.iter().filter(|group| enter(group)));
        at += 1;
    }
    all
}

/// A section's or group's name, as its catalog path ends.
pub fn entry_name(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".one").unwrap_or(name)
}

/// Whether the folder at `path` is the notebook's recycle bin, which OneNote keeps out of its lists.
pub fn recycle_bin(path: &str) -> bool {
    path.rsplit('/').next() == Some("OneNote_RecycleBin")
}

/// The art `notebook` maps its tags to, each picture read once a run.
fn read_tag_art(notebook: &Notebook) -> TagArt {
    let mut read = TagArt::default();
    let mappings = notebook.tag_art().unwrap_or_else(|error| {
        eprintln!("Reading the notebook's tag art failed: {error}");
        Vec::new()
    });
    for mapping in mappings {
        let bytes = || {
            notebook
                .tag_art_file(&mapping.art)
                .inspect_err(|error| eprintln!("Reading tag art {} failed: {error}", mapping.art))
                .ok()
        };
        if let Some(sources) = canvas::gpu::art_sources(&mapping.art, bytes) {
            read.map(&mapping.name, mapping.shape, &mapping.art, sources);
        }
    }
    read
}

/// A mounted notebook's background sync, following Work Offline, and the watch that reports
/// the folder's changes to it. A folder on a network volume is kept in offline copies, as on a
/// share, and where the system does not report its server's changes, checked more often.
fn local_background(
    notebook: &mut Notebook,
    location: &str,
) -> (Option<Arc<Background>>, Option<Arc<crate::watch::Watch>>) {
    let reports = Arc::new(OnceLock::new());
    let watch = watcher(location, Arc::clone(&reports));
    let copies = !crate::watch::on_this_computer(Path::new(location));
    let background = match crate::icloud::ubiquitous(Path::new(location)) {
        true => crate::icloud::background(notebook, notify_background),
        false => notebook.background(watch.is_some(), copies, notify_background),
    };
    let Some(background) = background
        .inspect_err(|error| eprintln!("Background sync did not start: {error}"))
        .ok()
    else {
        return (None, None);
    };
    background.set_offline(offline());
    background.watch(notebook.replicas());
    let background = Arc::new(background);
    let _ = reports.set(Arc::downgrade(&background));
    (Some(background), watch)
}

/// Reports the changes to the folder at `location` to the background `reports` names, once
/// set.
fn watcher(
    location: &str,
    reports: Arc<OnceLock<std::sync::Weak<Background>>>,
) -> Option<Arc<crate::watch::Watch>> {
    crate::watch::watch(Path::new(location), move |paths| {
        if let Some(background) = reports.get().and_then(std::sync::Weak::upgrade) {
            background.touched(&paths);
        }
    })
    .map(Arc::new)
}

/// How many sections a notebook keeps open besides the one shown.
const KEPT: usize = 3;

fn close(sections: Vec<Section>) {
    for section in sections {
        if let Err(error) = section.close() {
            eprintln!("Synchronization stopped: {error}");
        }
    }
}

pub fn file_name(path: &Path) -> String {
    notebook::fs::canonicalize(path)
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
        // OneNote names the recycle bin's file of deleted pages so.
        if path.ends_with("OneNote_RecycleBin/OneNote_DeletedPages.one") {
            return "Deleted Pages".to_owned();
        }
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
            SectionState::Readable { name, color, .. } => Some(Tab {
                name: section_name(&section.path, name),
                path: section.path.clone(),
                color: *color,
            }),
            // Its name and colour are inside the encryption, but its file is named for it.
            SectionState::Locked => Some(Tab {
                name: section_name(&section.path, &None),
                path: section.path.clone(),
                color: None,
            }),
            _ => None,
        })
        .collect()
}

/// What a path chosen to open opens.
#[derive(Debug, PartialEq)]
pub enum Located {
    /// The notebook in folder `root`, at the section at catalog path `section`.
    Notebook {
        root: PathBuf,
        section: Option<String>,
    },
    /// A section file outside any notebook.
    Section(PathBuf),
    /// A OneNote package, which unpacks into a notebook.
    Package(PathBuf),
    Nothing,
}

/// Whether `folder` holds a table of contents, as a notebook's or a section group's does.
fn has_toc(folder: &Path) -> bool {
    notebook::fs::read_dir(folder).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("onetoc2"))
        })
    })
}

/// What `path` opens: a folder is a notebook; a table of contents opens its folder's; a
/// section opens in the notebook the folders above it with tables of contents make up,
/// section groups included, or alone; a package asks to be unpacked.
pub fn locate(path: &Path) -> Located {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        _ if notebook::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) => {
            Located::Notebook {
                root: path.to_owned(),
                section: None,
            }
        }
        Some("onepkg") => Located::Package(path.to_owned()),
        Some("onetoc2") => match path.parent() {
            Some(root) => Located::Notebook {
                root: root.to_owned(),
                section: None,
            },
            None => Located::Nothing,
        },
        Some("one") => {
            let mut root = None;
            let mut folder = path.parent();
            while let Some(candidate) = folder.filter(|folder| has_toc(folder)) {
                root = Some(candidate);
                folder = candidate.parent();
            }
            match root {
                Some(root) => Located::Notebook {
                    root: root.to_owned(),
                    section: path.strip_prefix(root).ok().map(|relative| {
                        relative
                            .components()
                            .map(|part| part.as_os_str().to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("/")
                    }),
                },
                None => Located::Section(path.to_owned()),
            }
        }
        _ => Located::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A section kept open is what `open` hands out next, without opening it again, and
    /// readying a section another holds leaves it to that one at once.
    #[test]
    fn kept_sections_open_at_once() {
        let root = std::env::temp_dir().join(format!("snowbound-kept-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&root);
        let notebook = root.join("Personal");
        notebook::fs::create_dir_all(&notebook).unwrap();
        let sample =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/sample-notebook/Personal");
        for entry in notebook::fs::read_dir(sample).unwrap() {
            let entry = entry.unwrap();
            notebook::fs::copy(entry.path(), notebook.join(entry.file_name())).unwrap();
        }
        let library = Library::notebook(notebook.to_str().unwrap(), &root.join("cache"));
        let [first, second] = [0, 1].map(|tab| library.tabs("")[tab].path.clone());
        let readied = library.prefetch(&first, || {}).unwrap();
        let opened = library.open(&first, || {}).unwrap();
        assert!(Arc::ptr_eq(&readied, opened.replica()));
        drop(readied);
        let start = Instant::now();
        assert!(library.prefetch(&first, || {}).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        library.keep(&first, opened);
        library.prefetch(&second, || {}).unwrap();
        library.close_kept();
        library.open(&first, || {}).unwrap().close().unwrap();
        drop(library);
        notebook::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn within_mount_resolves_symlinks() {
        let base = std::env::temp_dir().join(format!("snowbound-within-{}", std::process::id()));
        let point = base.join("point");
        notebook::fs::create_dir_all(point.join("notes")).unwrap();
        std::os::unix::fs::symlink(&point, base.join("link")).unwrap();
        let point = notebook::fs::canonicalize(&point).unwrap();
        let point = point.to_str().unwrap();
        let found = within_mount(&base.join("link/notes"), point);
        let missing = within_mount(&base.join("link/gone"), point);
        notebook::fs::remove_dir_all(&base).unwrap();
        assert_eq!(found.as_deref(), Some("notes"));
        assert_eq!(missing, None);
    }

    /// Removes the folder at `path` on the share and everything in it.
    fn remove_tree(client: &Client, path: &str) {
        for entry in client.read_dir(path, 10_000).unwrap_or_default() {
            let inner = format!("{path}/{}", entry.name);
            if entry.attributes & 0x10 != 0 {
                remove_tree(client, &inner);
            } else {
                client.delete(&inner).unwrap();
            }
        }
        let _ = client.delete(path);
    }

    /// A notebook opened from its server's address opens, edits and publishes through the
    /// embedded client: `ONESTORE_SMB_LAB=HOST:PORT` with a share `agent` (guest, or
    /// `ONESTORE_SMB_LAB_USER` and `ONESTORE_SMB_LAB_PASSWORD`; `tools/w7/linux_vm.py up
    /// NAME`). Its notebook lives in the folder `ONESTORE_SMB_LAB_ROOT` (a fresh one by
    /// default), removed before and after unless `ONESTORE_SMB_LAB_KEEP` keeps it.
    #[test]
    #[ignore = "requires an owned Samba share at ONESTORE_SMB_LAB"]
    fn a_notebook_on_a_share_opens_through_the_embedded_client() {
        let address = std::env::var("ONESTORE_SMB_LAB").unwrap();
        let root = std::env::var("ONESTORE_SMB_LAB_ROOT").unwrap_or_else(|_| {
            let nanos = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos();
            format!("snowbound-{}-{nanos}", std::process::id())
        });
        let user = std::env::var("ONESTORE_SMB_LAB_USER").ok();
        let mount = Mount {
            server: address,
            share: "agent".into(),
            user: user.clone(),
            domain: String::new(),
            root: root.clone(),
        };
        let login = Login {
            user: user.unwrap_or_default(),
            password: std::env::var("ONESTORE_SMB_LAB_PASSWORD").unwrap_or_default(),
            domain: String::new(),
        };
        let server = Server {
            mount: mount.clone(),
            login: login.clone(),
        };
        let client = Arc::new(server.connect().unwrap());
        remove_tree(&client, &root);
        client.create_directory(&root).unwrap();
        let cache = std::env::temp_dir().join(format!("snowbound-share-{}", std::process::id()));
        let page = onestore::PageCreation::new(None, Some(""), "Rust Author").unwrap();
        Notebook::open_smb(Arc::clone(&client), &root, &cache)
            .unwrap()
            .create_section("", "New Section 1", &page)
            .unwrap();
        let location = mount.url();
        let library = Library::on_share(&location, mount, login, &cache).unwrap();
        assert!(library.folder().is_none());
        let path = library.first_section().unwrap();
        let section = library.open(&path, || {}).unwrap();
        let (space, ..) = section.pages().unwrap()[0].clone();
        let stored = section.page(space).unwrap();
        let title = stored
            .objects
            .iter()
            .find_map(|object| match object {
                onestore::page::PageObject::Title(title) => {
                    title.outlines[0].paragraphs[0].text().map(|text| text.id)
                }
                _ => None,
            })
            .unwrap();
        section
            .apply(
                "Rust Author",
                onestore::op::Edit {
                    at: crate::filetime(),
                    ops: vec![onestore::op::Op::Page {
                        space,
                        op: onestore::op::PageOp::Text {
                            text: title,
                            range: 0..0,
                            with: "Over SMB".into(),
                        },
                    }],
                },
            )
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let file = format!("{root}/{path}");
        loop {
            // A publication in progress holds the file from readers for a moment.
            let bytes = match client.read_storage(&file, LIMIT) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                read => read.unwrap(),
            };
            let arena = onestore::Arena::default();
            let titles = onestore::Section::open(&arena, bytes)
                .unwrap()
                .pages()
                .unwrap();
            if titles.iter().any(|(_, title, _)| title == "Over SMB") {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{titles:?}");
            section.wake();
            std::thread::sleep(Duration::from_millis(200));
        }
        section.close().unwrap();
        drop(library);
        let _ = notebook::fs::remove_dir_all(&cache);
        if std::env::var_os("ONESTORE_SMB_LAB_KEEP").is_none() {
            remove_tree(&client, &root);
            assert!(client.read_dir(&root, 1).is_err(), "the folder is gone");
        }
    }

    /// Renames a notebook folder on the lab share (`ONESTORE_SMB_LAB`, as above) with an edit
    /// queued offline, which publishes into the renamed folder; `notebook`'s
    /// `live_folder_rename` covers the refusal while another writer holds a section.
    #[test]
    #[ignore = "requires an owned Samba share at ONESTORE_SMB_LAB"]
    fn a_notebook_folder_on_a_share_renames_with_its_queue() {
        let address = std::env::var("ONESTORE_SMB_LAB").unwrap();
        let parent = format!("snowbound-rename-{}", std::process::id());
        let user = std::env::var("ONESTORE_SMB_LAB_USER").ok();
        let mount = |root: &str| Mount {
            server: address.clone(),
            share: "agent".into(),
            user: user.clone(),
            domain: String::new(),
            root: format!("{parent}/{root}"),
        };
        let login = Login {
            user: user.clone().unwrap_or_default(),
            password: std::env::var("ONESTORE_SMB_LAB_PASSWORD").unwrap_or_default(),
            domain: String::new(),
        };
        let connect = || {
            Server {
                mount: mount("Before"),
                login: login.clone(),
            }
            .connect()
            .unwrap()
        };
        let client = Arc::new(connect());
        remove_tree(&client, &parent);
        client.create_directory(&parent).unwrap();
        client.create_directory(&mount("Before").root).unwrap();
        let cache = std::env::temp_dir().join(&parent);
        let page = onestore::PageCreation::new(None, Some(""), "Rust Author").unwrap();
        Notebook::open_smb(Arc::clone(&client), &mount("Before").root, &cache)
            .unwrap()
            .create_section("", "Queued", &page)
            .unwrap();
        let open = |root: &str| {
            let location = mount(root).url();
            Library::on_share(&location, mount(root), login.clone(), &cache).unwrap()
        };
        let library = open("Before");
        let section = library.open("Queued.one", || {}).unwrap();
        section.set_offline(true);
        let (space, ..) = section.pages().unwrap()[0].clone();
        let title = (section.page(space).unwrap().objects.iter())
            .find_map(|object| match object {
                onestore::page::PageObject::Title(title) => {
                    title.outlines[0].paragraphs[0].text().map(|text| text.id)
                }
                _ => None,
            })
            .unwrap();
        let typed = onestore::op::Edit {
            at: crate::filetime(),
            ops: vec![onestore::op::Op::Page {
                space,
                op: onestore::op::PageOp::Text {
                    text: title,
                    range: 0..0,
                    with: "Renamed over SMB".into(),
                },
            }],
        };
        section.apply("Rust Author", typed).unwrap();
        section.close().unwrap();
        assert_eq!(
            library.rename_folder("After").unwrap(),
            mount("After").url()
        );
        assert!(client.read_dir(&mount("Before").root, 10).is_err());
        let renamed = open("After");
        let section = renamed.open("Queued.one", || {}).unwrap();
        let file = format!("{}/Queued.one", mount("After").root);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let titles = client
                .read_storage(&file, LIMIT)
                .ok()
                .and_then(|bytes| {
                    let arena = onestore::Arena::default();
                    onestore::Section::open(&arena, bytes).ok()?.pages().ok()
                })
                .unwrap_or_default();
            if titles
                .iter()
                .any(|(_, title, _)| title == "Renamed over SMB")
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{titles:?}");
            section.wake();
            std::thread::sleep(Duration::from_millis(200));
        }
        section.close().unwrap();
        drop((library, renamed));
        let _ = notebook::fs::remove_dir_all(&cache);
        remove_tree(&client, &parent);
    }

    #[test]
    fn a_notebook_folder_renames_unless_its_name_is_taken() {
        let directory =
            std::env::temp_dir().join(format!("snowbound-rename-{}", std::process::id()));
        let cache = directory.join("cache");
        let page = onestore::PageCreation::new(None, Some(""), "Author").unwrap();
        notebook::fs::create_dir_all(&directory).unwrap();
        for name in ["Mine", "Taken"] {
            Notebook::create(directory.join(name), &cache, Notebook::NEW_COLOR, &page).unwrap();
        }
        let location = |name: &str| directory.join(name).to_string_lossy().into_owned();
        let library = Library::notebook(&location("Mine"), &cache);
        library.set_display_name("Shown").unwrap();
        let refused = library.rename_folder("Taken").unwrap_err();
        assert!(refused.starts_with("A folder named “Taken”"), "{refused}");
        let library = Library::notebook(&location("Mine"), &cache);
        assert_eq!(library.name, "Shown");
        assert_eq!(library.rename_folder("Ours").unwrap(), location("Ours"));
        assert!(notebook::fs::metadata(location("Mine")).is_err());
        let renamed = Library::notebook(&location("Ours"), &cache);
        assert_eq!(renamed.name, "Ours");
        assert_eq!(renamed.color(), Some(Notebook::NEW_COLOR));
        notebook::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn share_relative_sections_show_under_their_mount() {
        let library = |root: &str| Library {
            server: Some(Arc::new(Server {
                mount: Mount {
                    server: "nas".into(),
                    share: "agent".into(),
                    user: None,
                    domain: String::new(),
                    root: root.into(),
                },
                login: Login {
                    user: String::new(),
                    password: String::new(),
                    domain: String::new(),
                },
            })),
            ..Library::new("/Volumes/agent/lab", "lab".into(), Path::new(""))
        };
        let shown = |root, file| library(root).local(Path::new(file));
        let under = Some(PathBuf::from("/Volumes/agent/lab/Group/New Section 1.one"));
        assert_eq!(
            shown("notes/lab", "notes/lab/Group/New Section 1.one"),
            under
        );
        assert_eq!(shown("", "Group/New Section 1.one"), under);
        assert_eq!(shown("notes/lab", "elsewhere/New Section 1.one"), None);
        let elsewhere = std::env::temp_dir().join("Section.one");
        assert_eq!(library("notes/lab").local(&elsewhere), Some(elsewhere));
    }

    #[test]
    fn mounts_name_their_server_share_account_and_folder() {
        assert_eq!(
            Mount::parse("//WORK;clover@nas.local/notes", "Personal/Garden", ""),
            Some(Mount {
                server: "nas.local".into(),
                share: "notes".into(),
                user: Some("clover".into()),
                domain: "WORK".into(),
                root: "Personal/Garden".into(),
            })
        );
        assert_eq!(
            Mount::parse(
                "//nas/notes/sub",
                "Personal",
                "rw,vers=3.0,username=amy,domain=HOME"
            ),
            Some(Mount {
                server: "nas".into(),
                share: "notes".into(),
                user: Some("amy".into()),
                domain: "HOME".into(),
                root: "sub/Personal".into(),
            })
        );
        let guest = Mount::parse("//GUEST:@nas/public", "", "").unwrap();
        assert_eq!((guest.user, guest.root), (None, String::new()));
        assert_eq!(Mount::parse("/dev/disk1", "", ""), None);
    }

    /// The Finder's mount keeps the Bonjour name its keychain entry is under; only dialing
    /// resolves it.
    #[test]
    fn bonjour_mounts_keep_their_service_name() {
        let mount = Mount::parse("//clo@My%20NAS._smb._tcp.local/agent", "", "").unwrap();
        assert_eq!(mount.host(), "My%20NAS._smb._tcp.local");
        assert_eq!(bonjour_instance(&mount.server).as_deref(), Some("My NAS"));
        assert_eq!(
            bonjour_instance("zenith._smb._tcp.local.").as_deref(),
            Some("zenith")
        );
        for server in ["zenith.local", "_smb._tcp.local", "10.0.0.1:445"] {
            assert_eq!(bonjour_instance(server), None, "{server}");
        }
    }

    #[test]
    fn chosen_paths_open_their_notebook_or_section() {
        let root = std::env::temp_dir().join(format!("snowbound-locate-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&root);
        let notebook = root.join("Notebook");
        notebook::fs::create_dir_all(notebook.join("Group")).unwrap();
        for file in [
            "Open Notebook.onetoc2",
            "Group/Open Notebook.onetoc2",
            "Group/Inner.one",
            "Top.one",
        ] {
            notebook::fs::write(notebook.join(file), b"").unwrap();
        }
        notebook::fs::write(root.join("Loose.one"), b"").unwrap();
        let at = |root: &Path, section: Option<&str>| Located::Notebook {
            root: root.to_owned(),
            section: section.map(str::to_owned),
        };
        assert_eq!(locate(&notebook), at(&notebook, None));
        assert_eq!(
            locate(&notebook.join("Open Notebook.onetoc2")),
            at(&notebook, None)
        );
        assert_eq!(
            locate(&notebook.join("Group/Inner.one")),
            at(&notebook, Some("Group/Inner.one"))
        );
        assert_eq!(
            locate(&notebook.join("Top.one")),
            at(&notebook, Some("Top.one"))
        );
        assert_eq!(
            locate(&root.join("Loose.one")),
            Located::Section(root.join("Loose.one"))
        );
        assert_eq!(
            locate(&root.join("Shared.onepkg")),
            Located::Package(root.join("Shared.onepkg"))
        );
        assert_eq!(locate(&root.join("missing.txt")), Located::Nothing);
        notebook::fs::remove_dir_all(&root).unwrap();
    }
}
