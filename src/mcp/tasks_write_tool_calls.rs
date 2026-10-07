use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
// `GhActionInput` is imported rather than named through `crate::mcp::` at the point of use: the
// ApplyJsonSchema derive resolves a nested type's schema by its bare name, so a path there does not compile.
use crate::mcp::{GhActionInput, SubtaskEditInput, SubtaskInput, SubtaskOps, TaskView};
use crate::scripts::{NewTask, TaskPatch};

/// What every write tool returns: the task as it now stands.
///
/// Returning the whole task rather than an "ok" means the caller sees the consequences of its own
/// write — most usefully `blocked`, which the write may have changed for other tasks too.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TaskWriteResponse {
    #[property(description = "The task as it now stands")]
    pub task: TaskView,
}

async fn read_back(app: &AppContext, handle: &str) -> Result<TaskWriteResponse, String> {
    let board = app.board.read();
    let resolved = crate::scripts::resolve_task(&board, handle)?;

    Ok(TaskWriteResponse {
        task: TaskView::from_model(&resolved.task, &resolved.project, &board),
    })
}

// ------------------------------------------------------------------------------------------ create

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TasksCreateInput {
    #[property(description = "Which board to put it on, by project prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "What the task says, as Markdown — the board renders it. One task, one piece of work: write the thing to be done, not the topic. **bold** for the headline, `code` for identifiers, paths and commands, `-` bullets for a short checklist. Keep it to a sticker's worth — a line or two, not a document"
    )]
    pub text: String,
    #[property(
        description = "How urgent it is: `super-high`, `high`, `normal`, `low` or `super-low`. It decides where the card sits — the board puts the most urgent at the top of its column. OMIT IT unless the work is genuinely out of the ordinary: `normal` is the default and the scale only says anything while most tasks are on it. Ask rather than guess when somebody's words are vague — \"important\" is not a priority"
    )]
    pub priority: Option<String>,
    #[property(
        description = "What kind of work it is, by kind id. Read the kind descriptions from projects_list first — the meanings are per project. Omit for no kind"
    )]
    pub kind: Option<String>,
    #[property(
        description = "The goal this task is part of, by the goal id from goals_list. A goal is a container — an epic. Omit to leave the task standalone; on an update pass an empty string to detach it"
    )]
    pub goal: Option<String>,
    #[property(
        description = "Who takes it: an email from users_list, or the literal `AI` when an agent does it. Never a first name — a name here names nobody. Omit to leave it unassigned"
    )]
    pub assignee: Option<String>,
    #[property(
        description = "Tags to put on it. Reuse what the project already uses (projects_list returns them) rather than coining a near-duplicate, which splits one idea across two tags and makes both filters lie. A tag that does not exist yet is created simply by using it"
    )]
    pub labels: Option<Vec<String>>,
    #[property(
        description = "Ids of the tasks that block this one, e.g. [\"RMS-7\"]. Bare numbers work too. Must be tasks of the same project. The task then reads as `blocked` and must not be started until every one of them is `done`"
    )]
    pub depends_on: Option<Vec<String>>,
    #[property(
        description = "A checklist to break this task down with, INSIDE it: each item a `title` and optionally a longer `text`. Not tasks — items have no status, no assignee and no place on the board, and an unticked one never stops the task from landing. Use it for the steps of one piece of work; anything somebody else has to see, schedule or depend on is a task of its own under the same goal. Usually omitted — a breakdown is normally written later, with tasks_update"
    )]
    pub subtasks: Option<Vec<SubtaskInput>>,
    #[property(
        description = "References to the documents this task is done against, from documents_list. A document is the specification the work is done against; attaching it here is what puts it in front of whoever picks the task up. A reference is a url and names either kind of document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository — so a specification that lives in a repository is attached without copying it in first. A bare id or the `path` a listing reports is accepted too and comes back as the url. Every one must be on THIS project and must resolve: a reference does not cross boards, and one that names nothing is refused rather than stored as a dead link"
    )]
    pub documents: Option<Vec<String>>,
}

pub struct TasksCreateHandler {
    app: Arc<AppContext>,
}

impl TasksCreateHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for TasksCreateHandler {
    const FUNC_NAME: &'static str = "tasks_create";
    const DESCRIPTION: &'static str = "Put a new task on a board. Use it when you are asked to \
remember a piece of work, or when work is agreed that nobody is doing yet. Read the board first: this \
creates a task unconditionally, so calling it twice for the same work leaves two of them. The \
returned `id` is how the task is named from then on.\
\
A new task ALWAYS starts in `todo` — there is no status to pass. Moving it on is tasks_update's job, \
which is also where landing work has to say what was done.";
}

