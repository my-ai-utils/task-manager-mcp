use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::{CommentView, TaskView};

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct AddCommentInput {
    #[property(description = "Which task to comment on, by id, e.g. `RMS-42`")]
    pub id: String,
    #[property(
        description = "Who is leaving it: an email from users_list, or the literal `AI` when an agent is commenting. Same rule as an assignee — resolve a spoken first name to an address, never store the name"
    )]
    pub who: String,
    #[property(description = "The comment, as Markdown — the board renders it")]
    pub text: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct AddCommentResponse {
    #[property(
        description = "The task, whose `comments_amount` now includes this comment. Its `updated` is deliberately unchanged"
    )]
    pub task: TaskView,
}

pub struct AddCommentHandler {
    app: Arc<AppContext>,
}

impl AddCommentHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for AddCommentHandler {
    const FUNC_NAME: &'static str = "tasks_add_comment";
    const DESCRIPTION: &'static str = "Leave a note, a finding or a decision on a task without \
touching what the task itself says. Use this instead of rewriting the text when you learned something \
about the work rather than changed what the work is — the text is the instruction, the thread is how \
it was figured out. Stamped with the moment and the author you pass, and it does not move the task's \
`updated`.";
}

#[async_trait::async_trait]
impl McpToolCall<AddCommentInput, AddCommentResponse> for AddCommentHandler {
    async fn execute_tool_call(
        &self,
        model: AddCommentInput,
    ) -> Result<AddCommentResponse, String> {
        let handle =
            crate::scripts::add_comment(&self.app, &model.id, &model.who, &model.text).await?;

        let board = self.app.board.read();
        let resolved = crate::scripts::resolve_task(&board, &handle)?;

        Ok(AddCommentResponse {
            task: TaskView::from_model(&resolved.task, &resolved.project, &board),
        })
    }
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GetCommentsInput {
    #[property(description = "Which task's thread to read, by id")]
    pub id: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GetCommentsResponse {
    #[property(description = "The task the thread belongs to")]
    pub id: String,
    #[property(description = "The comments, oldest first. Empty when nobody has commented")]
    pub comments: Vec<CommentView>,
    #[property(description = "Number of rows in `comments`")]
    pub amount: i32,
}

pub struct GetCommentsHandler {
    app: Arc<AppContext>,
}

impl GetCommentsHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GetCommentsHandler {
    const FUNC_NAME: &'static str = "tasks_get_comments";
    const DESCRIPTION: &'static str = "Read a task's thread. tasks_list only reports how many \
comments a task has — this is how to read what they say. Worth doing before picking up a task whose \
`comments_amount` is not zero: the reason the work is shaped the way it is usually lives there rather \
than in the task text.";
}

#[async_trait::async_trait]
impl McpToolCall<GetCommentsInput, GetCommentsResponse> for GetCommentsHandler {
    async fn execute_tool_call(
        &self,
        model: GetCommentsInput,
    ) -> Result<GetCommentsResponse, String> {
        let board = self.app.board.read();
        let resolved = crate::scripts::resolve_task(&board, &model.id)?;

        let comments: Vec<CommentView> = resolved
            .task
            .comments
            .iter()
            .map(|itm| CommentView {
                moment_unix_seconds: itm.moment.unix_microseconds / 1_000_000,
                who: itm.who.clone(),
                text: itm.text.clone(),
            })
            .collect();

        Ok(GetCommentsResponse {
            id: crate::board::compose_task_handle(&resolved.project.prefix, resolved.task.number),
            amount: comments.len() as i32,
            comments,
        })
    }
}
