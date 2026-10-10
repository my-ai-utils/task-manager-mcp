use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::{CommentView, GoalView, SubtaskEditInput, SubtaskInput, SubtaskOps};
use crate::scripts::{GoalPatch, NewGoal};

/// What every goal write returns: the goal as it now stands.
///
/// The whole goal rather than an "ok", because the progress counters are derived and a caller that just
/// landed a task wants to see whether the goal became closable.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalWriteResponse {
    #[property(description = "The goal as it now stands")]
    pub goal: GoalView,
}

async fn read_back(app: &AppContext, handle: &str) -> Result<GoalWriteResponse, String> {
    let board = app.board.read();
    let resolved = crate::scripts::resolve_goal_by_handle(&board, handle)?;

    Ok(GoalWriteResponse {
        goal: GoalView::from_model(&resolved.goal, &resolved.project, &board),
    })
}

// -------------------------------------------------------------------------------------------- list

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalsListInput {
    #[property(description = "Which board to read, by project prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "Include goals closed longer ago than the project's archive window. Omitted gives the live list, which is almost always what you want; pass true when you are deliberately looking back at finished epics"
    )]
    pub include_archived: Option<bool>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalsListResponse {
    #[property(
        description = "The goals, most urgent first and oldest first within one priority — the order the Goals screen shows them in"
    )]
    pub goals: Vec<GoalView>,
    #[property(description = "Number of rows in `goals`")]
    pub amount: i32,
}

pub struct GoalsListHandler {
    app: Arc<AppContext>,
}

impl GoalsListHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GoalsListHandler {
    const FUNC_NAME: &'static str = "goals_list";
    const DESCRIPTION: &'static str = "The goals of one project — the containers work is organised \
around. Read this before creating or picking up work: a task usually belongs under a goal, and this is \
where the `goal` ids come from. Each goal reports its progress as `done_amount` of `tasks_amount`, \
counting archived work, so a finished epic reads as finished rather than as half-done.\
\
Goals closed longer ago than the project's archive window are left out unless you ask for them. A \
project with no goals is a legitimate answer, not an error — tasks may stand on their own.";
}

#[async_trait::async_trait]
impl McpToolCall<GoalsListInput, GoalsListResponse> for GoalsListHandler {
    async fn execute_tool_call(&self, model: GoalsListInput) -> Result<GoalsListResponse, String> {
        let board = self.app.board.read();
        let project = crate::scripts::resolve_project_by_prefix(&board, &model.project)?;

        let include_archived = model.include_archived.unwrap_or(false);

        let goals: Vec<GoalView> = board
            .goals_of_project(&project.id)
            .iter()
            .filter(|goal| include_archived || !board.is_goal_archived(goal))
            .map(|goal| GoalView::from_model(goal, &project, &board))
            .collect();

        Ok(GoalsListResponse {
            amount: goals.len() as i32,
            goals,
        })
    }
}

// ------------------------------------------------------------------------------------------ create

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalsCreateInput {
    #[property(description = "Which board to put it on, by project prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "What the goal is called. A short line naming the outcome, not a description of the work — the tasks under it are the work"
    )]
    pub name: String,
    #[property(
        description = "What the goal is about, as Markdown. Where the shape of it goes: what is in scope, what is not, what it depends on. The reasoning as it develops belongs on the thread instead, with goals_add_comment"
    )]
    pub description: Option<String>,
    #[property(
        description = "A palette colour: gray, red, orange, amber, green, teal, blue or purple. Purely visual — it is how the board marks which goal a card belongs to, so pick one that is not already in use on that project. Omit for gray; a person can also recolour it in the browser"
    )]
    pub color: Option<String>,
    #[property(
        description = "How urgent the goal is: `super-high`, `high`, `normal`, `low` or `super-low`. It decides where the goal sits on the Goals screen — most urgent at the top. Omit for `normal`, which is what most epics are and what makes the other four mean something"
    )]
    pub priority: Option<String>,
    #[property(
        description = "A checklist for the goal itself, each item a `title` and optionally a longer `text`. For the small things an epic drags along that are not worth a card — NOT for the work, which is tasks under the goal. An unticked item does not hold the goal open. Usually omitted"
    )]
    pub subtasks: Option<Vec<SubtaskInput>>,
    #[property(
        description = "References to the documents this goal points at, from documents_list. A goal is where a decision is written down, so this is where the specification behind an epic belongs. A reference is a url and names either kind of document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository — so an existing spec in a repository is attached without copying it in. A bare id or a listing's `path` is accepted too. Every one must be on THIS project — a reference does not cross boards"
    )]
    pub documents: Option<Vec<String>>,
    #[property(
        description = "When work on the goal began: `now`, a date like `2026-10-07`, or a date and time like `2026-10-07T14:30:00Z` (a zone offset such as `+03:00` is honoured; no zone means UTC). Pass `now` when you open a goal and start on it at once. Omit while it is still being talked about: set it with goals_update when work begins — and if nobody does, it is stamped by itself when the first task under the goal leaves todo"
    )]
    pub started_at: Option<String>,
}

