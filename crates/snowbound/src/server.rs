//! Open Notebook from Server, as the Finder's Connect to Server reaches a share: an address,
//! a sign-in, then a folder on the share, all through Snowbound's own SMB client, which works
//! where the system's file sharing can't reach the server.

use crate::library::{Library, Login, Mount};
use crate::{State, art, platform};
use accesskit::Role;
use notebook::smb::{Client, Credentials, DirectoryEntry, Refusal};
use std::{io, path::Path, sync::mpsc, time::Duration};
use ui::{Anchor, Axis, Flags, Id, Spec, Ui, children, fill, px};
use winit::keyboard::NamedKey;

const WIDTH: f32 = 480.0;
/// The rows the folder list shows at once.
const ROWS: f32 = 9.0;
/// How long the server may take to answer before it counts as unreachable.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The most entries a folder lists.
const LIMIT: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Address,
    SignIn,
    Browse,
}

/// The dialog's fields and where it has reached.
pub struct Connect {
    step: Step,
    address: String,
    user: String,
    password: String,
    domain: String,
    guest: bool,
    /// Whether to keep the password once it signs in; `None` where nothing can keep it.
    remember: Option<bool>,
    /// The share and folder shown, or to be asked for once the address reads.
    mount: Option<Mount>,
    listing: Listing,
    status: Status,
    /// The latest request; replies to earlier ones are dropped.
    asked: u64,
    /// A notebook listed as open that could not sign in, which opens once it lists.
    reopen: bool,
    replies: (mpsc::Sender<Reply>, mpsc::Receiver<Reply>),
}

/// What a folder holds, or a server's shares.
#[derive(Debug, Default, PartialEq)]
pub struct Listing {
    /// Shares on the server, or folders in the folder.
    folders: Vec<String>,
    /// Section files in the folder.
    sections: Vec<String>,
    /// The folder holds a notebook's table of contents.
    toc: bool,
}

#[derive(Debug, PartialEq)]
enum Status {
    Idle,
    Waiting(&'static str),
    Failed(String),
}

/// Lists a share folder, or the server's shares, signed in as `login`.
pub struct Request {
    asked: u64,
    mount: Mount,
    login: Login,
}

pub struct Reply {
    asked: u64,
    mount: Mount,
    listed: io::Result<Listing>,
}

/// Where the dialog's requests go: the embedded SMB client, or a fake in tests.
pub trait Reach {
    fn shares(&self, mount: &Mount, login: &Login) -> io::Result<Vec<String>>;
    fn list(&self, mount: &Mount, login: &Login) -> io::Result<Vec<DirectoryEntry>>;
}

struct Embedded;

fn credentials(login: &Login) -> Credentials<'_> {
    Credentials {
        username: &login.user,
        password: &login.password,
        domain: &login.domain,
    }
}

impl Reach for Embedded {
    fn shares(&self, mount: &Mount, login: &Login) -> io::Result<Vec<String>> {
        notebook::smb::shares(&mount.endpoint(), credentials(login), TIMEOUT)
    }

    fn list(&self, mount: &Mount, login: &Login) -> io::Result<Vec<DirectoryEntry>> {
        Client::connect(&mount.endpoint(), &mount.share, credentials(login), TIMEOUT)?
            .read_dir(&mount.root, LIMIT)
    }
}

impl Request {
    pub fn run(self, reach: &impl Reach) -> Reply {
        let listed = if self.mount.share.is_empty() {
            reach.shares(&self.mount, &self.login).map(|mut shares| {
                shares.sort_by_key(|name| name.to_lowercase());
                Listing {
                    folders: shares,
                    ..Listing::default()
                }
            })
        } else {
            reach.list(&self.mount, &self.login).map(listing)
        };
        Reply {
            asked: self.asked,
            mount: self.mount,
            listed,
        }
    }
}

