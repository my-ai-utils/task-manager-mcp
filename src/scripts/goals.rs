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

    let number = app.board.reserve_task_number(&project.id).ok_or_else(|| {
        format!(
            "project {} vanished while creating the goal",
            project.prefix
        )
    })?;

    let now = DateTimeAsMicroseconds::now();

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
            "nothing to update: pass at least one of name, description, color, priority, close, a checklist change, a document reference, a release or comment"
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

    // Stamped on the way in and cleared on the way out, so a re-opened goal carries no close date and a
    // re-closed one is dated by its latest close. Without the clearing, an open goal with an old moment
    // would count as archived and quietly leave the screen.
    match patch.close {
        Some(true) => {
            if !was_closed {
                goal.close_moment = Some(DateTimeAsMicroseconds::now());
            }
        }
        Some(false) => goal.close_moment = None,
        None => {}
    }

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
