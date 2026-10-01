//! iCloud Drive: notebooks in folders it keeps, their section files read and written under
//! file coordination (so the iCloud daemon never swaps a file mid-commit), the conflict
//! versions it keeps beside a file merged into it, and the app's own folder in iCloud Drive.
//! Mac OS X 10.9 and earlier have no iCloud Drive; nothing here runs there.

use block2::{RcBlock, StackBlock};
use notebook::{
    Remote, Version,
    session::{Background, Notebook, Section},
};
use objc2::{
    ClassType, DeclaredClass, declare_class, msg_send_id, mutability,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_foundation::{
    NSFileCoordinator, NSFileCoordinatorReadingOptions, NSFileCoordinatorWritingOptions,
    NSFileManager, NSFilePresenter, NSFileVersion, NSNotification, NSNotificationCenter,
    NSOperationQueue, NSString, NSURL, NSURLIsUbiquitousItemKey,
    NSURLUbiquitousItemDownloadingStatusKey, NSURLUbiquitousItemDownloadingStatusNotDownloaded,
};
use onestore::{CommitError, CommitState, Stamp, Transaction};
use std::{
    cell::Cell,
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    ptr::NonNull,
    sync::Mutex,
    time::Duration,
};

/// The app's iCloud container, which iCloud Drive shows as a folder named Snowbound.
const CONTAINER: &str = "iCloud.net.paperclover.snowbound";

/// How long local edits in iCloud Drive wait for a pause in typing before they publish:
/// each publication is an upload, and one concurrent with another device's is a conflict
/// version.
pub const PAUSE: Duration = Duration::from_secs(3);

/// Whether this Mac has iCloud Drive at all: Mac OS X 10.10 (Darwin 14) and later.
fn available() -> bool {
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    (unsafe { libc::uname(&mut name) }) == 0
        && unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) }
            .to_str()
            .ok()
            .and_then(|release| release.split('.').next()?.parse::<u32>().ok())
            .is_some_and(|major| major >= 14)
}

fn url(path: &Path) -> Option<Retained<NSURL>> {
    Some(unsafe { NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)) })
}

fn resource(url: &NSURL, key: &NSString) -> Option<Retained<AnyObject>> {
    let mut value = None;
    unsafe { url.getResourceValue_forKey_error(&mut value, key) }.ok()?;
    value
}

/// Whether iCloud Drive keeps the file or folder at `path`, as first asked: the answer
/// outlasts a signed-out account's folder, which is then gone.
pub fn ubiquitous(path: &Path) -> bool {
    static KNOWN: Mutex<BTreeMap<PathBuf, bool>> = Mutex::new(BTreeMap::new());
    let Ok(mut known) = KNOWN.lock() else {
        return false;
    };
    *known.entry(path.to_owned()).or_insert_with(|| {
        available()
            && url(path)
                .and_then(|url| resource(&url, unsafe { NSURLIsUbiquitousItemKey }))
                .is_some_and(|value| {
                    let yes: bool = unsafe { objc2::msg_send![&value, boolValue] };
                    yes
                })
    })
}

/// The app's own folder in iCloud Drive as `look_up` last found it, and what reports its
/// notebooks coming and going.
static FOLDER: Mutex<Option<(PathBuf, Option<Presenter>)>> = Mutex::new(None);

/// The app's own folder in iCloud Drive (its container's Documents, shown as iCloud Drive's
/// Snowbound folder), as `look_up` last found it: while the app carries the container's
/// entitlement and iCloud Drive is on.
pub fn folder() -> Option<PathBuf> {
    Some(FOLDER.lock().ok()?.as_ref()?.0.clone())
}

/// Looks for the app's own folder in iCloud Drive on a thread of its own, as asking the
/// iCloud daemon can take a while, then calls `changed`, and again whenever a folder at its
/// top comes or goes, as another device adds or removes a notebook.
pub fn look_up(changed: impl Fn() + Send + Sync + 'static) {
    std::thread::spawn(move || {
        let changed = std::sync::Arc::new(changed);
        let found = container().map(|root| {
            let report = std::sync::Arc::clone(&changed);
            let presenter = presenter(&root, move |paths| {
                if paths.iter().any(|path| !path.contains('/')) {
                    report();
                }
            });
            (root, presenter)
        });
        if let Ok(mut folder) = FOLDER.lock() {
            *folder = found;
        }
        changed();
    });
}