/// A folder's entries as the browser lists them: folders to go into and the sections that
/// make it a notebook, leaving out hidden folders and OneNote's recycle bin.
fn listing(entries: Vec<DirectoryEntry>) -> Listing {
    let extension = |name: &str, wanted: &str| {
        Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
    };
    let mut listing = Listing {
        toc: entries
            .iter()
            .any(|entry| extension(&entry.name, "onetoc2")),
        ..Listing::default()
    };
    for entry in entries {
        if entry.name.starts_with('.') || crate::library::recycle_bin(&entry.name) {
            continue;
        }
        if entry.attributes & 0x10 != 0 {
            listing.folders.push(entry.name);
        } else if extension(&entry.name, "one") {
            listing.sections.push(entry.name);
        }
    }
    listing.folders.sort_by_key(|name| name.to_lowercase());
    listing.sections.sort_by_key(|name| name.to_lowercase());
    listing
}

/// Why connecting to `mount` failed, and what to do about it.
pub fn refusal(error: &io::Error, mount: &Mount, guest: bool) -> String {
    let server = mount.host();
    match Refusal::of(error) {
        Refusal::Unreachable => format!(
            "Can\u{2019}t reach \u{201c}{server}\u{201d}. Check the address and that the server is on this network."
        ),
        Refusal::Smb1 => format!(
            "\u{201c}{server}\u{201d} only uses SMB1, an old version of file sharing Snowbound doesn\u{2019}t support. Turn on SMB2 or later on the server."
        ),
        Refusal::SignIn if guest => format!(
            "\u{201c}{server}\u{201d} doesn\u{2019}t allow guests. Sign in with a name and password."
        ),
        Refusal::SignIn => "The name or password is incorrect.".to_owned(),
        Refusal::NoShare => format!(
            "\u{201c}{server}\u{201d} has no shared folder named \u{201c}{}\u{201d}. Check the address.",
            mount.share
        ),
        Refusal::Denied => format!(
            "This account can\u{2019}t open \u{201c}{}\u{201d}. Sign in with another account.",
            mount
                .root
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(&mount.share)
        ),
        Refusal::NoFolder => format!(
            "\u{201c}{}\u{201d} isn\u{2019}t on the share. Check the address.",
            mount.root
        ),
        Refusal::Other => {
            eprintln!("Connecting to {server}: {error}");
            format!("Couldn\u{2019}t connect to \u{201c}{server}\u{201d}. Try again.")
        }
    }
}

impl Connect {
    pub fn new(address: String, remember: Option<bool>) -> Self {
        Self {
            step: Step::Address,
            address,
            user: String::new(),
            password: String::new(),
            domain: String::new(),
            guest: false,
            remember,
            mount: None,
            listing: Listing::default(),
            status: Status::Idle,
            asked: 0,
            reopen: false,
            replies: mpsc::channel(),
        }
    }

    /// The account the sign-in names.
    pub fn login(&self) -> Login {
        match self.guest {
            true => Login {
                user: String::new(),
                password: String::new(),
                domain: String::new(),
            },
            false => Login {
                user: self.user.trim().to_owned(),
                password: self.password.clone(),
                domain: self.domain.trim().to_owned(),
            },
        }
    }

    /// Connect: reads the address and asks for a sign-in, or signs in and lists the address.
    pub fn submit(&mut self) -> Option<Request> {
        match self.step {
            Step::Address => {
                let Some(mount) = Mount::from_address(&self.address) else {
                    self.status = Status::Failed(
                        "Enter an address like smb://server/share/folder.".to_owned(),
                    );
                    return None;
                };
                if let Some(user) = &mount.user {
                    self.user = user.clone();
                    self.domain = mount.domain.clone();
                }
                self.mount = Some(mount);
                self.step = Step::SignIn;
                self.status = Status::Idle;
                None
            }
            Step::SignIn if !self.guest && self.user.trim().is_empty() => {
                self.status = Status::Failed("Enter your name, or connect as a guest.".to_owned());
                None
            }
            Step::SignIn => {
                let mut mount = self.mount.clone()?;
                let login = self.login();
                mount.user = (!login.user.is_empty()).then(|| login.user.clone());
                mount.domain = login.domain;
                Some(self.request(mount, "Connecting\u{2026}"))
            }
            Step::Browse => None,
        }
    }

