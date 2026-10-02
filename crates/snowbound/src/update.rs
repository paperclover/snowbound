//! Snowbound updating itself. Every published build keeps its own folder under `BASE`, with a
//! `build.json` the release key signs; `latest.json` names each platform's newest build. A
//! thread checks on launch and daily, downloads a newer build for this platform, verifies
//! it, and unpacks it beside the install. Restart to Update swaps it in once the app quits.
//! `tools/RELEASE.md` describes the publishing side. In the browser an update is a reload,
//! so nothing is fetched or installed there.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

#[cfg(target_os = "linux")]
use crate::loader::executable;
use crate::{EventLoopProxy, State, UserEvent, platform};
#[cfg(not(target_arch = "wasm32"))]
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;
use std::collections::HashMap;
#[cfg(not(target_os = "linux"))]
use std::env::current_exe as executable;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(not(target_arch = "wasm32"))]
use ureq::tls::{Certificate, RootCerts, TlsConfig};

/// Where the builds are published.
const BASE: &str = "https://file.paperclover.net/shr/snowbound/";

/// The release key's public half, in hex.
const KEY: &str = include_str!("../release-key.pub");

/// The argument `relaunch` starts the old executable with to finish an update.
pub const FINISH: &str = "--finish-update";

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// This build's version as `tools/release.py` stamped it; development builds have none.
fn running() -> Option<Version> {
    Version::parse(option_env!("SNOWBOUND_BUILD")?)
}

/// This build's platform, as `latest.json` and `build.json` key their entries; Apple
/// silicon's for an Intel build Rosetta runs.
fn platform() -> String {
    if translated() {
        "macos-aarch64".to_owned()
    } else if cfg!(target_os = "macos") && !cfg!(feature = "wgpu") {
        "macos-10.6".to_owned()
    } else {
        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn translated() -> bool {
    let mut translated: libc::c_int = 0;
    let mut size = std::mem::size_of_val(&translated);
    let found = unsafe {
        libc::sysctlbyname(
            c"sysctl.proc_translated".as_ptr(),
            (&raw mut translated).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    found == 0 && translated == 1
}

#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
fn translated() -> bool {
    false
}

/// A published build: the day of its commit on `main`, in Los Angeles, and how many of that
/// day's commits on `main` lead up to and include it. `2026-09-29-r10` names it in manifests.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    date: String,
    revision: u32,
}

impl Version {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        let (date, revision) = name.split_once("-r")?;
        let digits = |range: std::ops::Range<usize>| {
            date.get(range)
                .is_some_and(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
        };
        let dated = date.len() == 10
            && digits(0..4)
            && digits(5..7)
            && digits(8..10)
            && &date[4..5] == "-"
            && &date[7..8] == "-";
        let revision = revision.parse().ok().filter(|&revision| revision > 0)?;
        dated.then(|| Self {
            date: date.to_owned(),
            revision,
        })
    }

    fn name(&self) -> String {
        format!("{}-r{}", self.date, self.revision)
    }

    /// The build's folder under `BASE`.
    fn folder(&self) -> String {
        format!("{}.r{}/", self.date, self.revision)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "build {} revision {}", self.date, self.revision)
    }
}

/// The running build as Options and About name it.
pub fn describe_running() -> String {
    running().map_or_else(
        || "Snowbound development build".to_owned(),
        |version| format!("Snowbound {version}"),
    )
}

#[derive(Deserialize)]
struct Build {
    version: String,
    archives: HashMap<String, Archive>,
    /// Every change since the first published build, oldest first; builds before these
    /// were listed have none.
    #[serde(default)]
    changes: Vec<Change>,
}

/// One feature, bug fix or other change a build brings, as `tools/release.py` lists them.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Change {
    /// The version of the commit that made it.
    version: String,
    pub kind: Kind,
    pub title: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Feature,
    Fix,
    #[serde(other)]
    Other,
}

impl Kind {
    /// The kind's heading in a list of changes.
    pub fn heading(self) -> &'static str {
        match self {
            Kind::Feature => "Features",
            Kind::Fix => "Bug fixes",
            Kind::Other => "Other changes",
        }
    }
}

