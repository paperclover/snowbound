//! Live presence and Live Share in the window (feature `live`, `resources/live-share.md`).
//! In a notebook others reach too (on a server, in iCloud Drive, on another computer, or by
//! Live Share), whoever else has it open shows as an avatar beside the search box, a dot on
//! the tab of their page and their caret on the page, each in a colour of their own; they see
//! this window the same way unless Options turns that off. Live Share serves a notebook on
//! this computer to whoever types its code (`Host`), or opens one another computer shares
//! (`Joined`), as `share.rs`'s dialogs start them.

use crate::{Command, Library, Session, State, TAB_ROW, page, platform, settings};
use canvas::{document::TextPosition, editor::TextOutline};
use notebook::live::{
    self, Caret, Hello, Peer, Reach, Room, Spot,
    share::{Guest, Host, Sharing},
};
use notebook::session::{Background, Notebook};
use onestore::ExGuid;
use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak, mpsc},
};
use ui::{Flags, Id, Spec, children, fill, fit, px};

/// The side of the picture sent, in pixels.
const PICTURE: u32 = 96;
const AVATAR: f32 = 22.0;
/// The name flag above a caret, and how long it shows after the caret last moved.
const FLAG: f32 = 15.0;
const FLAG_PAD: f32 = 4.0;
const NAMED: std::time::Duration = std::time::Duration::from_secs(3);
/// The relay used where Options names none.
pub(crate) const DEFAULT_RELAY: &str = "wss://relay.snowbound.paperclover.net";
/// How long opening a notebook joined for the first time waits for the computer sharing it.
const FIRST_LISTING: std::time::Duration = std::time::Duration::from_secs(15);

/// Where a code's link goes: the page that opens it (`crates/relay`'s `snowbound-site`).
const SITE: &str = "https://snowbound.paperclover.net/";
/// What opens Snowbound at a code, as the site's page and the desktop pass it.
const SCHEME: &str = "snowbound://join/";

/// The link that opens `code`.
pub(crate) fn link(code: &str) -> String {
    format!("{SITE}{code}")
}

/// The code a link names, `snowbound://join/<code>` or the site's page for it.
pub(crate) fn linked(text: &str) -> Option<String> {
    let text = text.trim();
    let code = text
        .strip_prefix(SCHEME)
        .or_else(|| text.strip_prefix(SITE))?;
    notebook::live::share::code(code.trim_end_matches('/'))
}

/// Who this computer is to others and where it meets them, as Options last said.
struct Me {
    name: String,
    picture: bool,
    relay: Option<String>,
}

static ME: Mutex<Option<Me>> = Mutex::new(None);

/// Takes Options' name, picture and relay for every meeting from now on.
pub(crate) fn configure(author: &str, options: &settings::Live) {
    *ME.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Me {
        name: author.to_owned(),
        picture: options.picture,
        relay: options
            .relay
            .clone()
            .filter(|relay| !relay.trim().is_empty()),
    });
}

/// This computer's greeting: the user's name, and the account's picture where Options lets
/// it go and the name is the account's.
fn hello() -> io::Result<Hello> {
    let me = ME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (name, picture) = me.as_ref().map_or((platform::user_name(), true), |me| {
        (me.name.clone(), me.picture)
    });
    let picture = (picture && name == platform::user_name())
        .then(|| account_picture().clone())
        .flatten();
    Hello::new(name, picture)
}

/// The relay peers off this network meet through: `SNOWBOUND_LIVE_RELAY` (`off` for none),
/// else Options', else Snowbound's.
fn relay() -> Option<String> {
    match std::env::var("SNOWBOUND_LIVE_RELAY") {
        Ok(relay) if relay == "off" => None,
        Ok(relay) => Some(relay),
        Err(_) => Some(
            ME.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .and_then(|me| me.relay.clone())
                .unwrap_or_else(|| DEFAULT_RELAY.to_owned()),
        ),
    }
}

/// Where peers are looked for on this computer's networks: every network, or with
/// `SNOWBOUND_LIVE=loopback` this computer alone, as two copies side by side in a test do.
fn reach() -> Option<Reach> {
    match std::env::var("SNOWBOUND_LIVE").as_deref() {
        Ok("loopback") => Some(Reach::Loopback),
        Ok("off") => None,
        _ => Some(Reach::Network),
    }
}

