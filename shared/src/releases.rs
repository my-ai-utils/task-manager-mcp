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
// `release_link` is where the build of that version can be looked at — the GitHub release, or the run of the
// workflow that built the image. A link and nothing else: nothing here talks to GitHub. Empty when nobody
// recorded one, which is the ordinary state of a service that was built and rolled out by hand.
//
// `datetime_unix_seconds` is when this service went out, as whoever recorded the release said — unlike
// every other moment on this wire, it is not stamped by the server.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ServiceReleaseResponse {
    pub microservice_id: String,
    pub version: String,
    pub git_hash: String,
    #[serde(default)]
    pub release_link: String,
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
    // The environments this release is out on, as labels — `Dev`, `Prod` — in the order it reached them.
    // A release is written down when it ships somewhere, usually a test stand first, and reaching the next
    // environment is another label on the SAME release rather than a second release. Free text and not a
    // fixed list: which environments there are is each project's own business. A label is compared
    // ignoring case — ask through `is_on_env` rather than with `contains`.
    #[serde(default)]
    pub envs: Vec<String>,
    // When the release was CLOSED, and absent while it is still going out. Closing says the rollout is
    // over: it has reached every environment it is going to. That is somebody's statement and not
    // something worked out from `envs` — nothing here knows the list of environments a project has, so
    // "on all of them" cannot be computed, only said. One field for "is it done" and "since when", so the
    // two cannot disagree: read the first with `is_done`.
    #[serde(default)]
    pub done_unix_seconds: Option<i64>,
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

// One release, named the way its page's address names it: `release/{project}/{release}`.
#[derive(MyHttpInput)]
pub struct GetReleaseInputModel {
    #[http_body(name: "project", description: "Which project the release belongs to, by prefix — RMS")]
    pub project: String,
    #[http_body(name: "release", description: "Which release, by its id — RMS-R12 — or by its bare number")]
    pub release: String,
}

/// The address of a release's own page, as a path on this origin: `/release/RMS/RMS-R12`.
///
/// **The one place the shape of that address is written down** for whoever BUILDS a link — the button
/// that opens a release in a new tab, and anything that copies one. The router states the same shape to
/// read it, and a test holds the two together. The project comes first and on its own, although the id
/// repeats it: the address is `release/{project}/{release}`, so a link can be read without knowing how an
/// id is put together, and it is the project that access is decided on.
///
/// Nothing is encoded because nothing in either half can need it — a prefix and a handle are ASCII
/// alphanumerics, an underscore and a dash.
pub fn release_page_path(release: &ReleaseResponse) -> String {
    format!("/release/{}/{}", release.project, release.id)
}

/// Whether two labels name the same environment.
///
/// **The one definition of an environment's identity**, shared by the server that stores the labels and
/// the screen that filters by them: surrounding whitespace and letter case do not count, so `prod`, `Prod`
/// and ` PROD ` are one environment. It has to be one function — a list that de-duplicated by one rule and
/// a filter that matched by another would show a release under a label it could not be found by.
pub fn same_env(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

/// Whether a release is out on an environment.
pub fn is_on_env(release: &ReleaseResponse, env: &str) -> bool {
    release.envs.iter().any(|itm| same_env(itm, env))
}

/// Whether a release has been closed — its rollout is over.
///
/// A function over the one field rather than a second field beside it: a bool on the wire next to the
/// moment would be two statements of one fact, and the first build to set one and forget the other would
/// draw a release that is and is not done.
pub fn is_done(release: &ReleaseResponse) -> bool {
    release.done_unix_seconds.is_some()
}

/// Whether a label names production.
///
/// For the one thing a screen does differently for it: an environment's label is drawn as a quiet chip,
/// and production's is drawn in the colour of something finished, because "is it live" is the question a
/// list of releases is most often scanned for. Two spellings and no more — a label is free text, and
/// guessing that `live` or `main` means production would colour a row on a guess.
pub fn is_production_env(label: &str) -> bool {
    same_env(label, "prod") || same_env(label, "production")
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
            release_link: String::new(),
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
            envs: Vec::new(),
            done_unix_seconds: None,
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            deleted_unix_seconds: None,
        }
    }

    /// A release recorded by a build that did not know about environments or about threads has neither
    /// field on the wire, and has to read as what it was: out nowhere in particular, with nothing said
    /// about it. That includes what 0.2.0 sent instead — an unknown field is passed over, not an error.
    #[test]
    fn a_release_without_the_newer_fields_reads_as_out_nowhere_and_unremarked() {
        let raw = r#"{"id":"RMS-R1","project":"RMS","title":"t","description":"","release_notes":"",
            "date_unix_seconds":0,"released_on_prod_unix_seconds":1791331200,
            "created_unix_seconds":0,"updated_unix_seconds":0,"deleted_unix_seconds":null}"#;

        let read: ReleaseResponse = serde_json::from_str(raw).unwrap();

        assert!(read.envs.is_empty());
        assert!(!is_on_env(&read, "Prod"));
        assert!(!is_done(&read), "a release nobody closed is still going out");
        assert!(read.comments.is_empty());

        let mut closed = release(Vec::new());
        closed.done_unix_seconds = Some(MIDNIGHT);

        assert!(is_done(&closed));
    }

    /// A label is found however it is spelled, and only when it is the WHOLE label: `Prod` must not
    /// answer for `Pre-Prod`, which is exactly the release that has not got there yet.
    #[test]
    fn a_release_is_on_an_environment_whatever_the_case_of_the_label() {
        let mut out = release(Vec::new());
        out.envs = vec!["Dev".to_string(), "Pre-Prod".to_string()];

        assert!(is_on_env(&out, "Dev"));
        assert!(is_on_env(&out, "dev"));
        assert!(is_on_env(&out, " DEV "));
        assert!(is_on_env(&out, "pre-prod"));

        assert!(!is_on_env(&out, "Prod"));
        assert!(!is_on_env(&out, ""));
        assert!(!is_on_env(&release(Vec::new()), "Dev"));
    }

    #[test]
    fn production_is_recognised_by_its_two_spellings_and_nothing_else() {
        for label in ["Prod", "prod", "PROD", " Production "] {
            assert!(is_production_env(label), "{label:?}");
        }

        for label in ["Dev", "Pre-Prod", "prod-eu", "live", ""] {
            assert!(!is_production_env(label), "{label:?}");
        }
    }

    #[test]
    fn a_release_has_one_address() {
        assert_eq!(release_page_path(&release(Vec::new())), "/release/RMS/RMS-R1");
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

    /// A release written before a field existed still has to read: the two notes and the link are the
    /// fields most likely to be absent from an older payload.
    #[test]
    fn a_service_without_its_notes_or_its_link_still_reads() {
        let raw = r#"{"microservice_id":"a","version":"1","git_hash":"abc1234","datetime_unix_seconds":5}"#;

        let read: ServiceReleaseResponse = serde_json::from_str(raw).unwrap();

        assert_eq!(read.settings_update_note, "");
        assert_eq!(read.description, "");
        assert_eq!(read.release_link, "");
    }
}