/// What `changes` amount to, as "3 features, 5 bug fixes, and 2 other changes"; none when
/// there are none.
pub fn summary(changes: &[Change]) -> Option<String> {
    let parts: Vec<String> = [
        (Kind::Feature, "feature", "features"),
        (Kind::Fix, "bug fix", "bug fixes"),
        (Kind::Other, "other change", "other changes"),
    ]
    .into_iter()
    .filter_map(|(kind, one, many)| {
        match changes.iter().filter(|change| change.kind == kind).count() {
            0 => None,
            1 => Some(format!("1 {one}")),
            count => Some(format!("{count} {many}")),
        }
    })
    .collect();
    match parts.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        [first, second] => Some(format!("{first} and {second}")),
        [rest @ .., last] => Some(format!("{}, and {last}", rest.join(", "))),
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Archive {
    file: String,
    size: u64,
    sha256: String,
    /// The release key's signature of the archive's bytes.
    signature: String,
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    text.len()
        .is_multiple_of(2)
        .then(|| {
            (0..text.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
                .collect()
        })
        .flatten()
}

#[cfg(target_arch = "wasm32")]
fn verify(_: &[u8], _: &[u8], _: &str) -> Result<(), String> {
    Err(BROWSER.to_owned())
}

/// Why the browser installs no builds: it loads the newest each time the page opens.
#[cfg(target_arch = "wasm32")]
const BROWSER: &str = "The browser loads the newest Snowbound each time the page opens.";

#[cfg(not(target_arch = "wasm32"))]
fn verify(key: &[u8], message: &[u8], signature: &str) -> Result<(), String> {
    let signature = unhex(signature).ok_or("The signature isn’t hex")?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(message, &signature)
        .map_err(|_| "The signature doesn’t match the release key".to_owned())
}

/// The version `latest.json` names for `platform`, where it is newer than `running`.
fn newer(
    latest: &[u8],
    platform: &str,
    running: Option<&Version>,
) -> Result<Option<Version>, String> {
    let latest: HashMap<String, String> =
        serde_json::from_slice(latest).map_err(|error| format!("latest.json: {error}"))?;
    let Some(name) = latest.get(platform) else {
        return Ok(None);
    };
    let version = Version::parse(name).ok_or_else(|| format!("latest.json names {name}"))?;
    Ok(running
        .is_none_or(|running| version > *running)
        .then_some(version))
}

/// `platform`'s archive in the build `build.json` describes, once its signature holds and
/// it describes `version`, and its changes since `running`, features first.
fn archive(
    key: &[u8],
    build: &[u8],
    signature: &[u8],
    version: &Version,
    platform: &str,
    running: Option<&Version>,
) -> Result<(Archive, Vec<Change>), String> {
    verify(key, build, &String::from_utf8_lossy(signature))?;
    let mut build: Build =
        serde_json::from_slice(build).map_err(|error| format!("build.json: {error}"))?;
    if Version::parse(&build.version).as_ref() != Some(version) {
        return Err(format!("build.json describes {}", build.version));
    }
    let archive = build
        .archives
        .remove(platform)
        .ok_or_else(|| format!("{} has no {platform} archive", version.name()))?;
    let mut changes: Vec<Change> = build
        .changes
        .into_iter()
        .filter(|change| {
            Version::parse(&change.version)
                .is_some_and(|made| running.is_none_or(|running| made > *running))
        })
        .collect();
    changes.sort_by_key(|change| change.kind);
    Ok((archive, changes))
}

fn check_archive(key: &[u8], archive: &Archive, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != archive.size {
        return Err(format!(
            "{} is {} bytes, not {}",
            archive.file,
            bytes.len(),
            archive.size
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
        if unhex(&archive.sha256).as_deref() != Some(digest.as_ref()) {
            return Err(format!("{}’s SHA-256 doesn’t match", archive.file));
        }
    }
    verify(key, bytes, &archive.signature)
}

/// What an update replaces: the app bundle on macOS, the executable elsewhere. Development
/// builds and Mac OS X 10.6 have none, and point to the build's folder instead.
fn install() -> Option<PathBuf> {
    running()?;
    let executable = executable().ok()?;
    if !cfg!(feature = "wgpu") {
        None
    } else if cfg!(target_os = "macos") {
        let bundle = executable.ancestors().nth(3)?;
        (bundle.extension()? == "app").then(|| bundle.to_owned())
    } else {
        Some(executable)
    }
}

/// The folder an update to `install` unpacks into, beside it so the swap is a rename.
pub fn staging(install: &Path) -> Option<PathBuf> {
    let name = install.file_name()?.to_string_lossy();
    Some(install.with_file_name(format!(".{name}.update")))
}

/// Where in `folder` a staged update keeps what replaces the install.
fn staged(folder: &Path) -> PathBuf {
    folder.join(if cfg!(target_os = "macos") {
        "Snowbound.app"
    } else if cfg!(windows) {
        "snowbound.exe"
    } else {
        "snowbound"
    })
}

/// Stages a verified archive in `folder`, replacing what was there: the zipped app unpacked
/// on macOS, the executable itself elsewhere. Names the version last, so a folder naming
/// `version` holds all of it.
fn stage(bytes: &[u8], folder: &Path, version: &Version) -> std::io::Result<PathBuf> {
    match std::fs::remove_dir_all(folder) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    std::fs::create_dir(folder)?;
    let item = staged(folder);
    if cfg!(target_os = "macos") {
        let file = folder.join("archive.zip");
        std::fs::write(&file, bytes)?;
        let unpacked = Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&file)
            .arg(folder)
            .status()?;
        std::fs::remove_file(&file)?;
        if !unpacked.success() || !item.exists() {
            return Err(std::io::Error::other("The archive holds no Snowbound.app"));
        }
    } else {
        std::fs::write(&item, bytes)?;
        #[cfg(unix)]
        std::fs::set_permissions(&item, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    }
    std::fs::write(folder.join("version"), version.name())?;
    Ok(item)
}

/// Swaps `staged` in for `install`, putting the old bundle back if the new one can't take its
/// place, then removes the staging folder.
fn apply(staged: &Path, install: &Path) -> std::io::Result<()> {
    let folder = staging(install).ok_or_else(|| std::io::Error::other("No install name"))?;
    let old = folder.with_extension("old");
    if install.is_dir() {
        let _ = std::fs::remove_dir_all(&old);
    } else {
        let _ = std::fs::remove_file(&old);
    }
    // Windows renames a running executable, as the finisher is, but doesn't replace it.
    std::fs::rename(install, &old)?;
    if let Err(error) = std::fs::rename(staged, install) {
        std::fs::rename(&old, install)?;
        return Err(error);
    }
    if install.is_dir() {
        let _ = std::fs::remove_dir_all(&old);
    } else {
        // Where it is still running, the next update removes it.
        let _ = std::fs::remove_file(&old);
    }
    let _ = std::fs::remove_dir_all(folder);
    Ok(())
}

/// Where an update check has got to.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    UpToDate,
    Downloading(Version),
    /// Verified and unpacked beside the install, waiting for Restart to Update, with what it
    /// changes.
    Ready(Version, PathBuf, Vec<Change>),
    /// Newer than this build, which can't install it itself: its folder has the download.
    Available(Version, Vec<Change>),
    Failed(&'static str),
}

const UNREACHABLE: &str =
    "Snowbound couldn’t reach the update server. Check your connection, then try again.";
const UNVERIFIED: &str =
    "The update didn’t match Snowbound’s release signature, so it wasn’t installed.";

/// Fetches a path under `BASE`, refusing a body over the limit in bytes.
type Fetch<'a> = dyn Fn(&str, u64) -> Result<Vec<u8>, String> + 'a;

/// Looks for a build newer than `running` and stages it beside `install` if there is one.
fn check(
    get: &Fetch<'_>,
    key: &[u8],
    running: Option<&Version>,
    install: Option<&Path>,
    progress: &dyn Fn(Status),
) -> Status {
    let platform = platform();
    let since = running;
    // Under Rosetta the native build of the same version is still the update.
    let running = running.filter(|_| !translated());
    let fetch = |path: &str, limit| {
        get(path, limit).map_err(|error| {
            eprintln!("Cannot fetch {BASE}{path}: {error}");
            UNREACHABLE
        })
    };
    let unverified = |error: String| {
        eprintln!("Rejecting the update: {error}");
        UNVERIFIED
    };
    let result = (|| {
        let Some(version) =
            newer(&fetch("latest.json", 1 << 16)?, &platform, running).map_err(unverified)?
        else {
            return Ok(Status::UpToDate);
        };
        let build = fetch(&format!("{}build.json", version.folder()), 1 << 20)?;
        let signature = fetch(&format!("{}build.json.sig", version.folder()), 1 << 10)?;
        let (archive, changes) =
            archive(key, &build, &signature, &version, &platform, since).map_err(unverified)?;
        let Some(folder) = install.and_then(staging) else {
            return Ok(Status::Available(version, changes));
        };
        let item = staged(&folder);
        if std::fs::read_to_string(folder.join("version")).is_ok_and(|name| name == version.name())
            && item.exists()
        {
            return Ok(Status::Ready(version, item, changes));
        }
        progress(Status::Downloading(version.clone()));
        let bytes = fetch(
            &format!("{}{}", version.folder(), archive.file),
            archive.size,
        )?;
        check_archive(key, &archive, &bytes).map_err(unverified)?;
        Ok(match stage(&bytes, &folder, &version) {
            Ok(item) => Status::Ready(version, item, changes),
            Err(error) => {
                eprintln!("Cannot stage the update in {}: {error}", folder.display());
                Status::Available(version, changes)
            }
        })
    })();
    result.unwrap_or_else(Status::Failed)
}

#[cfg(target_arch = "wasm32")]
fn download(_: &str, _: u64) -> Result<Vec<u8>, String> {
    Err(BROWSER.to_owned())
}

/// The `Fetch` the app uses.
#[cfg(not(target_arch = "wasm32"))]
fn download(path: &str, limit: u64) -> Result<Vec<u8>, String> {
    let system = rustls_native_certs::load_native_certs().certs;
    let bundled = webpki_root_certs::TLS_SERVER_ROOT_CERTS;
    let roots = (system.iter().chain(bundled))
        .map(|der| Certificate::from_der(der).to_owned())
        .collect();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::Specific(Arc::new(roots)))
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(10 * 60)))
        .timeout_connect(Some(Duration::from_secs(30)))
        .build()
        .into();
    agent
        .get(&format!("{BASE}{path}"))
        .call()
        .and_then(|mut response| {
            // ureq refuses a body that reaches its limit, so an exact size needs one byte more.
            response
                .body_mut()
                .with_config()
                .limit(limit + 1)
                .read_to_vec()
        })
        .map_err(|error| error.to_string())
}

#[derive(Default)]
struct Shared {
    status: Status,
    automatic: bool,
    /// Check for Updates… asked for a check.
    asked: bool,
    /// What the asked-for check found, to be shown.
    answer: Option<Status>,
}

/// The checking thread and what it has found.
pub struct Updates {
    shared: Arc<Mutex<Shared>>,
    thread: std::thread::Thread,
    /// Restart to Update was chosen: swap the staged build in once the app quits.
    restart: bool,
    /// The sync popup lists what the update changes.
    pub(crate) changes_listed: bool,
}

impl Updates {
    /// Starts the thread, which checks on its own once `automatic` and this is a published
    /// build, shortly after launch and then daily, while the app isn't working offline.
    pub fn start(automatic: bool, proxy: EventLoopProxy<UserEvent>) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            automatic,
            ..Shared::default()
        }));
        let key = unhex(KEY).expect("release-key.pub holds a key in hex");
        #[cfg(target_arch = "wasm32")]
        let thread = {
            let _ = (key, proxy);
            std::thread::current()
        };
        #[cfg(not(target_arch = "wasm32"))]
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("updates".into())
                .spawn(move || {
                    let running = running();
                    let install = install();
                    // Launch settles before the first check.
                    std::thread::park_timeout(Duration::from_secs(10));
                    let mut last: Option<Instant> = None;
                    loop {
                        let asked = {
                            let mut shared = shared.lock().unwrap();
                            let due = shared.automatic
                                && running.is_some()
                                && !crate::library::offline()
                                && last.is_none_or(|last| last.elapsed() >= DAY);
                            let asked = std::mem::take(&mut shared.asked);
                            (asked || due).then_some(asked)
                        };
                        if let Some(asked) = asked {
                            last = Some(Instant::now());
                            // A failed check leaves what an earlier one found.
                            let set = |status: Status, asked| {
                                let mut shared = shared.lock().unwrap();
                                if asked {
                                    shared.answer = Some(status.clone());
                                }
                                if !matches!(status, Status::Failed(_)) {
                                    shared.status = status;
                                }
                                let _ = proxy.send_event(UserEvent::Update);
                            };
                            let status = check(
                                &download,
                                &key,
                                running.as_ref(),
                                install.as_deref(),
                                &|status| set(status, false),
                            );
                            set(status, asked);
                        }
                        std::thread::park_timeout(Duration::from_secs(60 * 60));
                    }
                })
                .expect("The update thread starts")
                .thread()
                .clone()
        };
        Self {
            shared,
            thread,
            restart: false,
            changes_listed: false,
        }
    }

    pub fn status(&self) -> Status {
        self.shared.lock().unwrap().status.clone()
    }

    pub fn automatic(&self) -> bool {
        self.shared.lock().unwrap().automatic
    }

    pub fn set_automatic(&self, automatic: bool) {
        self.shared.lock().unwrap().automatic = automatic;
        self.thread.unpark();
    }

    /// Checks now, and shows what the check finds when it finishes.
    pub fn check_now(&self) {
        #[cfg(target_arch = "wasm32")]
        platform::inform("Snowbound is up to date", BROWSER);
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.shared.lock().unwrap().asked = true;
            self.thread.unpark();
        }
    }

    /// The staged build to swap in, once Restart to Update has quit the app.
    pub fn restarting(&self) -> Option<PathBuf> {
        match self.status() {
            Status::Ready(_, staged, _) if self.restart => Some(staged),
            _ => None,
        }
    }
}