/// Where this computer keeps what it shares and what it joined: beside the replicas, which
/// hold the same notebooks' contents.
fn kept(cache: &Path, name: &str) -> PathBuf {
    cache.join("live").join(name)
}

fn read_kept<T: serde::de::DeserializeOwned + Default>(file: &Path) -> T {
    notebook::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Replaces `file` whole, readable by this account alone.
fn keep(file: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let folder = file.parent().unwrap_or(Path::new("."));
    notebook::fs::create_dir_all(folder)?;
    let partial = file.with_extension("partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    io::Write::write_all(
        &mut options.open(&partial)?,
        &serde_json::to_vec_pretty(value)?,
    )?;
    notebook::fs::rename(partial, file)
}

/// A notebook another computer shares, as this one joined it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Share {
    share: [u8; 16],
    secret: [u8; 16],
    notebook: String,
    host: String,
    /// It has been listed here, so it opens without waiting for the host.
    listed: bool,
}

const JOINED: &str = "joined.json";
const HOSTING: &str = "hosting.json";

/// A notebook another computer shares, open here: the way to it and its names.
pub struct Joined {
    pub guest: Arc<Guest>,
    pub name: String,
    /// The name of the person sharing it.
    pub host: String,
    background: OnceLock<Weak<Background>>,
}

