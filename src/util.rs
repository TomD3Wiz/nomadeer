use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn clean(value: &str) -> String {
    value.trim().to_owned()
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(crate) fn open_url(url: &str) -> Result<(), String> {
    if webbrowser::open(url).is_ok() {
        return Ok(());
    }

    let candidates: &[(&str, &[&str])] = &[
        ("xdg-open", &[url]),
        ("gio", &["open", url]),
        ("flatpak-spawn", &["--host", "xdg-open", url]),
        ("distrobox-host-exec", &["xdg-open", url]),
    ];

    for (program, args) in candidates {
        if Command::new(program).args(*args).spawn().is_ok() {
            return Ok(());
        }
    }

    Err(format!(
        "Could not open browser. Open this URL manually: {url}"
    ))
}
