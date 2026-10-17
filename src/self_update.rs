//! Installing a new release over this copy once the player presses Update.
//!
//! Only the Windows installer and the AppImage update themselves; every other
//! copy is updated by hand. Files come only from Nomadeer's GitHub releases,
//! each is checked against the SHA-256 published beside it before anything runs
//! or is replaced, and the player's data folder is never touched: Setup waits in
//! a folder of its own under the system's temp folder, and a new AppImage is
//! written next to the old one before it takes its place.

use crate::updates::{self, Edition, InstallKind, Release, ReleaseAsset};
use reqwest::blocking::{Client, Response};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Every release file is downloaded from under here, as `v<version>/<name>`.
const DOWNLOAD_ROOT: &str = "https://github.com/TomD3Wiz/nomadeer/releases/download/";
/// GitHub sends release downloads on to its file storage.
const DOWNLOAD_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
/// Nomadeer's installer and AppImage are tens of megabytes; a file far outside
/// this is not one of them.
pub(crate) const MIN_UPDATE_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;
/// A `.sha256` holds one line.
const MAX_CHECKSUM_BYTES: u64 = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_REDIRECTS: usize = 10;
const USER_AGENT: &str = concat!("Nomadeer/", env!("CARGO_PKG_VERSION"));
const DOWNLOAD_BUFFER: usize = 256 * 1024;

/// The folder under the temp folder where Setup waits to run.
const DOWNLOADS_DIR_NAME: &str = "nomadeer-updates";
/// Setup's log of the last update, kept for when something goes wrong.
const SETUP_LOG: &str = "setup.log";
/// A new AppImage is written beside the old one under this prefix.
const STAGING_PREFIX: &str = ".nomadeer-update-";

// Where files may come from ------------------------------------------------------

/// Where release files may be downloaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sources {
    /// NOMADEER_UPDATE_API's stand-in server, trusted for testing.
    test_server: Option<String>,
}

impl Sources {
    pub(crate) fn current() -> Self {
        Self {
            test_server: updates::test_server(),
        }
    }

    /// Whether `url` is where Nomadeer's release `version` keeps the file `name`.
    fn allows_file(&self, url: &str, version: &str, name: &str) -> bool {
        url == format!("{DOWNLOAD_ROOT}v{version}/{name}") || self.on_test_server(url)
    }

    /// Whether a download may follow GitHub's redirect to `url`.
    fn allows_redirect(&self, url: &reqwest::Url) -> bool {
        (url.scheme() == "https"
            && url.port().is_none()
            && url
                .host_str()
                .is_some_and(|host| DOWNLOAD_HOSTS.contains(&host)))
            || self.on_test_server(url.as_str())
    }

    fn on_test_server(&self, url: &str) -> bool {
        self.test_server.as_deref().is_some_and(|base| {
            url.strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('/'))
        })
    }
}

// What can go wrong ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateError {
    /// This copy is updated by hand.
    NotSelfUpdating,
    /// The release has no file for this copy.
    NoFile,
    /// The release has no `.sha256` for the file.
    NoChecksum(String),
    /// A download link that does not lead to Nomadeer's releases.
    UntrustedSource(String),
    /// The file is not the size the release lists, or no sensible size at all.
    WrongSize(String),
    /// The `.sha256` could not be read.
    UnreadableChecksum(String),
    /// The download does not match its `.sha256`.
    Mismatch(String),
    /// Where this copy is installed could not be worked out.
    NotFound,
    Network(String),
    Disk(String),
    /// The AppImage's folder cannot be written to.
    NotWritable(PathBuf),
    Cancelled,
    /// The player said no to Windows' permission prompt.
    #[cfg_attr(not(windows), allow(dead_code))]
    Declined,
    /// Setup or the update itself did not start.
    Launch(String),
}

impl UpdateError {
    /// What happened and what the player can do about it.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NotSelfUpdating => {
                "This copy is updated by hand. The download page has the new version.".to_owned()
            }
            Self::NoFile => {
                "This release has no download for your copy. The download page has every file."
                    .to_owned()
            }
            Self::NoChecksum(file) => format!(
                "The release has no checksum for {file}, so Nomadeer will not install it. Get it from the download page instead."
            ),
            Self::UntrustedSource(file) => format!(
                "The link for {file} does not lead to Nomadeer's releases on GitHub, so it was not used. Get it from the download page instead."
            ),
            Self::WrongSize(file) => format!(
                "{file} is not the size the release lists, so nothing was installed. Press Update to try again, or get it from the download page."
            ),
            Self::UnreadableChecksum(file) => format!(
                "The checksum for {file} could not be read, so nothing was installed. Get it from the download page instead."
            ),
            Self::Mismatch(file) => format!(
                "{file} did not match its checksum, so nothing was installed. Press Update to try again, or get it from the download page."
            ),
            Self::NotFound => {
                "Nomadeer could not find where it is installed, so it cannot update itself. Get the new version from the download page."
                    .to_owned()
            }
            Self::Network(error) => format!(
                "The download stopped ({error}). Check your connection and press Update to try again."
            ),
            Self::Disk(error) => format!(
                "The update could not be saved ({error}). Free up some disk space and press Update to try again."
            ),
            Self::NotWritable(dir) => format!(
                "Nomadeer cannot write to {}, so it cannot replace the AppImage there. Get the new AppImage from the download page and use it in place of this one.",
                dir.display()
            ),
            Self::Cancelled => "Update cancelled. Nothing was changed.".to_owned(),
            Self::Declined => {
                "Windows did not get permission to install the update. Press Update again and choose Yes; Nomadeer is installed for all users, so Windows asks first."
                    .to_owned()
            }
            Self::Launch(error) => format!(
                "The update did not start ({error}). Press Update to try again, or get it from the download page."
            ),
        }
    }

    /// Whether this is a problem rather than the player's own choice.
    pub(crate) fn is_failure(&self) -> bool {
        !matches!(self, Self::Cancelled)
    }
}