    fn request(&mut self, mount: Mount, waiting: &'static str) -> Request {
        self.asked += 1;
        self.status = Status::Waiting(waiting);
        Request {
            asked: self.asked,
            mount,
            login: self.login(),
        }
    }

    /// Goes into the share or folder listed `index`th.
    pub fn enter(&mut self, index: usize) -> Option<Request> {
        let mut mount = self.mount.clone()?;
        let name = self.listing.folders.get(index)?;
        if mount.share.is_empty() {
            mount.share = name.clone();
        } else if mount.root.is_empty() {
            mount.root = name.clone();
        } else {
            mount.root = format!("{}/{name}", mount.root);
        }
        Some(self.request(mount, "Loading\u{2026}"))
    }

    /// Goes up to the folder holding the one shown, or from a share to the server's shares.
    pub fn up(&mut self) -> Option<Request> {
        let mut mount = self.mount.clone()?;
        match mount.root.rsplit_once('/') {
            Some((parent, _)) => mount.root = parent.to_owned(),
            None if !mount.root.is_empty() => mount.root.clear(),
            None if !mount.share.is_empty() => mount.share.clear(),
            None => return None,
        }
        Some(self.request(mount, "Loading\u{2026}"))
    }

    /// Takes a request's outcome; whether it was the sign-in's, which the password may now
    /// be kept for.
    pub fn receive(&mut self, reply: Reply) -> bool {
        if reply.asked != self.asked {
            return false;
        }
        match reply.listed {
            Ok(listing) => {
                let signed_in = self.step == Step::SignIn;
                self.step = Step::Browse;
                self.listing = listing;
                self.mount = Some(reply.mount);
                self.status = Status::Idle;
                signed_in
            }
            Err(error) => {
                self.status = Status::Failed(refusal(&error, &reply.mount, self.guest));
                // A share or folder the address named, or the account, is for the sign-in to fix.
                if matches!(Refusal::of(&error), Refusal::SignIn | Refusal::Denied)
                    || self.step != Step::Browse
                {
                    self.step = Step::SignIn;
                    self.password.clear();
                }
                false
            }
        }
    }

    /// The notebook Open opens: the folder shown, where it holds sections.
    pub fn opens(&self) -> Option<(Mount, Login)> {
        let mount = self.mount.as_ref()?;
        let notebook = self.listing.toc || !self.listing.sections.is_empty();
        (self.step == Step::Browse && !mount.share.is_empty() && notebook)
            .then(|| (mount.clone(), self.login()))
    }
}

fn id() -> Id {
    Id::ROOT.child("server")
}

fn address_field() -> Id {
    id().child("address")
}

fn user_field() -> Id {
    id().child("user")
}

fn password_field() -> Id {
    id().child("password")
}

fn domain_field() -> Id {
    id().child("domain")
}

impl State {
    /// Opens the dialog; on `location`, a notebook opened from its server that couldn't sign
    /// in, at its sign-in with any password the keychain keeps tried first.
    pub(crate) fn open_server(&mut self, location: Option<&str>) {
        let mut connect = Connect::new(
            location.unwrap_or_default().to_owned(),
            platform::remember_label().map(|_| false),
        );
        let mut request = None;
        if location.is_some() {
            connect.reopen = true;
            connect.submit();
            if let Some(mount) = connect.mount.clone() {
                let login = match mount.user {
                    Some(_) => platform::smb_login(&mount),
                    None => {
                        connect.guest = true;
                        Ok(Login::guest(&mount))
                    }
                };
                match login {
                    Ok(login) => {
                        connect.password = login.password;
                        request = connect.submit();
                    }
                    Err(reason) => connect.status = Status::Failed(reason),
                }
            }
        }
        let focus = match connect.step {
            Step::Address => address_field(),
            _ if connect.user.is_empty() => user_field(),
            _ => password_field(),
        };
        self.server = Some(connect);
        if let Some(request) = request {
            self.ask(request);
        }
        self.ui.open_popup(id());
        self.ui.focus_all(focus);
    }

