use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

use crate::tasks::TaskCommentResponse;

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

// One microservice inside a release: which version of it went out, and built from which commit.
//
// `microservice_id` is the identity within its release — a release names a service once.
//
// `settings_update_note` is its own field rather than a line in `description`, and that is the reason it
// exists: whether a service's settings have to change is the one fact about a release somebody ACTS on
// while rolling it out, and a fact that has to be found in prose gets missed. Empty means the settings do
// not change, which is why the screen can mark the services where they do. `description` is everything
// else worth saying about this service's part of the release.
//
// `datetime_unix_seconds` is when this service went out, as whoever recorded the release said — unlike
// every other moment on this wire, it is not stamped by the server.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ServiceReleaseResponse {
    pub microservice_id: String,
    pub version: String,
    pub git_hash: String,
    pub datetime_unix_seconds: i64,
    #[serde(default)]
    pub settings_update_note: String,
    #[serde(default)]
    pub description: String,
}

// A goal a release is attached to, with just enough to draw it.
//
// The name and the colour ride along because the Releases screen lists releases, not goals, and a bare
// `RMS-G7` there says nothing about WHAT went out — the goal is the description of the feature. Resolved
// server-side rather than looked up in the snapshot: a goal closed longer ago than the archive window is
// not in it, and a release is exactly the thing that outlives its goal.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ReleaseGoalResponse {
    pub id: String,
    pub name: String,
    // A palette name — see `GoalResponse::color`.
    pub color: String,
}

// A release, as the Releases screen and a goal's dialog draw it.
//
// The record that a feature went out, and in what: one entry per microservice it touched. `id` is its
// handle, `RMS-R12` — the number comes from the same per-project counter task and goal numbers come from,
// so no number names two things, and the `R` says which kind you are holding.
//
// `goals` is derived, not stored here: the link lives on the GOAL, which lists the releases it went out
// in. Normally that is one goal — the feature this release shipped — but nothing stops a release being
// listed by two, and one listed by none is still a release of its project.
//
// `date_unix_seconds` is the date of the release as its author gave it, and what every list is ordered
// by. `created_unix_seconds` is when the record was written, which may be days later.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ReleaseResponse {
    pub id: String,
    // The board's PREFIX, like everywhere else on this wire — see `crate::projects::ProjectResponse`.
    pub project: String,
    pub title: String,
    pub description: String,
    pub release_notes: String,
    pub date_unix_seconds: i64,
    #[serde(default)]
    pub services: Vec<ServiceReleaseResponse>,
    #[serde(default)]
    pub goals: Vec<ReleaseGoalResponse>,
    // When this release was marked as out on PRODUCTION, and absent while it is not. A release is written
    // down when it ships somewhere — usually a test stand first — and reaching production is a later mark
    // on the same release. One field for both "is it on prod" and "since when", so the two cannot
    // disagree: read the first with `is_released_on_prod`.
    #[serde(default)]
    pub released_on_prod_unix_seconds: Option<i64>,
    // The release's thread, oldest first — what was said about the rollout, as opposed to `release_notes`,
    // which say what changed. The same shape a task's and a goal's thread travel in.
    #[serde(default)]
    pub comments: Vec<TaskCommentResponse>,
    pub created_unix_seconds: i64,
    pub updated_unix_seconds: i64,
    // When it was deleted, and absent for a release that is not — see `TaskResponse::deleted_unix_seconds`.
    pub deleted_unix_seconds: Option<i64>,
}

#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ReleasesResponse {
    pub releases: Vec<ReleaseResponse>,
}

#[derive(MyHttpInput)]
pub struct GetReleasesInputModel {
    #[http_body(name: "project", description: "Which project's releases to read, by prefix — RMS")]
    pub project: String,
}

/// Whether a release has gone out to production.
///
/// A function over the one field rather than a second field beside it: a bool on the wire next to the
/// moment would be two statements of one fact, and the first build to set one and forget the other would
/// draw a release that is and is not on prod.
pub fn is_released_on_prod(release: &ReleaseResponse) -> bool {
    release.released_on_prod_unix_seconds.is_some()
}

/// Whether any service in a release changes its settings.
///
/// One function for the two screens that mark it, so "this release needs a settings change" cannot mean
/// one thing on the Releases screen and another in a goal's dialog.
pub fn has_settings_update(release: &ReleaseResponse) -> bool {
    release
        .services
        .iter()
        .any(|itm| !itm.settings_update_note.trim().is_empty())
}