fn disk(error: io::Error) -> UpdateError {
    UpdateError::Disk(error.to_string())
}

// What pressing Update does --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// Run the new Setup over the install in `install_dir`.
    WindowsInstaller {
        per_user: bool,
        install_dir: PathBuf,
    },
    /// Put the new AppImage in place of the one at `path`.
    AppImage { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdatePlan {
    pub(crate) version: String,
    pub(crate) file: ReleaseAsset,
    pub(crate) checksum: ReleaseAsset,
    pub(crate) target: Target,
    pub(crate) edition: Edition,
}

/// Works out what Update would fetch and replace, refusing anything that is not
/// a sensibly sized file with a checksum from Nomadeer's releases. `exe` is the
/// running executable and `appimage` the AppImage it runs from, if any.
pub(crate) fn plan_update(
    release: &Release,
    kind: InstallKind,
    edition: Edition,
    exe: Option<&Path>,
    appimage: Option<&Path>,
    sources: &Sources,
) -> Result<UpdatePlan, UpdateError> {
    let target = match kind {
        InstallKind::WindowsInstaller { per_user } => Target::WindowsInstaller {
            per_user,
            install_dir: exe
                .and_then(Path::parent)
                .ok_or(UpdateError::NotFound)?
                .to_path_buf(),
        },
        InstallKind::AppImage => Target::AppImage {
            path: appimage
                .filter(|path| path.parent().is_some())
                .ok_or(UpdateError::NotFound)?
                .to_path_buf(),
        },
        _ => return Err(UpdateError::NotSelfUpdating),
    };
    let choice = updates::pick_asset(release, kind, edition).ok_or(UpdateError::NoFile)?;
    let (file, name) = (choice.file, &choice.file.name);
    let checksum = choice
        .checksum
        .ok_or_else(|| UpdateError::NoChecksum(name.clone()))?;
    for asset in [file, checksum] {
        if !sources.allows_file(&asset.download_url, &release.version, &asset.name) {
            return Err(UpdateError::UntrustedSource(asset.name.clone()));
        }
    }
    if !(MIN_UPDATE_BYTES..=MAX_UPDATE_BYTES).contains(&file.size) {
        return Err(UpdateError::WrongSize(name.clone()));
    }
    if !(1..=MAX_CHECKSUM_BYTES).contains(&checksum.size) {
        return Err(UpdateError::UnreadableChecksum(name.clone()));
    }
    Ok(UpdatePlan {
        version: release.version.clone(),
        file: file.clone(),
        checksum: checksum.clone(),
        target,
        edition,
    })
}

/// The AppImage this copy runs from, with links resolved so that the file
/// itself is replaced rather than a link to it.
pub(crate) fn current_appimage() -> Option<PathBuf> {
    let path = std::env::var_os("APPIMAGE").filter(|path| !path.is_empty())?;
    fs::canonicalize(path).ok()
}

/// Setup's command line for an update: silent, over the existing install, in
/// the same install mode and edition, starting Nomadeer again once it is done.
///
/// The edition is always the running one. Setup on its own picks by processor,
/// which gives the same answer unless the player chose the compatible edition on
/// a processor that can run the optimized one; passing it keeps that choice.
pub(crate) fn installer_arguments(
    per_user: bool,
    edition: Edition,
    install_dir: &Path,
    log: &Path,
) -> Vec<String> {
    vec![
        "/VERYSILENT".to_owned(),
        "/SUPPRESSMSGBOXES".to_owned(),
        "/NORESTART".to_owned(),
        "/CLOSEAPPLICATIONS".to_owned(),
        "/SP-".to_owned(),
        if per_user {
            "/CURRENTUSER"
        } else {
            "/ALLUSERS"
        }
        .to_owned(),
        format!("/EDITION={}", edition.label()),
        format!("/DIR={}", install_dir.display()),
        format!("/LOG={}", log.display()),
        // nomadeer.iss starts Nomadeer again after a silent install with this.
        "/RELAUNCH".to_owned(),
    ]
}

/// Whether Setup needs administrator rights: only to update an install for all
/// users.
pub(crate) fn needs_elevation(per_user: bool) -> bool {
    !per_user
}

/// Joins arguments the way Setup reads its command line, quoting values with
/// spaces: `/DIR="C:\Program Files\Nomadeer"`.
pub(crate) fn windows_parameters(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| {
            if !argument.contains([' ', '\t']) {
                return argument.clone();
            }
            match argument.split_once('=') {
                Some((name, value)) => format!("{name}=\"{value}\""),
                None => format!("\"{argument}\""),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// Downloading and installing ------------------------------------------------------

/// Where Setup is downloaded to.
pub(crate) fn downloads_dir() -> PathBuf {
    std::env::temp_dir().join(DOWNLOADS_DIR_NAME)
}

/// Removes installers and unfinished downloads earlier updates left in `dir`,
/// keeping Setup's last log. Files still in use stay until next time.
pub(crate) fn clean_downloads(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != SETUP_LOG && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Removes half-written AppImages an interrupted update left beside `appimage`.
pub(crate) fn clean_staged_appimages(appimage: &Path) {
    let Some(Ok(entries)) = appimage.parent().map(fs::read_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Tidies up after earlier updates of this copy, at launch.
pub(crate) fn clean_up_after_updates(kind: InstallKind) {
    match kind {
        InstallKind::WindowsInstaller { .. } => clean_downloads(&downloads_dir()),
        InstallKind::AppImage => {
            if let Some(appimage) = current_appimage() {
                clean_staged_appimages(&appimage);
            }
        }
        _ => {}
    }
}

/// How far an update has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Downloading {
        done: u64,
        total: u64,
    },
    /// The download checked out and is being put in place.
    Installing,
}

/// An update that is ready for Nomadeer to make way for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Prepared {
    /// Setup is downloaded and checked, ready to run with these arguments.
    Installer {
        path: PathBuf,
        sha256: String,
        arguments: Vec<String>,
        elevate: bool,
    },
    /// The new AppImage is in place at `path`.
    AppImage { path: PathBuf },
}

/// Downloads the plan's file and checks it against its `.sha256`, then puts a
/// new AppImage in place or leaves Setup in `downloads` ready to run. Stops
/// with `Cancelled` soon after `cancel` is set; nothing is replaced until the
/// download has checked out.
pub(crate) fn prepare_update(
    plan: &UpdatePlan,
    downloads: &Path,
    sources: &Sources,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Stage),
) -> Result<Prepared, UpdateError> {
    let client = download_client(sources)?;
    let expected = fetch_checksum(&client, &plan.checksum, &plan.file.name)?;
    match &plan.target {
        Target::WindowsInstaller {
            per_user,
            install_dir,
        } => {
            fs::create_dir_all(downloads).map_err(disk)?;
            clean_downloads(downloads);
            let part = downloads.join(format!("{}.part", plan.file.name));
            let installer = downloads.join(&plan.file.name);
            let output = File::create(&part).map_err(disk)?;
            let downloaded = download_into(
                &client, &plan.file, &expected, output, cancel, progress,
            )
            .and_then(|()| {
                progress(Stage::Installing);
                crate::util::replace_file(&part, &installer).map_err(disk)
            });
            if let Err(error) = downloaded {
                let _ = fs::remove_file(&part);
                return Err(error);
            }
            Ok(Prepared::Installer {
                path: installer,
                sha256: expected,
                arguments: installer_arguments(
                    *per_user,
                    plan.edition,
                    install_dir,
                    &downloads.join(SETUP_LOG),
                ),
                elevate: needs_elevation(*per_user),
            })
        }
        Target::AppImage { path } => {
            let dir = path.parent().ok_or(UpdateError::NotFound)?;
            let staged = dir.join(format!("{STAGING_PREFIX}{}", plan.file.name));
            let output = create_staging(&staged, dir)?;
            let installed = download_into(&client, &plan.file, &expected, output, cancel, progress)
                .and_then(|()| {
                    progress(Stage::Installing);
                    install_appimage(&staged, path)
                });
            if installed.is_err() {
                let _ = fs::remove_file(&staged);
            }
            installed.map(|()| Prepared::AppImage { path: path.clone() })
        }
    }
}

fn download_client(sources: &Sources) -> Result<Client, UpdateError> {
    let sources = sources.clone();
    let redirects = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if sources.allows_redirect(attempt.url()) {
            attempt.follow()
        } else {
            // Answered with the redirect itself, which `get` refuses.
            attempt.stop()
        }
    });
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT)
        .redirect(redirects)
        .build()
        .map_err(|error| UpdateError::Network(error.to_string()))
}

