use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn clean(value: &str) -> String {
    value.trim().to_owned()
}

pub(crate) fn empty_dash(value: &str) -> &str {
    if value.trim().is_empty() {
        "—"
    } else {
        value
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
