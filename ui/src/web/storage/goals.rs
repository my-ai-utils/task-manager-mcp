use crate::views::goals::GoalsRecord;

const KEY: &str = "task_manager_goals";

/// How the Goals screen was left, or `None` when this browser has never been on it.
///
/// Read ONCE, by `ComponentState::new` — see the same function for Releases. A value this build cannot
/// read is no record, and starting from scratch is the answer to it.
pub fn get() -> Option<GoalsRecord> {
    let raw = super::local_storage::get(KEY)?;
    serde_json::from_str(&raw).ok()
}

/// Written by `ComponentState::persist` and by nothing else.
pub fn set(record: &GoalsRecord) {
    if let Ok(json) = serde_json::to_string(record) {
        super::local_storage::set(KEY, &json);
    }
}