fn get(client: &Client, asset: &ReleaseAsset) -> Result<Response, UpdateError> {
    let response = client
        .get(&asset.download_url)
        .send()
        .map_err(|error| UpdateError::Network(error.without_url().to_string()))?;
    let status = response.status();
    if status.is_redirection() {
        return Err(UpdateError::UntrustedSource(asset.name.clone()));
    }
    if !status.is_success() {
        return Err(UpdateError::Network(format!(
            "GitHub answered with status {}",
            status.as_u16()
        )));
    }
    Ok(response)
}

fn fetch_checksum(
    client: &Client,
    checksum: &ReleaseAsset,
    file_name: &str,
) -> Result<String, UpdateError> {
    let unreadable = || UpdateError::UnreadableChecksum(file_name.to_owned());
    let mut text = String::new();
    get(client, checksum)?
        .take(MAX_CHECKSUM_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| unreadable())?;
    if text.len() as u64 > MAX_CHECKSUM_BYTES {
        return Err(unreadable());
    }
    parse_checksum(&text, file_name).ok_or_else(unreadable)
}

/// The SHA-256 a `sha256sum`-style line gives for `file_name`, in lowercase.
/// A line naming another file does not count.
pub(crate) fn parse_checksum(text: &str, file_name: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let (hash, name) = match line.split_once(char::is_whitespace) {
        // `*` marks a file hashed in binary mode.
        Some((hash, name)) => (hash, Some(name.trim_start().trim_start_matches('*'))),
        None => (line, None),
    };
    if name.is_some_and(|name| name != file_name)
        || hash.len() != 64
        || !hash.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    Some(hash.to_ascii_lowercase())
}

/// Streams `file` into `output`, which must then be exactly the size the
/// release lists and match `expected`.
fn download_into(
    client: &Client,
    file: &ReleaseAsset,
    expected: &str,
    mut output: File,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Stage),
) -> Result<(), UpdateError> {
    let mut response = get(client, file)?;
    if response
        .content_length()
        .is_some_and(|length| length != file.size)
    {
        return Err(UpdateError::WrongSize(file.name.clone()));
    }
    let total = file.size;
    progress(Stage::Downloading { done: 0, total });
    let (bytes, hash) = copy_hashed(
        &mut response,
        &mut output,
        &file.name,
        total,
        cancel,
        &mut |done| progress(Stage::Downloading { done, total }),
    )?;
    if bytes != total {
        return Err(UpdateError::WrongSize(file.name.clone()));
    }
    if hash != expected {
        return Err(UpdateError::Mismatch(file.name.clone()));
    }
    output.sync_all().map_err(disk)
}

/// Copies `reader` into `writer`, returning the byte count and SHA-256. Gives
/// up once more than `limit` bytes arrive, or when `cancel` is set.
fn copy_hashed(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    name: &str,
    limit: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<(u64, String), UpdateError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; DOWNLOAD_BUFFER];
    let mut done = 0_u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(UpdateError::Cancelled);
        }
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(UpdateError::Network(error.to_string())),
        };
        done += count as u64;
        if done > limit {
            return Err(UpdateError::WrongSize(name.to_owned()));
        }
        writer.write_all(&buffer[..count]).map_err(disk)?;
        hasher.update(&buffer[..count]);
        progress(done);
    }
    Ok((done, format!("{:x}", hasher.finalize())))
}

