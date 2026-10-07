use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;

use crate::app::AppContext;
use crate::board::{
    BoardInner, ProjectModel, ReleaseModel, ServiceReleaseModel, compose_release_handle,
    parse_release_handle,
};
use crate::postgres::{GoalDto, ReleaseDto};

use super::{resolve_project_by_prefix, resolve_release_by_handle};

/// The longest a release's title may be. A cap rather than a truncation, so a caller is told rather than
/// quietly shortened: a title is one line naming what went out, and anything longer is the description.
pub const MAX_RELEASE_TITLE_LEN: usize = 200;

/// The longest a microservice id and a version may be. Both are identifiers, not prose — generous enough
/// for `some-long-service-name` and `1.2.3-rc.1+build.45`, short enough that a sentence pasted into the
/// wrong field is refused instead of stored.
pub const MAX_MICROSERVICE_ID_LEN: usize = 120;
pub const MAX_VERSION_LEN: usize = 60;

/// One microservice of a release, as a caller hands it over.
///
/// `microservice_id`, `version` and `git_hash` are what the entry IS and are always given. The other three
/// are optional, and on a service the release already names an omitted one **keeps what is there** — see
/// [`ServicesPatch::apply`].
pub struct NewServiceRelease {
    pub microservice_id: String,
    pub version: String,
    pub git_hash: String,
    /// When this service went out, as the caller writes it — see `parse_caller_moment`. `None` is now.
    pub datetime: Option<String>,
    pub settings_update_note: Option<String>,
    pub description: Option<String>,
}

/// What a caller wants to change about the services of a release.
///
/// Add and remove rather than "here is the new list", the shape every list on this board is edited in and
/// for the same reason: a release collects its services as they go out, and a whole-list write would drop
/// the ones the caller had not read.
#[derive(Default)]
pub struct ServicesPatch {
    pub add: Vec<NewServiceRelease>,
    /// Microservice ids to take off the release.
    pub remove: Vec<String>,
}

impl ServicesPatch {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }

    /// Apply the patch to one release's services, or refuse it entirely.
    ///
    /// **Adding a service the release already names is a correction, not a duplicate.** A release names a
    /// microservice once, so the entry is found by its id and rewritten in place: the version and the
    /// commit are replaced, since stating them is what the call is for, and each of the three optional
    /// fields is replaced only when it was passed. That last part matters — correcting a mistyped version
    /// must not wipe a settings note somebody wrote, and an empty string is how a note is cleared on
    /// purpose.
    ///
    /// Removal after addition, so an id passed to both ends up removed — the order every list here uses.
    /// An id the release does not name is NOT an error: the caller's intent is already true.
    ///
    /// `now` is passed in rather than read here so one update stamps everything with the same moment, and
    /// so the default for a service's `datetime` is testable.
    pub fn apply(
        &self,
        services: &mut Vec<ServiceReleaseModel>,
        now: DateTimeAsMicroseconds,
    ) -> Result<(), String> {
        for new_service in &self.add {
            let microservice_id = validate_microservice_id(&new_service.microservice_id)?;
            let version = validate_version(&new_service.version, &microservice_id)?;
            let git_hash = validate_git_hash(&new_service.git_hash, &microservice_id)?;

            let datetime = match new_service.datetime.as_deref() {
                Some(datetime) => Some(super::parse_caller_moment(
                    datetime,
                    &format!("the datetime of {microservice_id}"),
                )?),
                None => None,
            };

            let settings_update_note = new_service
                .settings_update_note
                .as_deref()
                .map(|itm| itm.trim().to_string());

            let description = new_service
                .description
                .as_deref()
                .map(|itm| itm.trim().to_string());

            match services
                .iter_mut()
                .find(|itm| itm.microservice_id == microservice_id)
            {
                Some(existing) => {
                    existing.version = version;
                    existing.git_hash = git_hash;

                    if let Some(datetime) = datetime {
                        existing.datetime = datetime;
                    }

                    if let Some(settings_update_note) = settings_update_note {
                        existing.settings_update_note = settings_update_note;
                    }

                    if let Some(description) = description {
                        existing.description = description;
                    }
                }
                None => services.push(ServiceReleaseModel {
                    microservice_id,
                    version,
                    git_hash,
                    datetime: datetime.unwrap_or_else(|| to_the_second(now)),
                    settings_update_note: settings_update_note.unwrap_or_default(),
                    description: description.unwrap_or_default(),
                }),
            }
        }

        for microservice_id in &self.remove {
            let microservice_id = microservice_id.trim();
            services.retain(|itm| itm.microservice_id != microservice_id);
        }

        Ok(())
    }
}