/// The container's Documents, or for trying the app out without its entitlement, the folder
/// `SNOWBOUND_ICLOUD_FOLDER` names in its place.
fn container() -> Option<PathBuf> {
    if let Some(folder) = std::env::var_os("SNOWBOUND_ICLOUD_FOLDER") {
        return Some(folder.into());
    }
    if !available() {
        return None;
    }
    let manager = unsafe { NSFileManager::defaultManager() };
    let url =
        unsafe { manager.URLForUbiquityContainerIdentifier(Some(&NSString::from_str(CONTAINER))) }?;
    let documents = PathBuf::from(unsafe { url.path() }?.to_string()).join("Documents");
    std::fs::create_dir_all(&documents).ok()?;
    Some(documents)
}

/// The top of iCloud Drive, where it is on.
pub fn drive() -> Option<PathBuf> {
    if !available() {
        return None;
    }
    let drive = PathBuf::from(std::env::var_os("HOME")?)
        .join("Library/Mobile Documents/com~apple~CloudDocs");
    drive.is_dir().then_some(drive)
}

/// `work` within a coordinated read or, with `write`, write of `path`, as the iCloud daemon
/// waits for it and it for the daemon.
fn coordinated<T>(path: &Path, write: bool, work: impl FnOnce() -> T) -> io::Result<T> {
    let url = url(path).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let coordinator =
        unsafe { NSFileCoordinator::initWithFilePresenter(NSFileCoordinator::alloc(), None) };
    let (work, done) = (Cell::new(Some(work)), Cell::new(None));
    let accessor = StackBlock::new(|_: NonNull<NSURL>| {
        if let Some(work) = work.take() {
            done.set(Some(work()));
        }
    });
    let mut error = None;
    unsafe {
        if write {
            coordinator.coordinateWritingItemAtURL_options_error_byAccessor(
                &url,
                NSFileCoordinatorWritingOptions::NSFileCoordinatorWritingForMerging,
                Some(&mut error),
                &accessor,
            );
        } else {
            coordinator.coordinateReadingItemAtURL_options_error_byAccessor(
                &url,
                NSFileCoordinatorReadingOptions::empty(),
                Some(&mut error),
                &accessor,
            );
        }
    }
    done.take().ok_or_else(|| {
        io::Error::other(error.map_or_else(
            || "The file could not be coordinated".to_owned(),
            |error| error.localizedDescription().to_string(),
        ))
    })
}

/// Asks for a file iCloud Drive keeps elsewhere, which a read would otherwise wait for: the
/// sync thread tries again later, while the section's replica serves it.
fn present(path: &Path) -> io::Result<()> {
    let Some(url) = url(path) else {
        return Ok(());
    };
    let status = resource(&url, unsafe { NSURLUbiquitousItemDownloadingStatusKey });
    let missing = status.is_some_and(|status| {
        let equal: bool = unsafe {
            objc2::msg_send![&status, isEqual: NSURLUbiquitousItemDownloadingStatusNotDownloaded]
        };
        equal
    });
    if !missing {
        return Ok(());
    }
    let manager = unsafe { NSFileManager::defaultManager() };
    let _ = unsafe { manager.startDownloadingUbiquitousItemAtURL_error(&url) };
    Err(io::Error::new(
        io::ErrorKind::NotConnected,
        "Downloading from iCloud Drive",
    ))
}

/// Asks iCloud Drive for every file below `root` it keeps elsewhere, dataless or as a
/// `.Name.icloud` placeholder: how many are not here yet.
pub fn download(root: &Path) -> usize {
    use std::os::macos::fs::MetadataExt;
    let manager = unsafe { NSFileManager::defaultManager() };
    let mut missing = 0;
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let (path, name) = (entry.path(), entry.file_name());
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                folders.push(path);
                continue;
            }
            let placeholder = name
                .to_str()
                .and_then(|name| name.strip_prefix('.')?.strip_suffix(".icloud"));
            let file = match placeholder {
                Some(real) => path.with_file_name(real),
                None if metadata.st_flags() & SF_DATALESS != 0 => path,
                None => continue,
            };
            missing += 1;
            if let Some(url) = url(&file) {
                let _ = unsafe { manager.startDownloadingUbiquitousItemAtURL_error(&url) };
            }
        }
    }
    missing
}

