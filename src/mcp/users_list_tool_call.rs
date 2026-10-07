use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::UserView;

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct UsersListInput {
    #[property(
        description = "Return only the people who may see this project, by prefix. This is usually what you want — someone who is not on the board should not be given work on it. Omit for everyone"
    )]
    pub project: Option<String>,
    #[property(
        description = "Include people who can no longer sign in. Omit or pass false: a disabled person should not be given new work"
    )]
    pub include_disabled: Option<bool>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct UsersListResponse {
    #[property(
        description = "The people, by email, led by the reserved AI assignee. An entry with `reserved` true is not a person"
    )]
    pub users: Vec<UserView>,
    #[property(description = "Number of rows in `users`")]
    pub amount: i32,
}

pub struct UsersListHandler {
    app: Arc<AppContext>,
}

impl UsersListHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for UsersListHandler {
    const FUNC_NAME: &'static str = "users_list";
    const DESCRIPTION: &'static str = "Call this when you are about to put a person on a task or on \
a comment and you were given a NAME rather than an address. It maps the two: a task's `assignee` and \
a comment's `who` are emails, and a first name written there names nobody at all — the board shows \
names but stores addresses. Pass the project to see only the people who may work on that board. If \
more than one person still fits the name, ask which; do not guess. The list always begins with `AI`, the \
reserved assignee that means an agent does the task — assign work to yourself with that, not with an \
address.";
}

#[async_trait::async_trait]
impl McpToolCall<UsersListInput, UsersListResponse> for UsersListHandler {
    async fn execute_tool_call(&self, model: UsersListInput) -> Result<UsersListResponse, String> {
        let board = self.app.board.read();
        let include_disabled = model.include_disabled.unwrap_or(false);

        let allowed = match &model.project {
            None => None,
            Some(prefix) => {
                let project = crate::scripts::resolve_project_by_prefix(&board, prefix)?;
                Some(project.members.clone())
            }
        };

        // AI leads the list, on every project and whatever the filters say: it is assignable everywhere,
        // it is never disabled, and it is not a member of anything — so filtering it like a person would
        // hide the one assignee the caller can always use, which is usually itself.
        let mut users: Vec<UserView> = vec![UserView::ai()];

        let people: Vec<UserView> = board
            .users()
            .iter()
            .filter(|user| include_disabled || !user.disabled)
            .filter(|user| match &allowed {
                None => true,
                Some(members) => members.contains(&user.email),
            })
            .map(|user| UserView::from_model(user))
            .collect();

        users.extend(people);

        Ok(UsersListResponse {
            amount: users.len() as i32,
            users,
        })
    }
}