/// A moment cut to the second — the precision a service's `datetime` is stored at.
///
/// The services ride on the release row as `jsonb` and their moments are written there as unix seconds, so
/// a stamp kept to the microsecond in memory would be one value until the next restart and another after
/// it. Nothing would show the difference on a screen; an export taken before the restart and one taken
/// after would, by disagreeing about a release nobody had touched. A moment a caller WROTE needs none of
/// this — `parse_caller_moment` already drops the fraction.
fn to_the_second(moment: DateTimeAsMicroseconds) -> DateTimeAsMicroseconds {
    DateTimeAsMicroseconds::new(moment.unix_microseconds.div_euclid(1_000_000) * 1_000_000)
}

/// The id to store, or a refusal.
///
/// One word, as the service is called where it is deployed. Whitespace inside is refused rather than
/// tolerated because this string is an identity: it is what a later correction and a removal find the
/// entry by, and `my service` typed once and `my-service` typed the next time would be two entries for
/// one deployment.
fn validate_microservice_id(src: &str) -> Result<String, String> {
    let id = src.trim();

    if id.is_empty() {
        return Err(
            "a service in a release needs a `microservice_id` — the name the service is deployed under"
                .to_string(),
        );
    }

    if id.chars().any(char::is_whitespace) {
        return Err(format!(
            "'{id}' is not a microservice id — it is one word, the name the service is deployed under, like my-service"
        ));
    }

    let length = id.chars().count();

    if length > MAX_MICROSERVICE_ID_LEN {
        return Err(format!(
            "that microservice id is {length} characters — the limit is {MAX_MICROSERVICE_ID_LEN}"
        ));
    }

    Ok(id.to_string())
}

fn validate_version(src: &str, microservice_id: &str) -> Result<String, String> {
    let version = src.trim();

    if version.is_empty() {
        return Err(format!(
            "{microservice_id} needs a `version` — which version of it went out, like 1.2.3"
        ));
    }

    if version.chars().any(char::is_whitespace) {
        return Err(format!(
            "'{version}' is not a version of {microservice_id} — a version is one word, like 1.2.3. What changed belongs in `description`"
        ));
    }

    let length = version.chars().count();

    if length > MAX_VERSION_LEN {
        return Err(format!(
            "the version of {microservice_id} is {length} characters — the limit is {MAX_VERSION_LEN}"
        ));
    }

    Ok(version.to_string())
}

/// The commit to store, lower-cased, or a refusal.
///
/// **Held to being a hash**, where a version is not held to anything, because this is the half of the
/// entry that cannot be moved afterwards: a tag can be re-pointed and a branch moves by definition, so
/// `main` or `v1.2.3` stored here would be a record that reads as precise and names nothing in
/// particular. Seven hex characters is the shortest git abbreviates to; 40 is sha1 and 64 is sha256.
fn validate_git_hash(src: &str, microservice_id: &str) -> Result<String, String> {
    let hash = src.trim().to_lowercase();

    if hash.is_empty() {
        return Err(format!(
            "{microservice_id} needs a `git_hash` — the commit that version was built from, as `git rev-parse HEAD` prints it"
        ));
    }

    let is_hash = (7..=64).contains(&hash.len()) && hash.chars().all(|itm| itm.is_ascii_hexdigit());

    if !is_hash {
        return Err(format!(
            "'{hash}' is not a git hash — {microservice_id} needs the commit its version was built from, 7 to 64 hex characters as `git rev-parse HEAD` prints it. A branch or a tag is not one: both can move"
        ));
    }

    Ok(hash)
}

fn validate_title(src: &str) -> Result<String, String> {
    let title = src.trim();

    if title.is_empty() {
        return Err("a release needs a title — one line naming what went out".to_string());
    }

    let length = title.chars().count();

    if length > MAX_RELEASE_TITLE_LEN {
        return Err(format!(
            "that title is {length} characters — the limit is {MAX_RELEASE_TITLE_LEN}. A title is one line; the rest belongs in `description` or `release_notes`"
        ));
    }

    Ok(title.to_string())
}