impl Joined {
    /// The notebook shared at `location` and kept in `cache`, or its name and why it can't
    /// be read.
    pub fn open(location: &str, cache: &Path) -> Result<(Self, Notebook), (String, String)> {
        let file = kept(cache, JOINED);
        let mut shares: BTreeMap<String, Share> = read_kept(&file);
        let Some(share) = shares.get(location).cloned() else {
            return Err((
                "Shared notebook".into(),
                "This computer no longer has the code for this notebook. Close it, then open it \
                 again with a new code."
                    .into(),
            ));
        };
        let refused = |error: &dyn std::fmt::Display| (share.notebook.clone(), error.to_string());
        let guest = hello()
            .and_then(|me| {
                Guest::start(
                    me,
                    share.share,
                    share.secret,
                    reach(),
                    relay().as_deref(),
                    crate::library::notify_background,
                )
            })
            .map_err(|error| refused(&error))?;
        if !share.listed {
            let since = std::time::Instant::now();
            while guest.host().is_none() && since.elapsed() < FIRST_LISTING {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        let notebook = Notebook::open_hosted(Arc::clone(&guest), cache).map_err(|_| {
            refused(&format!(
                "Can’t reach {}’s computer. Make sure Snowbound is open there, then try again.",
                share.host
            ))
        })?;
        if !share.listed {
            shares.insert(
                location.to_owned(),
                Share {
                    listed: true,
                    ..share.clone()
                },
            );
            if let Err(error) = keep(&file, &shares) {
                eprintln!("Keeping the shared notebooks: {error}");
            }
        }
        let joined = Self {
            guest,
            name: share.notebook,
            host: share.host,
            background: OnceLock::new(),
        };
        Ok((joined, notebook))
    }

    /// Wakes `background` whenever the host comes back.
    pub fn follow(&self, background: &Arc<Background>) {
        let _ = self.background.set(Arc::downgrade(background));
    }
}

/// Meets whoever shares `code` (and `password`): the share it welcomes this computer to.
pub(crate) fn join(
    code: &str,
    password: &str,
) -> Result<live::wire::Welcome, live::share::Refusal> {
    let me = hello().map_err(|_| live::share::Refusal::Unreachable(live::Trouble::Other))?;
    live::share::join(me, code, password, reach(), relay().as_deref())
}

/// Keeps `welcome` as the notebook this computer joined: its location.
pub(crate) fn joined(cache: &Path, welcome: live::wire::Welcome) -> io::Result<String> {
    let location = live::share::location(&welcome.share);
    let file = kept(cache, JOINED);
    let mut shares: BTreeMap<String, Share> = read_kept(&file);
    shares.insert(
        location.clone(),
        Share {
            share: welcome.share,
            secret: welcome.secret,
            notebook: welcome.notebook,
            host: welcome.host,
            listed: false,
        },
    );
    keep(&file, &shares)?;
    Ok(location)
}

/// What a thread of its own sends the frame, and where the frame takes it.
pub(crate) type Channel<T> = (mpsc::Sender<T>, mpsc::Receiver<T>);

/// The presence room joined and the people met there, what this computer shares, and the
/// people's pictures.
#[derive(Default)]
pub(crate) struct Peers {
    /// The shown notebook's presence room: its location and secret, and the meeting, none
    /// where joining failed.
    room: Option<(String, [u8; 16], Option<live::Live>)>,
    /// Each shared notebook's presence secret, by location, read on a thread of its own:
    /// `None` while it reads or where it couldn't be read.
    secrets: HashMap<String, Option<[u8; 16]>>,
    found: Option<Channel<(String, Option<[u8; 16]>)>>,
    /// The notebooks this computer shares, by location.
    pub(crate) hosts: BTreeMap<String, Arc<Host>>,
    /// Shares as kept for the next launch.
    sharing: Option<BTreeMap<String, Sharing>>,
    /// Shares starting on threads of their own, by location.
    pub(crate) starting: HashMap<String, Option<String>>,
    started: Option<Channel<(String, Result<Host, String>)>>,
    /// Whether each joined notebook's host was there last frame.
    reached: HashMap<String, bool>,
    /// Each peer's picture as drawn, cut to a circle, decoded once.
    pictures: HashMap<[u8; 16], Option<draw::RasterImage>>,
    /// Where each peer's caret was last seen, and when it got there.
    moved: HashMap<[u8; 16], (Caret, std::time::Instant)>,
    pub(crate) share: Option<crate::share::ShareDialog>,
    pub(crate) join: Option<crate::share::JoinDialog>,
}

impl State {
    /// Keeps the presence rooms and shares in step with what is open, and says where this
    /// window is: in the shown notebook's room, and in each Live Share's.
    pub(crate) fn follow_peers(&mut self) {
        self.follow_shares();
        let shown = self
            .session
            .as_ref()
            .map(|session| Arc::clone(&session.library));
        let here = if self.live_options.presence {
            self.presence()
        } else {
            live::Presence::default()
        };
        let presence = |location: &str| match &shown {
            Some(shown) if shown.location == location => here.clone(),
            _ => live::Presence::default(),
        };
        for (location, host) in &self.peers.hosts {
            host.set_presence(presence(location));
        }
        for library in &self.notebooks {
            if let Some(joined) = &library.joined {
                joined.guest.set_presence(presence(&library.location));
            }
        }
        let room = shown
            .filter(|library| self.live_options.presence && reached_by_others(library))
            .and_then(|library| Some((library.location.clone(), self.secret(&library)?)));
        let current =
            (self.peers.room.as_ref()).map(|(location, secret, _)| (location.clone(), *secret));
        if current != room {
            self.peers.room = room.map(|(location, secret)| {
                let redraw = self.redraw.clone();
                let live = hello()
                    .and_then(|me| {
                        live::Live::start(
                            me,
                            &Room::Notebook(secret),
                            reach(),
                            relay().as_deref(),
                            move |_| redraw.wake_by_ref(),
                        )
                    })
                    .inspect_err(|error| eprintln!("Live presence: {error}"))
                    .ok();
                (location, secret, live)
            });
        }
        if let Some((.., Some(live))) = &self.peers.room {
            live.set_presence(here);
        }
    }

    /// The presence secret of `library`, read on a thread of its own the first time.
    fn secret(&mut self, library: &Arc<Library>) -> Option<[u8; 16]> {
        let (found, arrived) = self.peers.found.get_or_insert_with(mpsc::channel);
        for (location, secret) in arrived.try_iter() {
            self.peers.secrets.insert(location, secret);
        }
        if let Some(secret) = self.peers.secrets.get(&library.location) {
            return *secret;
        }
        self.peers.secrets.insert(library.location.clone(), None);
        let (found, library, redraw) = (found.clone(), Arc::clone(library), self.redraw.clone());
        crate::spawn(move || {
            let secret = match &library.notebook {
                Ok(Some(notebook)) => notebook
                    .presence_room()
                    .inspect_err(|error| {
                        eprintln!("{}: no presence room: {error}", library.location)
                    })
                    .ok(),
                _ => None,
            };
            let _ = found.send((library.location.clone(), secret));
            redraw.wake();
        });
        None
    }

    /// Starts the shares kept from before as their notebooks open, stops those whose
    /// notebook closed, keeps each share's code for the next launch, and wakes a joined
    /// notebook's sync when its host comes back.
    fn follow_shares(&mut self) {
        let (_, started) = self.peers.started.get_or_insert_with(mpsc::channel);
        for (location, host) in started.try_iter().collect::<Vec<_>>() {
            match host {
                Ok(host) => {
                    self.peers.starting.remove(&location);
                    self.hosting(location, Arc::new(host));
                }
                Err(error) => {
                    eprintln!("{location}: sharing didn’t start: {error}");
                    self.peers.starting.insert(location, Some(error));
                }
            }
        }
        let cache = self.cache.clone();
        let sharing = self
            .peers
            .sharing
            .get_or_insert_with(|| read_kept(&kept(&cache, HOSTING)))
            .clone();
        let open: Vec<Arc<Library>> = self.notebooks.clone();
        for library in &open {
            if let Some(kept) = sharing.get(&library.location)
                && !self.peers.hosts.contains_key(&library.location)
                && !self.peers.starting.contains_key(&library.location)
                && matches!(library.notebook, Ok(Some(_)))
            {
                self.start_sharing(library, kept.clone());
            }
            if let Some(joined) = &library.joined {
                let reached = joined.guest.host().is_some();
                let before = self.peers.reached.insert(library.location.clone(), reached);
                if reached && before == Some(false) {
                    if let Some(background) = joined.background.get().and_then(Weak::upgrade) {
                        background.wake();
                    }
                    if let Some(session) = &self.session
                        && Arc::ptr_eq(&session.library, library)
                    {
                        session.section.wake();
                    }
                }
            }
        }
        let closed: Vec<String> = (self.peers.hosts.keys())
            .filter(|location| !open.iter().any(|library| library.location == **location))
            .cloned()
            .collect();
        for location in closed {
            self.stop_sharing(&location);
        }
        let now: BTreeMap<String, Sharing> = (self.peers.hosts.iter())
            .map(|(location, host)| (location.clone(), host.sharing()))
            .collect();
        let before: BTreeMap<String, Sharing> = (sharing.into_iter())
            .filter(|(location, _)| {
                !self.peers.starting.contains_key(location)
                    && open.iter().any(|library| library.location == *location)
            })
            .collect();
        if now != before {
            if let Err(error) = keep(&kept(&cache, HOSTING), &now) {
                eprintln!("Keeping what this computer shares: {error}");
            }
            self.peers.sharing = Some(now);
        }
    }

    /// Shares `library` as `sharing` says, on a thread of its own.
    pub(crate) fn start_sharing(&mut self, library: &Arc<Library>, sharing: Sharing) {
        self.peers.starting.insert(library.location.clone(), None);
        let (started, _) = self.peers.started.get_or_insert_with(mpsc::channel);
        let (started, library, redraw) =
            (started.clone(), Arc::clone(library), self.redraw.clone());
        crate::spawn(move || {
            let host = (|| -> Result<Host, Box<dyn std::error::Error>> {
                let storage = library.reopen()?.into_storage();
                let told = redraw.clone();
                Ok(Host::start(
                    storage,
                    hello()?,
                    sharing,
                    &library.name,
                    reach(),
                    relay().as_deref(),
                    move || told.wake_by_ref(),
                )?)
            })();
            let _ = started.send((
                library.location.clone(),
                host.map_err(|error| error.to_string()),
            ));
            redraw.wake();
        });
    }

    /// `host` shares the notebook at `location`: its folder's changes reach its guests, and
    /// theirs its sections, at once.
    fn hosting(&mut self, location: String, host: Arc<Host>) {
        if let Some(background) = self
            .library_at(&location)
            .and_then(|library| library.background.clone())
        {
            background.set_settle(notebook::live::share::SETTLE);
            let shared = Arc::downgrade(&host);
            background.on_touched(Some(Box::new(move |paths| {
                if let Some(host) = shared.upgrade() {
                    host.touched(paths);
                }
            })));
            host.on_changed(Box::new(move |paths| background.touched(paths)));
        }
        self.peers.hosts.insert(location, host);
    }

    /// Stops sharing the notebook at `location`: its guests are let go, and its code and
    /// secret are never used again.
    pub(crate) fn stop_sharing(&mut self, location: &str) {
        self.peers.starting.remove(location);
        if let Some(background) = self
            .library_at(location)
            .and_then(|library| library.background.clone())
        {
            background.on_touched(None);
        }
        if let Some(host) = self.peers.hosts.remove(location) {
            crate::spawn(move || host.stop());
        }
        // Kept, the share would start again on the next frame, as after a relaunch.
        if let Some(sharing) = &mut self.peers.sharing
            && sharing.remove(location).is_some()
            && let Err(error) = keep(&kept(&self.cache, HOSTING), sharing)
        {
            eprintln!("Keeping what this computer shares: {error}");
        }
    }

    fn library_at(&self, location: &str) -> Option<&Arc<Library>> {
        self.notebooks
            .iter()
            .find(|library| library.location == location)
    }

    /// The open section and page, and the caret where it is in stored text.
    fn presence(&self) -> live::Presence {
        let Some(session) = &self.session else {
            return live::Presence::default();
        };
        let outline = self.view.editor.active_outline();
        let [anchor, focus] = self
            .view
            .editor
            .selection()
            .positions
            .map(|position| spot(outline, position));
        live::Presence {
            section: section(session),
            page: Some(session.space.into()),
            caret: anchor
                .zip(focus)
                .map(|(anchor, focus)| Caret { anchor, focus }),
        }
    }

    /// Everyone with the shown notebook open: in its presence room, and in its Live Share,
    /// as host or guest.
    fn connected(&self) -> Vec<Peer> {
        let Some(library) = self.session.as_ref().map(|session| &session.library) else {
            return Vec::new();
        };
        let mut peers: Vec<Peer> = Vec::new();
        let rooms = (self.peers.room.iter())
            .filter(|(location, ..)| *location == library.location)
            .filter_map(|(.., live)| live.as_ref().map(live::Live::peers))
            .chain(
                self.peers
                    .hosts
                    .get(&library.location)
                    .map(|host| host.guests()),
            )
            .chain(library.joined.as_ref().map(|joined| joined.guest.peers()));
        for peer in rooms.flatten() {
            if !peers
                .iter()
                .any(|known| known.hello.peer == peer.hello.peer)
            {
                peers.push(peer);
            }
        }
        peers
    }

    /// The colours of the peers on each page of the open section.
    pub(crate) fn peer_pages(&self) -> HashMap<ExGuid, Vec<[f32; 4]>> {
        let mut pages: HashMap<ExGuid, Vec<[f32; 4]>> = HashMap::new();
        let Some(session) = &self.session else {
            return pages;
        };
        for peer in self.connected() {
            if let Some(presence) = peer
                .presence
                .filter(|presence| presence.section == section(session))
                && let Some(page) = presence.page
            {
                pages
                    .entry(page.into())
                    .or_default()
                    .push(color(&peer.hello.peer));
            }
        }
        pages
    }

    /// The others with the notebook open, as avatars leftward from the search box: a click
    /// opens the page someone has open in this section.
    pub(crate) fn avatars(&mut self) {
        let peers = self.connected();
        if peers.is_empty() {
            return;
        }
        self.ui.open(
            "peers",
            Spec {
                size: [children(), px(TAB_ROW)],
                pad: [6.0, (TAB_ROW - AVATAR) / 2.0],
                gap: 4.0,
                ..Spec::default()
            },
        );
        for peer in peers.iter().rev() {
            let hello = &peer.hello;
            let picture = (self.peers.pictures)
                .entry(hello.peer)
                .or_insert_with(|| hello.picture.as_deref().and_then(circle))
                .clone();
            let avatar = self.ui.open(
                hello.peer,
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(AVATAR); 2],
                    fill: Some(color(&hello.peer)),
                    radius: AVATAR / 2.0,
                    pad: [2.0, 2.0],
                    role: Some(accesskit::Role::Button),
                    ..Spec::default()
                },
            );
            match &picture {
                Some(image) => self.ui.leaf(
                    "picture",
                    Spec {
                        size: [fill(), fill()],
                        image: Some(image),
                        ..Spec::default()
                    },
                ),
                None => self.ui.leaf(
                    "initials",
                    Spec {
                        size: [fill(), fill()],
                        text: Some(&initials(&hello.name)),
                        font_size: Some(9.0),
                        bold: true,
                        center: true,
                        color: Some([1.0; 4]),
                        ..Spec::default()
                    },
                ),
            };
            self.ui.close();
            if let Some(node) = self.ui.access(avatar) {
                node.set_label(hello.name.as_str());
            }
            let place = self.place_of(peer);
            ui::popup::tooltip(&mut self.ui, &hello.name, "", place.as_deref());
            if self.ui.signal(avatar).clicked
                && let Some(session) = &self.session
                && let Some(presence) = peer.presence.as_ref()
                && presence.section == section(session)
                && let Some(page) = presence.page
            {
                self.commands.push(Command::OpenPage(page.into()));
            }
        }
        if self.live_options.presence {
            self.seen();
        }
        self.ui.close();
    }

