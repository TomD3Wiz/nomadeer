//! Finding out when a new Nomadeer release is out.
//!
//! Once a day Nomadeer asks GitHub for the newest published release (drafts and
//! pre-releases are never offered) and compares it with the running version.
//! Nothing is downloaded or installed here; this only reads the release and works
//! out which of its files suits the way this copy was installed.

use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::util::{DAY_SECS, HOUR_SECS};

/// The version this build was released as.
pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = concat!("Nomadeer/", env!("CARGO_PKG_VERSION"));
const GITHUB_API: &str = "https://api.github.com";
const LATEST_RELEASE_PATH: &str = "/repos/TomD3Wiz/nomadeer/releases/latest";
/// Where players can always find every release.
pub(crate) const RELEASES_PAGE: &str = "https://github.com/TomD3Wiz/nomadeer/releases";
/// Points update checks at another server (`http://127.0.0.1:8080`, say) for
/// testing. The repository path is appended as it is for api.github.com.
const API_OVERRIDE_VAR: &str = "NOMADEER_UPDATE_API";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// How often automatic checks run.
pub(crate) const CHECK_INTERVAL_SECS: u64 = DAY_SECS;
/// How long to wait before trying again after a check failed.
pub(crate) const RETRY_AFTER_FAILURE_SECS: u64 = HOUR_SECS;
/// When GitHub turns a check away without saying for how long.
const DEFAULT_RATE_LIMIT_SECS: u64 = HOUR_SECS;
const MIN_RATE_LIMIT_SECS: u64 = 60;

// Versions -------------------------------------------------------------------

/// A semantic version. Build metadata (`+...`) is accepted and ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<PreRelease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreRelease {
    Numeric(u64),
    Text(String),
}