    /// Runs `request` off the frame, which wakes when it is answered.
    fn ask(&mut self, request: Request) {
        let Some(connect) = &self.server else {
            return;
        };
        let (replies, redraw) = (connect.replies.0.clone(), self.redraw.clone());
        std::thread::spawn(move || {
            let _ = replies.send(request.run(&Embedded));
            redraw.wake();
        });
    }

    /// Opens the notebook at `mount` through the embedded client signed in as `login`, and
    /// closes the dialog.
    fn open_from_server(&mut self, mount: Mount, login: Login) {
        self.ui.close_popup(id());
        self.server = None;
        let location = mount.url();
        self.open_notebook_with(location, None, move |location, cache| {
            Library::on_share(location, mount, login, cache)
        });
    }

    /// Builds the dialog while it is open.
    pub(crate) fn server_dialog(&mut self) {
        let Some(connect) = &mut self.server else {
            return;
        };
        if !self.ui.popup_open(id()) {
            self.server = None;
            return;
        }
        let replies: Vec<Reply> = connect.replies.1.try_iter().collect();
        for reply in replies {
            if connect.receive(reply)
                && connect.remember == Some(true)
                && !connect.guest
                && let (Some(mount), login) = (&connect.mount, connect.login())
                && let Err(error) = platform::save_login(mount, &login)
            {
                eprintln!("Keeping the password for {}: {error}", mount.host());
            }
        }
        if connect.reopen
            && let Some((mount, login)) = connect.opens()
        {
            return self.open_from_server(mount, login);
        }
        let ui = &mut self.ui;
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        let fields: &[Id] = match connect.step {
            Step::Address => &[address_field()],
            Step::SignIn if connect.guest => &[],
            Step::SignIn => &[user_field(), password_field(), domain_field()],
            Step::Browse => &[],
        };
        let entered =
            ui::popup::navigation(ui, fields, &[NamedKey::Enter]).contains(&NamedKey::Enter);
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(WIDTH), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [16.0, 12.0],
                gap: 6.0,
                anchor: Some(Anchor::Dialog),
                role: Some(Role::Dialog),
                ..Spec::default()
            },
        );
        let server = connect
            .mount
            .as_ref()
            .map(|mount| mount.host().to_owned())
            .unwrap_or_default();
        let title = match connect.step {
            Step::Address => "Connect to Server".to_owned(),
            Step::SignIn => {
                format!("Enter your name and password for the server \u{201c}{server}\u{201d}.")
            }
            Step::Browse => "Choose a Notebook".to_owned(),
        };
        if let Some(node) = ui.access(id()) {
            node.set_label("Open Notebook from Server");
        }
        ui.leaf(
            "title",
            Spec {
                size: [fill(), px(row)],
                text: Some(&title),
                bold: true,
                role: Some(Role::Heading),
                ..Spec::default()
            },
        );
        let field_spec = Spec {
            size: [fill(), px(row)],
            fill: Some(theme.base),
            border: Some(theme.accent),
            radius: 4.0,
            pad: [6.0, 0.0],
            ..Spec::default()
        };
        let mut chosen = None;
        let mut back = false;
        match connect.step {
            Step::Address => {
                labelled(ui, "Server Address:", |ui| {
                    ui::text_field(
                        ui,
                        address_field(),
                        &mut connect.address,
                        "smb://server/share",
                        field_spec.clone(),
                    );
                    name(ui, address_field(), "Server Address");
                });
            }
            Step::SignIn => {
                if ui::check_box(ui, "guest", "Connect as guest", connect.guest).clicked {
                    connect.guest = !connect.guest;
                }
                if !connect.guest {
                    labelled(ui, "Name:", |ui| {
                        ui::text_field(ui, user_field(), &mut connect.user, "", field_spec.clone());
                        name(ui, user_field(), "Name");
                    });
                    labelled(ui, "Password:", |ui| {
                        ui::password_field(
                            ui,
                            password_field(),
                            &mut connect.password,
                            "",
                            field_spec.clone(),
                        );
                        name(ui, password_field(), "Password");
                    });
                    labelled(ui, "Domain:", |ui| {
                        ui::text_field(
                            ui,
                            domain_field(),
                            &mut connect.domain,
                            "Optional",
                            field_spec.clone(),
                        );
                        name(ui, domain_field(), "Domain");
                    });
                    if let (Some(remember), Some(label)) =
                        (connect.remember, platform::remember_label())
                        && ui::check_box(ui, "remember", label, remember).clicked
                    {
                        connect.remember = Some(!remember);
                    }
                }
            }
            Step::Browse => {
                let mount = connect.mount.as_ref();
                let place: Vec<&str> = mount
                    .map(|mount| {
                        [mount.host(), mount.share.as_str()]
                            .into_iter()
                            .chain(mount.root.split('/'))
                            .filter(|part| !part.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                ui.leaf(
                    "place",
                    Spec {
                        flags: Flags::CLIP,
                        size: [fill(), px(row)],
                        text: Some(&place.join(" \u{25b8} ")),
                        color: Some(theme.text_dim),
                        ..Spec::default()
                    },
                );
                let list = ui.open(
                    "entries",
                    Spec {
                        flags: Flags::SCROLL | Flags::CLIP,
                        axis: Axis::Y,
                        size: [fill(), px(row * ROWS + 8.0)],
                        fill: Some(theme.base),
                        border: Some(theme.chip),
                        radius: 4.0,
                        pad: [4.0, 4.0],
                        role: Some(Role::List),
                        ..Spec::default()
                    },
                );
                if let Some(node) = ui.access(list) {
                    node.set_label("Folders");
                }
                let shares = mount.is_some_and(|mount| mount.share.is_empty());
                for (index, folder) in connect.listing.folders.iter().enumerate() {
                    let entry = ui.leaf(
                        ("folder", folder),
                        Spec {
                            flags: Flags::CLICKABLE,
                            size: [fill(), px(row)],
                            icon: Some(if shares { art::SERVER } else { art::FOLDER }),
                            text: Some(folder),
                            hover_fill: Some(theme.hover()),
                            radius: 4.0,
                            pad: [8.0, 0.0],
                            gap: 6.0,
                            role: Some(Role::ListItem),
                            ..Spec::default()
                        },
                    );
                    if entry.clicked {
                        chosen = Some(index);
                    }
                }
                for section in &connect.listing.sections {
                    ui.leaf(
                        ("section", section),
                        Spec {
                            size: [fill(), px(row)],
                            icon: Some(art::SECTION),
                            text: Some(section.strip_suffix(".one").unwrap_or(section)),
                            color: Some(theme.text_dim),
                            pad: [8.0, 0.0],
                            gap: 6.0,
                            role: Some(Role::ListItem),
                            ..Spec::default()
                        },
                    );
                }
                if connect.listing.folders.is_empty() && connect.listing.sections.is_empty() {
                    ui.leaf(
                        "empty",
                        Spec {
                            size: [fill(), px(row)],
                            text: Some(if shares {
                                "No shared folders."
                            } else {
                                "No folders here."
                            }),
                            color: Some(theme.text_dim),
                            pad: [8.0, 0.0],
                            ..Spec::default()
                        },
                    );
                }
                ui.close();
            }
        }
        let status = match &connect.status {
            Status::Idle if connect.step == Step::Browse && connect.opens().is_none() => {
                Some(("Open a folder that holds a notebook.", theme.text_dim))
            }
            Status::Idle => None,
            Status::Waiting(waiting) => Some((*waiting, theme.text_dim)),
            Status::Failed(reason) => Some((reason.as_str(), theme.text)),
        };
        if let Some((text, color)) = status {
            ui.leaf(
                "status",
                Spec {
                    size: [fill(), ui::fit()],
                    text: Some(text),
                    overflow: ui::Overflow::Wrap,
                    pad: [0.0, 4.0],
                    color: Some(color),
                    role: Some(Role::Status),
                    ..Spec::default()
                },
            );
        }
        ui.open(
            "buttons",
            Spec {
                size: [fill(), children()],
                pad: [0.0, 8.0],
                gap: 8.0,
                ..Spec::default()
            },
        );
        if connect.step != Step::Address {
            back = ui::button(ui, "back", "Back").clicked;
        }
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let waiting = matches!(connect.status, Status::Waiting(_));
        let go = match connect.step {
            Step::Browse if connect.opens().is_none() => {
                ui.leaf(
                    "open",
                    Spec {
                        size: [ui::fit(), px(row)],
                        text: Some("Open"),
                        color: Some(theme.text_dim),
                        fill: Some(theme.chip),
                        radius: 4.0,
                        pad: [theme.font_size * 0.75, 0.0],
                        center: true,
                        role: Some(Role::Button),
                        ..Spec::default()
                    },
                );
                false
            }
            Step::Browse => ui::button(ui, "open", "Open").clicked,
            _ => ui::button(ui, "connect", "Connect").clicked || entered,
        };
        ui.close();
        ui.close();
        if cancel {
            self.ui.close_popup(id());
            self.ui.set_focus(Some(crate::page()));
            self.server = None;
            return;
        }
        let connect = self.server.as_mut().expect("the dialog is open");
        let request = if let Some(index) = chosen {
            connect.enter(index)
        } else if back {
            match connect.step {
                Step::Browse => connect.up().or_else(|| {
                    connect.step = Step::SignIn;
                    None
                }),
                _ => {
                    connect.asked += 1;
                    connect.step = Step::Address;
                    connect.status = Status::Idle;
                    None
                }
            }
        } else if go && !waiting {
            match connect.step {
                Step::Browse => {
                    if let Some((mount, login)) = connect.opens() {
                        return self.open_from_server(mount, login);
                    }
                    None
                }
                step => {
                    let request = connect.submit();
                    if step == Step::Address && connect.step == Step::SignIn {
                        let focus = if connect.user.is_empty() {
                            user_field()
                        } else {
                            password_field()
                        };
                        self.ui.focus_all(focus);
                    }
                    request
                }
            }
        } else {
            None
        };
        if let Some(request) = request {
            self.ask(request);
        }
    }
}

/// A row of `label` before the field `build` adds, as the Finder's sign-in lays them out.
fn labelled(ui: &mut Ui, label: &str, build: impl FnOnce(&mut Ui)) {
    let row = ui.theme.font_size * 2.0;
    ui.open(
        label,
        Spec {
            size: [fill(), px(row)],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "label",
        Spec {
            size: [px(110.0), px(row)],
            text: Some(label),
            ..Spec::default()
        },
    );
    build(ui);
    ui.close();
}

fn name(ui: &mut Ui, field: Id, label: &str) {
    if let Some(node) = ui.access(field) {
        node.set_label(label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A server of folders by share-relative path, taking one account.
    struct Fake {
        shares: Vec<&'static str>,
        folders: HashMap<&'static str, Vec<(&'static str, bool)>>,
        user: &'static str,
        password: &'static str,
    }

    impl Fake {
        fn signs_in(&self, login: &Login) -> io::Result<()> {
            if login.user == self.user && login.password == self.password {
                Ok(())
            } else {
                Err(io::Error::other(Refusal::SignIn))
            }
        }
    }

    impl Reach for Fake {
        fn shares(&self, _: &Mount, login: &Login) -> io::Result<Vec<String>> {
            self.signs_in(login)?;
            Ok(self.shares.iter().map(|share| share.to_string()).collect())
        }

        fn list(&self, mount: &Mount, login: &Login) -> io::Result<Vec<DirectoryEntry>> {
            self.signs_in(login)?;
            let path = format!("{}/{}", mount.share, mount.root);
            let entries = (self.folders.get(path.trim_end_matches('/')))
                .ok_or_else(|| io::Error::other(Refusal::NoFolder))?;
            Ok(entries
                .iter()
                .map(|(name, folder)| DirectoryEntry {
                    name: name.to_string(),
                    size: 0,
                    modified: 0,
                    attributes: if *folder { 0x10 } else { 0x20 },
                })
                .collect())
        }
    }

    fn fake() -> Fake {
        Fake {
            shares: vec!["public", "Notes"],
            folders: HashMap::from([
                (
                    "Notes",
                    vec![
                        ("Garden", true),
                        (".snapshots", true),
                        ("readme.txt", false),
                    ],
                ),
                (
                    "Notes/Garden",
                    vec![
                        ("Open Notebook.onetoc2", false),
                        ("Beds.one", false),
                        ("OneNote_RecycleBin", true),
                    ],
                ),
            ]),
            user: "clover",
            password: "hunter2",
        }
    }

    /// Makes the request `ask` makes and takes its reply.
    fn run(connect: &mut Connect, ask: impl FnOnce(&mut Connect) -> Option<Request>) -> bool {
        let request = ask(connect).expect("a request");
        connect.receive(request.run(&fake()))
    }

    #[test]
    fn addresses_read_as_urls_unc_paths_or_bare_names() {
        let mount = |text| Mount::from_address(text);
        let garden = Mount {
            server: "nas.local".into(),
            share: "Notes".into(),
            user: None,
            domain: String::new(),
            root: "Personal/My Garden".into(),
        };
        assert_eq!(
            mount("smb://nas.local/Notes/Personal/My%20Garden/"),
            Some(garden.clone())
        );
        assert_eq!(
            mount(r"\\nas.local\Notes\Personal\My Garden"),
            Some(garden.clone())
        );
        assert_eq!(
            mount("  //nas.local/Notes/Personal/My Garden"),
            Some(garden.clone())
        );
        assert_eq!(
            mount("nas.local/Notes/Personal/My Garden"),
            Some(garden.clone())
        );
        assert_eq!(
            mount("SMB://nas.local/Notes/Personal/My Garden"),
            Some(garden)
        );
        let account = mount("smb://WORK;clover:secret@10.0.0.2:1445").unwrap();
        assert_eq!(
            (
                account.user.as_deref(),
                account.domain.as_str(),
                account.share.as_str()
            ),
            (Some("clover"), "WORK", "")
        );
        assert_eq!(
            (account.host(), account.endpoint().as_str()),
            ("10.0.0.2", "10.0.0.2:1445")
        );
        assert_eq!(mount("nas").unwrap().endpoint(), "nas:445");
        assert_eq!(mount("smb://GUEST:@nas/public").unwrap().user, None);
        assert_eq!(mount(""), None);
        assert_eq!(mount("smb:///share"), None);
        assert_eq!(mount("two words/share"), None);
    }

    #[test]
    fn addresses_kept_read_back_without_the_password() {
        let mount =
            Mount::from_address("smb://WORK;clover:secret@nas:1445/Notes/My Garden").unwrap();
        let url = mount.url();
        assert_eq!(url, "smb://WORK;clover@nas:1445/Notes/My%20Garden");
        assert_eq!(Mount::from_address(&url), Some(mount));
        assert_eq!(
            crate::library::server_address("smb://nas/Notes").map(|mount| mount.share),
            Some("Notes".to_owned())
        );
        assert_eq!(crate::library::server_address("/Volumes/Notes"), None);
    }

    /// Address, sign-in, the server's shares, a share's folders, then a notebook to open.
    #[test]
    fn signing_in_browses_to_a_notebook() {
        let mut connect = Connect::new("smb://nas".into(), Some(true));
        assert!(connect.submit().is_none());
        assert_eq!(connect.step, Step::SignIn);
        assert!(connect.submit().is_none(), "no name and not a guest");
        assert!(matches!(connect.status, Status::Failed(_)));
        connect.user = "clover".into();
        connect.password = "wrong".into();
        assert!(!run(&mut connect, Connect::submit));
        assert_eq!(connect.step, Step::SignIn);
        assert_eq!(
            connect.status,
            Status::Failed("The name or password is incorrect.".into())
        );
        assert!(connect.password.is_empty());
        connect.password = "hunter2".into();
        assert!(run(&mut connect, Connect::submit), "signed in");
        assert_eq!(connect.listing.folders, ["Notes", "public"]);
        assert!(connect.opens().is_none());
        assert!(!run(&mut connect, |connect| connect.enter(0)));
        assert_eq!(
            connect.listing.folders,
            ["Garden"],
            "hidden and plain files left out"
        );
        assert!(connect.opens().is_none());
        run(&mut connect, |connect| connect.enter(0));
        assert_eq!(connect.listing.sections, ["Beds.one"]);
        assert!(
            connect.listing.folders.is_empty(),
            "the recycle bin is left out"
        );
        let (mount, login) = connect.opens().unwrap();
        assert_eq!(mount.url(), "smb://clover@nas/Notes/Garden");
        assert_eq!(
            (login.user.as_str(), login.password.as_str()),
            ("clover", "hunter2")
        );
        run(&mut connect, Connect::up);
        run(&mut connect, Connect::up);
        assert_eq!(connect.listing.folders, ["Notes", "public"]);
        assert!(connect.up().is_none(), "the server's shares are the top");
    }

    #[test]
    fn a_missing_folder_or_a_stale_reply_leaves_the_browser_as_it_was() {
        let mut connect = Connect::new("smb://clover@nas/Notes/Gone".into(), None);
        connect.submit();
        assert_eq!(connect.user, "clover");
        connect.password = "hunter2".into();
        assert!(!run(&mut connect, Connect::submit));
        assert_eq!(connect.step, Step::SignIn, "the address named the folder");
        assert_eq!(
            connect.status,
            Status::Failed(
                "\u{201c}Gone\u{201d} isn\u{2019}t on the share. Check the address.".into()
            )
        );
        connect.mount.as_mut().unwrap().root.clear();
        connect.password = "hunter2".into();
        run(&mut connect, Connect::submit);
        let stale = connect.enter(0).unwrap();
        let current = connect.up().unwrap();
        assert!(!connect.receive(stale.run(&fake())));
        assert_eq!(connect.status, Status::Waiting("Loading\u{2026}"));
        connect.receive(current.run(&fake()));
        assert_eq!(connect.listing.folders, ["Notes", "public"]);
    }

    #[test]
    fn guests_sign_in_without_an_account() {
        let mut connect = Connect::new("nas/Notes".into(), None);
        connect.submit();
        connect.guest = true;
        connect.user = "clover".into();
        let request = connect.submit().unwrap();
        assert_eq!(
            (request.mount.user.as_deref(), request.login.user.as_str()),
            (None, "")
        );
        connect.receive(request.run(&fake()));
        assert_eq!(
            connect.status,
            Status::Failed(
                "\u{201c}nas\u{201d} doesn\u{2019}t allow guests. Sign in with a name and password."
                    .into()
            )
        );
    }
}