impl State {
    /// Quits, and swaps the staged build in once the app has.
    pub(crate) fn restart_to_update(&mut self) {
        self.updates.restart = true;
        let _ = self.proxy.send_event(UserEvent::Quit);
    }

    /// Shows what a check Check for Updates… asked for found, once it finishes.
    pub(crate) fn updated(&mut self) {
        let Some(status) = self.updates.shared.lock().unwrap().answer.take() else {
            return;
        };
        match status {
            Status::Ready(version, _, changes) => {
                if platform::confirm(
                    "Update ready",
                    &described(
                        format!("Snowbound {version} is ready to install."),
                        &changes,
                    ),
                    "Later",
                    "Restart to Update",
                ) {
                    self.restart_to_update();
                }
            }
            Status::Available(version, changes) => {
                if platform::confirm(
                    "Update available",
                    &described(
                        format!("Download Snowbound {version} from its build folder."),
                        &changes,
                    ),
                    "Later",
                    "Open Build Folder",
                ) {
                    show_build(&version);
                }
            }
            Status::UpToDate => platform::alert(
                "Snowbound is up to date",
                &format!("You have {}.", describe_running()),
            ),
            Status::Failed(reason) => platform::alert("Unable to check for updates", reason),
            Status::Idle | Status::Downloading(_) => {}
        }
    }
}