/// `st_flags` of a file whose contents the system keeps elsewhere until read.
const SF_DATALESS: u32 = 0x4000_0000;

/// A section file in iCloud Drive, read and published under file coordination, with the
/// conflict versions iCloud keeps beside it.
pub struct Coordinated(pub PathBuf);

fn uncommitted(error: io::Error) -> CommitError {
    CommitError {
        state: CommitState::NotCommitted,
        error,
    }
}

impl Coordinated {
    fn conflicts(&self) -> io::Result<Vec<Retained<NSFileVersion>>> {
        let url = url(&self.0).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(
            unsafe { NSFileVersion::unresolvedConflictVersionsOfItemAtURL(&url) }
                .map(|versions| versions.to_vec_retained())
                .unwrap_or_default(),
        )
    }

    fn conflict(&self, id: &str) -> io::Result<Retained<NSFileVersion>> {
        self.conflicts()?
            .into_iter()
            .find(|version| version_path(version).as_deref() == Some(Path::new(id)))
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

fn version_path(version: &NSFileVersion) -> Option<PathBuf> {
    Some(PathBuf::from(unsafe { version.URL().path() }?.to_string()))
}

impl Remote for Coordinated {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        present(&self.0)?;
        coordinated(&self.0, false, || onestore::read_file(&self.0))?
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        present(&self.0)?;
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

    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        present(&self.0).map_err(uncommitted)?;
        coordinated(&self.0, true, || transaction.commit_file(&self.0)).map_err(uncommitted)?
    }

    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        coordinated(&self.0, true, || onestore::confirm_file(&self.0, base)).map_err(uncommitted)?
    }

    fn versions(&mut self) -> io::Result<Vec<Version>> {
        Ok(self
            .conflicts()?
            .into_iter()
            .filter_map(|version| {
                Some(Version {
                    id: version_path(&version)?.to_string_lossy().into_owned(),
                    device: unsafe { version.localizedNameOfSavingComputer() }
                        .map(|name| name.to_string()),
                })
            })
            .collect())
    }

    fn version(&mut self, id: &str) -> io::Result<Vec<u8>> {
        let version = self.conflict(id)?;
        if !unsafe { version.hasLocalContents() } {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Downloading another device’s version",
            ));
        }
        std::fs::read(id)
    }

    fn retire(&mut self, id: &str, keep: bool) -> io::Result<()> {
        let version = match self.conflict(id) {
            Ok(version) => version,
            // Another device resolved it first.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        coordinated(&self.0, true, || {
            if keep {
                // Beside the file, under a name of its own: "Section (Clover's iPhone).one".
                let device = unsafe { version.localizedNameOfSavingComputer() }
                    .map_or_else(|| "another device".to_owned(), |name| name.to_string());
                let stem = self.0.file_stem().unwrap_or_default().to_string_lossy();
                let folder = self.0.parent().unwrap_or(Path::new("."));
                let kept = (1..)
                    .map(|n| match n {
                        1 => folder.join(format!("{stem} ({device}).one")),
                        n => folder.join(format!("{stem} ({device} {n}).one")),
                    })
                    .find(|kept| !kept.exists())
                    .unwrap_or_default();
                std::fs::write(kept, std::fs::read(id)?)?;
            }
            unsafe {
                version.setResolved(true);
                version
                    .removeAndReturnError()
                    .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
            }
        })?
    }
}

/// Opens the section at catalog `path` of the notebook in iCloud Drive's folder `root`. One
/// without a replica here yet waits for iCloud Drive to bring its file down, which the sync
/// thread's reads would not.
pub fn section(
    notebook: &Notebook,
    root: &Path,
    path: &str,
    key: Option<&onestore::protected::Key>,
    notify: impl Fn() + Send + 'static,
) -> Result<Section, notebook::Error> {
    if !notebook.replica_path(path)?.exists() {
        fetch(&root.join(path))?;
    }
    let connect = |file: &Path| Ok(Coordinated(file.to_owned()));
    let section = match key {
        Some(key) => notebook.section_unlocked_with(path, key, connect, notify)?,
        None => notebook.section_with(path, connect, notify)?,
    };
    section.set_pause(PAUSE);
    Ok(section)
}