pub struct GoalsCreateHandler {
    app: Arc<AppContext>,
}

impl GoalsCreateHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GoalsCreateHandler {
    const FUNC_NAME: &'static str = "goals_create";
    const DESCRIPTION: &'static str = "Open a goal — a container for work, an epic. Use it when a \
conversation is about an outcome rather than a single task: create the goal, keep the reasoning on its \
thread with goals_add_comment, and create the tasks under it as they become clear.\
\
Read goals_list first: this creates a goal unconditionally, and two goals for the same outcome split \
the work in half where a person reads it by eye. A goal always starts open — there is no state to pass, \
and closing it is goals_update's job, which is where the resolution has to be written.\
\
Pass `started_at: now` if work on it begins right away: a goal has a start and an end, and the Goals \
timeline draws it between the two.";
}

#[async_trait::async_trait]
impl McpToolCall<GoalsCreateInput, GoalWriteResponse> for GoalsCreateHandler {
    async fn execute_tool_call(&self, model: GoalsCreateInput) -> Result<GoalWriteResponse, String> {
        let handle = crate::scripts::create_goal(
            &self.app,
            NewGoal {
                project_prefix: model.project,
                name: model.name,
                description: model.description.unwrap_or_default(),
                color: model.color,
                priority: model.priority,
                subtasks: SubtaskInput::into_new(model.subtasks),
                documents: model.documents.unwrap_or_default(),
                started_at: model.started_at,
            },
        )
        .await?;

        read_back(&self.app, &handle).await
    }
}

// ------------------------------------------------------------------------------------------ update

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalsUpdateInput {
    #[property(description = "Which goal to change, by id, e.g. `RMS-G7`")]
    pub id: String,
    #[property(description = "Rename it. Omit to leave the name alone")]
    pub name: Option<String>,
    #[property(description = "Rewrite what the goal is about, as Markdown. Omit to leave it alone")]
    pub description: Option<String>,
    #[property(
        description = "Recolour it: gray, red, orange, amber, green, teal, blue or purple. Visual only — the colour the board marks this goal's cards with. Omit to leave it alone"
    )]
    pub color: Option<String>,
    #[property(
        description = "Re-rank it: `super-high`, `high`, `normal`, `low` or `super-low`. The goal moves up or down the Goals screen at once. Omit to leave it alone"
    )]
    pub priority: Option<String>,
    #[property(
        description = "Pass true to CLOSE the goal — move it to done — which is only allowed once every one of its tasks is `done` and always requires `comment` — the resolution. Closing stamps the goal's END: now, or `closed_at` if you pass it. Pass false to re-open a closed goal. Omit to leave its state alone"
    )]
    pub close: Option<bool>,
    #[property(
        description = "When work on the goal began: `now`, `2026-10-07`, or `2026-10-07T14:30:00Z`. SET IT WHEN YOU START WORKING ON THE GOAL — it is the goal's START on the Goals timeline, as closing is its end. If nobody sets it, it is stamped when the first task under the goal leaves todo, and a goal closed without one is given the earliest start among its tasks. Pass it again to correct it; an empty string clears it. Not in the future"
    )]
    pub started_at: Option<String>,
    #[property(
        description = "When the goal was closed, for a close that happened before this call: `2026-10-07` or `2026-10-07T14:30:00Z`. Only with `close: true`, or on a goal that is already closed to re-date its close. Omit to close it now, which is the usual case. Not in the future, and not before `started_at`"
    )]
    pub closed_at: Option<String>,
    #[property(
        description = "Checklist items to add to the goal, each a `title` and optionally a longer `text`. Added to what it already carries. For the goal's own loose ends — the work belongs in tasks under it"
    )]
    pub add_subtasks: Option<Vec<SubtaskInput>>,
    #[property(
        description = "TICK the goal's checklist items off, by the `id` each one reports. An id that names no item is refused rather than ignored"
    )]
    pub check_subtasks: Option<Vec<String>>,
    #[property(
        description = "Un-tick items, by id. Applied after check_subtasks, so an id passed to both ends up unticked"
    )]
    pub uncheck_subtasks: Option<Vec<String>>,
    #[property(
        description = "Reword items in place: each entry an `id` plus the `title` and/or `text` to replace. Keeps the item's id and its place in the list"
    )]
    pub edit_subtasks: Option<Vec<SubtaskEditInput>>,
    #[property(
        description = "Take items off the list, by id. Applied last. A done item is normally left ticked rather than removed — it is the record of what the goal involved"
    )]
    pub remove_subtasks: Option<Vec<String>>,
    #[property(
        description = "Documents to attach to this goal, from documents_list. Added to whatever it already references. Each is a reference url — `raw/{project}/document/{id}` or `raw/{project}/github/{repository}/{path}` — and a bare id or a listing's `path` is accepted too. A document of this project's own must be live: one in the trash is refused with the path it had. A file in a repository must be one the mirror actually holds"
    )]
    pub add_documents: Option<Vec<String>>,
    #[property(
        description = "Documents to detach, by reference or by any spelling of one. Applied after add_documents. Detaching is not deleting — the document is untouched, only this goal stops pointing at it"
    )]
    pub remove_documents: Option<Vec<String>>,
    #[property(
        description = "Releases this goal went out in, by id — `RMS-R12`, from releases_list — or by bare number. Added to whatever it already lists. This is how a release recorded without a `goal` is put on the goal it shipped. Each must be a live release of THIS project: one that does not exist is refused, and so is a deleted one"
    )]
    pub add_releases: Option<Vec<String>>,
    #[property(
        description = "Releases to take off this goal, by id or bare number. Applied after add_releases. Detaching is not deleting — the release is untouched and stays in releases_list, only this goal stops listing it"
    )]
    pub remove_releases: Option<Vec<String>>,
    #[property(
        description = "Pass false to UNDELETE a goal somebody removed. Pass true to delete it, which goals_delete also does. Omit to leave it alone"
    )]
    pub deleted: Option<bool>,
    #[property(
        description = "A note for the goal's thread as part of this same change, as Markdown. REQUIRED when closing, where it is the resolution: what came of the goal and anything the next person should know. Optional otherwise"
    )]
    pub comment: Option<String>,
    #[property(
        description = "Who the comment is from: an email, or the literal `AI`. Required whenever `comment` is passed"
    )]
    pub comment_by: Option<String>,
}

