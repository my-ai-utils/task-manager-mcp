use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;

use crate::app::AppContext;
use crate::board::{CommentModel, GoalModel, compose_goal_handle, compose_task_handle};
use crate::postgres::GoalDto;

use super::{resolve_goal_by_handle, resolve_project_by_prefix};

/// Everything needed to create a goal. A struct rather than three positional strings, two of which are
/// prose and would be trivially swappable.
pub struct NewGoal {
    pub project_prefix: String,
    pub name: String,
    pub description: String,
    /// A palette colour name. `None` takes the default swatch — a goal without an opinion about its
    /// colour is normal, and picking one is what the browser is for.
    pub color: Option<String>,
    /// How urgent the goal is. `None` is Normal.
    pub priority: Option<String>,
    /// The goal's own checklist, if it opens with one.
    pub subtasks: Vec<super::NewSubtask>,
    /// Documents to point at from the start, by id. A goal is where a decision gets written down, so this is
    /// the likelier of the two to open with one.
    pub documents: Vec<String>,
    /// When work on it began, as the caller writes it — see [`parse_goal_moment`]. `None` opens it
    /// unstarted, which is what a goal still being talked about is.
    pub started_at: Option<String>,
}

/// A change to a goal. Every field is optional; `None` means "leave it alone".
///
/// There is deliberately no `status`: a goal has two states and `close` is the transition between them,
/// which is also where the resolution is demanded. Passing a column id would invite the second iteration's
/// vocabulary a version early.
#[derive(Default)]
pub struct GoalPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
    /// Re-rank it. `None` leaves the priority alone — every goal has one, so there is nothing to clear.
    pub priority: Option<String>,
    /// `Some(true)` closes the goal, `Some(false)` re-opens it, `None` leaves its state alone.
    pub close: Option<bool>,
    /// When work on the goal began — see [`parse_goal_moment`]. An empty string clears it: a start set by
    /// mistake has to be undoable.
    pub started_at: Option<String>,
    /// When the goal was closed, for a close that happened earlier than the call recording it — see
    /// [`parse_goal_moment`]. Dates the close this call makes, or re-dates the one already made; on a goal
    /// that is neither, it is refused.
    pub closed_at: Option<String>,
    /// `Some(false)` brings a deleted goal back. `Some(true)` deletes it, which `delete_goal` also does —
    /// both are here because undoing has to live somewhere, and a delete tool that also undeletes reads as a
    /// trick question.
    pub deleted: Option<bool>,
    /// Changes to the goal's checklist. Says nothing about whether the goal may close — that is decided by
    /// its tasks, and an unticked item is not unfinished work the board knows about.
    pub subtasks: super::SubtasksPatch,
    /// Which documents this goal points at: ids to attach, ids to detach — see [`super::DocumentsPatch`].
    pub documents: super::DocumentsPatch,
    /// Which releases this goal went out in: ones to attach, ones to detach — see
    /// [`super::ReleasesPatch`]. Says nothing about whether the goal may close, any more than a document
    /// does: a goal is finished when its tasks are, and shipped when somebody says so here.
    pub releases: super::ReleasesPatch,
    pub comment: Option<String>,
    pub comment_by: Option<String>,
}

impl GoalPatch {
    /// Whether this patch would do nothing at all. Refused rather than performed, because a write that
    /// changes nothing still moves `updated` and still pushes a snapshot to every open screen.
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.description.is_none()
            && self.color.is_none()
            && self.priority.is_none()
            && self.close.is_none()
            && self.started_at.is_none()
            && self.closed_at.is_none()
            && self.deleted.is_none()
            && self.subtasks.is_empty()
            && self.documents.is_empty()
            && self.releases.is_empty()
            && self.trimmed_comment().is_none()
    }

    /// The comment, if there is anything to it. Whitespace is not a comment.
    pub fn trimmed_comment(&self) -> Option<&str> {
        self.comment
            .as_deref()
            .map(str::trim)
            .filter(|itm| !itm.is_empty())
    }
}