impl Version {
    /// Parses `1.2.3`, `v1.2.3`, `1.2.3-rc.1` or `1.2.3+build`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let text = text.split_once('+').map_or(text, |(version, _)| version);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core.split('.').map(parse_number);
        let (major, minor, patch) = (numbers.next()??, numbers.next()??, numbers.next()??);
        if numbers.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|part| {
                    if part.is_empty()
                        || !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    {
                        None
                    } else if part.chars().all(|c| c.is_ascii_digit()) {
                        part.parse().ok().map(PreRelease::Numeric)
                    } else {
                        Some(PreRelease::Text(part.to_owned()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

fn parse_number(part: &str) -> Option<u64> {
    if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                // A pre-release comes before the release it leads up to.
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PreRelease {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(a), Self::Numeric(b)) => a.cmp(b),
            (Self::Numeric(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for PreRelease {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Whether `latest` is newer than `current`. Anything that is not a version, on
/// either side, is never offered.
pub(crate) fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

// Releases -------------------------------------------------------------------

/// A published release, as much of it as Nomadeer shows or downloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Release {
    /// The tag without its leading `v`: `0.2.0`.
    pub(crate) version: String,
    /// The release notes, in GitHub's Markdown.
    #[serde(default)]
    pub(crate) notes: String,
    /// The release's page on GitHub.
    #[serde(default)]
    pub(crate) page_url: String,
    /// When it was published, as GitHub gives it: `2026-10-17T09:30:00Z`.
    #[serde(default)]
    pub(crate) published_at: String,
    #[serde(default)]
    pub(crate) assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReleaseAsset {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) download_url: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    #[serde(default)]
    size: u64,
    browser_download_url: String,
}

/// Reads GitHub's release JSON. `Ok(None)` for a draft or pre-release, which
/// `/releases/latest` never returns but which are not offered either way.
pub(crate) fn parse_release(json: &str) -> Result<Option<Release>, String> {
    let release: GithubRelease =
        serde_json::from_str(json).map_err(|error| format!("Unreadable release: {error}"))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let tag = release.tag_name.trim();
    if Version::parse(tag).is_none() {
        return Err(format!("Release {tag} has no version number"));
    }
    let version = tag.strip_prefix(['v', 'V']).unwrap_or(tag).to_owned();
    Ok(Some(Release {
        version,
        notes: release.body.unwrap_or_default().replace("\r\n", "\n"),
        page_url: release.html_url.unwrap_or_default(),
        published_at: release.published_at.unwrap_or_default(),
        assets: release
            .assets
            .into_iter()
            .map(|asset| ReleaseAsset {
                name: asset.name,
                size: asset.size,
                download_url: asset.browser_download_url,
            })
            .collect(),
    }))
}

impl Release {
    /// The release's GitHub page, or the list of releases when the page given is
    /// not on GitHub.
    pub(crate) fn download_page(&self) -> &str {
        if self.page_url.starts_with("https://github.com/") {
            &self.page_url
        } else {
            RELEASES_PAGE
        }
    }

    /// `17 Oct 2026`, or nothing when GitHub gave no date.
    pub(crate) fn published_date(&self) -> Option<String> {
        format_date(&self.published_at)
    }

    pub(crate) fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

/// `2026-10-17T09:30:00Z` as `17 Oct 2026`.
fn format_date(timestamp: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = timestamp.get(..10)?;
    let mut parts = date.split('-');
    let year = parse_number(parts.next()?)?;
    let month = parse_number(parts.next()?)?;
    let day = parse_number(parts.next()?)?;
    let month = MONTHS.get(usize::try_from(month).ok()?.checked_sub(1)?)?;
    if !(1..=31).contains(&day) {
        return None;
    }
    Some(format!("{day} {month} {year}"))
}

/// One piece of release notes, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteBlock {
    Heading(String),
    Paragraph(String),
    Bullet(String),
}

/// Release notes as headings, paragraphs and bullets, with Markdown emphasis,
/// code marks and links reduced to their text. The downloads table GitHub
/// releases end with is left out; the update panel says which file to pick.
pub(crate) fn note_blocks(notes: &str) -> Vec<NoteBlock> {
    let mut blocks = Vec::new();
    let mut current: Option<NoteBlock> = None;
    let flush = |current: &mut Option<NoteBlock>, blocks: &mut Vec<NoteBlock>| {
        if let Some(block) = current.take() {
            blocks.push(block);
        }
    };
    for line in notes.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut current, &mut blocks);
            continue;
        }
        if trimmed.starts_with('#') {
            flush(&mut current, &mut blocks);
            let heading = plain_text(trimmed.trim_start_matches('#').trim());
            if heading.eq_ignore_ascii_case("downloads") {
                break;
            }
            if !heading.is_empty() {
                blocks.push(NoteBlock::Heading(heading));
            }
            continue;
        }
        if trimmed.starts_with('|') {
            // Tables do not fit a narrow panel; GitHub shows them in full.
            flush(&mut current, &mut blocks);
            continue;
        }
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            flush(&mut current, &mut blocks);
            current = Some(NoteBlock::Bullet(plain_text(item)));
            continue;
        }
        let text = plain_text(trimmed);
        match &mut current {
            Some(NoteBlock::Paragraph(existing) | NoteBlock::Bullet(existing)) => {
                existing.push(' ');
                existing.push_str(&text);
            }
            _ => current = Some(NoteBlock::Paragraph(text)),
        }
    }
    flush(&mut current, &mut blocks);
    blocks
}

/// Drops `**`, `__` and backticks, and turns `[text](url)` into `text`.
fn plain_text(markdown: &str) -> String {
    let mut text = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(start) = rest.find('[') {
        text.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(close) = after.find("](")
            && let Some(end) = after[close + 2..].find(')')
        {
            text.push_str(&after[..close]);
            rest = &after[close + 2 + end + 1..];
        } else {
            text.push('[');
            rest = after;
        }
    }
    text.push_str(rest);
    text.replace("**", "").replace("__", "").replace('`', "")
}

// How this copy was installed --------------------------------------------------

/// Which build is running: the optimized one needs AVX2 (x86-64-v3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edition {
    Optimized,
    Compatible,
}

impl Edition {
    pub(crate) fn current() -> Self {
        if cfg!(all(target_arch = "x86_64", target_feature = "avx2")) {
            Self::Optimized
        } else {
            Self::Compatible
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Optimized => "optimized",
            Self::Compatible => "compatible",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallKind {
    /// Installed with Nomadeer-x.y.z-Setup.exe, for this user or for all users.
    WindowsInstaller {
        per_user: bool,
    },
    /// Unzipped from Nomadeer-x.y.z-Windows-Portable.zip.
    WindowsPortable,
    AppImage,
    Flatpak,
    /// Unpacked from one of the Linux tarballs.
    LinuxTarball,
    /// Built from source and run from the checkout; it never updates itself.
    Development,
    /// Any other system, which has no packages yet.
    Other,
}

/// Everything install detection depends on, so it can be decided without the
/// real environment.
pub(crate) struct InstallInputs<'a> {
    /// `std::env::consts::OS`.
    pub(crate) os: &'a str,
    /// The running executable.
    pub(crate) exe: Option<&'a Path>,
    pub(crate) env: &'a dyn Fn(&str) -> Option<OsString>,
    pub(crate) is_file: &'a dyn Fn(&Path) -> bool,
    /// Built without optimisations (`debug_assertions`).
    pub(crate) debug_build: bool,
}

/// Inno Setup's uninstaller, which it puts next to what it installed.
const WINDOWS_UNINSTALLER: &str = "unins000.exe";

pub(crate) fn current_install_kind() -> InstallKind {
    let exe = std::env::current_exe().ok();
    detect_install_kind(&InstallInputs {
        os: std::env::consts::OS,
        exe: exe.as_deref(),
        env: &|name| std::env::var_os(name),
        is_file: &|path| path.is_file(),
        debug_build: cfg!(debug_assertions),
    })
}

pub(crate) fn detect_install_kind(inputs: &InstallInputs<'_>) -> InstallKind {
    let var = |name: &str| (inputs.env)(name).filter(|value| !value.is_empty());
    let exe_dir = inputs.exe.and_then(Path::parent);

    if inputs.os == "linux"
        && (var("FLATPAK_ID").is_some() || (inputs.is_file)(Path::new("/.flatpak-info")))
    {
        return InstallKind::Flatpak;
    }
    // The AppImage runtime sets APPIMAGE for everything started from it, so the
    // executable must also be inside the mounted image when APPDIR says where.
    if inputs.os == "linux"
        && var("APPIMAGE").is_some()
        && var("APPDIR").is_none_or(|appdir| inputs.exe.is_some_and(|exe| exe.starts_with(appdir)))
    {
        return InstallKind::AppImage;
    }
    if inputs.debug_build || exe_dir.is_some_and(|dir| in_cargo_target_dir(dir, inputs.is_file)) {
        return InstallKind::Development;
    }
    match inputs.os {
        "windows" => {
            let Some(exe_dir) = exe_dir else {
                return InstallKind::WindowsPortable;
            };
            if (inputs.is_file)(&exe_dir.join(WINDOWS_UNINSTALLER)) {
                let per_user = var("LOCALAPPDATA").is_some_and(|local| {
                    windows_path_starts_with(&exe_dir.to_string_lossy(), &local.to_string_lossy())
                });
                InstallKind::WindowsInstaller { per_user }
            } else {
                // The portable marker, a data folder beside the exe, or a copy
                // that lost both: whatever is not installed is updated by hand.
                InstallKind::WindowsPortable
            }
        }
        "linux" => InstallKind::LinuxTarball,
        _ => InstallKind::Other,
    }
}

/// Cargo puts builds under `target/` next to the project's Cargo.toml.
fn in_cargo_target_dir(exe_dir: &Path, is_file: &dyn Fn(&Path) -> bool) -> bool {
    exe_dir.ancestors().any(|dir| {
        dir.file_name().is_some_and(|name| name == "target")
            && dir
                .parent()
                .is_some_and(|project| is_file(&project.join("Cargo.toml")))
    })
}

/// Case-insensitive and indifferent to `/` or `\`, as Windows paths are.
fn windows_path_starts_with(path: &str, prefix: &str) -> bool {
    let normalise = |text: &str| {
        text.replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    let (path, prefix) = (normalise(path), normalise(prefix));
    !prefix.is_empty()
        && path
            .strip_prefix(&prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

/// What the update panel can offer this copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateRoute {
    /// Nomadeer can fetch and install the new file itself once the player
    /// presses Update.
    InApp,
    /// The player installs the downloaded Flatpak bundle with a command.
    FlatpakBundle,
    /// The player downloads the file and replaces this copy by hand.
    Download,
    /// Only the news: this copy is not one Nomadeer releases.
    NoticeOnly,
}

impl InstallKind {
    pub(crate) fn update_route(self) -> UpdateRoute {
        match self {
            Self::WindowsInstaller { .. } | Self::AppImage => UpdateRoute::InApp,
            Self::Flatpak => UpdateRoute::FlatpakBundle,
            Self::WindowsPortable | Self::LinuxTarball => UpdateRoute::Download,
            Self::Development | Self::Other => UpdateRoute::NoticeOnly,
        }
    }

    /// How the player would describe this copy.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::WindowsInstaller { .. } => "installed",
            Self::WindowsPortable => "portable",
            Self::AppImage => "AppImage",
            Self::Flatpak => "Flatpak",
            Self::LinuxTarball => "Linux download",
            Self::Development => "development build",
            Self::Other => "",
        }
    }
}

/// The release file that replaces this copy, named as tools/release.py names it.
pub(crate) fn asset_name(kind: InstallKind, edition: Edition, version: &str) -> Option<String> {
    match kind {
        InstallKind::WindowsInstaller { .. } => Some(format!("Nomadeer-{version}-Setup.exe")),
        InstallKind::WindowsPortable => Some(format!("Nomadeer-{version}-Windows-Portable.zip")),
        InstallKind::AppImage => Some(format!("Nomadeer-{version}-x86_64.AppImage")),
        InstallKind::Flatpak => Some(format!("Nomadeer-{version}-x86_64.flatpak")),
        InstallKind::LinuxTarball => Some(format!(
            "nomadeer-{version}-linux-x86_64-{}.tar.gz",
            edition.label()
        )),
        InstallKind::Development | InstallKind::Other => None,
    }
}

/// A release file and the `.sha256` published beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AssetChoice<'a> {
    pub(crate) file: &'a ReleaseAsset,
    pub(crate) checksum: Option<&'a ReleaseAsset>,
}

pub(crate) fn pick_asset(
    release: &Release,
    kind: InstallKind,
    edition: Edition,
) -> Option<AssetChoice<'_>> {
    let name = asset_name(kind, edition, &release.version)?;
    let file = release.asset(&name)?;
    Some(AssetChoice {
        file,
        checksum: release.asset(&format!("{name}.sha256")),
    })
}

/// The command that installs a downloaded Flatpak bundle over this one.
pub(crate) fn flatpak_install_command(file_name: &str) -> String {
    format!("flatpak install --user {file_name}")
}

// Checking -------------------------------------------------------------------

/// What Nomadeer remembers about update checks between launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateState {
    /// Check once a day on its own. On unless the player turns it off.
    pub(crate) check_automatically: bool,
    /// The one-line note about update checks has been shown.
    pub(crate) notice_shown: bool,
    /// The last check GitHub answered, in Unix seconds.
    pub(crate) last_checked_at: Option<u64>,
    /// GitHub's tag for `latest`, sent back so an unchanged answer is free.
    pub(crate) etag: String,
    /// The newest release GitHub reported, if any.
    pub(crate) latest: Option<Release>,
    /// A version the player chose to skip.
    pub(crate) skipped_version: String,
    /// GitHub asked for no more checks until then.
    pub(crate) rate_limited_until: Option<u64>,
    /// The last check failed; automatic checks wait until then.
    pub(crate) retry_at: Option<u64>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            check_automatically: true,
            notice_shown: false,
            last_checked_at: None,
            etag: String::new(),
            latest: None,
            skipped_version: String::new(),
            rate_limited_until: None,
            retry_at: None,
        }
    }
}