/// Everything needed to record a release.
pub struct NewRelease {
    pub project_prefix: String,
    pub title: String,
    pub description: String,
    pub release_notes: String,
    /// The date of the release, as the caller writes it — see `parse_caller_moment`. `None` is now, which
    /// is right for a release recorded as it goes out and wrong for one written down the day after.
    pub date: Option<String>,
    pub services: Vec<NewServiceRelease>,
    /// The goal this release ships, by handle or bare number — attached in the same call, so recording a
    /// release and saying what it was for cannot be done as two steps of which the second is forgotten.
    pub goal: Option<String>,
}

/// A change to a release. Every field is optional; `None` means "leave it alone".
#[derive(Default)]
pub struct ReleasePatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub release_notes: Option<String>,
    pub date: Option<String>,
    pub services: ServicesPatch,
    /// `Some(false)` brings a deleted release back. `Some(true)` deletes it, which `delete_release` also
    /// does — both are here for the reason they are both on a goal's patch.
    pub deleted: Option<bool>,
}

impl ReleasePatch {
    /// Whether this patch would do nothing at all. Refused rather than performed — a write that changes
    /// nothing still moves `updated` and still pushes a snapshot to every open screen.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.release_notes.is_none()
            && self.date.is_none()
            && self.services.is_empty()
            && self.deleted.is_none()
    }
}

/// Which release a caller named, as a number within one project — WITHOUT checking that it exists.
///
/// Accepts a handle (`RMS-R12`) or a bare number, the leniency a goal reference gets: a number is
/// unambiguous within a project, since one counter serves everything on it.
///
/// Its own function because detaching needs the reading and must not get the existence check: a release
/// that has since been deleted is exactly the one somebody is trying to take off a goal.
fn read_release_reference(project: &ProjectModel, reference: &str) -> Result<i64, String> {
    let reference = reference.trim();

    match reference.parse::<i64>() {
        Ok(number) if number > 0 => Ok(number),
        Ok(_) => Err(format!("'{reference}' is not a release number")),
        Err(_) => {
            let parsed = parse_release_handle(reference).ok_or_else(|| {
                format!(
                    "'{reference}' is not a release id — expected something like {}-R1, or a bare number",
                    project.prefix
                )
            })?;

            if parsed.prefix != project.prefix {
                return Err(format!(
                    "'{reference}' belongs to another project — this call is about {}, and a release is attached only to goals of its own project",
                    project.prefix
                ));
            }

            Ok(parsed.number)
        }
    }
}

/// What a caller wants to change about the releases a goal lists.
///
/// The same add-and-remove shape as a goal's document references, and like them an added reference has to
/// point at something: a release that does not exist, or one that has been deleted, is refused.
#[derive(Default)]
pub struct ReleasesPatch {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

impl ReleasesPatch {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }

    /// Apply the patch to one goal's list, or refuse it entirely.
    ///
    /// Adding a release the goal already lists changes nothing and is not an error. **Adding one another
    /// goal already lists is not refused either**: nothing here keeps a release to a single goal. It is
    /// rarely what anybody means — a release is one feature going out and the goal is that feature — but
    /// a rule that guessed which of two goals a release "really" belongs to would be wrong exactly when
    /// it mattered.
    ///
    /// Removal after addition. Removing one the goal does not list is not an error, and neither is
    /// removing one that has since been deleted — that is the usual reason to be removing it.
    pub fn apply(
        &self,
        board: &BoardInner,
        project: &ProjectModel,
        releases: &mut Vec<i64>,
    ) -> Result<(), String> {
        for reference in &self.add {
            let number = read_release_reference(project, reference)?;
            let handle = compose_release_handle(&project.prefix, number);

            match board.get_release_including_deleted(&project.id, number) {
                None => return Err(format!("no release {handle} on {}", project.prefix)),
                Some(release) if release.is_deleted() => {
                    return Err(format!(
                        "{handle} has been deleted, so it cannot be attached — bring it back with releases_update and `deleted: false` first"
                    ));
                }
                Some(_) => {}
            }

            if !releases.contains(&number) {
                releases.push(number);
            }
        }

        for reference in &self.remove {
            let number = read_release_reference(project, reference)?;
            releases.retain(|itm| *itm != number);
        }

        Ok(())
    }
}