/// The number of the goal a caller named, validated against one project.
///
/// Accepts a handle (`RMS-G7`) or a bare number, the same leniency `depends_on` gets: a number is
/// unambiguous within a project, and a caller working on one board should not have to retype the prefix.
///
/// Says nothing about whether the goal is open — reading a closed goal's work is legitimate. The callers
/// that care add that check themselves; see `resolve_goal` in `scripts/tasks.rs`, which refuses to hang
/// new work on a closed goal.
pub fn resolve_goal_reference(
    board: &crate::board::BoardInner,
    project: &crate::board::ProjectModel,
    goal: &str,
) -> Result<i64, String> {
    let goal = goal.trim();

    let number = match goal.parse::<i64>() {
        Ok(number) if number > 0 => number,
        Ok(_) => return Err(format!("'{goal}' is not a goal number")),
        Err(_) => {
            let parsed = crate::board::parse_goal_handle(goal).ok_or_else(|| {
                format!(
                    "'{goal}' is not a goal id — expected something like {}-G1, or a bare number",
                    project.prefix
                )
            })?;

            if parsed.prefix != project.prefix {
                return Err(format!(
                    "'{goal}' belongs to another project — this call is about {}",
                    project.prefix
                ));
            }

            parsed.number
        }
    };

    if board.get_goal(&project.id, number).is_none() {
        return Err(format!(
            "no goal {} on {}",
            compose_goal_handle(&project.prefix, number),
            project.prefix
        ));
    }

    Ok(number)
}

/// Put a new goal on a board. Returns its handle, `RMS-G7`.
///
/// The number comes from the project's ONE counter, the same one task numbers come from, so a number never
/// names both a task and a goal. Validation happens before the number is reserved, so a refused call does
/// not burn one.
pub async fn create_goal(app: &AppContext, new_goal: NewGoal) -> Result<String, String> {
    let board = app.board.read();
    let project = resolve_project_by_prefix(&board, &new_goal.project_prefix)?;

    if new_goal.name.trim().is_empty() {
        return Err("a goal needs a name".to_string());
    }

    let color = match &new_goal.color {
        None => task_manager_shared::kind_color::KindColor::default(),
        Some(color) => super::parse_kind_color(color)?,
    };

    let priority = match &new_goal.priority {
        None => task_manager_shared::priority::Priority::default(),
        Some(priority) => super::parse_priority(priority)?,
    };

    // Before the number is reserved, with the rest of the validation: a checklist item with no title must not
    // burn a goal number.
    let subtasks = super::build_subtasks(&new_goal.subtasks)?;

    // Before the number is reserved, with everything else — and the one check here that reads Postgres,
    // because documents are not in memory.
    let mut documents = Vec::new();

    super::DocumentsPatch {
        add: new_goal.documents.clone(),
        ..Default::default()
    }
    .apply(app, &project.id, &mut documents, "this goal")
    .await?;

    let now = DateTimeAsMicroseconds::now();

    // With the rest of the validation, before the number is reserved.
    let start_moment = match new_goal.started_at.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(started) => Some(parse_goal_moment(started, "started_at", now)?),
    };

    let number = app.board.reserve_task_number(&project.id).ok_or_else(|| {
        format!(
            "project {} vanished while creating the goal",
            project.prefix
        )
    })?;

    let goal = GoalModel {
        project_id: project.id.clone(),
        number,
        name: new_goal.name.trim().to_string(),
        description: new_goal.description.trim().to_string(),
        color,
        priority,
        subtasks,
        documents,
        // A goal is opened before anything under it has shipped. Releases are attached as they go out.
        releases: Vec::new(),
        comments: Vec::new(),
        created: now,
        updated: now,
        start_moment,
        // Nothing is created closed. A goal closes only once its tasks are done, and it has none yet.
        close_moment: None,
        deleted_moment: None,
    };

    let ctx = MyTelemetryContext::create_empty();
    let dto: GoalDto = (&goal).into();
    app.goals_repo.upsert(&dto, &ctx).await;

    // The counter moved in memory when the number was reserved; persist it so a restart does not hand the
    // same number out again — to a task OR to another goal.
    super::persist_project_counter(app, &project.id, &ctx).await;

    app.board.upsert_goal(goal);
    app.notify_project_changed(&project.id).await;

    Ok(compose_goal_handle(&project.prefix, number))
}