impl UpdateState {
    /// Whether an automatic check should run now.
    pub(crate) fn automatic_check_due(&self, now: u64) -> bool {
        self.check_automatically
            && self.rate_limit_wait(now).is_none()
            && self.retry_at.is_none_or(|at| now >= at)
            && self.last_checked_at.is_none_or(|last| {
                // A clock that went backwards must not stop checks until it
                // catches up.
                last > now || now - last >= CHECK_INTERVAL_SECS
            })
    }

    /// Seconds until GitHub accepts checks again, while it is turning them away.
    pub(crate) fn rate_limit_wait(&self, now: u64) -> Option<u64> {
        self.rate_limited_until
            .filter(|&until| until > now)
            .map(|until| until - now)
    }

    /// The ETag to send, only while the release it stands for is still known.
    pub(crate) fn request_etag(&self) -> Option<&str> {
        (!self.etag.is_empty() && self.latest.is_some()).then_some(self.etag.as_str())
    }

    /// The release to offer: newer than `current` and not skipped.
    pub(crate) fn offered_update(&self, current: &str) -> Option<&Release> {
        self.newer_release(current)
            .filter(|release| release.version != self.skipped_version)
    }

    /// The newest release when it is newer than `current`, skipped or not.
    pub(crate) fn newer_release(&self, current: &str) -> Option<&Release> {
        self.latest
            .as_ref()
            .filter(|release| is_newer(&release.version, current))
    }

    pub(crate) fn apply(&mut self, outcome: CheckOutcome, now: u64) {
        match outcome {
            CheckOutcome::Latest { release, etag } => {
                self.latest = release;
                self.etag = etag;
                self.last_checked_at = Some(now);
                self.retry_at = None;
                self.rate_limited_until = None;
            }
            CheckOutcome::NotModified => {
                self.last_checked_at = Some(now);
                self.retry_at = None;
                self.rate_limited_until = None;
            }
            CheckOutcome::RateLimited { until } => self.rate_limited_until = Some(until),
            CheckOutcome::Failed { .. } => self.retry_at = Some(now + RETRY_AFTER_FAILURE_SECS),
        }
    }
}

/// How a check went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckOutcome {
    /// GitHub named the newest release, or said there is none yet.
    Latest {
        release: Option<Release>,
        etag: String,
    },
    /// Nothing changed since the ETag sent.
    NotModified,
    /// GitHub asked to wait until then (Unix seconds).
    RateLimited {
        until: u64,
    },
    Failed {
        error: String,
    },
}

