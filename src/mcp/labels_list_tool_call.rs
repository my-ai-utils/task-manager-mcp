use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct LabelsListInput {
    #[property(description = "Which board's tags to read, by project prefix")]
    pub project: String,
}

/// One tag and how much of the board wears it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct LabelView {
    #[property(description = "The tag, in the spelling it is stored and filtered by")]
    pub label: String,
    #[property(
        description = "How many tasks currently carry it, in every column. A count of 1 is worth looking at — it is often a near-duplicate of another tag rather than a category of its own"
    )]
    pub used_by: i32,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct LabelsListResponse {
    #[property(description = "The tags in use on this board, alphabetical")]
    pub labels: Vec<LabelView>,
    #[property(description = "Number of rows in `labels`")]
    pub amount: i32,
}

pub struct LabelsListHandler {
    app: Arc<AppContext>,
}

impl LabelsListHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for LabelsListHandler {
    const FUNC_NAME: &'static str = "labels_list";
    const DESCRIPTION: &'static str = "See which tags a board already uses, before you put one on a \
task. Tags have no definitions here — they are just words — which is exactly why reusing the existing \
spelling matters: `net-summary` and `netsummary` are two tags, and a filter on either one then lies \
about half the board. There is no separate list to maintain: a tag exists for as long as some task \
wears it and disappears when the last one drops it.";
}

#[async_trait::async_trait]
impl McpToolCall<LabelsListInput, LabelsListResponse> for LabelsListHandler {
    async fn execute_tool_call(
        &self,
        model: LabelsListInput,
    ) -> Result<LabelsListResponse, String> {
        let board = self.app.board.read();
        let project = crate::scripts::resolve_project_by_prefix(&board, &model.project)?;

        let tasks = board.tasks_of_project(&project.id);

        let labels: Vec<LabelView> = board
            .labels_of_project(&project.id)
            .into_iter()
            .map(|label| {
                let used_by = tasks
                    .iter()
                    .filter(|task| task.labels.contains(&label))
                    .count() as i32;

                LabelView { label, used_by }
            })
            .collect();

        Ok(LabelsListResponse {
            amount: labels.len() as i32,
            labels,
        })
    }
}