/// Change a goal: its name, its text, its state, or a note on its thread. Returns its handle.
///
/// Closing is the one transition with a rule attached, and it has two halves:
///
/// * **every task must be done.** This is the first and most important of the three doors into "a closed
///   goal has no live tasks" — the other two live in `scripts/tasks.rs`. Refused with the open tasks named,
///   because "some tasks are open" leaves the caller hunting for which.
/// * **it needs a resolution.** A closed epic is the thing somebody reads months later to find out how it
///   went, and "closed" on its own records nothing anybody can use. Same rule and same reason as a task
///   arriving in Done.
pub async fn update_goal(
    app: &AppContext,
    handle: &str,
    patch: GoalPatch,
) -> Result<String, String> {
    if patch.is_empty() {
        return Err(
            "nothing to update: pass at least one of name, description, color, priority, close, started_at, closed_at, a checklist change, a document reference, a release or comment"
                .to_string(),
        );
    }

    let board = app.board.read();
    let resolved = resolve_goal_by_handle(&board, handle)?;
    let project = resolved.project;
    let mut goal = resolved.goal.as_ref().clone();

    let was_closed = goal.is_closed();
    let handle = compose_goal_handle(&project.prefix, goal.number);

    if let Some(name) = &patch.name {
        if name.trim().is_empty() {
            return Err("a goal needs a name".to_string());
        }
        goal.name = name.trim().to_string();
    }

    if let Some(description) = &patch.description {
        goal.description = description.trim().to_string();
    }

    if let Some(color) = &patch.color {
        goal.color = super::parse_kind_color(color)?;
    }

    if let Some(priority) = &patch.priority {
        goal.priority = super::parse_priority(priority)?;
    }

    // On the clone, like every other field here: an op naming an item that is not there refuses the whole
    // call rather than half of it.
    patch.subtasks.apply(&mut goal.subtasks, &handle)?;

    // On the clone as well; this one queries Postgres, because a document is not in memory.
    patch
        .documents
        .apply(app, &project.id, &mut goal.documents, &handle)
        .await?;

    // On the clone too, and against the snapshot this call started from: a release that is not on this
    // project, or has been deleted, refuses the whole call.
    patch.releases.apply(&board, &project, &mut goal.releases)?;

    // Everything below is validation, and all of it runs before a single field is written back.
    let closing = patch.close == Some(true) && !was_closed;

    if closing {
        let open = board.open_tasks_of_goal(&project.id, goal.number);

        if !open.is_empty() {
            let named: Vec<String> = open
                .iter()
                .map(|task| compose_task_handle(&project.prefix, task.number))
                .collect();

            return Err(format!(
                "{handle} still has unfinished work: {}. A goal closes only once all of its tasks are done — land them, or take them out of the goal first.",
                named.join(", ")
            ));
        }
    }

    let comment = build_goal_comment(patch.trimmed_comment(), patch.comment_by.as_deref())?;

    if closing && comment.is_none() {
        return Err(format!(
            "{handle} is being closed, so it needs a resolution — pass `comment` (and `comment_by`) with this same call. What came of the goal, and anything the next person should know; \"done\" records nothing."
        ));
    }

    if let Some(comment) = comment {
        goal.comments.push(comment);
    }

    apply_moments(
        &mut goal,
        &patch,
        was_closed,
        DateTimeAsMicroseconds::now(),
        &handle,
        |goal| first_start_of_work(&board, goal),
    )?;

    // Stamped once and cleared whole: deleting twice must not rewrite when it happened, and restoring has to
    // leave no trace of the flag or the goal would read as deleted for ever.
    match patch.deleted {
        Some(true) => {
            if goal.deleted_moment.is_none() {
                goal.deleted_moment = Some(DateTimeAsMicroseconds::now());
            }
        }
        Some(false) => goal.deleted_moment = None,
        None => {}
    }

    goal.updated = DateTimeAsMicroseconds::now();

    let ctx = MyTelemetryContext::create_empty();
    let dto: GoalDto = (&goal).into();
    app.goals_repo.upsert(&dto, &ctx).await;

    app.board.upsert_goal(goal);
    app.notify_project_changed(&project.id).await;

    Ok(handle)
}