/// The latest-release URL: GitHub's, unless NOMADEER_UPDATE_API names another
/// server for testing.
pub(crate) fn latest_release_url() -> String {
    let base = std::env::var(API_OVERRIDE_VAR)
        .ok()
        .map(|base| base.trim().trim_end_matches('/').to_owned())
        .filter(|base| base.starts_with("http://") || base.starts_with("https://"))
        .unwrap_or_else(|| GITHUB_API.to_owned());
    format!("{base}{LATEST_RELEASE_PATH}")
}

/// Asks `url` for the latest release. Never panics; every problem is an outcome.
pub(crate) fn check_latest(url: &str, etag: Option<&str>, now: u64) -> CheckOutcome {
    let client = match Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return CheckOutcome::Failed {
                error: error.to_string(),
            };
        }
    };
    let mut request = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }
    let response = match request.send() {
        Ok(response) => response,
        Err(error) => {
            return CheckOutcome::Failed {
                error: format!("Could not reach GitHub: {}", without_url(error)),
            };
        }
    };
    let status = response.status();
    let headers = response.headers().clone();
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let body = if status == StatusCode::OK {
        match response.text() {
            Ok(body) => body,
            Err(error) => {
                return CheckOutcome::Failed {
                    error: format!("GitHub's answer was cut short: {}", without_url(error)),
                };
            }
        }
    } else {
        String::new()
    };
    interpret_response(status.as_u16(), &header, &body, now)
}