/// Checks a downloaded file against its SHA-256 once more, just before it runs.
pub(crate) fn verify_file(path: &Path, expected: &str) -> Result<(), UpdateError> {
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let mut file = File::open(path).map_err(disk)?;
    let never = AtomicBool::new(false);
    let (_, hash) = copy_hashed(
        &mut file,
        &mut io::sink(),
        &name,
        MAX_UPDATE_BYTES,
        &never,
        &mut |_| {},
    )?;
    if hash == expected {
        Ok(())
    } else {
        Err(UpdateError::Mismatch(name))
    }
}

/// Creates the file a new AppImage is downloaded into, beside the old one so it
/// can take its place in one step.
fn create_staging(staged: &Path, dir: &Path) -> Result<File, UpdateError> {
    let _ = fs::remove_file(staged);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged)
        .map_err(|error| match error.kind() {
            ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => {
                UpdateError::NotWritable(dir.to_path_buf())
            }
            _ => disk(error),
        })
}

/// Makes the downloaded AppImage executable and moves it over the old one in a
/// single rename, so the old one stays whole until the new one is complete.
fn install_appimage(staged: &Path, appimage: &Path) -> Result<(), UpdateError> {
    let dir = appimage.parent().ok_or(UpdateError::NotFound)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let current = fs::metadata(appimage)
            .ok()
            .map(|metadata| metadata.permissions().mode());
        fs::set_permissions(staged, fs::Permissions::from_mode(executable_mode(current)))
            .map_err(disk)?;
    }
    // Flushing needs a handle that may write, on Windows at least.
    OpenOptions::new()
        .write(true)
        .open(staged)
        .and_then(|file| file.sync_all())
        .map_err(disk)?;
    fs::rename(staged, appimage).map_err(|error| match error.kind() {
        ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => {
            UpdateError::NotWritable(dir.to_path_buf())
        }
        _ => disk(error),
    })?;
    // Makes the rename itself last through a crash, where the file system can.
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The new AppImage's permissions: the old one's, executable wherever readable.
#[cfg_attr(not(unix), allow(dead_code))]
fn executable_mode(current: Option<u32>) -> u32 {
    let mode = current.map_or(0o755, |mode| mode & 0o777);
    mode | 0o100 | ((mode & 0o044) >> 2)
}

// Handing over to the new version ----------------------------------------------------

/// Starts the downloaded Setup after checking it once more. Setup for an
/// install for all users is started through Windows' permission prompt.
pub(crate) fn launch_installer(
    path: &Path,
    sha256: &str,
    arguments: &[String],
    elevate: bool,
) -> Result<(), UpdateError> {
    verify_file(path, sha256)?;
    shell_execute(path, &windows_parameters(arguments), elevate)
}

#[cfg(windows)]
fn shell_execute(path: &Path, parameters: &str, elevate: bool) -> Result<(), UpdateError> {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;

    const SW_SHOWNORMAL: i32 = 1;
    const SE_ERR_ACCESSDENIED: isize = 5;
    const ERROR_CANCELLED: i32 = 1223;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut c_void;
    }

    let wide = |text: &OsStr| text.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let runas = wide(OsStr::new("runas"));
    let file = wide(path.as_os_str());
    let parameters = wide(OsStr::new(parameters));
    // SAFETY: every string is NUL-terminated UTF-16 that outlives the call; a
    // null window, verb (the default, "open") and directory are allowed.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            if elevate {
                runas.as_ptr()
            } else {
                std::ptr::null()
            },
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result > 32 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_CANCELLED) || (elevate && result == SE_ERR_ACCESSDENIED) {
        Err(UpdateError::Declined)
    } else {
        Err(UpdateError::Launch(error.to_string()))
    }
}

#[cfg(not(windows))]
fn shell_execute(_path: &Path, _parameters: &str, _elevate: bool) -> Result<(), UpdateError> {
    Err(UpdateError::Launch("Setup only runs on Windows".to_owned()))
}