/// Mark a goal deleted.
///
/// **Not removed, and not the same act as closing.** Closing says how a goal WENT and demands a resolution
/// for that reason; deleting says it should never have existed. The two are orthogonal — a deleted goal may
/// have been open or closed — and neither is reachable from the other.
///
/// A deleted goal drops out of the Goals screen, out of every project listing and out of `effective_goal`, so
/// a task that was under it reads as standalone rather than pointing at something nobody can open. The tasks
/// themselves are NOT deleted with it: they are work, and whether work survives its container is a decision
/// for whoever is deleting, not a side effect.
///
/// Deleting twice is not an error and does not move the moment.
pub async fn delete_goal(app: &AppContext, handle: &str) -> Result<String, String> {
    let board = app.board.read();
    let resolved = resolve_goal_by_handle(&board, handle)?;
    let project = resolved.project;
    let mut goal = resolved.goal.as_ref().clone();

    if goal.deleted_moment.is_none() {
        goal.deleted_moment = Some(DateTimeAsMicroseconds::now());
        goal.updated = DateTimeAsMicroseconds::now();
    }

    let ctx = MyTelemetryContext::create_empty();
    let dto: GoalDto = (&goal).into();
    app.goals_repo.upsert(&dto, &ctx).await;

    let handle = compose_goal_handle(&project.prefix, goal.number);
    app.board.upsert_goal(goal);
    app.notify_project_changed(&project.id).await;

    Ok(handle)
}

/// Append a comment to a goal's thread.
///
/// Does **not** move the goal's `updated`, for the same reason a task's thread does not move its: the
/// conversation about the work is a separate record from the work. Which matters more here than on a task —
/// the conversation is what a goal is for.
pub async fn add_goal_comment(
    app: &AppContext,
    handle: &str,
    who: &str,
    text: &str,
) -> Result<String, String> {
    if who.trim().is_empty() {
        return Err("a comment needs an author — an email, or `AI`".to_string());
    }

    if text.trim().is_empty() {
        return Err("a comment needs some text".to_string());
    }

    let board = app.board.read();
    let resolved = resolve_goal_by_handle(&board, handle)?;
    let project = resolved.project;
    let mut goal = resolved.goal.as_ref().clone();

    goal.comments.push(CommentModel {
        moment: DateTimeAsMicroseconds::now(),
        who: super::normalise_actor(who.trim()),
        text: text.trim().to_string(),
    });

    let ctx = MyTelemetryContext::create_empty();
    let dto: GoalDto = (&goal).into();
    app.goals_repo.upsert(&dto, &ctx).await;

    let handle = compose_goal_handle(&project.prefix, goal.number);
    app.board.upsert_goal(goal);
    app.notify_project_changed(&project.id).await;

    Ok(handle)
}