/// Record a release on a project. Returns its handle, `RMS-R12`.
///
/// The number comes from the project's ONE counter, the same one task and goal numbers come from. Every
/// check runs before the number is reserved, so a refused call does not burn one.
///
/// **Two rows are written when a goal is named**, the release and then the goal that now lists it, with no
/// transaction around them. The order is the safe one: a crash between the two leaves a release nobody has
/// attached yet, which is a legitimate state and visible on the Releases screen — the other order would
/// leave a goal pointing at a number that names nothing.
pub async fn create_release(app: &AppContext, new_release: NewRelease) -> Result<String, String> {
    let board = app.board.read();
    let project = resolve_project_by_prefix(&board, &new_release.project_prefix)?;

    let title = validate_title(&new_release.title)?;

    let now = DateTimeAsMicroseconds::now();

    let date = match new_release.date.as_deref() {
        Some(date) => super::parse_caller_moment(date, "the release date")?,
        None => now,
    };

    let mut services = Vec::new();

    ServicesPatch {
        add: new_release.services,
        remove: Vec::new(),
    }
    .apply(&mut services, now)?;

    // Open or closed, either is fine: a feature is very often released as its goal is being closed, in
    // whichever order the two calls happen to be made. Only a goal that is not there is refused.
    let goal_number = match new_release
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|itm| !itm.is_empty())
    {
        Some(goal) => Some(super::resolve_goal_reference(&board, &project, goal)?),
        None => None,
    };

    let number = app.board.reserve_task_number(&project.id).ok_or_else(|| {
        format!(
            "project {} vanished while recording the release",
            project.prefix
        )
    })?;

    let release = ReleaseModel {
        project_id: project.id.clone(),
        number,
        title,
        description: new_release.description.trim().to_string(),
        release_notes: new_release.release_notes.trim().to_string(),
        date,
        services,
        created: now,
        updated: now,
        deleted_moment: None,
    };

    let ctx = MyTelemetryContext::create_empty();
    let dto: ReleaseDto = (&release).into();
    app.releases_repo.upsert(&dto, &ctx).await;

    // The counter moved in memory when the number was reserved; persist it so a restart does not hand the
    // same number out again — to a task, a goal or another release.
    super::persist_project_counter(app, &project.id, &ctx).await;

    app.board.upsert_release(release);

    if let Some(goal_number) = goal_number {
        attach_to_goal(app, &project.id, goal_number, number, &ctx).await;
    }

    app.notify_project_changed(&project.id).await;

    Ok(compose_release_handle(&project.prefix, number))
}

/// Put a freshly recorded release on the goal it ships.
///
/// The goal is read again rather than taken from the snapshot `create_release` validated against: awaits
/// have happened since, and writing back a goal captured before them would undo whatever landed on it in
/// between. A goal that has disappeared in that window is left alone — the release is recorded either way.
async fn attach_to_goal(
    app: &AppContext,
    project_id: &str,
    goal_number: i64,
    release_number: i64,
    ctx: &MyTelemetryContext,
) {
    let Some(goal) = app.board.read().get_goal(project_id, goal_number) else {
        return;
    };

    if goal.releases.contains(&release_number) {
        return;
    }

    let mut goal = goal.as_ref().clone();
    goal.releases.push(release_number);
    goal.updated = DateTimeAsMicroseconds::now();

    let dto: GoalDto = (&goal).into();
    app.goals_repo.upsert(&dto, ctx).await;

    app.board.upsert_goal(goal);
}