/// A moment somebody GAVE, as a person reads it: `2026-10-07`, or `2026-10-07 14:30 UTC`.
///
/// For the two moments of a release that are statements rather than stamps — its date, and when each
/// service went out. Two decisions, both about not lying:
///
/// * **UTC, and it says so.** A release recorded as `2026-10-07` is stored as that midnight in UTC; drawn
///   in the browser's own zone it would read as the 6th to anybody west of Greenwich, which is a different
///   date for the same release depending on who is looking. The zone is spelled out beside a time because
///   a bare `14:30` invites the reader to assume it is theirs.
/// * **A bare date stays a bare date.** Midnight exactly is what a date with no time is stored as, so it
///   is drawn without one rather than as `00:00` — a precision nobody claimed.
///
/// ISO order rather than a locale's, so a column of them sorts by eye and `03-04` is never ambiguous.
pub fn moment_for_display(unix_seconds: i64) -> String {
    let stamp = rust_extensions::date_time::DateTimeAsMicroseconds::new(unix_seconds * 1_000_000)
        .to_rfc3339_utc();

    // `2026-10-07T14:30:00.000000Z`: the date is the first ten characters and the clock the five after
    // the `T`. Read by position because the width is fixed — that is what `to_rfc3339_utc` is for.
    let (Some(date), Some(clock)) = (stamp.get(0..10), stamp.get(11..16)) else {
        return stamp;
    };

    if unix_seconds.rem_euclid(24 * 60 * 60) == 0 {
        return date.to_string();
    }

    format!("{date} {clock} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-07T00:00:00Z.
    const MIDNIGHT: i64 = 1_791_331_200;

    #[test]
    fn a_bare_date_is_drawn_as_a_date_and_a_time_says_its_zone() {
        assert_eq!(moment_for_display(MIDNIGHT), "2026-10-07");
        assert_eq!(
            moment_for_display(MIDNIGHT + 14 * 3600 + 30 * 60),
            "2026-10-07 14:30 UTC"
        );
        // Seconds are not drawn, but they are enough to make it a time rather than a date.
        assert_eq!(moment_for_display(MIDNIGHT + 5), "2026-10-07 00:00 UTC");
        assert_eq!(moment_for_display(0), "1970-01-01");
    }

    fn service(settings_update_note: &str) -> ServiceReleaseResponse {
        ServiceReleaseResponse {
            microservice_id: "my-service".to_string(),
            version: "1.2.3".to_string(),
            git_hash: "0e299dc".to_string(),
            datetime_unix_seconds: 0,
            settings_update_note: settings_update_note.to_string(),
            description: String::new(),
        }
    }

    fn release(services: Vec<ServiceReleaseResponse>) -> ReleaseResponse {
        ReleaseResponse {
            id: "RMS-R1".to_string(),
            project: "RMS".to_string(),
            title: "A release".to_string(),
            description: String::new(),
            release_notes: String::new(),
            date_unix_seconds: 0,
            services,
            goals: Vec::new(),
            released_on_prod_unix_seconds: None,
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            deleted_unix_seconds: None,
        }
    }

    /// A release recorded by a build that did not know about production or about threads has neither
    /// field on the wire, and has to read as what it was: not on prod, with nothing said about it.
    #[test]
    fn a_release_without_the_newer_fields_reads_as_not_on_prod_and_unremarked() {
        let raw = r#"{"id":"RMS-R1","project":"RMS","title":"t","description":"","release_notes":"",
            "date_unix_seconds":0,"created_unix_seconds":0,"updated_unix_seconds":0,
            "deleted_unix_seconds":null}"#;

        let read: ReleaseResponse = serde_json::from_str(raw).unwrap();

        assert!(!is_released_on_prod(&read));
        assert!(read.comments.is_empty());

        let mut on_prod = release(Vec::new());
        on_prod.released_on_prod_unix_seconds = Some(MIDNIGHT);

        assert!(is_released_on_prod(&on_prod));
    }

    /// Whitespace is not a note: a release whose services all say nothing must not be flagged, or the flag
    /// stops meaning "somebody has to change a setting".
    #[test]
    fn a_settings_update_is_a_note_with_something_in_it() {
        assert!(!has_settings_update(&release(Vec::new())));
        assert!(!has_settings_update(&release(vec![service(""), service("   ")])));
        assert!(has_settings_update(&release(vec![
            service(""),
            service("add `releases_ttl` to settings")
        ])));
    }

    /// A release written before a field existed still has to read: the two notes are the fields most
    /// likely to be absent from an older payload.
    #[test]
    fn a_service_without_its_notes_still_reads() {
        let raw = r#"{"microservice_id":"a","version":"1","git_hash":"abc1234","datetime_unix_seconds":5}"#;

        let read: ServiceReleaseResponse = serde_json::from_str(raw).unwrap();

        assert_eq!(read.settings_update_note, "");
        assert_eq!(read.description, "");
    }
}
