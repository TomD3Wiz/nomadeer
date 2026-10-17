use std::io::ErrorKind;
use std::path::Path;
#[cfg(not(any(windows, target_os = "macos")))]
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) fn clean(value: &str) -> String {
    value.trim().to_owned()
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(crate) const HOUR_SECS: u64 = 60 * 60;
pub(crate) const DAY_SECS: u64 = 24 * HOUR_SECS;

/// Start of the UTC hour containing `timestamp`.
pub(crate) fn hour_start(timestamp: u64) -> u64 {
    timestamp / HOUR_SECS * HOUR_SECS
}

/// Parses an HTTP date such as `Sun, 06 Nov 1994 08:49:37 GMT` into Unix seconds.
pub(crate) fn parse_http_date(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let _weekday = parts.next()?;
    let day = parts.next()?.parse::<u64>().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse::<u64>().ok()?;
    let mut time = parts.next()?.split(':');
    let hour = time.next()?.parse::<u64>().ok()?;
    let minute = time.next()?.parse::<u64>().ok()?;
    let second = time.next()?.parse::<u64>().ok()?;
    if parts.next()? != "GMT"
        || time.next().is_some()
        || year < 1970
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    // Days since the Unix epoch for a proleptic Gregorian date (Hinnant's algorithm).
    let (year, month) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = year / 400;
    let year_of_era = year % 400;
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = (era * 146_097 + day_of_era).checked_sub(719_468)?;
    Some(days * DAY_SECS + hour * HOUR_SECS + minute * 60 + second)
}

/// Moves `from` into place at `to`, replacing any file already there. On Windows a
/// virus scanner or the search indexer can hold a just-written file open for a
/// moment, so a refused move is retried briefly before giving up.
pub(crate) fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let attempts = if cfg!(windows) { 20 } else { 1 };
    let mut attempt = 1;
    loop {
        match std::fs::rename(from, to) {
            Err(error) if attempt < attempts && error.kind() == ErrorKind::PermissionDenied => {
                std::thread::sleep(Duration::from_millis(100));
                attempt += 1;
            }
            result => return result,
        }
    }
}

pub(crate) fn open_url(url: &str) -> Result<(), String> {
    if webbrowser::open(url).is_ok() {
        return Ok(());
    }

    // Desktop helpers for Linux setups the webbrowser crate misses, such as
    // sandboxes. Windows and macOS have no such fallbacks.
    #[cfg(not(any(windows, target_os = "macos")))]
    {
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
    }

    Err(format!(
        "Could not open browser. Open this URL manually: {url}"
    ))
}

#[cfg(test)]
mod tests {
    use super::{hour_start, parse_http_date, replace_file};

    #[test]
    fn replace_file_overwrites_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("nomadeer-util-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (part, installed) = (dir.join("core.part"), dir.join("core.gguf"));
        std::fs::write(&installed, b"old core").unwrap();
        std::fs::write(&part, b"new core").unwrap();

        replace_file(&part, &installed).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), b"new core");
        assert!(!part.exists());
        assert!(replace_file(&part, &installed).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parses_http_dates() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777)
        );
        assert_eq!(
            parse_http_date("Thu, 29 Feb 2024 23:59:59 GMT"),
            Some(1_709_251_199)
        );
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 PST"), None);
        assert_eq!(parse_http_date("Sun, 06 Foo 1994 08:49:37 GMT"), None);
        assert_eq!(parse_http_date("not a date"), None);
    }

    #[test]
    fn hour_start_floors_to_the_hour() {
        assert_eq!(hour_start(784_111_777), 784_108_800);
        assert_eq!(hour_start(784_108_800), 784_108_800);
    }
}