/// Change a release: its texts, its date, the services in it, or whether it is deleted. Returns its handle.
pub async fn update_release(
    app: &AppContext,
    handle: &str,
    patch: ReleasePatch,
) -> Result<String, String> {
    if patch.is_empty() {
        return Err(
            "nothing to update: pass at least one of title, description, release_notes, date, a service to add or remove, or deleted"
                .to_string(),
        );
    }

    let board = app.board.read();
    let resolved = resolve_release_by_handle(&board, handle)?;
    let project = resolved.project;
    let mut release = resolved.release.as_ref().clone();

    let now = DateTimeAsMicroseconds::now();

    if let Some(title) = &patch.title {
        release.title = validate_title(title)?;
    }

    if let Some(description) = &patch.description {
        release.description = description.trim().to_string();
    }

    if let Some(release_notes) = &patch.release_notes {
        release.release_notes = release_notes.trim().to_string();
    }

    if let Some(date) = &patch.date {
        release.date = super::parse_caller_moment(date, "the release date")?;
    }

    // On the clone, like every other field here: one service that does not validate refuses the whole
    // call rather than half of it.
    patch.services.apply(&mut release.services, now)?;

    // Stamped once and cleared whole, exactly as on a task and a goal.
    match patch.deleted {
        Some(true) => {
            if release.deleted_moment.is_none() {
                release.deleted_moment = Some(now);
            }
        }
        Some(false) => release.deleted_moment = None,
        None => {}
    }

    release.updated = now;

    let ctx = MyTelemetryContext::create_empty();
    let dto: ReleaseDto = (&release).into();
    app.releases_repo.upsert(&dto, &ctx).await;

    let handle = compose_release_handle(&project.prefix, release.number);
    app.board.upsert_release(release);
    app.notify_project_changed(&project.id).await;

    Ok(handle)
}