#[async_trait::async_trait]
impl McpToolCall<TasksCreateInput, TaskWriteResponse> for TasksCreateHandler {
    async fn execute_tool_call(
        &self,
        model: TasksCreateInput,
    ) -> Result<TaskWriteResponse, String> {
        let handle = crate::scripts::create_task(
            &self.app,
            NewTask {
                goal: model.goal.clone(),
                project_prefix: model.project,
                text: model.text,
                priority: model.priority,
                kind: model.kind,
                assignee: model.assignee,
                labels: model.labels.unwrap_or_default(),
                depends_on: model.depends_on.unwrap_or_default(),
                subtasks: SubtaskInput::into_new(model.subtasks),
                documents: model.documents.unwrap_or_default(),
            },
        )
        .await?;

        read_back(&self.app, &handle).await
    }
}

// ------------------------------------------------------------------------------------------ update

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TasksUpdateInput {
    #[property(
        description = "Which task to change, by id, e.g. `RMS-42`. Not its text — two tasks may read alike"
    )]
    pub id: String,
    #[property(
        description = "Move it to this column, by column id. This is what to call when work starts and when it lands. Do not move a `blocked` task out of a waiting column until its blockers are done"
    )]
    pub status: Option<String>,
    #[property(
        description = "Rewrite what the task says, as Markdown. Omit to leave the text alone"
    )]
    pub text: Option<String>,
    #[property(
        description = "Re-rank it: `super-high`, `high`, `normal`, `low` or `super-low`. The card moves up or down its column immediately, on every screen watching the board. Omit to leave the priority alone; there is no way to clear it, because `normal` IS the absence of a ranking"
    )]
    pub priority: Option<String>,
    #[property(
        description = "Reclassify it, by kind id. Pass an empty string to clear the kind; omit to leave it alone"
    )]
    pub kind: Option<String>,
    #[property(
        description = "The goal this task is part of, by the goal id from goals_list. A goal is a container — an epic. Omit to leave the task standalone; on an update pass an empty string to detach it"
    )]
    pub goal: Option<String>,
    #[property(
        description = "Reassign it — an email, or `AI`. Pass an empty string to clear the assignee; omit to leave it as it is"
    )]
    pub assignee: Option<String>,
    #[property(
        description = "Tags to add. Added to what the task already carries, so you need not know the current set"
    )]
    pub add_labels: Option<Vec<String>>,
    #[property(
        description = "Tags to take off. Applied after add_labels, so a tag passed to both ends up removed. A tag that is not there is not an error"
    )]
    pub remove_labels: Option<Vec<String>>,
    #[property(
        description = "Replace the WHOLE set of blocking task ids. Pass [] to clear every dependency; omit to leave them untouched"
    )]
    pub depends_on: Option<Vec<String>>,
    #[property(
        description = "Checklist items to add to this task, each a `title` and optionally a longer `text`. Added to whatever the task already carries, so you need not know the current list. This is where a breakdown normally gets written — after reading the task, before starting it"
    )]
    pub add_subtasks: Option<Vec<SubtaskInput>>,
    #[property(
        description = "TICK checklist items off, by the `id` each one reports. This is the call to make as you work through a breakdown — it is how the next reader sees where you got to. An id that names no item is refused, so read the task if you are unsure"
    )]
    pub check_subtasks: Option<Vec<String>>,
    #[property(
        description = "Un-tick checklist items, by id — for something that turned out not to be done after all. Applied after check_subtasks, so an id passed to both ends up unticked"
    )]
    pub uncheck_subtasks: Option<Vec<String>>,
    #[property(
        description = "Reword checklist items in place: each entry an `id` plus the `title` and/or `text` to replace. Keeps the item's id and its place in the list, which removing and re-adding would not"
    )]
    pub edit_subtasks: Option<Vec<SubtaskEditInput>>,
    #[property(
        description = "Take checklist items off the list, by id — for a step that turned out not to be needed. Applied last. An item that is done is normally left ticked rather than removed: it is the record of what the work involved"
    )]
    pub remove_subtasks: Option<Vec<String>>,
    #[property(
        description = "Documents to attach to this task, from documents_list. Added to whatever it already references, so you need not know the current set. Each is a reference url — `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository — and a bare id or a listing's `path` is accepted too. A document of this project's own must be live: one in the trash is refused with the path it had, because restoring it is the fix. A file in a repository must be one the mirror actually holds, so a path that has not been pulled yet is refused saying so rather than stored"
    )]
    pub add_documents: Option<Vec<String>>,
    #[property(
        description = "Documents to detach, by reference or by any spelling of one — whichever you have detaches the same reference. Applied after add_documents, so one passed to both ends up detached. Detaching is not deleting — the document is untouched, this only takes the reference off this task. One that is not there is not an error"
    )]
    pub remove_documents: Option<Vec<String>>,
    #[property(
        description = "Builds this task produced, to record on it: each one the `url` of its GitHub Actions run and what to call it. CALL THIS WHEN A CHANGE MADE IN THIS TASK IS BUILT — the link is how somebody months later gets from the work to what shipped from it, and nothing else in this service records that. Added to whatever the task already carries, so you need not know the current list; the same url twice is one build, not two"
    )]
    pub add_gh_actions: Option<Vec<GhActionInput>>,
    #[property(
        description = "Build links to take off, by url — for one recorded against the wrong task. Applied after add_gh_actions. A url that is not there is not an error"
    )]
    pub remove_gh_actions: Option<Vec<String>>,
    #[property(
        description = "Pass false to UNDELETE a task somebody removed — a deletion is a flag, not a removal, so it undoes cleanly. Pass true to delete it, which tasks_delete also does. Omit to leave it alone"
    )]
    pub deleted: Option<bool>,
    #[property(
        description = "A note to put on the task's thread as part of this same change, as Markdown. Optional in general — and REQUIRED when this change moves the task to `done`, where it has to say what was actually done. Write a line or two: what changed, and anything the next person should know"
    )]
    pub comment: Option<String>,
    #[property(
        description = "Who the comment is from: an email, or the literal `AI`. Required whenever `comment` is passed — there is no session here to derive an author from"
    )]
    pub comment_by: Option<String>,
}