/// `lead`, then what `changes` amount to and the first dozen of their titles under their
/// kinds, as a dialog's detail.
fn described(lead: String, changes: &[Change]) -> String {
    const LISTED: usize = 12;
    let Some(summary) = summary(changes) else {
        return lead;
    };
    let mut detail = format!("{lead} It brings {summary}.\n");
    let mut kind = None;
    for change in changes.iter().take(LISTED) {
        if kind != Some(change.kind) {
            kind = Some(change.kind);
            detail += &format!("\n{}\n", change.kind.heading());
        }
        detail += &format!("• {}\n", change.title);
    }
    if changes.len() > LISTED {
        detail += &format!("and {} more\n", changes.len() - LISTED);
    }
    detail.trim_end().to_owned()
}

/// Opens the folder a build is published in.
pub fn show_build(version: &Version) {
    platform::reveal(format!("{BASE}{}", version.folder()));
}

/// Starts this executable again to swap `staged` in once this process has quit, then open
/// the app with the arguments this one had.
pub fn relaunch(staged: &Path) -> std::io::Result<()> {
    let install = install().ok_or_else(|| std::io::Error::other("Nothing to update"))?;
    Command::new(executable()?)
        .arg(FINISH)
        .arg(std::process::id().to_string())
        .arg(staged)
        .arg(install)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map(drop)
}