/// Mark a release deleted.
///
/// A flag, like every deletion here: the release leaves the Releases screen and every goal that lists it,
/// and stays reachable by its id, which reports it as deleted. **The goals are not edited** — their lists
/// keep the number, read past it, and show the release again the moment it is brought back. That is the
/// same arrangement a deleted document's references have, and for the same reason: an undo that had to
/// remember which goals to re-attach to would not be an undo.
///
/// Deleting twice is not an error and does not move the moment.
pub async fn delete_release(app: &AppContext, handle: &str) -> Result<String, String> {
    let board = app.board.read();
    let resolved = resolve_release_by_handle(&board, handle)?;
    let project = resolved.project;
    let mut release = resolved.release.as_ref().clone();

    if release.deleted_moment.is_none() {
        let now = DateTimeAsMicroseconds::now();

        release.deleted_moment = Some(now);
        release.updated = now;
    }

    let ctx = MyTelemetryContext::create_empty();
    let dto: ReleaseDto = (&release).into();
    app.releases_repo.upsert(&dto, &ctx).await;

    let handle = compose_release_handle(&project.prefix, release.number);
    app.board.upsert_release(release);
    app.notify_project_changed(&project.id).await;

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const HASH: &str = "099602e4c1a9b7d2f3e5a6b8c9d0e1f2a3b4c5d6";

    fn now(seconds: i64) -> DateTimeAsMicroseconds {
        DateTimeAsMicroseconds::new(seconds * 1_000_000)
    }

    fn service(microservice_id: &str, version: &str) -> NewServiceRelease {
        NewServiceRelease {
            microservice_id: microservice_id.to_string(),
            version: version.to_string(),
            git_hash: HASH.to_string(),
            datetime: None,
            settings_update_note: None,
            description: None,
        }
    }

    fn add(services: Vec<NewServiceRelease>) -> ServicesPatch {
        ServicesPatch {
            add: services,
            ..Default::default()
        }
    }

    #[test]
    fn a_service_is_added_with_the_moment_it_was_recorded_when_none_is_given() {
        let mut services = Vec::new();

        add(vec![service("my-service", "1.2.3")])
            .apply(&mut services, now(100))
            .unwrap();

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].microservice_id, "my-service");
        assert_eq!(services[0].version, "1.2.3");
        assert_eq!(services[0].git_hash, HASH);
        assert_eq!(services[0].datetime, now(100));
        assert_eq!(services[0].settings_update_note, "");
        assert_eq!(services[0].description, "");
    }

    /// A stamp is kept to the second, because that is all the row stores of it: finer than that, memory
    /// and Postgres would hold two different moments for one service until the next restart.
    #[test]
    fn a_stamped_datetime_is_what_the_row_will_hold() {
        let mut services = Vec::new();

        add(vec![service("my-service", "1.2.3")])
            .apply(
                &mut services,
                DateTimeAsMicroseconds::new(100 * 1_000_000 + 654_321),
            )
            .unwrap();

        assert_eq!(services[0].datetime, now(100));

        let stored: crate::postgres::ServiceReleaseJsonModel = (&services[0]).into();
        let read_back: ServiceReleaseModel = (&stored).into();

        assert_eq!(read_back, services[0]);
    }

    /// When the service went out is the caller's to say, and a zone they wrote is honoured — a release
    /// recorded in local time must not be filed hours away from when it happened.
    #[test]
    fn a_service_takes_the_datetime_the_caller_gave() {
        let mut services = Vec::new();

        let mut given = service("my-service", "1.2.3");
        given.datetime = Some("2026-10-07T14:30:00+03:00".to_string());

        add(vec![given]).apply(&mut services, now(100)).unwrap();

        assert_eq!(
            services[0].datetime,
            DateTimeAsMicroseconds::from_str("2026-10-07T11:30:00.000000Z").unwrap()
        );
    }

    /// A release names a microservice once. Reporting it again is a correction of that entry, in place —
    /// two rows for one deployment would make the release say something that did not happen.
    #[test]
    fn the_same_microservice_twice_is_one_entry_corrected() {
        let mut services = Vec::new();

        let mut first = service("my-service", "1.2.3");
        first.settings_update_note = Some("add `ttl` to settings".to_string());
        first.description = Some("the first cut".to_string());

        add(vec![first, service("other-service", "0.4.0")])
            .apply(&mut services, now(100))
            .unwrap();

        // The version was mistyped. Nothing else is restated.
        add(vec![service("my-service", "1.2.4")])
            .apply(&mut services, now(500))
            .unwrap();

        assert_eq!(services.len(), 2, "corrected in place, not added");
        assert_eq!(services[0].microservice_id, "my-service", "and it kept its place");
        assert_eq!(services[0].version, "1.2.4");
        assert_eq!(
            services[0].datetime,
            now(100),
            "a correction is not a second rollout — the moment stays"
        );
        assert_eq!(
            services[0].settings_update_note, "add `ttl` to settings",
            "a note nobody restated must survive the correction"
        );
        assert_eq!(services[0].description, "the first cut");
    }

    /// An empty string is something a caller passed, so it replaces — that is how a note is cleared.
    #[test]
    fn an_empty_note_clears_what_was_there() {
        let mut services = Vec::new();

        let mut first = service("my-service", "1.2.3");
        first.settings_update_note = Some("add `ttl` to settings".to_string());

        add(vec![first]).apply(&mut services, now(100)).unwrap();

        let mut cleared = service("my-service", "1.2.3");
        cleared.settings_update_note = Some("  ".to_string());

        add(vec![cleared]).apply(&mut services, now(200)).unwrap();

        assert_eq!(services[0].settings_update_note, "");
    }

    #[test]
    fn removal_runs_after_addition_and_ignores_what_is_not_there() {
        let mut services = Vec::new();

        ServicesPatch {
            add: vec![service("a", "1"), service("b", "1")],
            remove: vec![" a ".to_string(), "never-was-here".to_string()],
        }
        .apply(&mut services, now(100))
        .unwrap();

        let left: Vec<&str> = services
            .iter()
            .map(|itm| itm.microservice_id.as_str())
            .collect();

        assert_eq!(left, vec!["b"]);
    }

    /// A branch and a tag both move, so neither is a record of what was built. The refusal is the point:
    /// `main` stored as a commit would read as precise and name nothing.
    #[test]
    fn a_git_hash_has_to_be_a_hash() {
        for refused in ["", "main", "v1.2.3", "abc12", "099602g", "https://github.com/o/r/commit/099602e"] {
            let mut entry = service("my-service", "1.2.3");
            entry.git_hash = refused.to_string();

            assert!(
                add(vec![entry]).apply(&mut Vec::new(), now(1)).is_err(),
                "{refused:?} should be refused"
            );
        }

        for accepted in ["099602e", "099602E4C1A9B7D2F3E5A6B8C9D0E1F2A3B4C5D6"] {
            let mut services = Vec::new();
            let mut entry = service("my-service", "1.2.3");
            entry.git_hash = accepted.to_string();

            add(vec![entry]).apply(&mut services, now(1)).unwrap();

            assert_eq!(
                services[0].git_hash,
                accepted.to_lowercase(),
                "stored lower-cased, so one commit is one spelling"
            );
        }
    }

    #[test]
    fn a_service_needs_an_id_and_a_version_that_are_identifiers() {
        for (microservice_id, version) in [
            ("", "1.2.3"),
            ("my service", "1.2.3"),
            ("my-service", ""),
            ("my-service", "the one with the fix"),
        ] {
            assert!(
                add(vec![service(microservice_id, version)])
                    .apply(&mut Vec::new(), now(1))
                    .is_err(),
                "{microservice_id:?} / {version:?} should be refused"
            );
        }
    }

    /// A title is one line naming what went out: required, trimmed, and capped rather than truncated so a
    /// caller who pasted the notes into it is told.
    #[test]
    fn a_title_is_required_and_capped() {
        assert!(validate_title("   ").is_err());
        assert!(validate_title(&"x".repeat(MAX_RELEASE_TITLE_LEN + 1)).is_err());
        assert_eq!(validate_title("  Releases  ").unwrap(), "Releases");
    }

    #[test]
    fn an_empty_patch_is_recognised() {
        assert!(ReleasePatch::default().is_empty());

        for patch in [
            ReleasePatch {
                deleted: Some(false),
                ..Default::default()
            },
            ReleasePatch {
                release_notes: Some(String::new()),
                ..Default::default()
            },
            ReleasePatch {
                services: ServicesPatch {
                    remove: vec!["my-service".to_string()],
                    ..Default::default()
                },
                ..Default::default()
            },
        ] {
            assert!(!patch.is_empty());
        }
    }

    fn project() -> ProjectModel {
        ProjectModel {
            id: "p".to_string(),
            name: "p".to_string(),
            description: String::new(),
            prefix: "RMS".to_string(),
            prefix_history: Vec::new(),
            column_template_id: None,
            columns: Vec::new(),
            kind_template_id: None,
            kinds: Vec::new(),
            members: BTreeSet::new(),
            last_task_number: 0,
            archive_days: None,
            archived_moment: None,
            github_connections: Vec::new(),
            created: now(0),
        }
    }

    fn release(number: i64, deleted: bool) -> ReleaseModel {
        ReleaseModel {
            project_id: "p".to_string(),
            number,
            title: format!("release {number}"),
            description: String::new(),
            release_notes: String::new(),
            date: now(number),
            services: Vec::new(),
            created: now(0),
            updated: now(0),
            deleted_moment: deleted.then(|| now(1)),
        }
    }

    fn board_with(releases: Vec<ReleaseModel>) -> BoardInner {
        BoardInner::from_loaded(
            vec![project()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            releases,
        )
    }

    fn attach(add: &[&str], remove: &[&str]) -> ReleasesPatch {
        ReleasesPatch {
            add: add.iter().map(|itm| itm.to_string()).collect(),
            remove: remove.iter().map(|itm| itm.to_string()).collect(),
        }
    }

    #[test]
    fn a_goal_takes_a_release_by_handle_or_by_bare_number_and_only_once() {
        let board = board_with(vec![release(5, false), release(9, false)]);
        let project = project();
        let mut listed = Vec::new();

        attach(&["RMS-R5", "rms-r9", "5", " 9 "], &[])
            .apply(&board, &project, &mut listed)
            .unwrap();

        assert_eq!(listed, vec![5, 9]);
    }

    /// A reference has to point at something a reader can open: a number nothing answers to, a release of
    /// another board and a goal's id spelled where a release's belongs are all refused, and nothing is kept
    /// from the call.
    #[test]
    fn a_release_that_is_not_there_is_refused() {
        let board = board_with(vec![release(5, false), release(6, true)]);
        let project = project();

        for refused in ["RMS-R7", "7", "OTHER-R5", "RMS-G5", "RMS-5", "0", "five"] {
            let mut listed = Vec::new();

            assert!(
                attach(&[refused], &[])
                    .apply(&board, &project, &mut listed)
                    .is_err(),
                "{refused:?} should be refused"
            );
        }

        // Deleted is refused on the way in, with what would fix it.
        let deleted = attach(&["RMS-R6"], &[])
            .apply(&board, &project, &mut Vec::new())
            .unwrap_err();

        assert!(deleted.contains("deleted: false"), "{deleted}");
    }

    /// Detaching is the one thing that must work on a release that has been deleted — that is usually why
    /// it is being detached.
    #[test]
    fn a_goal_lets_go_of_a_release_whatever_became_of_it() {
        let board = board_with(vec![release(5, false), release(6, true)]);
        let project = project();
        let mut listed = vec![5, 6, 40];

        attach(&[], &["RMS-R6", "40", "RMS-R99"])
            .apply(&board, &project, &mut listed)
            .unwrap();

        assert_eq!(listed, vec![5]);
    }
}