/// Starts the AppImage at `path` on its own, so it outlives this copy.
pub(crate) fn relaunch_appimage(path: &Path) -> Result<(), String> {
    let mut command = std::process::Command::new(path);
    // The AppImage runtime sets these for what it starts; the new one sets its own.
    for name in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
        command.env_remove(name);
    }
    // Where the player first started Nomadeer from.
    if let Some(dir) = std::env::var_os("OWD")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
    {
        command.current_dir(dir);
    }
    command.stdin(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Not stopped along with this copy, say by Ctrl+C in its terminal.
        command.process_group(0);
    }
    command.spawn().map(drop).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    const VERSION: &str = "0.2.0";
    const SETUP: &str = "Nomadeer-0.2.0-Setup.exe";
    const APPIMAGE: &str = "Nomadeer-0.2.0-x86_64.AppImage";

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nomadeer-self-update-{}-{name}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    /// Something AppImage-sized that is not all one byte.
    fn payload(seed: u8) -> Vec<u8> {
        (0..MIN_UPDATE_BYTES as usize + 12_345)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn github_asset(name: &str, size: u64) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_owned(),
            size,
            download_url: format!("{DOWNLOAD_ROOT}v{VERSION}/{name}"),
        }
    }

    fn release(size: u64) -> Release {
        let mut assets = Vec::new();
        for name in [SETUP, APPIMAGE, "Nomadeer-0.2.0-Windows-Portable.zip"] {
            assets.push(github_asset(name, size));
            assets.push(github_asset(&format!("{name}.sha256"), 91));
        }
        Release {
            version: VERSION.to_owned(),
            notes: String::new(),
            page_url: String::new(),
            published_at: String::new(),
            assets,
        }
    }

    fn github() -> Sources {
        Sources { test_server: None }
    }

    #[test]
    fn installer_flags_follow_the_install() {
        let dir = Path::new(r"C:\Users\pilot\AppData\Local\Programs\Nomadeer");
        let log = Path::new(r"C:\Users\pilot\AppData\Local\Temp\nomadeer-updates\setup.log");
        let per_user = installer_arguments(true, Edition::Optimized, dir, log);
        assert_eq!(
            per_user,
            [
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/CLOSEAPPLICATIONS",
                "/SP-",
                "/CURRENTUSER",
                "/EDITION=optimized",
                r"/DIR=C:\Users\pilot\AppData\Local\Programs\Nomadeer",
                r"/LOG=C:\Users\pilot\AppData\Local\Temp\nomadeer-updates\setup.log",
                "/RELAUNCH",
            ]
        );
        assert!(!needs_elevation(true));

        let all_users = installer_arguments(
            false,
            Edition::Compatible,
            Path::new(r"C:\Program Files\Nomadeer"),
            Path::new(r"C:\Users\Jo Pilot\AppData\Local\Temp\nomadeer-updates\setup.log"),
        );
        assert!(all_users.contains(&"/ALLUSERS".to_owned()));
        assert!(!all_users.contains(&"/CURRENTUSER".to_owned()));
        assert!(all_users.contains(&"/EDITION=compatible".to_owned()));
        assert!(needs_elevation(false));
        assert_eq!(
            windows_parameters(&all_users),
            "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /SP- /ALLUSERS \
             /EDITION=compatible /DIR=\"C:\\Program Files\\Nomadeer\" \
             /LOG=\"C:\\Users\\Jo Pilot\\AppData\\Local\\Temp\\nomadeer-updates\\setup.log\" /RELAUNCH"
        );
        assert_eq!(
            windows_parameters(&["a b".to_owned(), "/X".to_owned()]),
            "\"a b\" /X"
        );
    }

    #[test]
    fn the_installer_script_knows_every_flag() {
        let script = include_str!("../packaging/windows/nomadeer.iss");
        assert!(script.contains("'/RELAUNCH'"));
        assert!(script.contains("{param:edition|auto}"));
        assert!(script.contains("PrivilegesRequiredOverridesAllowed=dialog"));
    }

    #[test]
    fn only_nomadeers_releases_are_trusted() {
        let sources = github();
        let url =
            |rest: &str| format!("https://github.com/TomD3Wiz/nomadeer/releases/download/{rest}");
        assert!(sources.allows_file(&url("v0.2.0/Nomadeer-0.2.0-Setup.exe"), VERSION, SETUP));
        for wrong in [
            url("v0.1.0/Nomadeer-0.2.0-Setup.exe"),
            url("v0.2.0/Nomadeer-0.2.0-Setup.exe?x=1"),
            url("v0.2.0/Other.exe"),
            "http://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Setup.exe".to_owned(),
            "https://github.com/someone/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Setup.exe".to_owned(),
            "https://github.com.evil.example/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Setup.exe".to_owned(),
            "http://127.0.0.1:18733/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Setup.exe".to_owned(),
        ] {
            assert!(!sources.allows_file(&wrong, VERSION, SETUP), "{wrong}");
        }

        let redirect = |url: &str| sources.allows_redirect(&reqwest::Url::parse(url).unwrap());
        assert!(redirect(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1?sig=x"
        ));
        assert!(redirect("https://objects.githubusercontent.com/x"));
        assert!(redirect(
            "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/x"
        ));
        assert!(!redirect("http://objects.githubusercontent.com/x"));
        assert!(!redirect("https://objects.githubusercontent.com:8443/x"));
        assert!(!redirect("https://evil.example/x"));
        assert!(!redirect("https://github.com@evil.example/x"));

        // A stand-in server is trusted only when NOMADEER_UPDATE_API names it.
        let test = Sources {
            test_server: Some("http://127.0.0.1:18733".to_owned()),
        };
        assert!(test.allows_file("http://127.0.0.1:18733/files/x", VERSION, SETUP));
        assert!(!test.allows_file("http://127.0.0.1:187334/files/x", VERSION, SETUP));
        assert!(test.allows_file(&url("v0.2.0/Nomadeer-0.2.0-Setup.exe"), VERSION, SETUP));
    }

    #[test]
    fn reads_sha256sum_lines() {
        let hash = "a".repeat(64);
        let upper = "A".repeat(64);
        assert_eq!(
            parse_checksum(&format!("{hash}  {SETUP}\n"), SETUP),
            Some(hash.clone())
        );
        assert_eq!(
            parse_checksum(&format!("{upper} *{SETUP}\r\n"), SETUP),
            Some(hash.clone())
        );
        assert_eq!(
            parse_checksum(&format!("\n{hash}\n"), SETUP),
            Some(hash.clone())
        );
        assert_eq!(parse_checksum(&format!("{hash}  Other.exe\n"), SETUP), None);
        assert_eq!(
            parse_checksum(&format!("{}  {SETUP}\n", "a".repeat(63)), SETUP),
            None
        );
        assert_eq!(
            parse_checksum(&format!("{}  {SETUP}\n", "g".repeat(64)), SETUP),
            None
        );
        assert_eq!(parse_checksum("", SETUP), None);
        assert_eq!(parse_checksum("<html>Not Found</html>", SETUP), None);
    }

    #[test]
    fn plans_follow_the_install() {
        let release = release(30 * 1024 * 1024);
        // Forward slashes keep the test meaningful on Linux too.
        let exe = Path::new("C:/Program Files/Nomadeer/Nomadeer.exe");
        let appimage = Path::new("/home/pilot/Apps/Nomadeer-0.1.0-x86_64.AppImage");
        let plan = |kind, exe, appimage| {
            plan_update(&release, kind, Edition::Optimized, exe, appimage, &github())
        };

        let windows = plan(
            InstallKind::WindowsInstaller { per_user: false },
            Some(exe),
            None,
        )
        .unwrap();
        assert_eq!(windows.file.name, SETUP);
        assert_eq!(windows.checksum.name, format!("{SETUP}.sha256"));
        assert_eq!(
            windows.target,
            Target::WindowsInstaller {
                per_user: false,
                install_dir: PathBuf::from("C:/Program Files/Nomadeer"),
            }
        );

        let linux = plan(InstallKind::AppImage, None, Some(appimage)).unwrap();
        assert_eq!(linux.file.name, APPIMAGE);
        assert_eq!(
            linux.target,
            Target::AppImage {
                path: appimage.to_path_buf()
            }
        );

        assert_eq!(
            plan(InstallKind::AppImage, None, None),
            Err(UpdateError::NotFound)
        );
        assert_eq!(
            plan(InstallKind::WindowsInstaller { per_user: true }, None, None),
            Err(UpdateError::NotFound)
        );
        for kind in [
            InstallKind::WindowsPortable,
            InstallKind::LinuxTarball,
            InstallKind::Flatpak,
            InstallKind::Development,
            InstallKind::Other,
        ] {
            assert_eq!(
                plan(kind, Some(exe), Some(appimage)),
                Err(UpdateError::NotSelfUpdating),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn plans_refuse_unsafe_files() {
        let appimage = Some(Path::new("/home/pilot/Apps/Nomadeer.AppImage"));
        let plan = |release: &Release| {
            plan_update(
                release,
                InstallKind::AppImage,
                Edition::Compatible,
                None,
                appimage,
                &github(),
            )
        };
        let good = release(40 * 1024 * 1024);
        assert!(plan(&good).is_ok());

        let mut missing = good.clone();
        missing.assets.retain(|asset| asset.name != APPIMAGE);
        assert_eq!(plan(&missing), Err(UpdateError::NoFile));

        let mut unchecked = good.clone();
        unchecked
            .assets
            .retain(|asset| !asset.name.ends_with(".sha256"));
        assert_eq!(
            plan(&unchecked),
            Err(UpdateError::NoChecksum(APPIMAGE.to_owned()))
        );

        for (name, url) in [
            (
                APPIMAGE,
                "https://evil.example/Nomadeer-0.2.0-x86_64.AppImage",
            ),
            (
                "Nomadeer-0.2.0-x86_64.AppImage.sha256",
                "https://evil.example/sum",
            ),
        ] {
            let mut moved = good.clone();
            moved
                .assets
                .iter_mut()
                .find(|asset| asset.name == name)
                .unwrap()
                .download_url = url.to_owned();
            assert_eq!(
                plan(&moved),
                Err(UpdateError::UntrustedSource(name.to_owned()))
            );
        }

        for size in [0, MIN_UPDATE_BYTES - 1, MAX_UPDATE_BYTES + 1] {
            assert_eq!(
                plan(&release(size)),
                Err(UpdateError::WrongSize(APPIMAGE.to_owned())),
                "{size}"
            );
        }
        let mut huge_sum = good.clone();
        huge_sum.assets.iter_mut().for_each(|asset| {
            if asset.name.ends_with(".sha256") {
                asset.size = MAX_CHECKSUM_BYTES + 1;
            }
        });
        assert_eq!(
            plan(&huge_sum),
            Err(UpdateError::UnreadableChecksum(APPIMAGE.to_owned()))
        );
    }

    #[test]
    fn copies_hash_as_they_go_and_stop_when_asked() {
        let bytes = payload(1);
        let never = AtomicBool::new(false);
        let mut written = Vec::new();
        let mut last = 0;
        let (count, hash) = copy_hashed(
            &mut bytes.as_slice(),
            &mut written,
            APPIMAGE,
            bytes.len() as u64,
            &never,
            &mut |done| last = done,
        )
        .unwrap();
        assert_eq!((count, last), (bytes.len() as u64, bytes.len() as u64));
        assert_eq!(hash, sha256(&bytes));
        assert_eq!(written, bytes);

        // More than the release lists is refused before it is all written.
        let mut sink = Vec::new();
        assert_eq!(
            copy_hashed(
                &mut bytes.as_slice(),
                &mut sink,
                APPIMAGE,
                1000,
                &never,
                &mut |_| {},
            ),
            Err(UpdateError::WrongSize(APPIMAGE.to_owned()))
        );
        assert!(sink.len() <= 1000);

        let cancel = AtomicBool::new(false);
        let mut chunks = 0;
        let result = copy_hashed(
            &mut bytes.as_slice(),
            &mut io::sink(),
            APPIMAGE,
            bytes.len() as u64,
            &cancel,
            &mut |_| {
                chunks += 1;
                cancel.store(true, Ordering::Relaxed);
            },
        );
        assert_eq!(result, Err(UpdateError::Cancelled));
        assert_eq!(chunks, 1);
    }

    #[test]
    fn verifies_files_before_they_run() {
        let dir = TempDir::new("verify");
        let path = dir.path.join(SETUP);
        fs::write(&path, b"setup").unwrap();
        assert_eq!(verify_file(&path, &sha256(b"setup")), Ok(()));
        assert_eq!(
            verify_file(&path, &sha256(b"other")),
            Err(UpdateError::Mismatch(SETUP.to_owned()))
        );
        // Nothing is started when the file does not check out.
        assert_eq!(
            launch_installer(&path, &sha256(b"other"), &[], false),
            Err(UpdateError::Mismatch(SETUP.to_owned()))
        );
        assert!(matches!(
            verify_file(&dir.path.join("missing.exe"), &sha256(b"")),
            Err(UpdateError::Disk(_))
        ));
    }

    #[test]
    fn new_appimages_stay_executable() {
        assert_eq!(executable_mode(None), 0o755);
        assert_eq!(executable_mode(Some(0o100_755)), 0o755);
        assert_eq!(executable_mode(Some(0o100_700)), 0o700);
        assert_eq!(executable_mode(Some(0o100_644)), 0o755);
        assert_eq!(executable_mode(Some(0o100_600)), 0o700);
        assert_eq!(executable_mode(Some(0o104_755)), 0o755, "no setuid");
    }

    #[test]
    fn replaces_the_appimage_in_one_step() {
        let dir = TempDir::new("replace");
        let appimage = dir.path.join("Nomadeer.AppImage");
        let staged = dir.path.join(format!("{STAGING_PREFIX}{APPIMAGE}"));
        fs::write(&appimage, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&appimage, fs::Permissions::from_mode(0o750)).unwrap();
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
        }
        install_appimage(&staged, &appimage).unwrap();
        assert_eq!(fs::read(&appimage).unwrap(), b"new");
        assert!(!staged.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&appimage).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o750);
        }
    }

    #[test]
    fn cleans_up_only_its_own_leftovers() {
        let downloads = TempDir::new("downloads");
        for name in [SETUP, "Nomadeer-0.1.5-Setup.exe.part", SETUP_LOG] {
            fs::write(downloads.path.join(name), b"x").unwrap();
        }
        fs::create_dir(downloads.path.join("folder")).unwrap();
        clean_downloads(&downloads.path);
        let left: Vec<_> = fs::read_dir(&downloads.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left.contains(&SETUP_LOG.to_owned()));
        assert!(left.contains(&"folder".to_owned()));
        clean_downloads(&downloads.path.join("missing"));

        let apps = TempDir::new("apps");
        let appimage = apps.path.join("Nomadeer.AppImage");
        let staged = apps.path.join(format!("{STAGING_PREFIX}{APPIMAGE}"));
        let other = apps.path.join("Other.AppImage");
        for path in [&appimage, &staged, &other] {
            fs::write(path, b"x").unwrap();
        }
        clean_staged_appimages(&appimage);
        assert!(appimage.exists() && other.exists());
        assert!(!staged.exists());
    }

    /// Serves files by path on localhost; a value starting with `->` redirects.
    fn serve(files: HashMap<String, Vec<u8>>) -> (String, Arc<AtomicUsize>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let served = Arc::new(AtomicUsize::new(0));
        let counter = served.clone();
        std::thread::spawn(move || {
            while let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(10)) {
                counter.fetch_add(1, Ordering::SeqCst);
                let response = match files.get(request.url()) {
                    Some(body) if body.starts_with(b"->") => {
                        let target = String::from_utf8_lossy(&body[2..]).into_owned();
                        tiny_http::Response::from_data(Vec::new())
                            .with_status_code(302)
                            .with_header(tiny_http::Header::from_bytes("Location", target).unwrap())
                    }
                    Some(body) => tiny_http::Response::from_data(body.clone()),
                    None => tiny_http::Response::from_data(Vec::new()).with_status_code(404),
                };
                let _ = request.respond(response);
            }
        });
        (base, served)
    }

    fn served_asset(base: &str, name: &str, size: u64) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_owned(),
            size,
            download_url: format!("{base}/dl/{name}"),
        }
    }

    fn served_plan(base: &str, name: &str, size: u64, target: Target) -> UpdatePlan {
        UpdatePlan {
            version: VERSION.to_owned(),
            file: served_asset(base, name, size),
            checksum: served_asset(base, &format!("{name}.sha256"), 100),
            target,
            edition: Edition::Compatible,
        }
    }

    #[test]
    fn downloads_and_replaces_an_appimage() {
        let new = payload(7);
        let sum = format!("{}  {APPIMAGE}\n", sha256(&new));
        let (base, _) = serve(HashMap::from([
            (format!("/dl/{APPIMAGE}"), new.clone()),
            (format!("/dl/{APPIMAGE}.sha256"), sum.into_bytes()),
        ]));
        let sources = Sources {
            test_server: Some(base.clone()),
        };
        let apps = TempDir::new("appimage");
        let appimage = apps.path.join("Nomadeer-0.1.0-x86_64.AppImage");
        fs::write(&appimage, b"old").unwrap();
        let plan = served_plan(
            &base,
            APPIMAGE,
            new.len() as u64,
            Target::AppImage {
                path: appimage.clone(),
            },
        );
        let mut stages = Vec::new();
        let prepared = prepare_update(
            &plan,
            &apps.path.join("unused"),
            &sources,
            &AtomicBool::new(false),
            &mut |stage| stages.push(stage),
        )
        .unwrap();
        assert_eq!(
            prepared,
            Prepared::AppImage {
                path: appimage.clone()
            }
        );
        assert_eq!(fs::read(&appimage).unwrap(), new);
        assert_eq!(stages.last(), Some(&Stage::Installing));
        assert!(stages.contains(&Stage::Downloading {
            done: new.len() as u64,
            total: new.len() as u64
        }));
        assert_eq!(
            fs::read_dir(&apps.path).unwrap().count(),
            1,
            "nothing left over"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&appimage).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
    }

    #[test]
    fn a_bad_download_never_replaces_anything() {
        let new = payload(9);
        let wrong_sum = format!("{}  {APPIMAGE}\n", sha256(b"something else"));
        let (base, _) = serve(HashMap::from([
            (format!("/dl/{APPIMAGE}"), new.clone()),
            (format!("/dl/{APPIMAGE}.sha256"), wrong_sum.into_bytes()),
            (
                format!("/dl/other/{APPIMAGE}.sha256"),
                b"->https://evil.example/sum".to_vec(),
            ),
        ]));
        let sources = Sources {
            test_server: Some(base.clone()),
        };
        let apps = TempDir::new("mismatch");
        let appimage = apps.path.join("Nomadeer.AppImage");
        fs::write(&appimage, b"old").unwrap();
        let target = Target::AppImage {
            path: appimage.clone(),
        };
        let run = |plan: &UpdatePlan| {
            prepare_update(
                plan,
                &apps.path,
                &sources,
                &AtomicBool::new(false),
                &mut |_| {},
            )
        };

        let plan = served_plan(&base, APPIMAGE, new.len() as u64, target.clone());
        assert_eq!(run(&plan), Err(UpdateError::Mismatch(APPIMAGE.to_owned())));

        let mut wrong_size = plan.clone();
        wrong_size.file.size += 1;
        assert_eq!(
            run(&wrong_size),
            Err(UpdateError::WrongSize(APPIMAGE.to_owned()))
        );

        let mut redirected = plan.clone();
        redirected.checksum.download_url = format!("{base}/dl/other/{APPIMAGE}.sha256");
        assert_eq!(
            run(&redirected),
            Err(UpdateError::UntrustedSource(format!("{APPIMAGE}.sha256")))
        );

        let mut missing = plan.clone();
        missing.file.download_url = format!("{base}/dl/missing");
        assert!(matches!(run(&missing), Err(UpdateError::Network(_))));

        assert_eq!(fs::read(&appimage).unwrap(), b"old");
        assert_eq!(
            fs::read_dir(&apps.path).unwrap().count(),
            1,
            "nothing left over"
        );
    }

    #[test]
    fn cancelling_leaves_the_appimage_alone() {
        let new = payload(3);
        let sum = format!("{}  {APPIMAGE}\n", sha256(&new));
        let (base, _) = serve(HashMap::from([
            (format!("/dl/{APPIMAGE}"), new.clone()),
            (format!("/dl/{APPIMAGE}.sha256"), sum.into_bytes()),
        ]));
        let apps = TempDir::new("cancel");
        let appimage = apps.path.join("Nomadeer.AppImage");
        fs::write(&appimage, b"old").unwrap();
        let plan = served_plan(
            &base,
            APPIMAGE,
            new.len() as u64,
            Target::AppImage {
                path: appimage.clone(),
            },
        );
        let cancel = AtomicBool::new(false);
        let result = prepare_update(
            &plan,
            &apps.path,
            &Sources {
                test_server: Some(base),
            },
            &cancel,
            &mut |stage| {
                if matches!(stage, Stage::Downloading { done, .. } if done > 0) {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(result, Err(UpdateError::Cancelled));
        assert_eq!(fs::read(&appimage).unwrap(), b"old");
        assert_eq!(fs::read_dir(&apps.path).unwrap().count(), 1);
    }

    #[test]
    fn a_read_only_folder_is_explained() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let apps = TempDir::new("read-only");
            let appimage = apps.path.join("Nomadeer.AppImage");
            fs::write(&appimage, b"old").unwrap();
            fs::set_permissions(&apps.path, fs::Permissions::from_mode(0o555)).unwrap();
            let staged = apps.path.join(format!("{STAGING_PREFIX}{APPIMAGE}"));
            let result = create_staging(&staged, &apps.path);
            fs::set_permissions(&apps.path, fs::Permissions::from_mode(0o755)).unwrap();
            // Root writes anywhere, so there is nothing to explain.
            if result.is_err() {
                assert_eq!(
                    result.err(),
                    Some(UpdateError::NotWritable(apps.path.clone()))
                );
            }
        }
    }

    #[test]
    fn downloads_setup_for_windows_installs() {
        let setup = payload(5);
        let sum = format!("{}  {SETUP}\n", sha256(&setup));
        let (base, _) = serve(HashMap::from([
            (format!("/dl/{SETUP}"), setup.clone()),
            (format!("/dl/{SETUP}.sha256"), sum.into_bytes()),
        ]));
        let downloads = TempDir::new("setup");
        fs::write(
            downloads.path.join("Nomadeer-0.1.5-Setup.exe"),
            b"old setup",
        )
        .unwrap();
        let install_dir = PathBuf::from(r"C:\Users\pilot\AppData\Local\Programs\Nomadeer");
        let plan = served_plan(
            &base,
            SETUP,
            setup.len() as u64,
            Target::WindowsInstaller {
                per_user: true,
                install_dir: install_dir.clone(),
            },
        );
        let prepared = prepare_update(
            &plan,
            &downloads.path,
            &Sources {
                test_server: Some(base),
            },
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let installer = downloads.path.join(SETUP);
        assert_eq!(
            prepared,
            Prepared::Installer {
                path: installer.clone(),
                sha256: sha256(&setup),
                arguments: installer_arguments(
                    true,
                    Edition::Compatible,
                    &install_dir,
                    &downloads.path.join(SETUP_LOG)
                ),
                elevate: false,
            }
        );
        assert_eq!(fs::read(&installer).unwrap(), setup);
        // The older download was cleared away first.
        assert_eq!(fs::read_dir(&downloads.path).unwrap().count(), 1);
        assert_eq!(verify_file(&installer, &sha256(&setup)), Ok(()));
    }

    #[test]
    fn every_problem_says_what_to_do() {
        for error in [
            UpdateError::NotSelfUpdating,
            UpdateError::NoFile,
            UpdateError::NoChecksum(SETUP.to_owned()),
            UpdateError::UntrustedSource(SETUP.to_owned()),
            UpdateError::WrongSize(SETUP.to_owned()),
            UpdateError::UnreadableChecksum(SETUP.to_owned()),
            UpdateError::Mismatch(SETUP.to_owned()),
            UpdateError::NotFound,
            UpdateError::Network("timed out".to_owned()),
            UpdateError::Disk("disk full".to_owned()),
            UpdateError::NotWritable(PathBuf::from("/opt/apps")),
            UpdateError::Declined,
            UpdateError::Launch("missing".to_owned()),
        ] {
            let message = error.message();
            assert!(error.is_failure());
            assert!(
                message.contains("download page")
                    || message.contains("press Update")
                    || message.contains("Press Update"),
                "{message}"
            );
        }
        assert!(!UpdateError::Cancelled.is_failure());
    }
}