/// Waits for iCloud Drive to bring down `file`, where it keeps it elsewhere.
fn fetch(file: &Path) -> io::Result<()> {
    // Reading any of a dataless file brings all of it down.
    coordinated(file, false, || {
        use std::io::Read;
        std::fs::File::open(file)?.read(&mut [0]).map(drop)
    })?
}

/// Opens a section file in iCloud Drive on its own, once here.
pub fn lone_section(
    file: &Path,
    cache: &Path,
    notify: impl Fn() + Send + 'static,
) -> Result<Section, notebook::Error> {
    fetch(file)?;
    let section = Section::open_with(file, cache, |file| Ok(Coordinated(file.to_owned())), notify)?;
    section.set_pause(PAUSE);
    Ok(section)
}

/// Keeps a notebook in iCloud Drive in sync while its sections are not open: offline copies
/// of every section, as a folder kept elsewhere needs, each section checked every
/// `Background::UNWATCHED` for conflict versions, which change no file, and at once when the
/// presenter or the folder's watch reports it.
pub fn background(
    notebook: &Notebook,
    notify: impl Fn() + Send + 'static,
) -> Result<Background, notebook::Error> {
    notebook.background_with(false, true, |file| Coordinated(file.to_owned()), notify)
}

struct Ivars {
    url: Retained<NSURL>,
    root: PathBuf,
    queue: Retained<NSOperationQueue>,
    changed: Box<dyn Fn(Vec<String>) + Send + Sync>,
}

declare_class!(
    /// Hears what other processes, the iCloud daemon among them, do to a notebook folder.
    struct FolderPresenter;

    unsafe impl ClassType for FolderPresenter {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "SnowboundFolderPresenter";
    }

    impl DeclaredClass for FolderPresenter {
        type Ivars = Ivars;
    }

    unsafe impl NSObjectProtocol for FolderPresenter {}

    unsafe impl NSFilePresenter for FolderPresenter {
        #[method_id(presentedItemURL)]
        unsafe fn presented_item_url(&self) -> Option<Retained<NSURL>> {
            Some(self.ivars().url.clone())
        }

        #[method_id(presentedItemOperationQueue)]
        unsafe fn presented_item_operation_queue(&self) -> Retained<NSOperationQueue> {
            self.ivars().queue.clone()
        }

        #[method(presentedSubitemDidChangeAtURL:)]
        unsafe fn subitem_did_change(&self, url: &NSURL) {
            self.report(url);
        }

        #[method(presentedSubitemDidAppearAtURL:)]
        unsafe fn subitem_did_appear(&self, url: &NSURL) {
            self.report(url);
        }

        #[method(presentedSubitemAtURL:didGainVersion:)]
        unsafe fn subitem_did_gain_version(&self, url: &NSURL, _: &NSFileVersion) {
            self.report(url);
        }

        #[method(presentedSubitemAtURL:didMoveToURL:)]
        unsafe fn subitem_did_move(&self, from: &NSURL, to: &NSURL) {
            self.report(from);
            self.report(to);
        }

        #[method(accommodatePresentedSubitemDeletionAtURL:completionHandler:)]
        unsafe fn accommodate_subitem_deletion(
            &self,
            url: &NSURL,
            done: &block2::Block<dyn Fn(*mut objc2_foundation::NSError)>,
        ) {
            self.report(url);
            done.call((std::ptr::null_mut(),));
        }

        #[method(presentedSubitemAtURL:didResolveConflictVersion:)]
        unsafe fn subitem_did_resolve_conflict_version(
            &self,
            url: &NSURL,
            _: &NSFileVersion,
        ) {
            self.report(url);
        }
    }
);