/// The two ends of a goal, as one patch moves them: `started_at`, the close, and `closed_at`. Pure, so the
/// rules are tested without a board — `first_start` is asked only when a closed goal turns out to have no
/// start.
///
/// * `started_at` sets the start — `now`, a date, a date and time — and an empty string clears it.
/// * Closing stamps the end: `closed_at` if given, now if not. Re-opening clears it.
/// * `closed_at` dates the close this patch makes, or re-dates one already made — and on a goal that is
///   neither it is refused, because there is no close to date.
/// * A closed goal always has a start: one nobody gave is `first_start`'s answer.
/// * A goal never starts after it closed.
fn apply_moments(
    goal: &mut GoalModel,
    patch: &GoalPatch,
    was_closed: bool,
    now: DateTimeAsMicroseconds,
    handle: &str,
    first_start: impl FnOnce(&GoalModel) -> DateTimeAsMicroseconds,
) -> Result<(), String> {
    if let Some(started) = &patch.started_at {
        goal.start_moment = match started.trim() {
            "" => None,
            started => Some(parse_goal_moment(started, "started_at", now)?),
        };
    }

    let closed_at = match &patch.closed_at {
        None => None,
        Some(closed_at) => Some(parse_goal_moment(closed_at, "closed_at", now)?),
    };

    // Stamped on the way in and cleared on the way out, so a re-opened goal carries no close date and a
    // re-closed one is dated by its latest close. Without the clearing, an open goal with an old moment
    // would count as archived and quietly leave the screen.
    match (patch.close, closed_at) {
        (Some(true), closed_at) if !was_closed => {
            goal.close_moment = Some(closed_at.unwrap_or(now))
        }
        (Some(true) | None, Some(closed_at)) if was_closed => goal.close_moment = Some(closed_at),
        (Some(true), None) | (None, None) => {}
        (Some(false), None) => goal.close_moment = None,
        (_, Some(_)) => {
            return Err(format!(
                "{handle} is not closed, so there is no close to date — pass `closed_at` with `close: true`, or on a goal that is already closed"
            ));
        }
    }

    // A goal is never closed without a start: its span on the timeline needs both ends.
    if goal.close_moment.is_some() && goal.start_moment.is_none() {
        goal.start_moment = Some(first_start(goal));
    }

    if let (Some(started), Some(closed)) = (goal.start_moment, goal.close_moment) {
        if started.unix_microseconds > closed.unix_microseconds {
            return Err(format!(
                "{handle} would start after it closed: started {} is later than closed {}",
                started.to_rfc3339_utc(),
                closed.to_rfc3339_utc()
            ));
        }
    }

    Ok(())
}

/// A moment a caller gives for a goal — when it started, or when it closed: `now`, a date, or a date and
/// time (see `parse_caller_moment`). Not in the future: both are records of what happened, not a plan.
pub fn parse_goal_moment(
    src: &str,
    what: &str,
    now: DateTimeAsMicroseconds,
) -> Result<DateTimeAsMicroseconds, String> {
    let src = src.trim();

    if src.eq_ignore_ascii_case("now") {
        return Ok(now);
    }

    let moment = super::parse_caller_moment(src, what)?;

    // A minute of slack for a caller whose clock runs ahead of this one.
    if moment.unix_microseconds > now.unix_microseconds + 60 * 1_000_000 {
        return Err(format!(
            "{what} '{src}' is in the future — it records when something happened, so pass `now` or a moment that has passed"
        ));
    }

    Ok(moment)
}

/// When work on a goal began, as far as its tasks can tell: the earliest moment one of them left Todo. A
/// task that left it before starts were recorded counts from when it was created; a task still in Todo has
/// not begun anything. With nothing to go on, the goal's own opening.
fn first_start_of_work(
    board: &crate::board::BoardInner,
    goal: &GoalModel,
) -> DateTimeAsMicroseconds {
    board
        .tasks_of_goal(&goal.project_id, goal.number)
        .iter()
        .filter_map(|task| match task.start_moment {
            Some(started) => Some(started),
            None if task.status != task_manager_shared::projects::COLUMN_ID_TODO => {
                Some(task.created)
            }
            None => None,
        })
        .min_by_key(|moment| moment.unix_microseconds)
        .unwrap_or(goal.created)
}