    /// The mark beside the avatars that says the others see this window too, which opens
    /// Options to turn that off.
    fn seen(&mut self) {
        let accent = self.ui.theme.accent;
        let mark = self.ui.open(
            "seen",
            Spec {
                flags: Flags::CLICKABLE,
                size: [px(AVATAR); 2],
                pad: [5.0, 5.0],
                role: Some(accesskit::Role::Button),
                ..Spec::default()
            },
        );
        self.ui.open(
            "ring",
            Spec {
                size: [fill(), fill()],
                border: Some(accent),
                radius: (AVATAR - 10.0) / 2.0,
                pad: [3.5, 3.5],
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "dot",
            Spec {
                size: [fill(), fill()],
                fill: Some(accent),
                radius: (AVATAR - 17.0) / 2.0,
                ..Spec::default()
            },
        );
        self.ui.close();
        self.ui.close();
        if let Some(node) = self.ui.access(mark) {
            node.set_label("Others see you here");
        }
        ui::popup::tooltip(
            &mut self.ui,
            "Others see you here",
            "",
            Some("Your name, page and cursor. Turn this off in Options."),
        );
        if self.ui.signal(mark).clicked {
            self.open_options();
        }
    }

    /// The page, or the section, `peer` has open.
    fn place_of(&self, peer: &Peer) -> Option<String> {
        let session = self.session.as_ref()?;
        let presence = peer.presence.as_ref()?;
        if presence.section == section(session) {
            let page = ExGuid::from(presence.page?);
            let (_, title, _) = session.pages.iter().find(|(space, ..)| *space == page)?;
            return Some(if title.is_empty() {
                "Untitled page".into()
            } else {
                title.clone()
            });
        }
        let tab = session.tabs.iter().find(|tab| {
            session.library.section_identity(&tab.path) == presence.section
                && presence.section.is_some()
        })?;
        Some(format!("In {}", tab.name))
    }

    /// Each peer's caret on the open page with a flag naming them, and what they have
    /// selected, as boxes floating in the page's box.
    pub(crate) fn peer_carets(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let Some([left, top, right, bottom]) = self.ui.rect(page()) else {
            return;
        };
        let here = (section(session), Some(session.space.into()));
        let scale = self.ui.scale();
        let viewport = self.view.viewport;
        // A point on the page in points from the page box's corner.
        let shown = |[x, y]: [f32; 2]| {
            [
                (x * viewport.scale + viewport.origin[0]) / scale,
                (y * viewport.scale + viewport.origin[1]) / scale,
            ]
        };
        for peer in self.connected() {
            let Some(caret) = peer
                .presence
                .filter(|presence| (presence.section, presence.page) == here)
                .and_then(|presence| presence.caret)
            else {
                continue;
            };
            let Some((outline, focus)) = find(&self.view.editor, caret.focus) else {
                continue;
            };
            let color = color(&peer.hello.peer);
            let [x, y] = outline.origin();
            if let Some((_, anchor)) =
                find(&self.view.editor, caret.anchor).filter(|(other, _)| other.id == outline.id)
            {
                let rects = outline
                    .range_rects([anchor, focus].into())
                    .unwrap_or_default();
                for (index, rect) in rects.into_iter().enumerate() {
                    let [x0, y0] = shown([rect.x0 as f32 + x, rect.y0 as f32 + y]);
                    let [x1, y1] = shown([rect.x1 as f32 + x, rect.y1 as f32 + y]);
                    self.ui.leaf(
                        (peer.hello.peer, "selection", index),
                        Spec {
                            flags: Flags::FLOAT,
                            position: [x0, y0],
                            size: [px(x1 - x0), px(y1 - y0)],
                            fill: Some([color[0], color[1], color[2], 0.25]),
                            ..Spec::default()
                        },
                    );
                }
            }
            let Ok(rect) = outline.caret_at(focus, parley::Affinity::Downstream, 0.0) else {
                continue;
            };
            let [x0, y0] = shown([rect.x0 as f32 + x, rect.y0 as f32 + y]);
            let [_, y1] = shown([rect.x0 as f32 + x, rect.y1 as f32 + y]);
            if !(0.0..right - left).contains(&x0) || y1 < 0.0 || y0 > bottom - top {
                continue;
            }
            let id = peer.hello.peer;
            let now = std::time::Instant::now();
            let moved = match self.peers.moved.get(&id) {
                Some((at, when)) if *at == caret => *when,
                _ => {
                    self.peers.moved.insert(id, (caret, now));
                    now
                }
            };
            let quiet = now.saturating_duration_since(moved);
            if quiet < NAMED {
                self.ui.wake_after(NAMED - quiet);
            }
            let name = (peer.hello.name.split_whitespace().next()).unwrap_or("Someone");
            // Below the caret where the page's top would cut the flag off.
            let below = y0 < FLAG;
            let flag_top = if below { y1 } else { y0 - FLAG };
            // About the flag's width, for hovering it.
            let width = FLAG_PAD * 2.0 + self.ui.measure(name)[0] * 10.0 / self.ui.theme.font_size;
            let hovered = self.ui.pointer().is_some_and(|[px, py]| {
                let [px, py] = [px - left, py - top];
                (x0 - 4.0..=x0 + 4.0).contains(&px) && (y0..=y1).contains(&py)
                    || (x0 - 1.0..=x0 - 1.0 + width).contains(&px)
                        && (flag_top..=flag_top + FLAG).contains(&py)
            });
            let shown_flag = self.ui.animate(
                Id::ROOT.child((id, "flag-shown")),
                if quiet < NAMED || hovered { 1.0 } else { 0.0 },
            );
            let faded =
                |[red, green, blue, alpha]: [f32; 4], by: f32| [red, green, blue, alpha * by];
            self.ui.leaf(
                (id, "caret"),
                Spec {
                    flags: Flags::FLOAT,
                    position: [x0 - 1.0, y0],
                    size: [px(2.0), px(y1 - y0)],
                    fill: Some(color),
                    ..Spec::default()
                },
            );
            // The caret's cap, where its name is not shown.
            self.ui.leaf(
                (id, "cap"),
                Spec {
                    flags: Flags::FLOAT,
                    position: [x0 - 3.0, if below { y1 - 3.0 } else { y0 - 3.0 }],
                    size: [px(6.0), px(6.0)],
                    fill: Some(faded(color, 1.0 - shown_flag)),
                    radius: 3.0,
                    ..Spec::default()
                },
            );
            if shown_flag > 0.0 {
                self.ui.leaf(
                    (id, "flag"),
                    Spec {
                        flags: Flags::FLOAT,
                        position: [x0 - 1.0, flag_top],
                        size: [fit(), px(FLAG)],
                        text: Some(name),
                        font_size: Some(10.0),
                        bold: true,
                        color: Some(faded([1.0; 4], shown_flag)),
                        fill: Some(faded(color, shown_flag)),
                        radius: 3.0,
                        pad: [FLAG_PAD, 0.0],
                        ..Spec::default()
                    },
                );
                // The corner on the caret square, so flag and caret are one shape.
                self.ui.leaf(
                    (id, "joint"),
                    Spec {
                        flags: Flags::FLOAT,
                        position: [x0 - 1.0, if below { y1 } else { y0 - 3.0 }],
                        size: [px(3.0), px(3.0)],
                        fill: Some(faded(color, shown_flag)),
                        ..Spec::default()
                    },
                );
            }
        }
    }
}

/// Whether others may have `library` open too: on a server, in iCloud Drive, or in a folder
/// another computer holds.
fn reached_by_others(library: &Library) -> bool {
    library.on_smb()
        || library.in_icloud()
        || library
            .folder()
            .is_some_and(|folder| !crate::watch::on_this_computer(folder))
}

/// The open section's file identity.
fn section(session: &Session) -> Option<[u8; 16]> {
    session
        .library
        .section_identity(&session.tabs[session.tab].path)
}

/// `position` in `outline` as ops address it: its text object and offset.
fn spot(outline: &TextOutline, position: TextPosition) -> Option<Spot> {
    let text = outline
        .document()
        .text_nodes()
        .nth(position.paragraph)?
        .text()?;
    Some(Spot {
        text: text.id.into(),
        offset: position.offset,
    })
}

/// The outline on the page holding `spot`, and where in it.
fn find(editor: &canvas::editor::CanvasEditor, spot: Spot) -> Option<(&TextOutline, TextPosition)> {
    let text = ExGuid::from(spot.text);
    editor.visible_outlines().find_map(|outline| {
        let paragraph = outline
            .document()
            .text_nodes()
            .position(|node| node.text().is_some_and(|object| object.id == text))?;
        Some((
            outline,
            TextPosition {
                paragraph,
                offset: spot.offset,
            },
        ))
    })
}

/// A peer's colour, the same in every window that shows them.
fn color(peer: &[u8; 16]) -> [f32; 4] {
    const COLORS: [[u8; 3]; 8] = [
        [0x1a, 0x73, 0xe8],
        [0xd9, 0x30, 0x25],
        [0x18, 0x80, 0x38],
        [0xe3, 0x74, 0x00],
        [0x93, 0x34, 0xe6],
        [0x00, 0x89, 0x7b],
        [0xd0, 0x1b, 0x8c],
        [0x80, 0x5a, 0x2e],
    ];
    let [red, green, blue] = COLORS[usize::from(peer[0]) % COLORS.len()];
    draw::srgb(red, green, blue)
}

/// The first letters of a name's first and last words.
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = |word: Option<&&str>| word.and_then(|word| word.chars().next());
    [
        first(words.first()),
        first(words.last()).filter(|_| words.len() > 1),
    ]
    .into_iter()
    .flatten()
    .flat_map(char::to_uppercase)
    .collect()
}