pub struct GoalsUpdateHandler {
    app: Arc<AppContext>,
}

impl GoalsUpdateHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GoalsUpdateHandler {
    const FUNC_NAME: &'static str = "goals_update";
    const DESCRIPTION: &'static str = "Rename a goal, rewrite it, start it or close it. Only the fields \
you pass change.\
\
A GOAL HAS A START AND AN END, and the Goals timeline draws it between the two. `started_at` is when work \
on it began: set it when you begin, or it is stamped when the first task under the goal leaves todo. The \
end is the close — `close: true`, which is moving the goal to done — stamped as you close it, or dated by \
`closed_at`.\
\
CLOSING A GOAL HAS TWO CONDITIONS, and both are enforced. Every task under it must be `done` — the \
call is refused with the unfinished ones named, and the fix is to land them or take them out of the \
goal. And it needs `comment` (with `comment_by`): the resolution, which is what somebody reads months \
later to find out how the goal went. A closed goal leaves the screen once the project's archive window \
has passed and is then reachable by its id.\
\
DELETING IS NOT CLOSING, and they answer different questions. Closing says how a goal WENT and demands a \
resolution for exactly that reason; goals_delete says it should never have existed. A goal can be deleted \
whether it was open or closed, and `deleted: false` here brings it back.";
}

#[async_trait::async_trait]
impl McpToolCall<GoalsUpdateInput, GoalWriteResponse> for GoalsUpdateHandler {
    async fn execute_tool_call(&self, model: GoalsUpdateInput) -> Result<GoalWriteResponse, String> {
        let handle = crate::scripts::update_goal(
            &self.app,
            &model.id,
            GoalPatch {
                name: model.name,
                description: model.description,
                color: model.color,
                priority: model.priority,
                close: model.close,
                started_at: model.started_at,
                closed_at: model.closed_at,
                subtasks: SubtaskOps {
                    add: model.add_subtasks,
                    edit: model.edit_subtasks,
                    check: model.check_subtasks,
                    uncheck: model.uncheck_subtasks,
                    remove: model.remove_subtasks,
                }
                .into_patch(),
                documents: crate::mcp::DocumentOps {
                    add: model.add_documents,
                    remove: model.remove_documents,
                }
                .into_patch(),
                releases: crate::mcp::ReleaseOps {
                    add: model.add_releases,
                    remove: model.remove_releases,
                }
                .into_patch(),
                deleted: model.deleted,
                comment: model.comment,
                comment_by: model.comment_by,
            },
        )
        .await?;

        read_back(&self.app, &handle).await
    }
}

