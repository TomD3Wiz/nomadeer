//! Where Nomadeer keeps the player's data.
//!
//! Installed copies (Program Files, AppImage, Flatpak) cannot write next to the
//! executable or in whatever directory they were started from, so data lives in the
//! platform's per-user location unless an existing or portable data folder says
//! otherwise.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const DATA_DIR_NAME: &str = "data";
pub(crate) const DATABASE_FILE: &str = "nomadeer.sqlite3";
pub(crate) const LEGACY_DATABASE_FILE: &str = "eve_wanderer.sqlite3";
pub(crate) const LEGACY_JSON_FILES: [&str; 2] = ["nomadeer.json", "eve_wanderer.json"];
/// A file with this name next to the executable keeps data beside it.
const PORTABLE_MARKER: &str = "portable";

/// Everything the data folder choice depends on, so it can be decided without
/// touching the real environment.
struct DataDirInputs<'a> {
    cwd: Option<&'a Path>,
    exe_dir: Option<&'a Path>,
    /// `std::env::consts::OS`: "windows", "macos", "linux", ...
    os: &'a str,
    env: &'a dyn Fn(&str) -> Option<OsString>,
    is_file: &'a dyn Fn(&Path) -> bool,
}

/// The folder holding the database, chain archive and Umbra's core. Resolved once
/// and created on first use.
pub(crate) fn data_dir() -> &'static Path {
    static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
    DATA_DIR.get_or_init(|| {
        let cwd = std::env::current_dir().ok();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let directory = resolve_data_dir(&DataDirInputs {
            cwd: cwd.as_deref(),
            exe_dir: exe_dir.as_deref(),
            os: std::env::consts::OS,
            env: &|name| std::env::var_os(name),
            is_file: &|path| path.is_file(),
        });
        // A failure here resurfaces with a clearer error on the first write.
        let _ = fs::create_dir_all(&directory);
        directory
    })
}

pub(crate) fn database_path() -> PathBuf {
    data_dir().join(DATABASE_FILE)
}

pub(crate) fn chain_archive_dir() -> PathBuf {
    data_dir().join("chains")
}

pub(crate) fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Folders older Nomadeer versions may have left data in: the data folder itself and
/// `./data`, where every version before per-user folders kept it.
pub(crate) fn legacy_data_dirs() -> Vec<PathBuf> {
    let cwd_data = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(DATA_DIR_NAME));
    legacy_dirs_for(data_dir(), cwd_data)
}

fn legacy_dirs_for(data_dir: &Path, cwd_data: Option<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = vec![data_dir.to_path_buf()];
    if let Some(cwd_data) = cwd_data
        && cwd_data != data_dir
    {
        dirs.push(cwd_data);
    }
    dirs
}

fn resolve_data_dir(inputs: &DataDirInputs<'_>) -> PathBuf {
    // 1. A checkout or folder that already has Nomadeer data keeps using it. A bare
    //    "data" folder is not enough: a home directory may well have one.
    if let Some(cwd) = inputs.cwd {
        let candidate = cwd.join(DATA_DIR_NAME);
        if holds_nomadeer_data(&candidate, inputs.is_file) {
            return candidate;
        }
    }

    // 2. A portable copy keeps data beside the executable. Once that folder has a
    //    database it stays in use even if the marker goes (see the portable
    //    README in packaging/windows).
    if let Some(exe_dir) = inputs.exe_dir {
        let candidate = exe_dir.join(DATA_DIR_NAME);
        if (inputs.is_file)(&exe_dir.join(PORTABLE_MARKER))
            || (inputs.is_file)(&candidate.join(DATABASE_FILE))
        {
            return candidate;
        }
    }

    // 3. The platform's per-user data folder.
    per_user_data_dir(inputs.os, inputs.env)
        // Without even a home directory, fall back to where older versions wrote.
        .unwrap_or_else(|| {
            inputs.cwd.map_or_else(
                || PathBuf::from(DATA_DIR_NAME),
                |cwd| cwd.join(DATA_DIR_NAME),
            )
        })
}