/// `picture`'s middle square, cut to a circle.
fn circle(picture: &[u8]) -> Option<draw::RasterImage> {
    let image = draw::RasterImage::decode(picture, [PICTURE; 2]).ok()?;
    let [width, height] = image.size();
    let side = width.min(height);
    let [dx, dy] = [(width - side) / 2, (height - side) / 2];
    let radius = side as f32 / 2.0;
    let mut pixels = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let at = (((y + dy) * width + x + dx) * 4) as usize;
            let pixel = &image.pixels()[at..at + 4];
            let distance = (x as f32 + 0.5 - radius).hypot(y as f32 + 0.5 - radius);
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            pixels.extend_from_slice(&pixel[..3]);
            pixels.push((f32::from(pixel[3]) * coverage) as u8);
        }
    }
    draw::RasterImage::new([side; 2], pixels).ok()
}

/// The account's picture, as the system shows it at sign-in, as a PNG at most `PICTURE`
/// pixels a side.
fn account_picture() -> &'static Option<Vec<u8>> {
    static PNG: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    PNG.get_or_init(|| {
        let image = draw::RasterImage::decode(&system_picture()?, [PICTURE; 2]).ok()?;
        let [width, height] = image.size();
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        // Decoding premultiplies, which leaves an opaque photo as it was.
        encoder
            .write_header()
            .ok()?
            .write_image_data(image.pixels())
            .ok()?;
        Some(png)
    })
}