// ---------------------------------------------------------------------------------------- comments

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalsAddCommentInput {
    #[property(description = "Which goal to add to, by id, e.g. `RMS-G7`")]
    pub id: String,
    #[property(
        description = "Who it is from: an email from users_list, or the literal `AI` when it is an agent's own note"
    )]
    pub who: String,
    #[property(
        description = "The note, as Markdown. What was decided, what was learned, what was ruled out — the reasoning behind the work rather than the work itself"
    )]
    pub text: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalsGetCommentsInput {
    #[property(description = "Which goal's thread to read, by id, e.g. `RMS-G7`")]
    pub id: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalCommentsResponse {
    #[property(description = "The goal the thread belongs to, by id")]
    pub id: String,
    #[property(description = "The notes, oldest first")]
    pub comments: Vec<CommentView>,
    #[property(description = "Number of rows in `comments`")]
    pub amount: i32,
}

pub struct GoalsAddCommentHandler {
    app: Arc<AppContext>,
}

impl GoalsAddCommentHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GoalsAddCommentHandler {
    const FUNC_NAME: &'static str = "goals_add_comment";
    const DESCRIPTION: &'static str = "Put a note on a goal's thread. THIS IS WHERE A CONVERSATION \
ABOUT WORK BELONGS: what was decided and why, what was tried, what was ruled out, what is still open. \
The goal's own text says what the outcome is; the thread says how the thinking got there, and it is \
what makes the tasks underneath make sense to whoever picks them up.\
\
A comment does not move the goal's `updated` — a busy thread does not make a goal look like changing \
work.";
}

#[async_trait::async_trait]
impl McpToolCall<GoalsAddCommentInput, GoalCommentsResponse> for GoalsAddCommentHandler {
    async fn execute_tool_call(
        &self,
        model: GoalsAddCommentInput,
    ) -> Result<GoalCommentsResponse, String> {
        let handle =
            crate::scripts::add_goal_comment(&self.app, &model.id, &model.who, &model.text).await?;

        read_comments(&self.app, &handle)
    }
}

pub struct GoalsGetCommentsHandler {
    app: Arc<AppContext>,
}

impl GoalsGetCommentsHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GoalsGetCommentsHandler {
    const FUNC_NAME: &'static str = "goals_get_comments";
    const DESCRIPTION: &'static str = "Read a goal's thread, oldest first. Worth reading whenever \
`comments_amount` is not zero and you are about to work on the goal or on a task under it: the reason \
the work is shaped the way it is usually lives here rather than in the goal's text.";
}

#[async_trait::async_trait]
impl McpToolCall<GoalsGetCommentsInput, GoalCommentsResponse> for GoalsGetCommentsHandler {
    async fn execute_tool_call(
        &self,
        model: GoalsGetCommentsInput,
    ) -> Result<GoalCommentsResponse, String> {
        read_comments(&self.app, &model.id)
    }
}

fn read_comments(app: &AppContext, handle: &str) -> Result<GoalCommentsResponse, String> {
    let board = app.board.read();
    let resolved = crate::scripts::resolve_goal_by_handle(&board, handle)?;

    let comments: Vec<CommentView> = resolved
        .goal
        .comments
        .iter()
        .map(|itm| CommentView {
            moment_unix_seconds: itm.moment.unix_microseconds / 1_000_000,
            who: itm.who.clone(),
            text: itm.text.clone(),
        })
        .collect();

    Ok(GoalCommentsResponse {
        id: crate::board::compose_goal_handle(&resolved.project.prefix, resolved.goal.number),
        amount: comments.len() as i32,
        comments,
    })
}

// ------------------------------------------------------------------------------------------ delete

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalsDeleteInput {
    #[property(description = "Which goal to delete, by id — `RMS-G7`")]
    pub id: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalsDeleteResponse {
    #[property(description = "The id of the goal that was deleted")]
    pub id: String,
}

pub struct GoalsDeleteHandler {
    app: Arc<AppContext>,
}

impl GoalsDeleteHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GoalsDeleteHandler {
    const FUNC_NAME: &'static str = "goals_delete";
    const DESCRIPTION: &'static str = "Delete a goal that should never have existed.\
\
NOT THE SAME AS CLOSING, and the two answer different questions. Closing says how a goal WENT — which is \
why it demands a resolution and refuses while any task is open. Deleting says it should not be there at \
all, and asks nothing, because there is nothing to record about work that was never real.\
\
IT IS A FLAG, NOT A REMOVAL. The goal leaves the Goals screen, every project listing and every task that \
pointed at it — such a task reads as standalone rather than pointing at something nobody can open — and it \
stays reachable by its id, which reports it as deleted. Undo it with goals_update and `deleted: false`.\
\
THE TASKS UNDER IT ARE NOT DELETED. They are work, and whether work outlives its container is your \
decision to make explicitly, not a side effect of this call.";
}

#[async_trait::async_trait]
impl McpToolCall<GoalsDeleteInput, GoalsDeleteResponse> for GoalsDeleteHandler {
    async fn execute_tool_call(
        &self,
        model: GoalsDeleteInput,
    ) -> Result<GoalsDeleteResponse, String> {
        let id = crate::scripts::delete_goal(&self.app, &model.id).await?;

        Ok(GoalsDeleteResponse { id })
    }
}