fn holds_nomadeer_data(directory: &Path, is_file: &dyn Fn(&Path) -> bool) -> bool {
    [DATABASE_FILE, LEGACY_DATABASE_FILE]
        .into_iter()
        .chain(LEGACY_JSON_FILES)
        .any(|name| is_file(&directory.join(name)))
}

fn per_user_data_dir(os: &str, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // Unset, empty and relative values are all ignored, as the XDG spec asks.
    let absolute_var = |name: &str| {
        env(name)
            .map(PathBuf::from)
            .filter(|path| is_absolute_on(os, path))
    };
    match os {
        "windows" => absolute_var("LOCALAPPDATA")
            .or_else(|| absolute_var("APPDATA"))
            .map(|base| base.join("Nomadeer")),
        "macos" => absolute_var("HOME").map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("Nomadeer")
        }),
        _ => absolute_var("XDG_DATA_HOME")
            .or_else(|| absolute_var("HOME").map(|home| home.join(".local").join("share")))
            .map(|base| base.join("nomadeer")),
    }
}

/// `Path::is_absolute` judges by the host's rules; this judges by `os`, so every
/// platform's rules can be checked on any host.
fn is_absolute_on(os: &str, path: &Path) -> bool {
    let text = path.to_string_lossy();
    if os != "windows" {
        return text.starts_with('/');
    }
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    drive || text.starts_with(r"\\")
}

#[cfg(test)]
mod tests {
    use super::{DataDirInputs, legacy_dirs_for, resolve_data_dir};
    use std::collections::{HashMap, HashSet};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    struct Case {
        os: &'static str,
        cwd: Option<&'static str>,
        exe_dir: Option<&'static str>,
        env: HashMap<&'static str, &'static str>,
        files: HashSet<PathBuf>,
    }

    impl Case {
        fn linux() -> Self {
            Self {
                os: "linux",
                cwd: Some("/home/pilot"),
                exe_dir: Some("/usr/bin"),
                env: HashMap::from([("HOME", "/home/pilot")]),
                files: HashSet::new(),
            }
        }

        fn file(mut self, path: &str) -> Self {
            self.files.insert(PathBuf::from(path));
            self
        }

        fn var(mut self, name: &'static str, value: &'static str) -> Self {
            self.env.insert(name, value);
            self
        }

        fn resolve(&self) -> PathBuf {
            let env = |name: &str| self.env.get(name).map(OsString::from);
            let is_file = |path: &Path| self.files.contains(path);
            resolve_data_dir(&DataDirInputs {
                cwd: self.cwd.map(Path::new),
                exe_dir: self.exe_dir.map(Path::new),
                os: self.os,
                env: &env,
                is_file: &is_file,
            })
        }
    }

    #[test]
    fn existing_data_in_the_working_directory_is_kept() {
        for name in [
            "nomadeer.sqlite3",
            "eve_wanderer.sqlite3",
            "nomadeer.json",
            "eve_wanderer.json",
        ] {
            let case = Case::linux().file(&format!("/home/pilot/data/{name}"));
            assert_eq!(case.resolve(), PathBuf::from("/home/pilot/data"), "{name}");
        }
    }

    #[test]
    fn an_unrelated_data_folder_is_not_used() {
        let case = Case::linux().file("/home/pilot/data/notes.txt");
        assert_eq!(
            case.resolve(),
            PathBuf::from("/home/pilot/.local/share/nomadeer")
        );
    }

