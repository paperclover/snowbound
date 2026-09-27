//! Open notebooks: where each lives and the sections its tabs offer.

use notebook::discover::{Folder, SectionState};
use notebook::session::{Notebook, Section};
use notebook::smb::{Client, Credentials};
use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

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

impl Mount {
    /// The mount of `source` (`//[domain;][user[:…]@]server/share[/folder]`), holding the
    /// folder `within` its mount point; `options` are the mount's (`username=`, `user=`,
    /// `domain=`), where the source names no account.
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

impl Server {
    fn connect(&self) -> io::Result<Client> {
        Client::connect(
            &self.mount.server,
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
}

impl Library {
    /// The notebook in the folder at `location`, or why it cannot be read. A folder on a
    /// mounted SMB share opens through Snowbound's own SMB client, signed in with the
    /// account the system keeps for the mount, as OneNote's own client coordinates with
    /// OneNote; without that account it opens through the mount.
    pub fn notebook(location: &str, cache: &Path) -> Self {
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
        Self {
            location: location.to_owned(),
            name: file_name(Path::new(location)),
            notebook: Notebook::open(location, cache)
                .map(Some)
                .map_err(|error| error.to_string()),
            server: None,
            notice,
            cache: cache.to_owned(),
        }
    }

    /// The notebook at `location`, which `mount` names on its server, opened through the
    /// embedded client signed in as `login`.
    pub fn on_share(
        location: &str,
        mount: Mount,
        login: Login,
        cache: &Path,
    ) -> Result<Self, String> {
        let server = Arc::new(Server { mount, login });
        let client = server.connect().map_err(|error| error.to_string())?;
        let notebook = Notebook::open_smb(Arc::new(client), &server.mount.root, cache)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            location: location.to_owned(),
            name: file_name(Path::new(location)),
            notebook: Ok(Some(notebook)),
            server: Some(server),
            notice: None,
            cache: cache.to_owned(),
        })
    }

    /// This notebook read again, the way it was opened, for changing its structure.
    pub fn reopen(&self) -> Result<Notebook, Box<dyn Error>> {
        Ok(match &self.server {
            Some(server) => {
                Notebook::open_smb(Arc::new(server.connect()?), &server.mount.root, &self.cache)?
            }
            None => Notebook::open(&self.location, &self.cache)?,
        })
    }

    /// This notebook as `notebook`, read again after a change.
    pub fn with(&self, notebook: Notebook) -> Self {
        Self {
            location: self.location.clone(),
            name: self.name.clone(),
            notebook: Ok(Some(notebook)),
            server: self.server.clone(),
            notice: self.notice.clone(),
            cache: self.cache.clone(),
        }
    }

    /// A notebook `Notebook::create` just made in the folder at `location`.
    pub fn created(location: &str, notebook: Notebook, cache: &Path) -> Self {
        Self {
            location: location.to_owned(),
            name: file_name(Path::new(location)),
            notebook: Ok(Some(notebook)),
            server: None,
            notice: None,
            cache: cache.to_owned(),
        }
    }

    /// The section file at `file` as a notebook of one tab.
    pub fn section(file: &Path, cache: &Path) -> Self {
        Self {
            location: file.to_string_lossy().into_owned(),
            name: file_name(file.parent().unwrap_or(file)),
            notebook: Ok(None),
            server: None,
            notice: None,
            cache: cache.to_owned(),
        }
    }