pub struct TasksUpdateHandler {
    app: Arc<AppContext>,
}

impl TasksUpdateHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for TasksUpdateHandler {
    const FUNC_NAME: &'static str = "tasks_update";
    const DESCRIPTION: &'static str = "Move a task between columns, rewrite it, or change who is on \
it. This is what to call when work starts and when it lands. Only the fields you pass change; \
omitted ones keep their value. An update that would change nothing is refused rather than quietly \
doing nothing — which is almost always a status that was meant to be passed and was not.\
\
MOVING A TASK TO `done` REQUIRES A COMMENT saying what was actually done — pass `comment` and \
`comment_by` in the same call. Without one the move is refused. The Done column is what the board is \
worth reading for later, and \"moved to done\" on its own records nothing. Only the transition into \
Done needs this: editing a task that is already there does not.\
\
IT IS ALSO WHERE THE CHECKLIST IS KEPT. add_subtasks writes the breakdown of the work, check_subtasks \
ticks items off as you go, and every item names itself by the `id` the task reports — an id that is not \
there is refused rather than ignored. The checklist is INSIDE the task and nothing derives from it: an \
unticked item does not stop the task from landing, so it is a note to whoever reads the task next, not a \
second board.\
\
AND IT IS WHERE A BUILD IS RECORDED. When a change made in this task gets built, put the run on it with \
add_gh_actions — the url and what it is called. That link is the only thing connecting the work to what \
shipped from it, and it is what somebody reading the task months later needs first. Do it in the same \
call that lands the task where you can.";
}

#[async_trait::async_trait]
impl McpToolCall<TasksUpdateInput, TaskWriteResponse> for TasksUpdateHandler {
    async fn execute_tool_call(
        &self,
        model: TasksUpdateInput,
    ) -> Result<TaskWriteResponse, String> {
        let handle = crate::scripts::update_task(
            &self.app,
            &model.id,
            TaskPatch {
                goal: model.goal.clone(),
                text: model.text,
                status: model.status,
                priority: model.priority,
                kind: model.kind,
                assignee: model.assignee,
                add_labels: model.add_labels.unwrap_or_default(),
                remove_labels: model.remove_labels.unwrap_or_default(),
                depends_on: model.depends_on,
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
                gh_actions: crate::mcp::GhActionOps {
                    add: model.add_gh_actions,
                    remove: model.remove_gh_actions,
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

// ------------------------------------------------------------------------------------------ delete

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TasksDeleteInput {
    #[property(description = "Which task to delete, by id")]
    pub id: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TasksDeleteResponse {
    #[property(description = "The id of the task that was removed")]
    pub id: String,
}

pub struct TasksDeleteHandler {
    app: Arc<AppContext>,
}

impl TasksDeleteHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for TasksDeleteHandler {
    const FUNC_NAME: &'static str = "tasks_delete";
    const DESCRIPTION: &'static str = "Delete a task. Finished work is NOT deleted — it moves to `done` \
with tasks_update, which is what keeps a board readable as a history. Delete only what should never have \
been there.\
\
IT IS A FLAG, NOT A REMOVAL. The task leaves every board, list and count, and stays exactly where it was: \
searching for its id still finds it and reports it as deleted. That is the point — an id that came back as \
\"no such task\" would be indistinguishable from a typo and from another board's id. Undo it with \
tasks_update and `deleted: false`. The number is never reused either way.";
}

#[async_trait::async_trait]
impl McpToolCall<TasksDeleteInput, TasksDeleteResponse> for TasksDeleteHandler {
    async fn execute_tool_call(
        &self,
        model: TasksDeleteInput,
    ) -> Result<TasksDeleteResponse, String> {
        let id = crate::scripts::delete_task(&self.app, &model.id).await?;

        Ok(TasksDeleteResponse { id })
    }
}