    #[test]
    fn portable_copies_keep_data_beside_the_executable() {
        let marker = Case::linux().file("/usr/bin/portable");
        assert_eq!(marker.resolve(), PathBuf::from("/usr/bin/data"));

        let existing = Case::linux().file("/usr/bin/data/nomadeer.sqlite3");
        assert_eq!(existing.resolve(), PathBuf::from("/usr/bin/data"));

        // Working directory data still wins over a portable copy.
        let both = Case::linux()
            .file("/usr/bin/portable")
            .file("/home/pilot/data/nomadeer.sqlite3");
        assert_eq!(both.resolve(), PathBuf::from("/home/pilot/data"));
    }

    #[test]
    fn linux_follows_xdg_data_home() {
        assert_eq!(
            Case::linux().resolve(),
            PathBuf::from("/home/pilot/.local/share/nomadeer")
        );

        // Flatpak points XDG_DATA_HOME into the app's sandbox.
        let flatpak = Case::linux().var(
            "XDG_DATA_HOME",
            "/home/pilot/.var/app/io.github.TomD3Wiz.Nomadeer/data",
        );
        assert_eq!(
            flatpak.resolve(),
            PathBuf::from("/home/pilot/.var/app/io.github.TomD3Wiz.Nomadeer/data/nomadeer")
        );

        for ignored in ["", "relative/share"] {
            let case = Case::linux().var("XDG_DATA_HOME", ignored);
            assert_eq!(
                case.resolve(),
                PathBuf::from("/home/pilot/.local/share/nomadeer"),
                "{ignored:?}"
            );
        }
    }

    #[test]
    fn windows_uses_local_app_data_then_app_data() {
        let mut case = Case {
            os: "windows",
            cwd: Some(r"C:\Users\pilot"),
            exe_dir: Some(r"C:\Program Files\Nomadeer"),
            env: HashMap::from([
                ("LOCALAPPDATA", r"C:\Users\pilot\AppData\Local"),
                ("APPDATA", r"C:\Users\pilot\AppData\Roaming"),
            ]),
            files: HashSet::new(),
        };
        assert_eq!(
            case.resolve(),
            Path::new(r"C:\Users\pilot\AppData\Local").join("Nomadeer")
        );

        case.env.remove("LOCALAPPDATA");
        assert_eq!(
            case.resolve(),
            Path::new(r"C:\Users\pilot\AppData\Roaming").join("Nomadeer")
        );

        // A relative value would follow the working directory around.
        case.env.insert("LOCALAPPDATA", r"AppData\Local");
        assert_eq!(
            case.resolve(),
            Path::new(r"C:\Users\pilot\AppData\Roaming").join("Nomadeer")
        );

        // The portable zip keeps data beside nomadeer.exe.
        let portable = Case {
            files: HashSet::from([Path::new(r"C:\Games\Nomadeer").join("portable")]),
            exe_dir: Some(r"C:\Games\Nomadeer"),
            ..case
        };
        assert_eq!(
            portable.resolve(),
            Path::new(r"C:\Games\Nomadeer").join("data")
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let case = Case {
            os: "macos",
            ..Case::linux()
        };
        assert_eq!(
            case.resolve(),
            PathBuf::from("/home/pilot/Library/Application Support/Nomadeer")
        );
    }

    #[test]
    fn without_a_home_data_stays_in_the_working_directory() {
        let mut case = Case::linux();
        case.env.clear();
        assert_eq!(case.resolve(), PathBuf::from("/home/pilot/data"));

        case.cwd = None;
        assert_eq!(case.resolve(), PathBuf::from("data"));
    }

    #[test]
    fn legacy_dirs_include_the_working_directory_once() {
        let data = Path::new("/home/pilot/.local/share/nomadeer");
        assert_eq!(
            legacy_dirs_for(data, Some(PathBuf::from("/home/pilot/data"))),
            vec![data.to_path_buf(), PathBuf::from("/home/pilot/data")]
        );
        assert_eq!(
            legacy_dirs_for(data, Some(data.to_path_buf())),
            vec![data.to_path_buf()]
        );
        assert_eq!(legacy_dirs_for(data, None), vec![data.to_path_buf()]);
    }
}