/// Directory Services keeps the picture as hex under `JPEGPhoto`, or names a file.
#[cfg(target_os = "macos")]
fn system_picture() -> Option<Vec<u8>> {
    let user = format!("/Users/{}", std::env::var("USER").ok()?);
    let read = |attribute: &str| {
        let output = std::process::Command::new("/usr/bin/dscl")
            .args([".", "-read", &user, attribute])
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        Some(text.split_once(':')?.1.trim().to_owned())
    };
    if let Some(hex) = read("JPEGPhoto") {
        let digits: Vec<u8> = hex.bytes().filter(u8::is_ascii_hexdigit).collect();
        let bytes: Option<Vec<u8>> = digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect();
        if let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) {
            return Some(bytes);
        }
    }
    notebook::fs::read(read("Picture")?).ok()
}

/// Where desktops keep it: `~/.face`, or AccountsService's icon for the account.
#[cfg(target_os = "linux")]
fn system_picture() -> Option<Vec<u8>> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let user = std::env::var("USER").unwrap_or_default();
    [
        home.join(".face"),
        home.join(".face.icon"),
        std::path::Path::new("/var/lib/AccountsService/icons").join(user),
    ]
    .into_iter()
    .find_map(|path| notebook::fs::read(path).ok())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn system_picture() -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link names its code however it came, and anything else names none.
    #[test]
    fn links_name_their_codes() {
        let code = notebook::live::code::format(412, "4MZ9XR").unwrap();
        assert_eq!(linked(&link(&code)), Some(code.clone()));
        let typed = format!("snowbound://join/{}/", code.to_lowercase());
        assert_eq!(linked(&typed), Some(code.clone()));
        assert_eq!(linked(&code), None, "a bare code is not a link");
        assert_eq!(linked("https://example.com/7KQ-4MZ-9XR"), None);
        assert_eq!(linked("snowbound://join/not-a-code"), None);
    }
}
