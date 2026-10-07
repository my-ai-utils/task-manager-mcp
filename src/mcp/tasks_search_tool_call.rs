use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::BoardSearchHitView;

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TasksSearchInput {
    #[property(description = "Which board to search, by project prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "What to look for. Literal text by default. Matching is per LINE, so a query cannot span a newline"
    )]
    pub query: String,
    #[property(
        description = "Read `query` as a regular expression instead — `\\bauth\\b`, `RMS-\\d+`. Rust regex syntax; an invalid pattern is refused with the reason"
    )]
    pub is_regex: Option<bool>,
    #[property(
        description = "Match case exactly. OFF by default, which is the safe direction: case-insensitive finds a superset, so it cannot hide the hit you wanted"
    )]
    pub case_sensitive: Option<bool>,
    #[property(
        description = "Search the comment threads as well as the cards. ON by default, and it is where most of the value is — the reasoning behind a decision lives on a thread, and no other tool will show you one without being told which card to open"
    )]
    pub include_comments: Option<bool>,
    #[property(
        description = "Search GOALS only, skipping tasks. Pass true when you are looking for where an outcome was discussed rather than for a specific piece of work"
    )]
    pub goals_only: Option<bool>,
    #[property(
        description = "Include work closed longer ago than the project's archive window. Off by default, matching tasks_list — turn it on when you are deliberately looking backwards, which for a search is more often than for a listing: \"how did we solve this last time\" is a question about archived work"
    )]
    pub include_archived: Option<bool>,
    #[property(
        description = "Include deleted tasks and goals. Off by default. Deleted work is off every board and every count, and it is here at all only because searching for it is the one legitimate reason to look"
    )]
    pub include_deleted: Option<bool>,
    #[property(
        description = "How many cards to return, most-matched first. Thirty by default, forty at most — a search that returns the whole board has answered nothing"
    )]
    pub max_results: Option<i64>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TasksSearchResponse {
    #[property(
        description = "The cards that matched, MOST MATCHES FIRST — not by priority, because relevance is what a search is sorted by. Goals and tasks together; `is_goal` says which"
    )]
    pub results: Vec<BoardSearchHitView>,
    #[property(description = "How many cards are in `results`")]
    pub amount: i32,
    #[property(description = "How many tasks were looked at")]
    pub searched_tasks: i32,
    #[property(description = "How many goals were looked at")]
    pub searched_goals: i32,
    #[property(
        description = "True when more cards matched than were returned. Narrow the query rather than reading on: the ones you did not get are the ones that matched least"
    )]
    pub truncated: bool,
}

pub struct TasksSearchHandler {
    app: Arc<AppContext>,
}

impl TasksSearchHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for TasksSearchHandler {
    const FUNC_NAME: &'static str = "tasks_search";
    const DESCRIPTION: &'static str = "Find work by what was WRITTEN about it — across every task, \
every goal, and every comment on both. This is the tool for \"where did we discuss X\", \"has this come \
up before\", \"which task was the one about the WebSocket reconnect\".\
\
IT IS THE ONLY WAY TO SEE INSIDE THE THREADS. tasks_list reports `comments_amount` and not one comment, \
deliberately — a board's worth of threads would be most of a context window. So the reasoning behind \
every decision on this board is, without this tool, reachable only by listing the board and then \
opening threads one call at a time until you find it. THE THREAD IS USUALLY WHERE THE ANSWER IS: the \
card says what is to be done, the thread says what was learned, what was tried and why it is shaped \
this way. Each match says which of the two it came from.\
\
IT IS NOT A REPLACEMENT FOR tasks_list. That one answers \"what is in this column, what is ready to \
start, what is Yuri on\" — the questions a board is FOR, and it returns whole tasks. This one answers \
\"which card mentions this\", and returns a headline and the matching lines so that a search costs a \
fraction of a listing. Search to find the card, then read it properly.\
\
SEARCH BEFORE YOU CREATE, and search wider than you list. A near-duplicate task is a real mistake on a \
board a person reads by eye, and the work you are about to file may already exist under a wording you \
would not have guessed. `include_archived` is worth passing here more often than on a listing: \"how \
did we solve this last time\" is a question about work that has already landed.\
\
Results come back MOST-MATCHED FIRST rather than by priority, because a card that mentions the subject \
six times is more likely the one you meant than one that mentions it in passing.\
\
To search DOCUMENTS — specifications and anything else written up at a path — use documents_search. \
The two are separate on purpose: a document outlives the work, a card is the work.";
}

#[async_trait::async_trait]
impl McpToolCall<TasksSearchInput, TasksSearchResponse> for TasksSearchHandler {
    async fn execute_tool_call(&self, model: TasksSearchInput) -> Result<TasksSearchResponse, String> {
        let query = crate::scripts::BoardSearchQuery {
            query: model.query,
            is_regex: model.is_regex.unwrap_or(false),
            case_sensitive: model.case_sensitive.unwrap_or(false),
            // The one default that is ON. Off, this tool would answer a strictly worse question than the
            // one it exists for.
            include_comments: model.include_comments.unwrap_or(true),
            include_archived: model.include_archived.unwrap_or(false),
            include_deleted: model.include_deleted.unwrap_or(false),
            goals_only: model.goals_only.unwrap_or(false),
            max_results: model
                .max_results
                .unwrap_or(crate::scripts::DEFAULT_BOARD_HITS),
        };

        let outcome = crate::scripts::search_board(&self.app, &model.project, &query)?;

        Ok(TasksSearchResponse {
            amount: outcome.hits.len() as i32,
            searched_tasks: outcome.searched_tasks,
            searched_goals: outcome.searched_goals,
            truncated: outcome.truncated,
            results: outcome
                .hits
                .into_iter()
                .map(BoardSearchHitView::from_hit)
                .collect(),
        })
    }
}
