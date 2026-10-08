use crate::views::releases::ReleasesRecord;

const KEY: &str = "task_manager_releases";

/// How the Releases screen was left, or `None` when this browser has never been on it.
///
/// Read ONCE, by `ComponentState::new` — the state is created from it, so the first thing drawn is already
/// the screen the reader left. Nothing draws from here, and nothing asks again.
///
/// A value this build cannot read is no record: the key is this product's own, so the only way that
/// happens is a record written by a build with a different shape, and starting from scratch is the right
/// answer to that.
pub fn get() -> Option<ReleasesRecord> {
    let raw = super::local_storage::get(KEY)?;
    serde_json::from_str(&raw).ok()
}

/// Written by `ComponentState::persist` and by nothing else.
pub fn set(record: &ReleasesRecord) {
    if let Ok(json) = serde_json::to_string(record) {
        super::local_storage::set(KEY, &json);
    }
}