    /// Opens the section at catalog `path`; on a share, through a replica named by its
    /// file identity that publishes over the embedded client.
    pub fn open(
        &self,
        path: &str,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Section, Box<dyn Error>> {
        Ok(match (&self.notebook, &self.server) {
            (Ok(Some(notebook)), Some(server)) => {
                let identity = self
                    .catalog_section(notebook.catalog(), path)
                    .ok_or("The notebook doesn’t list this section")?;
                let file = match server.mount.root.as_str() {
                    "" => path.to_owned(),
                    root => format!("{root}/{path}"),
                };
                let cache = self.cache.join("smb").join(format!("{identity}.sqlite"));
                std::fs::create_dir_all(self.cache.join("smb"))?;
                let replica = if cache.exists() {
                    notebook::Replica::open(&cache)?
                } else {
                    notebook::Replica::create(
                        &cache,
                        &server.connect()?.read_storage(&file, LIMIT)?,
                    )?
                };
                let server = Arc::clone(server);
                Section::resume_smb(file, replica, LIMIT, move || server.connect(), notify)?
            }
            (Ok(Some(notebook)), None) => notebook.section(path, notify)?,
            (Ok(None), _) => Section::open(path, &self.cache, notify)?,
            (Err(error), _) => return Err(error.clone().into()),
        })
    }

    /// The file identity, in hex, of the section at catalog `path`.
    fn catalog_section(&self, catalog: &Folder, path: &str) -> Option<String> {
        let mut folders = vec![catalog];
        while let Some(folder) = folders.pop() {
            if let Some(section) = folder.sections.iter().find(|section| section.path == path) {
                return Some(
                    section
                        .file_id
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                );
            }
            folders.extend(&folder.groups);
        }
        None
    }

    /// Names the section at catalog `path` across every open notebook, for remembering
    /// its pages.
    pub fn key(&self, path: &str) -> String {
        format!("{}\n{path}", self.location)
    }

    /// The readable sections of the folder at catalog path `folder`, as tabs in its order.
    pub fn tabs(&self, folder: &str) -> Vec<Tab> {
        match &self.notebook {
            Ok(Some(notebook)) => {
                let mut folders = vec![notebook.catalog()];
                while let Some(candidate) = folders.pop() {
                    if candidate.path == folder {
                        return tabs(candidate);
                    }
                    folders.extend(&candidate.groups);
                }
                Vec::new()
            }
            Ok(None) => vec![Tab {
                path: self.location.clone(),
                name: section_name(&self.location, &None),
                color: None,
            }],
            Err(_) => Vec::new(),
        }
    }

    /// The first readable section, searching groups after sections, as OneNote opens a
    /// notebook.
    pub fn first_section(&self) -> Option<String> {
        let Ok(Some(notebook)) = &self.notebook else {
            return self.tabs("").pop().map(|tab| tab.path);
        };
        let mut folders = std::collections::VecDeque::from([notebook.catalog()]);
        while let Some(folder) = folders.pop_front() {
            if let Some(tab) = tabs(folder).into_iter().next() {
                return Some(tab.path);
            }
            folders.extend(folder.groups.iter().filter(|group| !recycle_bin(group)));
        }
        None
    }

    /// Whether the notebook lists a section at catalog path `path`.
    pub fn contains(&self, path: &str) -> bool {
        let Some(catalog) = self.catalog() else {
            return self.location == path;
        };
        let mut folders = vec![catalog];
        while let Some(folder) = folders.pop() {
            if folder.sections.iter().any(|section| section.path == path) {
                return true;
            }
            folders.extend(&folder.groups);
        }
        false
    }

    /// The notebook's folders of sections, for the sidebar.
    pub fn catalog(&self) -> Option<&Folder> {
        match &self.notebook {
            Ok(Some(notebook)) => Some(notebook.catalog()),
            _ => None,
        }
    }
}

/// Whether `folder` is the notebook's recycle bin, which OneNote keeps out of its lists.
pub fn recycle_bin(folder: &Folder) -> bool {
    folder.path.rsplit('/').next() == Some("OneNote_RecycleBin")
}

pub fn file_name(path: &Path) -> String {
    path.canonicalize()
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
            SectionState::Readable { name, color } => Some(Tab {
                name: section_name(&section.path, name),
                path: section.path.clone(),
                color: *color,
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
    Nothing,
}

/// Whether `folder` holds a table of contents, as a notebook's or a section group's does.
fn has_toc(folder: &Path) -> bool {
    std::fs::read_dir(folder).is_ok_and(|entries| {
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
/// section groups included, or alone.
pub fn locate(path: &Path) -> Located {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        _ if path.is_dir() => Located::Notebook {
            root: path.to_owned(),
            section: None,
        },
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

    /// A notebook on a share opens, edits and publishes through the embedded client, as a
    /// mounted share's notebook does: `ONESTORE_SMB_LAB=127.0.0.1:PORT` (a disposable Samba
    /// share `agent`, guest access; `tools/w7/linux_vm.py up NAME`).
    #[test]
    #[ignore = "requires an owned Samba share at ONESTORE_SMB_LAB"]
    fn a_notebook_on_a_share_opens_through_the_embedded_client() {
        let address = std::env::var("ONESTORE_SMB_LAB").unwrap();
        let root = format!("snowbound-{}", std::process::id());
        let mount = Mount {
            server: address,
            share: "agent".into(),
            user: None,
            domain: String::new(),
            root: root.clone(),
        };
        let login = Login::guest(&mount);
        let server = Server {
            mount: mount.clone(),
            login: login.clone(),
        };
        let client = Arc::new(server.connect().unwrap());
        client.create_directory(&root).unwrap();
        let cache = std::env::temp_dir().join(format!("snowbound-share-{}", std::process::id()));
        let page = onestore::PageCreation::new(None, Some(""), "Rust Author").unwrap();
        Notebook::open_smb(Arc::clone(&client), &root, &cache)
            .unwrap()
            .create_section("", "New Section 1", &page)
            .unwrap();
        let library = Library::on_share("/Volumes/agent/lab", mount, login, &cache).unwrap();
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
            let bytes = client.read_storage(&file, LIMIT).unwrap();
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
        let _ = std::fs::remove_dir_all(&cache);
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

    #[test]
    fn chosen_paths_open_their_notebook_or_section() {
        let root = std::env::temp_dir().join(format!("snowbound-locate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let notebook = root.join("Notebook");
        std::fs::create_dir_all(notebook.join("Group")).unwrap();
        for file in [
            "Open Notebook.onetoc2",
            "Group/Open Notebook.onetoc2",
            "Group/Inner.one",
            "Top.one",
        ] {
            std::fs::write(notebook.join(file), b"").unwrap();
        }
        std::fs::write(root.join("Loose.one"), b"").unwrap();
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
        assert_eq!(locate(&root.join("missing.txt")), Located::Nothing);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