/// The other half of `relaunch`, given the arguments after `FINISH`: waits up to a minute
/// for the app to quit, swaps the update in and opens the app, updated or not.
pub fn finish(mut args: impl Iterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let usage = "Usage: snowbound --finish-update PID STAGED INSTALL [ARGUMENT]...";
    let pid: u32 = args.next().ok_or(usage)?.to_str().ok_or(usage)?.parse()?;
    let staged = PathBuf::from(args.next().ok_or(usage)?);
    let install = PathBuf::from(args.next().ok_or(usage)?);
    if !quits(pid, Duration::from_secs(60)) {
        return Err("Snowbound didn’t quit, so the update wasn’t installed.".into());
    }
    let applied = apply(&staged, &install);
    let mut open = if cfg!(target_os = "macos") {
        let mut open = Command::new("/usr/bin/open");
        open.arg(&install);
        let rest: Vec<OsString> = args.collect();
        if !rest.is_empty() {
            open.arg("--args").args(rest);
        }
        open
    } else {
        let mut open = Command::new(&install);
        open.args(args);
        open
    };
    open.spawn()?;
    Ok(applied?)
}

/// Whether process `pid` is gone within `patience`.
#[cfg(unix)]
fn quits(pid: u32, patience: Duration) -> bool {
    let deadline = Instant::now() + patience;
    // Signal 0 only asks whether the process is still there.
    while unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

#[cfg(target_arch = "wasm32")]
fn quits(_: u32, _: Duration) -> bool {
    false
}

#[cfg(windows)]
fn quits(pid: u32, patience: Duration) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        // A process that can't be opened has gone.
        if process.is_null() {
            return true;
        }
        let waited = WaitForSingleObject(process, patience.as_millis() as u32);
        CloseHandle(process);
        waited != WAIT_TIMEOUT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn version(name: &str) -> Version {
        Version::parse(name).unwrap()
    }

    fn generate() -> Ed25519KeyPair {
        let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("snowbound-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn versions_parse_order_and_name_their_folders() {
        let tenth = version("2026-09-29-r10");
        assert_eq!(tenth.to_string(), "build 2026-09-29 revision 10");
        assert_eq!(tenth.folder(), "2026-09-29.r10/");
        assert_eq!(tenth.name(), "2026-09-29-r10");
        assert!(version("2026-09-29-r9") < tenth);
        assert!(tenth < version("2026-09-30-r1"));
        assert!(version("2026-10-01-r1") > version("2026-09-30-r12"));
        for bad in [
            "2026-09-29",
            "2026-09-29-r0",
            "2026-9-29-r1",
            "20260929-r1",
            "2026-09-29-rx",
            "abcd-09-29-r1",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn latest_names_a_newer_build_for_this_platform_only() {
        let latest = br#"{"macos-aarch64": "2026-09-29-r10", "linux-x86_64": "2026-09-28-r3"}"#;
        let running = version("2026-09-29-r2");
        assert_eq!(
            newer(latest, "macos-aarch64", Some(&running)),
            Ok(Some(version("2026-09-29-r10")))
        );
        assert_eq!(newer(latest, "linux-x86_64", Some(&running)), Ok(None));
        assert_eq!(
            newer(latest, "macos-aarch64", Some(&version("2026-09-29-r10"))),
            Ok(None)
        );
        assert_eq!(newer(latest, "linux-aarch64", Some(&running)), Ok(None));
        assert_eq!(
            newer(latest, "linux-x86_64", None),
            Ok(Some(version("2026-09-28-r3")))
        );
        assert!(newer(b"{\"macos-aarch64\": \"soon\"}", "macos-aarch64", None).is_err());
        assert!(newer(b"<html>", "macos-aarch64", None).is_err());
    }

    /// What each build `publish` describes lists: r9 and r10 published, r7 and r8 skipped.
    fn changes() -> serde_json::Value {
        serde_json::json!([
            {"version": "2026-09-29-r7", "kind": "fix", "title": "Old fix"},
            {"version": "2026-09-29-r8", "kind": "fix", "title": "Pasted pictures keep their size"},
            {"version": "2026-09-29-r8", "kind": "feature", "title": "Pinch zoom"},
            {"version": "2026-09-29-r9", "kind": "release", "title": "Stable download names"},
            {"version": "2026-09-29-r10", "kind": "feature", "title": "Styles and themes"},
            {"version": "someday", "kind": "feature", "title": "Unversioned"},
        ])
    }

    /// A build.json for `archive`'s bytes under `platform`, with its signature.
    fn publish(
        pair: &Ed25519KeyPair,
        name: &str,
        platform: &str,
        file: &str,
        bytes: &[u8],
    ) -> (Vec<u8>, Vec<u8>) {
        let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
        let build = serde_json::to_vec_pretty(&serde_json::json!({
            "version": name,
            "commit": "0123456789abcdef",
            "changes": changes(),
            "archives": {platform: {
                "file": file,
                "size": bytes.len(),
                "sha256": hex(digest.as_ref()),
                "signature": hex(pair.sign(bytes).as_ref()),
            }},
        }))
        .unwrap();
        let signature = hex(pair.sign(&build).as_ref()).into_bytes();
        (build, signature)
    }

    #[test]
    fn signatures_and_hashes_are_checked() {
        let pair = generate();
        let key = pair.public_key().as_ref();
        let tenth = version("2026-09-29-r10");
        let bytes = b"an archive".to_vec();
        let (build, signature) =
            publish(&pair, "2026-09-29-r10", "linux-x86_64", "a.tar.gz", &bytes);
        let (found, _) = archive(key, &build, &signature, &tenth, "linux-x86_64", None).unwrap();
        check_archive(key, &found, &bytes).unwrap();

        let mut tampered = build.clone();
        let at = tampered.iter().position(|&byte| byte == b'a').unwrap();
        tampered[at] = b'b';
        assert!(archive(key, &tampered, &signature, &tenth, "linux-x86_64", None).is_err());
        assert!(
            archive(
                generate().public_key().as_ref(),
                &build,
                &signature,
                &tenth,
                "linux-x86_64",
                None
            )
            .is_err()
        );
        assert!(archive(key, &build, b"zz", &tenth, "linux-x86_64", None).is_err());
        assert!(
            archive(
                key,
                &build,
                &signature,
                &version("2026-09-29-r11"),
                "linux-x86_64",
                None
            )
            .is_err()
        );
        assert!(archive(key, &build, &signature, &tenth, "macos-aarch64", None).is_err());

        assert!(
            check_archive(key, &found, b"an archivf")
                .unwrap_err()
                .contains("SHA-256")
        );
        assert!(
            check_archive(key, &found, b"an archive!")
                .unwrap_err()
                .contains("bytes")
        );
        // Right size and hash, but signed by another key.
        let forged = Archive {
            signature: hex(generate().sign(&bytes).as_ref()),
            ..found
        };
        assert!(
            check_archive(key, &forged, &bytes)
                .unwrap_err()
                .contains("signature")
        );
    }

    fn change(kind: Kind, title: &str) -> Change {
        Change {
            version: "2026-09-29-r10".to_owned(),
            kind,
            title: title.to_owned(),
        }
    }

    #[test]
    fn summaries_name_each_kind_with_plurals_and_list_grammar() {
        let fix = || change(Kind::Fix, "A fix");
        let feature = || change(Kind::Feature, "A feature");
        let other = || change(Kind::Other, "Another change");
        assert_eq!(summary(&[]), None);
        assert_eq!(summary(&[fix()]).unwrap(), "1 bug fix");
        assert_eq!(summary(&[other(), other()]).unwrap(), "2 other changes");
        assert_eq!(
            summary(&[fix(), feature(), fix()]).unwrap(),
            "1 feature and 2 bug fixes"
        );
        let mut all = vec![feature(), feature(), feature(), other(), other()];
        all.extend(std::iter::repeat_with(fix).take(5));
        assert_eq!(
            summary(&all).unwrap(),
            "3 features, 5 bug fixes, and 2 other changes"
        );
        assert_eq!(
            described("Ready.".to_owned(), &[feature(), fix(), fix()]),
            "Ready. It brings 1 feature and 2 bug fixes.\n\nFeatures\n• A feature\n\nBug fixes\n• A fix\n• A fix"
        );
        assert_eq!(described("Ready.".to_owned(), &[]), "Ready.");
    }

    /// A build lists what every build since the first brought; a check sums those newer than
    /// the running build, skipped releases included, features first, and an unknown kind is
    /// another change.
    #[test]
    fn changes_sum_across_skipped_releases() {
        let pair = generate();
        let key = pair.public_key().as_ref();
        let tenth = version("2026-09-29-r10");
        let (build, signature) = publish(&pair, "2026-09-29-r10", "linux-x86_64", "a", b"a");
        let since = |running: Option<&str>| {
            let running = running.map(version);
            let (_, changes) = archive(
                key,
                &build,
                &signature,
                &tenth,
                "linux-x86_64",
                running.as_ref(),
            )
            .unwrap();
            changes
                .into_iter()
                .map(|change| (change.kind, change.title))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            since(Some("2026-09-29-r9")),
            [(Kind::Feature, "Styles and themes".to_owned())]
        );
        assert_eq!(
            since(Some("2026-09-29-r7")),
            [
                (Kind::Feature, "Pinch zoom".to_owned()),
                (Kind::Feature, "Styles and themes".to_owned()),
                (Kind::Fix, "Pasted pictures keep their size".to_owned()),
                (Kind::Other, "Stable download names".to_owned()),
            ]
        );
        assert_eq!(since(Some("2026-09-29-r10")), []);
        assert_eq!(since(None).len(), 5);

        // Builds published before changes were listed still read, as builds listing them do
        // for clients that predate them.
        let old = serde_json::json!({"version": "2026-09-29-r10", "archives": {"linux-x86_64": {
            "file": "a", "size": 1, "sha256": "", "signature": "",
        }}});
        let old = serde_json::to_vec(&old).unwrap();
        let signature = hex(pair.sign(&old).as_ref()).into_bytes();
        let (_, changes) = archive(key, &old, &signature, &tenth, "linux-x86_64", None).unwrap();
        assert_eq!(changes, []);
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Earlier {
            version: String,
            archives: HashMap<String, Archive>,
        }
        assert!(serde_json::from_slice::<Earlier>(&build).is_ok());
    }

    /// An archive as `tools/release.py` packs this platform's, holding `marker`.
    fn pack(folder: &Path, marker: &str) -> Vec<u8> {
        if !cfg!(target_os = "macos") {
            return marker.as_bytes().to_vec();
        }
        let source = folder.join("source");
        let _ = std::fs::remove_dir_all(&source);
        let bundle = staged(&source);
        let file = bundle.join("Contents/MacOS/Snowbound");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, marker).unwrap();
        let archive = folder.join("archive.zip");
        let status = Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&bundle)
            .arg(&archive)
            .status();
        assert!(status.unwrap().success());
        std::fs::read(archive).unwrap()
    }

    /// Reads the marker an install's executable holds.
    fn marker(install: &Path) -> String {
        let file = if cfg!(target_os = "macos") {
            install.join("Contents/MacOS/Snowbound")
        } else {
            install.to_owned()
        };
        std::fs::read_to_string(file).unwrap()
    }

    /// A check against a published folder finds, verifies and stages the newer build, a second
    /// check reuses the staged one, and applying it replaces the install.
    #[test]
    fn a_newer_build_is_staged_and_swapped_in() {
        let folder = scratch("stage");
        let platform = platform();
        let pair = generate();
        let key = pair.public_key().as_ref().to_vec();
        let published = folder.join("published");
        let build = published.join("2026-09-29.r10");
        std::fs::create_dir_all(&build).unwrap();
        let bytes = pack(&folder, "new");
        let (manifest, signature) =
            publish(&pair, "2026-09-29-r10", &platform, "app.archive", &bytes);
        std::fs::write(build.join("app.archive"), &bytes).unwrap();
        std::fs::write(build.join("build.json"), manifest).unwrap();
        std::fs::write(build.join("build.json.sig"), signature).unwrap();
        std::fs::write(
            published.join("latest.json"),
            serde_json::to_vec(&serde_json::json!({ &platform: "2026-09-29-r10" })).unwrap(),
        )
        .unwrap();
        let fetched = Mutex::new(Vec::new());
        let fetch = |path: &str, limit: u64| {
            fetched.lock().unwrap().push(path.to_owned());
            let bytes = std::fs::read(published.join(path)).map_err(|error| error.to_string())?;
            if bytes.len() as u64 > limit {
                return Err("too big".to_owned());
            }
            Ok(bytes)
        };

        let apps = folder.join("Applications");
        std::fs::create_dir_all(&apps).unwrap();
        let old = pack(&folder, "old");
        let installed = stage(&old, &folder.join("unpacked"), &version("2026-09-28-r1")).unwrap();
        let install = apps.join(installed.file_name().unwrap());
        std::fs::rename(installed, &install).unwrap();
        assert_eq!(marker(&install), "old");

        let running = version("2026-09-28-r1");
        let progress = Mutex::new(Vec::new());
        let status = check(&fetch, &key, Some(&running), Some(&install), &|status| {
            progress.lock().unwrap().push(status)
        });
        let Status::Ready(found, item, listed) = status else {
            panic!("{status:?}");
        };
        assert_eq!(found, version("2026-09-29-r10"));
        assert_eq!(
            summary(&listed).unwrap(),
            "2 features, 2 bug fixes, and 1 other change"
        );
        assert_eq!(
            *progress.lock().unwrap(),
            [Status::Downloading(found.clone())]
        );
        assert!(item.starts_with(staging(&install).unwrap()));
        assert_eq!(marker(&install), "old");

        fetched.lock().unwrap().clear();
        let again = check(&fetch, &key, Some(&running), Some(&install), &|_| {});
        assert_eq!(
            again,
            Status::Ready(found.clone(), item.clone(), listed.clone())
        );
        assert!(
            !fetched
                .lock()
                .unwrap()
                .iter()
                .any(|path| path.ends_with("app.archive"))
        );

        assert_eq!(
            check(&fetch, &key, Some(&found), Some(&install), &|_| {}),
            Status::UpToDate
        );
        assert_eq!(
            check(&fetch, &key, Some(&running), None, &|_| {}),
            Status::Available(found.clone(), listed.clone())
        );
        assert_eq!(
            check(
                &fetch,
                generate().public_key().as_ref(),
                Some(&running),
                Some(&install),
                &|_| {}
            ),
            Status::Failed(UNVERIFIED)
        );
        assert_eq!(
            check(
                &|_, _| Err("offline".into()),
                &key,
                Some(&running),
                Some(&install),
                &|_| {}
            ),
            Status::Failed(UNREACHABLE)
        );

        apply(&item, &install).unwrap();
        assert_eq!(marker(&install), "new");
        assert!(!staging(&install).unwrap().exists());
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 1);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_tampered_archive_is_not_staged() {
        let folder = scratch("tampered");
        let platform = platform();
        let pair = generate();
        let bytes = pack(&folder, "new");
        let (manifest, signature) =
            publish(&pair, "2026-09-29-r10", &platform, "app.archive", &bytes);
        let mut served = bytes.clone();
        let last = served.len() - 1;
        served[last] ^= 1;
        let fetch = |path: &str, _| {
            Ok(match path {
                "latest.json" => {
                    serde_json::to_vec(&serde_json::json!({ &platform: "2026-09-29-r10" })).unwrap()
                }
                "2026-09-29.r10/build.json" => manifest.clone(),
                "2026-09-29.r10/build.json.sig" => signature.clone(),
                _ => served.clone(),
            })
        };
        let install = folder.join("Snowbound.app");
        let status = check(
            &fetch,
            pair.public_key().as_ref(),
            None,
            Some(&install),
            &|_| {},
        );
        assert_eq!(status, Status::Failed(UNVERIFIED));
        assert!(!staging(&install).unwrap().exists());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// The newest published build for this platform, checked from an older build over the
    /// network with the release key, stages and swaps into a scratch install.
    #[test]
    #[ignore = "reads the published builds"]
    fn the_published_build_installs() {
        let folder = scratch("published");
        let install = staged(&folder);
        let placeholder = pack(&folder, "old");
        let unpacked = stage(
            &placeholder,
            &folder.join("unpacked"),
            &version("2000-01-01-r1"),
        );
        std::fs::rename(unpacked.unwrap(), &install).unwrap();
        let old = version("2000-01-01-r1");
        let status = check(
            &download,
            &unhex(KEY).unwrap(),
            Some(&old),
            Some(&install),
            &|_| {},
        );
        let Status::Ready(found, staged, _) = status else {
            panic!("{status:?}");
        };
        apply(&staged, &install).unwrap();
        // 10.6's builds are unsigned.
        if cfg!(target_os = "macos") && cfg!(feature = "wgpu") {
            let verified = Command::new("codesign")
                .args(["--verify", "--strict"])
                .arg(&install)
                .status();
            assert!(verified.unwrap().success());
        }
        eprintln!("Installed {found} into {}", install.display());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