fn without_url(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

/// Turns GitHub's answer into an outcome. `header` looks up a response header.
pub(crate) fn interpret_response(
    status: u16,
    header: &dyn Fn(&str) -> Option<String>,
    body: &str,
    now: u64,
) -> CheckOutcome {
    match status {
        200 => match parse_release(body) {
            Ok(release) => CheckOutcome::Latest {
                release,
                etag: header("etag").unwrap_or_default(),
            },
            Err(error) => CheckOutcome::Failed { error },
        },
        304 => CheckOutcome::NotModified,
        // No release has been published yet.
        404 => CheckOutcome::Latest {
            release: None,
            etag: String::new(),
        },
        403 | 429 => CheckOutcome::RateLimited {
            until: rate_limit_until(header, now),
        },
        status => CheckOutcome::Failed {
            error: format!("GitHub answered with status {status}"),
        },
    }
}

/// When GitHub will take checks again, following Retry-After, then the rate
/// limit's reset time, and otherwise waiting an hour.
fn rate_limit_until(header: &dyn Fn(&str) -> Option<String>, now: u64) -> u64 {
    let number = |name: &str| header(name).and_then(|value| value.trim().parse::<u64>().ok());
    let until = number("retry-after")
        .map(|secs| now.saturating_add(secs))
        .or_else(|| {
            (number("x-ratelimit-remaining") == Some(0))
                .then(|| number("x-ratelimit-reset"))
                .flatten()
        })
        .unwrap_or(now + DEFAULT_RATE_LIMIT_SECS);
    until.clamp(now + MIN_RATE_LIMIT_SECS, now + CHECK_INTERVAL_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    const NOW: u64 = 1_792_000_000;
    const PORTABLE_MARKER: &str = "portable";

    /// Shaped like GitHub's answer for a published Nomadeer release, trimmed of
    /// the fields Nomadeer does not read.
    const RELEASE_JSON: &str = r#"{
      "url": "https://api.github.com/repos/TomD3Wiz/nomadeer/releases/250000000",
      "html_url": "https://github.com/TomD3Wiz/nomadeer/releases/tag/v0.2.0",
      "id": 250000000,
      "author": {"login": "TomD3Wiz", "id": 1, "type": "User"},
      "tag_name": "v0.2.0",
      "target_commitish": "main",
      "name": "Nomadeer v0.2.0",
      "draft": false,
      "immutable": false,
      "prerelease": false,
      "created_at": "2026-11-02T08:12:44Z",
      "published_at": "2026-11-03T10:01:02Z",
      "assets": [
        {"name": "Nomadeer-0.2.0-Setup.exe", "size": 31457280, "content_type": "application/x-msdownload", "state": "uploaded", "download_count": 3,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Setup.exe"},
        {"name": "Nomadeer-0.2.0-Setup.exe.sha256", "size": 91,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Setup.exe.sha256"},
        {"name": "Nomadeer-0.2.0-Windows-Portable.zip", "size": 40000000,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Windows-Portable.zip"},
        {"name": "Nomadeer-0.2.0-Windows-Portable.zip.sha256", "size": 103,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Windows-Portable.zip.sha256"},
        {"name": "Nomadeer-0.2.0-x86_64.AppImage", "size": 45000000,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-x86_64.AppImage"},
        {"name": "Nomadeer-0.2.0-x86_64.AppImage.sha256", "size": 97,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-x86_64.AppImage.sha256"},
        {"name": "Nomadeer-0.2.0-x86_64.AppImage.zsync", "size": 120000,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-x86_64.AppImage.zsync"},
        {"name": "Nomadeer-0.2.0-x86_64.flatpak", "size": 44000000,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-x86_64.flatpak"},
        {"name": "Nomadeer-0.2.0-x86_64.flatpak.sha256", "size": 96,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-x86_64.flatpak.sha256"},
        {"name": "nomadeer-0.2.0-linux-x86_64-optimized.tar.gz", "size": 20000000,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/nomadeer-0.2.0-linux-x86_64-optimized.tar.gz"},
        {"name": "nomadeer-0.2.0-linux-x86_64-optimized.tar.gz.sha256", "size": 112,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/nomadeer-0.2.0-linux-x86_64-optimized.tar.gz.sha256"},
        {"name": "nomadeer-0.2.0-linux-x86_64-compatible.tar.gz", "size": 19000000,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/nomadeer-0.2.0-linux-x86_64-compatible.tar.gz"},
        {"name": "nomadeer-0.2.0-linux-x86_64-compatible.tar.gz.sha256", "size": 113,
         "browser_download_url": "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/nomadeer-0.2.0-linux-x86_64-compatible.tar.gz.sha256"}
      ],
      "tarball_url": "https://api.github.com/repos/TomD3Wiz/nomadeer/tarball/v0.2.0",
      "zipball_url": "https://api.github.com/repos/TomD3Wiz/nomadeer/zipball/v0.2.0",
      "body": "Faster chain maps and calmer alerts.\r\n\r\n### Added\r\n\r\n- **Bookmarks** for your\r\n  favourite holes, with `Ctrl+B`.\r\n- See [the guide](https://example.com/guide).\r\n\r\n### Fixed\r\n\r\n- The map no longer jumps.\r\n\r\n## Downloads\r\n\r\n| Platform | File |\r\n|---|---|\r\n| Windows | `Nomadeer-0.2.0-Setup.exe` |\r\n"
    }"#;

    fn release() -> Release {
        parse_release(RELEASE_JSON).unwrap().unwrap()
    }

    #[test]
    fn versions_compare_as_semver() {
        let ordered = [
            "0.1.0-alpha",
            "0.1.0-alpha.1",
            "0.1.0-alpha.beta",
            "0.1.0-beta",
            "0.1.0-beta.2",
            "0.1.0-beta.11",
            "0.1.0-rc.1",
            "0.1.0",
            "0.1.1",
            "0.2.0",
            "0.10.0",
            "1.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(is_newer(pair[1], pair[0]), "{} > {}", pair[1], pair[0]);
            assert!(!is_newer(pair[0], pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("0.2.0+build.7", "0.2.0"));
        assert!(is_newer("v0.2.0", "0.1.0"));
        // A development build ahead of the newest release is offered nothing.
        assert!(!is_newer("0.2.0", "0.3.0-dev"));
        assert!(is_newer("0.3.0", "0.3.0-dev"));
        // Nothing that is not a version is ever offered.
        for bad in [
            "",
            "latest",
            "1.2",
            "1.2.3.4",
            "1.x.0",
            "1.2.3-",
            "1.2.3-a..b",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad:?}");
            assert!(!is_newer(bad, "0.1.0"), "{bad:?}");
            assert!(!is_newer("9.9.9", bad), "{bad:?}");
        }
    }

    #[test]
    fn parses_a_github_release() {
        let release = release();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(
            release.page_url,
            "https://github.com/TomD3Wiz/nomadeer/releases/tag/v0.2.0"
        );
        assert_eq!(release.download_page(), release.page_url);
        assert_eq!(release.published_date().as_deref(), Some("3 Nov 2026"));
        assert_eq!(release.assets.len(), 13);
        let setup = release.asset("Nomadeer-0.2.0-Setup.exe").unwrap();
        assert_eq!(setup.size, 31_457_280);
        assert_eq!(
            setup.download_url,
            "https://github.com/TomD3Wiz/nomadeer/releases/download/v0.2.0/Nomadeer-0.2.0-Setup.exe"
        );
        assert!(!release.notes.contains('\r'));

        // What is kept survives being stored and read back.
        let stored = serde_json::to_string(&release).unwrap();
        assert_eq!(serde_json::from_str::<Release>(&stored).unwrap(), release);
    }

    #[test]
    fn drafts_pre_releases_and_bad_tags_are_not_offered() {
        let draft = RELEASE_JSON.replace("\"draft\": false", "\"draft\": true");
        assert_eq!(parse_release(&draft), Ok(None));
        let pre = RELEASE_JSON.replace("\"prerelease\": false", "\"prerelease\": true");
        assert_eq!(parse_release(&pre), Ok(None));
        let bad_tag = RELEASE_JSON.replace("\"v0.2.0\",", "\"nightly\",");
        assert!(parse_release(&bad_tag).is_err());
        assert!(parse_release("<html>").is_err());

        // Missing optional fields are fine; a page off GitHub is not linked.
        let minimal =
            parse_release(r#"{"tag_name": "0.3.0", "html_url": "https://evil.example/"}"#)
                .unwrap()
                .unwrap();
        assert_eq!(minimal.version, "0.3.0");
        assert!(minimal.assets.is_empty());
        assert_eq!(minimal.published_date(), None);
        assert_eq!(minimal.download_page(), RELEASES_PAGE);
    }

    #[test]
    fn release_notes_read_as_plain_blocks() {
        assert_eq!(
            note_blocks(&release().notes),
            vec![
                NoteBlock::Paragraph("Faster chain maps and calmer alerts.".to_owned()),
                NoteBlock::Heading("Added".to_owned()),
                NoteBlock::Bullet("Bookmarks for your favourite holes, with Ctrl+B.".to_owned()),
                NoteBlock::Bullet("See the guide.".to_owned()),
                NoteBlock::Heading("Fixed".to_owned()),
                NoteBlock::Bullet("The map no longer jumps.".to_owned()),
            ]
        );
        assert_eq!(
            note_blocks("A [broken link and\n| a | table |\n* star"),
            vec![
                NoteBlock::Paragraph("A [broken link and".to_owned()),
                NoteBlock::Bullet("star".to_owned()),
            ]
        );
        assert!(note_blocks("").is_empty());
    }

    #[test]
    fn picks_the_file_for_each_install() {
        let release = release();
        let pick = |kind, edition| {
            pick_asset(&release, kind, edition).map(|choice| {
                (
                    choice.file.name.clone(),
                    choice.checksum.map(|sum| sum.name.clone()),
                )
            })
        };
        let named = |file: &str| Some((file.to_owned(), Some(format!("{file}.sha256"))));
        for edition in [Edition::Optimized, Edition::Compatible] {
            for per_user in [true, false] {
                assert_eq!(
                    pick(InstallKind::WindowsInstaller { per_user }, edition),
                    named("Nomadeer-0.2.0-Setup.exe")
                );
            }
            assert_eq!(
                pick(InstallKind::WindowsPortable, edition),
                named("Nomadeer-0.2.0-Windows-Portable.zip")
            );
            assert_eq!(
                pick(InstallKind::AppImage, edition),
                named("Nomadeer-0.2.0-x86_64.AppImage")
            );
            assert_eq!(
                pick(InstallKind::Flatpak, edition),
                named("Nomadeer-0.2.0-x86_64.flatpak")
            );
            assert_eq!(pick(InstallKind::Development, edition), None);
            assert_eq!(pick(InstallKind::Other, edition), None);
        }
        assert_eq!(
            pick(InstallKind::LinuxTarball, Edition::Optimized),
            named("nomadeer-0.2.0-linux-x86_64-optimized.tar.gz")
        );
        assert_eq!(
            pick(InstallKind::LinuxTarball, Edition::Compatible),
            named("nomadeer-0.2.0-linux-x86_64-compatible.tar.gz")
        );

        // A release missing the file offers no download for it.
        let mut partial = release.clone();
        partial
            .assets
            .retain(|asset| !asset.name.contains("AppImage"));
        assert!(pick_asset(&partial, InstallKind::AppImage, Edition::Optimized).is_none());
        partial
            .assets
            .retain(|asset| !asset.name.ends_with(".sha256"));
        let choice = pick_asset(&partial, InstallKind::Flatpak, Edition::Optimized).unwrap();
        assert_eq!(choice.checksum, None);

        assert_eq!(
            flatpak_install_command("Nomadeer-0.2.0-x86_64.flatpak"),
            "flatpak install --user Nomadeer-0.2.0-x86_64.flatpak"
        );
    }

    #[test]
    fn asset_names_match_the_release_tooling() {
        // tools/release.py's release_assets() is the source of these names.
        let script = include_str!("../tools/release.py");
        for kind in [
            InstallKind::WindowsInstaller { per_user: true },
            InstallKind::WindowsPortable,
            InstallKind::AppImage,
            InstallKind::Flatpak,
            InstallKind::LinuxTarball,
        ] {
            for edition in [Edition::Optimized, Edition::Compatible] {
                let name = asset_name(kind, edition, "{version}").unwrap();
                // The tarballs are named in a loop over both editions.
                let name =
                    name.replace(&format!("-{}.tar.gz", edition.label()), "-{variant}.tar.gz");
                assert!(script.contains(&format!("f\"{name}\"")), "{name}");
            }
        }
        assert!(script.contains(r#"for variant in ("optimized", "compatible"):"#));
    }

    #[test]
    fn install_routes() {
        assert_eq!(
            InstallKind::WindowsInstaller { per_user: true }.update_route(),
            UpdateRoute::InApp
        );
        assert_eq!(InstallKind::AppImage.update_route(), UpdateRoute::InApp);
        assert_eq!(
            InstallKind::Flatpak.update_route(),
            UpdateRoute::FlatpakBundle
        );
        assert_eq!(
            InstallKind::WindowsPortable.update_route(),
            UpdateRoute::Download
        );
        assert_eq!(
            InstallKind::LinuxTarball.update_route(),
            UpdateRoute::Download
        );
        assert_eq!(
            InstallKind::Development.update_route(),
            UpdateRoute::NoticeOnly
        );
        assert_eq!(InstallKind::Other.update_route(), UpdateRoute::NoticeOnly);
    }

    struct Machine {
        os: &'static str,
        exe: PathBuf,
        env: HashMap<&'static str, &'static str>,
        files: HashSet<PathBuf>,
        debug_build: bool,
    }

    impl Machine {
        fn linux(exe: &str) -> Self {
            Self {
                os: "linux",
                exe: PathBuf::from(exe),
                env: HashMap::from([("HOME", "/home/pilot")]),
                files: HashSet::new(),
                debug_build: false,
            }
        }

        fn windows(exe_dir: &str) -> Self {
            Self {
                os: "windows",
                exe: Path::new(exe_dir).join("Nomadeer.exe"),
                env: HashMap::from([("LOCALAPPDATA", r"C:\Users\pilot\AppData\Local")]),
                files: HashSet::new(),
                debug_build: false,
            }
        }

        fn file(mut self, path: PathBuf) -> Self {
            self.files.insert(path);
            self
        }

        /// A file next to the executable.
        fn beside_exe(self, name: &str) -> Self {
            let path = self.exe.parent().unwrap().join(name);
            self.file(path)
        }

        fn var(mut self, name: &'static str, value: &'static str) -> Self {
            self.env.insert(name, value);
            self
        }

        fn detect(&self) -> InstallKind {
            let env = |name: &str| self.env.get(name).map(OsString::from);
            let is_file = |path: &Path| self.files.contains(path);
            detect_install_kind(&InstallInputs {
                os: self.os,
                exe: Some(&self.exe),
                env: &env,
                is_file: &is_file,
                debug_build: self.debug_build,
            })
        }
    }

    #[test]
    fn detects_windows_installs() {
        let per_user = Machine::windows(r"C:\Users\pilot\AppData\Local\Programs\Nomadeer")
            .beside_exe(WINDOWS_UNINSTALLER);
        assert_eq!(
            per_user.detect(),
            InstallKind::WindowsInstaller { per_user: true }
        );
        // Windows paths ignore case.
        let shouted = Machine::windows(r"c:\users\PILOT\appdata\local\Programs\Nomadeer")
            .beside_exe(WINDOWS_UNINSTALLER);
        assert_eq!(
            shouted.detect(),
            InstallKind::WindowsInstaller { per_user: true }
        );

        let all_users =
            Machine::windows(r"C:\Program Files\Nomadeer").beside_exe(WINDOWS_UNINSTALLER);
        assert_eq!(
            all_users.detect(),
            InstallKind::WindowsInstaller { per_user: false }
        );

        // A sibling folder that merely starts with the same letters is not inside it.
        let lookalike = Machine::windows(r"C:\Users\pilot\AppData\LocalGames\Nomadeer")
            .beside_exe(WINDOWS_UNINSTALLER);
        assert_eq!(
            lookalike.detect(),
            InstallKind::WindowsInstaller { per_user: false }
        );

        let portable = Machine::windows(r"D:\Games\Nomadeer").beside_exe(PORTABLE_MARKER);
        assert_eq!(portable.detect(), InstallKind::WindowsPortable);
        // Without the uninstaller even a copy in Program Files is updated by hand.
        assert_eq!(
            Machine::windows(r"C:\Program Files\Nomadeer").detect(),
            InstallKind::WindowsPortable
        );
    }

    #[test]
    fn detects_linux_packages() {
        assert_eq!(
            Machine::linux("/home/pilot/nomadeer-0.1.0/nomadeer").detect(),
            InstallKind::LinuxTarball
        );

        let flatpak = Machine::linux("/app/lib/nomadeer/nomadeer-optimized");
        assert_eq!(
            flatpak
                .var("FLATPAK_ID", "io.github.TomD3Wiz.Nomadeer")
                .detect(),
            InstallKind::Flatpak
        );
        let flatpak_info = Machine::linux("/app/lib/nomadeer/nomadeer-optimized")
            .file(PathBuf::from("/.flatpak-info"));
        assert_eq!(flatpak_info.detect(), InstallKind::Flatpak);

        let appimage = Machine::linux("/tmp/.mount_NomadXY/usr/lib/nomadeer/nomadeer-compatible")
            .var(
                "APPIMAGE",
                "/home/pilot/Apps/Nomadeer-0.1.0-x86_64.AppImage",
            )
            .var("APPDIR", "/tmp/.mount_NomadXY");
        assert_eq!(appimage.detect(), InstallKind::AppImage);
        // Started from a terminal that itself runs inside some other AppImage.
        let inherited = Machine::linux("/home/pilot/nomadeer/nomadeer")
            .var("APPIMAGE", "/home/pilot/Apps/Terminal.AppImage")
            .var("APPDIR", "/tmp/.mount_TermAB");
        assert_eq!(inherited.detect(), InstallKind::LinuxTarball);
        // Empty variables mean nothing.
        let empty = Machine::linux("/home/pilot/nomadeer/nomadeer")
            .var("APPIMAGE", "")
            .var("FLATPAK_ID", "");
        assert_eq!(empty.detect(), InstallKind::LinuxTarball);

        // Neither variable means anything on Windows.
        let windows = Machine::windows(r"D:\Games\Nomadeer")
            .var("APPIMAGE", "/x.AppImage")
            .var("FLATPAK_ID", "x");
        assert_eq!(windows.detect(), InstallKind::WindowsPortable);
    }

    #[test]
    fn development_builds_never_update_themselves() {
        let checkout = Path::new("/home/pilot/src/nomadeer");
        for exe in [
            "/home/pilot/src/nomadeer/target/release/nomadeer",
            "/home/pilot/src/nomadeer/target/optimized/release/nomadeer",
            "/home/pilot/src/nomadeer/target/x86_64-pc-windows-gnu/release/nomadeer.exe",
        ] {
            let machine = Machine::linux(exe).file(checkout.join("Cargo.toml"));
            assert_eq!(machine.detect(), InstallKind::Development, "{exe}");
        }
        // A folder called target that is not a Cargo project's is just a folder.
        assert_eq!(
            Machine::linux("/home/pilot/target/nomadeer").detect(),
            InstallKind::LinuxTarball
        );
        // Unoptimised builds are always development builds.
        let mut debug = Machine::windows(r"C:\Program Files\Nomadeer").beside_exe("unins000.exe");
        debug.debug_build = true;
        assert_eq!(debug.detect(), InstallKind::Development);
        assert_eq!(
            Machine {
                os: "macos",
                ..Machine::linux("/Applications/Nomadeer")
            }
            .detect(),
            InstallKind::Other
        );
    }

    #[test]
    fn automatic_checks_run_daily() {
        let mut state = UpdateState::default();
        assert!(state.check_automatically);
        assert!(state.automatic_check_due(NOW), "never checked");

        state.last_checked_at = Some(NOW - CHECK_INTERVAL_SECS + 60);
        assert!(!state.automatic_check_due(NOW), "checked within a day");
        state.last_checked_at = Some(NOW - CHECK_INTERVAL_SECS);
        assert!(state.automatic_check_due(NOW), "a day ago");
        state.last_checked_at = Some(NOW + 3 * DAY_SECS);
        assert!(state.automatic_check_due(NOW), "clock went backwards");

        state.last_checked_at = None;
        state.check_automatically = false;
        assert!(!state.automatic_check_due(NOW), "turned off");
    }

    #[test]
    fn checks_wait_after_failures_and_rate_limits() {
        let mut state = UpdateState::default();
        state.apply(
            CheckOutcome::Failed {
                error: "offline".to_owned(),
            },
            NOW,
        );
        assert!(!state.automatic_check_due(NOW + 60));
        assert!(state.automatic_check_due(NOW + RETRY_AFTER_FAILURE_SECS));
        assert_eq!(state.rate_limit_wait(NOW), None);

        let until = NOW + 2 * RETRY_AFTER_FAILURE_SECS;
        state.apply(CheckOutcome::RateLimited { until }, NOW);
        assert_eq!(
            state.rate_limit_wait(NOW),
            Some(2 * RETRY_AFTER_FAILURE_SECS)
        );
        assert!(!state.automatic_check_due(NOW + RETRY_AFTER_FAILURE_SECS));
        assert!(state.automatic_check_due(until));
        assert_eq!(state.rate_limit_wait(until), None);

        // A good answer clears both.
        state.apply(CheckOutcome::NotModified, until);
        assert_eq!(state.retry_at, None);
        assert_eq!(state.rate_limited_until, None);
        assert_eq!(state.last_checked_at, Some(until));
    }

    #[test]
    fn etags_are_sent_only_with_a_known_release() {
        let mut state = UpdateState::default();
        state.apply(
            CheckOutcome::Latest {
                release: Some(release()),
                etag: "W/\"abc\"".to_owned(),
            },
            NOW,
        );
        assert_eq!(state.request_etag(), Some("W/\"abc\""));

        // Not modified keeps the release and its tag.
        state.apply(CheckOutcome::NotModified, NOW + DAY_SECS);
        assert_eq!(state.latest.as_ref().unwrap().version, "0.2.0");
        assert_eq!(state.request_etag(), Some("W/\"abc\""));

        // Without the release a 304 would leave nothing to show.
        state.latest = None;
        assert_eq!(state.request_etag(), None);

        state.apply(
            CheckOutcome::Latest {
                release: None,
                etag: String::new(),
            },
            NOW,
        );
        assert_eq!(state.request_etag(), None);
    }

    #[test]
    fn offers_newer_releases_that_were_not_skipped() {
        let mut state = UpdateState {
            latest: Some(release()),
            ..UpdateState::default()
        };
        assert_eq!(state.offered_update("0.1.0").unwrap().version, "0.2.0");
        assert!(state.offered_update("0.2.0").is_none());
        assert!(state.offered_update("0.3.0-dev").is_none());

        state.skipped_version = "0.2.0".to_owned();
        assert!(state.offered_update("0.1.0").is_none());
        assert!(state.newer_release("0.1.0").is_some());
        state.skipped_version = "0.1.5".to_owned();
        assert!(state.offered_update("0.1.0").is_some());
    }

    fn headers(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn reads_githubs_answers() {
        let none = headers(&[]);
        match interpret_response(200, &headers(&[("etag", "\"v2\"")]), RELEASE_JSON, NOW) {
            CheckOutcome::Latest { release, etag } => {
                assert_eq!(release.unwrap().version, "0.2.0");
                assert_eq!(etag, "\"v2\"");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            interpret_response(304, &none, "", NOW),
            CheckOutcome::NotModified
        );
        assert_eq!(
            interpret_response(404, &none, "", NOW),
            CheckOutcome::Latest {
                release: None,
                etag: String::new()
            }
        );
        assert!(matches!(
            interpret_response(200, &none, "{", NOW),
            CheckOutcome::Failed { .. }
        ));
        assert!(matches!(
            interpret_response(502, &none, "", NOW),
            CheckOutcome::Failed { .. }
        ));
    }

    #[test]
    fn respects_githubs_rate_limits() {
        let until = |status, pairs: &[(&str, &str)]| match interpret_response(
            status,
            &headers(pairs),
            "",
            NOW,
        ) {
            CheckOutcome::RateLimited { until } => until - NOW,
            other => panic!("{other:?}"),
        };
        assert_eq!(until(429, &[("retry-after", "120")]), 120);
        assert_eq!(
            until(
                403,
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &(NOW + 1800).to_string())
                ]
            ),
            1800
        );
        // Retry-After wins over the reset time.
        assert_eq!(
            until(
                403,
                &[
                    ("retry-after", "90"),
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &(NOW + 1800).to_string())
                ]
            ),
            90
        );
        // A reset time is only trusted when the limit is used up.
        assert_eq!(
            until(
                403,
                &[
                    ("x-ratelimit-remaining", "12"),
                    ("x-ratelimit-reset", &(NOW + 1800).to_string())
                ]
            ),
            DEFAULT_RATE_LIMIT_SECS
        );
        assert_eq!(until(429, &[]), DEFAULT_RATE_LIMIT_SECS);
        // Never sooner than a minute, never longer than a day.
        assert_eq!(until(429, &[("retry-after", "0")]), MIN_RATE_LIMIT_SECS);
        assert_eq!(
            until(429, &[("retry-after", "9999999")]),
            CHECK_INTERVAL_SECS
        );
        assert_eq!(
            until(
                403,
                &[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "5")]
            ),
            MIN_RATE_LIMIT_SECS
        );
    }

    #[test]
    fn dates_read_as_day_month_year() {
        assert_eq!(
            format_date("2026-10-17T09:30:00Z").as_deref(),
            Some("17 Oct 2026")
        );
        assert_eq!(format_date("2026-01-01").as_deref(), Some("1 Jan 2026"));
        assert_eq!(format_date("2026-13-01T00:00:00Z"), None);
        assert_eq!(format_date("2026-00-01T00:00:00Z"), None);
        assert_eq!(format_date("soon"), None);
    }

    /// A status, its headers and its body.
    type Answer = (u16, Vec<(&'static str, String)>, &'static str);

    /// Serves GitHub-shaped answers on localhost and records what was asked.
    struct FakeGithub {
        url: String,
        requests: std::sync::mpsc::Receiver<(String, Option<String>, Option<String>)>,
        handle: std::thread::JoinHandle<()>,
    }

    impl FakeGithub {
        fn start(answers: Vec<Answer>) -> Self {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let url = format!(
                "http://{}{LATEST_RELEASE_PATH}",
                server.server_addr().to_ip().unwrap()
            );
            let (tx, requests) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                for (status, headers, body) in answers {
                    let request = server.recv().unwrap();
                    let header = |name: &'static str| {
                        request
                            .headers()
                            .iter()
                            .find(|header| header.field.equiv(name))
                            .map(|header| header.value.to_string())
                    };
                    tx.send((
                        request.url().to_owned(),
                        header("If-None-Match"),
                        header("User-Agent")
                            .zip(header("Accept"))
                            .map(|(agent, accept)| format!("{agent} {accept}")),
                    ))
                    .unwrap();
                    let mut response =
                        tiny_http::Response::from_string(body).with_status_code(status);
                    for (name, value) in headers {
                        response.add_header(
                            tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                                .unwrap(),
                        );
                    }
                    request.respond(response).unwrap();
                }
            });
            Self {
                url,
                requests,
                handle,
            }
        }
    }

    #[test]
    fn checks_github_over_http_with_etags() {
        let github = FakeGithub::start(vec![
            (200, vec![("ETag", "W/\"first\"".to_owned())], RELEASE_JSON),
            (304, vec![], ""),
            (403, vec![("Retry-After", "300".to_owned())], "{}"),
        ]);
        let mut state = UpdateState::default();

        let outcome = check_latest(&github.url, state.request_etag(), NOW);
        state.apply(outcome, NOW);
        let (path, etag, client) = github.requests.recv().unwrap();
        assert_eq!(path, LATEST_RELEASE_PATH);
        assert_eq!(etag, None);
        assert_eq!(
            client.as_deref(),
            Some(concat!(
                "Nomadeer/",
                env!("CARGO_PKG_VERSION"),
                " application/vnd.github+json"
            ))
        );
        assert_eq!(state.etag, "W/\"first\"");
        assert_eq!(state.offered_update("0.1.0").unwrap().version, "0.2.0");

        let outcome = check_latest(&github.url, state.request_etag(), NOW + DAY_SECS);
        assert_eq!(outcome, CheckOutcome::NotModified);
        state.apply(outcome, NOW + DAY_SECS);
        assert_eq!(
            github.requests.recv().unwrap().1.as_deref(),
            Some("W/\"first\"")
        );
        assert_eq!(state.latest.as_ref().unwrap().version, "0.2.0");

        let outcome = check_latest(&github.url, state.request_etag(), NOW);
        assert_eq!(outcome, CheckOutcome::RateLimited { until: NOW + 300 });

        github.handle.join().unwrap();
        // Nothing listening is a quiet failure.
        assert!(matches!(
            check_latest(&github.url, None, NOW),
            CheckOutcome::Failed { .. }
        ));
    }
}