/// The author half of a comment on a goal, with the same rule a task's thread applies: text without an
/// author would leave a thread of anonymous notes, and MCP has no session to derive one from.
fn build_goal_comment(
    comment: Option<&str>,
    comment_by: Option<&str>,
) -> Result<Option<CommentModel>, String> {
    let Some(comment) = comment else {
        return Ok(None);
    };

    let who = comment_by
        .map(str::trim)
        .filter(|itm| !itm.is_empty())
        .ok_or_else(|| {
            "a comment needs an author — pass `comment_by` as an email, or `AI`".to_string()
        })?;

    Ok(Some(CommentModel {
        moment: DateTimeAsMicroseconds::now(),
        who: super::normalise_actor(who),
        text: comment.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A patch that would change nothing is refused rather than performed: it still moves `updated` and
    /// still pushes a snapshot to every screen watching the project.
    #[test]
    fn an_empty_patch_is_recognised() {
        assert!(GoalPatch::default().is_empty());

        let blank_comment = GoalPatch {
            comment: Some("   ".to_string()),
            ..Default::default()
        };

        assert!(
            blank_comment.is_empty(),
            "whitespace is not a comment, so it is not a change either"
        );
    }

    fn at(src: &str) -> DateTimeAsMicroseconds {
        DateTimeAsMicroseconds::from_str(src).unwrap()
    }

    fn open_goal() -> GoalModel {
        GoalModel {
            project_id: "p0".to_string(),
            number: 7,
            name: "Timeline".to_string(),
            description: String::new(),
            color: task_manager_shared::kind_color::KindColor::default(),
            priority: task_manager_shared::priority::Priority::default(),
            subtasks: Vec::new(),
            documents: Vec::new(),
            releases: Vec::new(),
            comments: Vec::new(),
            created: at("2026-10-01T09:00:00"),
            updated: at("2026-10-01T09:00:00"),
            start_moment: None,
            close_moment: None,
            deleted_moment: None,
        }
    }

    const NOW: &str = "2026-10-10T12:00:00";

    fn apply(goal: &mut GoalModel, patch: GoalPatch, was_closed: bool) -> Result<(), String> {
        apply_moments(goal, &patch, was_closed, at(NOW), "TM-G7", |_| {
            at("2026-10-03T08:00:00")
        })
    }

    /// `now` is what an agent passes as it begins; a date is what it passes when recording the past.
    #[test]
    fn a_start_is_now_or_a_moment_that_has_passed() {
        let now = at(NOW);

        assert_eq!(parse_goal_moment("now", "started_at", now).unwrap(), now);
        assert_eq!(parse_goal_moment(" NOW ", "started_at", now).unwrap(), now);
        assert_eq!(
            parse_goal_moment("2026-10-05", "started_at", now).unwrap(),
            at("2026-10-05T00:00:00")
        );

        let future = parse_goal_moment("2026-10-11", "started_at", now).unwrap_err();
        assert!(future.contains("in the future"), "{future}");

        let nonsense = parse_goal_moment("yesterday", "started_at", now).unwrap_err();
        assert!(
            nonsense.contains("started_at"),
            "the refusal names the field: {nonsense}"
        );
    }

    #[test]
    fn a_start_is_set_corrected_and_cleared() {
        let mut goal = open_goal();

        let set = GoalPatch {
            started_at: Some("now".to_string()),
            ..Default::default()
        };
        assert!(!set.is_empty());
        apply(&mut goal, set, false).unwrap();
        assert_eq!(goal.start_moment, Some(at(NOW)));

        let cleared = GoalPatch {
            started_at: Some(" ".to_string()),
            ..Default::default()
        };
        apply(&mut goal, cleared, false).unwrap();
        assert_eq!(
            goal.start_moment, None,
            "an empty string undoes a start set by mistake"
        );
    }

    /// Closing is moving the goal to done, and it stamps the end — now, or when the caller says it was.
    #[test]
    fn closing_stamps_the_end_and_a_closed_goal_always_has_a_start() {
        let mut goal = open_goal();

        let close = GoalPatch {
            close: Some(true),
            ..Default::default()
        };
        apply(&mut goal, close, false).unwrap();

        assert_eq!(goal.close_moment, Some(at(NOW)));
        assert_eq!(
            goal.start_moment,
            Some(at("2026-10-03T08:00:00")),
            "nobody gave a start, so it is when its work began"
        );

        let mut backdated = open_goal();
        backdated.start_moment = Some(at("2026-10-02T10:00:00"));

        let close_earlier = GoalPatch {
            close: Some(true),
            closed_at: Some("2026-10-08T17:00:00+03:00".to_string()),
            ..Default::default()
        };
        apply(&mut backdated, close_earlier, false).unwrap();

        assert_eq!(backdated.close_moment, Some(at("2026-10-08T14:00:00")));
        assert_eq!(
            backdated.start_moment,
            Some(at("2026-10-02T10:00:00")),
            "a start somebody gave is kept"
        );
    }

    #[test]
    fn closed_at_dates_a_close_and_nothing_else() {
        let mut closed = open_goal();
        closed.start_moment = Some(at("2026-10-02T10:00:00"));
        closed.close_moment = Some(at("2026-10-09T10:00:00"));

        let redate = GoalPatch {
            closed_at: Some("2026-10-07".to_string()),
            ..Default::default()
        };
        assert!(!redate.is_empty());
        apply(&mut closed, redate, true).unwrap();
        assert_eq!(closed.close_moment, Some(at("2026-10-07T00:00:00")));

        let mut open = open_goal();
        let nothing_to_date = GoalPatch {
            closed_at: Some("2026-10-07".to_string()),
            ..Default::default()
        };
        let refused = apply(&mut open, nothing_to_date, false).unwrap_err();
        assert!(refused.contains("not closed"), "{refused}");

        let mut reopening = closed.clone();
        let contradiction = GoalPatch {
            close: Some(false),
            closed_at: Some("2026-10-07".to_string()),
            ..Default::default()
        };
        assert!(apply(&mut reopening, contradiction, true).is_err());
    }

    #[test]
    fn re_opening_clears_the_end_and_keeps_the_start() {
        let mut goal = open_goal();
        goal.start_moment = Some(at("2026-10-02T10:00:00"));
        goal.close_moment = Some(at("2026-10-09T10:00:00"));

        let reopen = GoalPatch {
            close: Some(false),
            ..Default::default()
        };
        apply(&mut goal, reopen, true).unwrap();

        assert_eq!(goal.close_moment, None);
        assert_eq!(goal.start_moment, Some(at("2026-10-02T10:00:00")));
    }

    #[test]
    fn a_goal_does_not_start_after_it_closed() {
        let mut goal = open_goal();
        goal.start_moment = Some(at("2026-10-02T10:00:00"));
        goal.close_moment = Some(at("2026-10-05T10:00:00"));

        let too_late = GoalPatch {
            started_at: Some("2026-10-06".to_string()),
            ..Default::default()
        };
        let refused = apply(&mut goal, too_late, true).unwrap_err();
        assert!(refused.contains("start after it closed"), "{refused}");
    }

    /// A colour is a change like any other — the Goals screen makes exactly this call and nothing else.
    #[test]
    fn recolouring_is_a_change() {
        let patch = GoalPatch {
            color: Some("blue".to_string()),
            ..Default::default()
        };

        assert!(!patch.is_empty());
    }

    /// Re-opening is a change like any other — `close: Some(false)` on an open goal is still something to
    /// apply, and the emptiness check must not swallow it.
    #[test]
    fn asking_to_close_or_re_open_is_never_empty() {
        for close in [true, false] {
            let patch = GoalPatch {
                close: Some(close),
                ..Default::default()
            };

            assert!(!patch.is_empty(), "close: {close} is a change");
        }
    }

    /// A checklist change arrives with nothing else set — the emptiness check has to see it, or ticking an
    /// item on a goal is refused as "nothing to update".
    #[test]
    fn a_checklist_change_alone_is_not_an_empty_update() {
        let patch = GoalPatch {
            subtasks: crate::scripts::SubtasksPatch {
                check: vec!["some-id".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!patch.is_empty());
    }

    /// Attaching a release arrives with nothing else set, exactly as a checklist change does — and it is
    /// the usual shape of the call: the work was done, it went out, and the goal is told so.
    #[test]
    fn a_release_reference_alone_is_not_an_empty_update() {
        let patch = GoalPatch {
            releases: crate::scripts::ReleasesPatch {
                add: vec!["RMS-R12".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!patch.is_empty());
    }

    /// Text without an author is refused. A goal's thread is the record of how the work was decided, and an
    /// anonymous entry in it is worth less than no entry.
    #[test]
    fn a_comment_without_an_author_is_refused() {
        assert!(build_goal_comment(Some("we split this in two"), None).is_err());
        assert!(build_goal_comment(Some("we split this in two"), Some("  ")).is_err());
        assert!(build_goal_comment(Some("we split this in two"), Some("AI")).is_ok());
    }
}