impl FolderPresenter {
    fn report(&self, url: &NSURL) {
        let Some(path) = (unsafe { url.path() }) else {
            return;
        };
        let path = PathBuf::from(path.to_string());
        let ivars = self.ivars();
        let relative = path
            .strip_prefix(&ivars.root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        (ivars.changed)(vec![relative]);
    }
}

/// Reports changes to a folder in iCloud Drive until dropped.
pub struct Presenter(Retained<FolderPresenter>);

// The presenter is only registered and removed, each safe from any thread.
unsafe impl Send for Presenter {}
unsafe impl Sync for Presenter {}

/// Hears the changes other processes make below the folder at `root`, and the conflict
/// versions iCloud Drive gains for its files: `changed` hears their paths relative to it,
/// `/`-separated.
pub fn presenter(
    root: &Path,
    changed: impl Fn(Vec<String>) + Send + Sync + 'static,
) -> Option<Presenter> {
    if !available() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let queue = unsafe { NSOperationQueue::new() };
    unsafe { queue.setMaxConcurrentOperationCount(1) };
    let presenter = FolderPresenter::alloc().set_ivars(Ivars {
        url: url(&root)?,
        root,
        queue,
        changed: Box::new(changed),
    });
    let presenter: Retained<FolderPresenter> = unsafe { msg_send_id![super(presenter), init] };
    unsafe { NSFileCoordinator::addFilePresenter(ProtocolObject::from_ref(&*presenter)) };
    Some(Presenter(presenter))
}

impl Drop for Presenter {
    fn drop(&mut self) {
        unsafe {
            NSFileCoordinator::removeFilePresenter(ProtocolObject::from_ref(&*self.0));
            self.0.ivars().queue.waitUntilAllOperationsAreFinished();
        }
    }
}

/// Calls `changed` on the main thread whenever the iCloud account signs out, signs in or
/// switches, or iCloud Drive is turned off, for as long as the app runs.
pub fn on_account_change(changed: impl Fn() + 'static) {
    if !available() {
        return;
    }
    let block = RcBlock::new(move |_: NonNull<NSNotification>| changed());
    unsafe {
        let center = NSNotificationCenter::defaultCenter();
        let observer = center.addObserverForName_object_queue_usingBlock(
            Some(&NSString::from_str(
                "NSUbiquityIdentityDidChangeNotification",
            )),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        );
        // Observes for the app's life.
        std::mem::forget(observer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A notebook in a scratch folder of this Mac's iCloud Drive opens, publishes an edit under
    /// file coordination and reads back; the folder is deleted after.
    #[test]
    #[ignore = "writes a scratch folder in this Mac's iCloud Drive"]
    fn a_notebook_in_icloud_drive_publishes_under_coordination() {
        let drive = drive().expect("iCloud Drive is on");
        let root = drive.join(format!("snowbound-icloud-test-{}", std::process::id()));
        let cache =
            std::env::temp_dir().join(format!("snowbound-icloud-cache-{}", std::process::id()));
        let page = onestore::PageCreation::new(None, Some(""), "Test").unwrap();
        let result = std::panic::catch_unwind(|| {
            let notebook = Notebook::create(&root, &cache, Notebook::NEW_COLOR, &page).unwrap();
            assert!(ubiquitous(&root));
            let path = notebook.catalog().sections[0].path.clone();
            let section = section(&notebook, &root, &path, None, || {}).unwrap();
            let (space, ..) = section.pages().unwrap()[0].clone();
            let title = section
                .page(space)
                .unwrap()
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
                    "Test",
                    onestore::op::Edit {
                        at: 134_030_000_000_000_000,
                        ops: vec![onestore::op::Op::Page {
                            space,
                            op: onestore::op::PageOp::Text {
                                text: title,
                                range: 0..0,
                                with: "In iCloud".into(),
                            },
                        }],
                    },
                )
                .unwrap();
            // Wakes skip the pause in typing, as leaving the window does.
            let file = root.join(&path);
            let mut remote = Coordinated(file.clone());
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                section.wake();
                let arena = onestore::Arena::default();
                let titles = onestore::Section::open(&arena, remote.read().unwrap())
                    .unwrap()
                    .pages()
                    .unwrap();
                if titles[0].1 == "In iCloud" {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{titles:?}");
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(remote.versions().unwrap().is_empty());
            section.close().unwrap();
        });
        let root = root.as_path();
        assert!(root.starts_with(&drive) && root != drive);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(&cache);
        result.unwrap();
    }
}
