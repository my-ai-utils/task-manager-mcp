use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::{CommentView, ReleaseView, ServiceReleaseInput};
use crate::scripts::{NewRelease, ReleasePatch, ServicesPatch};

/// How many releases one listing returns when the caller does not say, and the most it will return.
///
/// A cap where `tasks_list` has none, and the difference is the archive window: a board is the last few
/// days of work, while a list of releases is the whole history of a project and only grows. Each row
/// carries its notes and every service, so an uncapped listing of a project two years old would spend a
/// caller's context answering "what went out last".
pub const DEFAULT_RELEASES: i32 = 20;
pub const MAX_RELEASES: i32 = 100;

/// What every release write returns: the release as it now stands.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleaseWriteResponse {
    #[property(description = "The release as it now stands")]
    pub release: ReleaseView,
}

fn read_back(app: &AppContext, handle: &str) -> Result<ReleaseWriteResponse, String> {
    let board = app.board.read();
    let resolved = crate::scripts::resolve_release_by_handle(&board, handle)?;

    Ok(ReleaseWriteResponse {
        release: ReleaseView::from_model(&resolved.release, &resolved.project, &board),
    })
}

// -------------------------------------------------------------------------------------------- list

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleasesListInput {
    #[property(description = "Which project's releases to read, by project prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "Only the releases this goal lists, by goal id — `RMS-G7`. The same list the goal itself reports in `releases`. Omit for every release of the project"
    )]
    pub goal: Option<String>,
    #[property(
        description = "Only the releases that include this microservice, by its exact id. With the default order this answers 'what is the latest released version of my-service' in one row. Omit for releases of any service"
    )]
    pub microservice_id: Option<String>,
    #[property(
        description = "Pass true for ONLY the releases that are out on production, false for only the ones that are not yet. Omit for both. With `microservice_id` and the default order, true answers 'which version of this service is live' in one row"
    )]
    pub released_on_prod: Option<bool>,
    #[property(
        description = "How many to return, newest first. Omitted gives 20; the most is 100. `total` in the answer says how many there are, so you can tell a short history from a truncated one"
    )]
    pub limit: Option<i32>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleasesListResponse {
    #[property(
        description = "The releases, NEWEST FIRST by the date of the release — the order they went out in, most recent at the top. Deleted ones are left out"
    )]
    pub releases: Vec<ReleaseView>,
    #[property(description = "Number of rows in `releases`")]
    pub amount: i32,
    #[property(
        description = "How many releases matched altogether. Bigger than `amount` means `limit` cut the list and the older ones are not shown"
    )]
    pub total: i32,
}

pub struct ReleasesListHandler {
    app: Arc<AppContext>,
}

impl ReleasesListHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for ReleasesListHandler {
    const FUNC_NAME: &'static str = "releases_list";
    const DESCRIPTION: &'static str = "The releases of one project, newest first — what went out, \
when, and in which version of which microservice. Read this to answer 'is it out', 'what is deployed' \
and 'what changed in the last release', and BEFORE recording one: releases_create adds a release \
unconditionally, and the same rollout written down twice is two releases on a list a person reads by eye.\
\
Each release comes back whole: its notes, and one entry per microservice with the version, the commit it \
was built from and — in `settings_update_note` — whatever has to change in that service's settings. \
Filter by `goal` for the releases one feature went out in, by `microservice_id` for the history of one \
service, or by `released_on_prod` for what has actually reached production — a release is recorded when \
it ships anywhere, so the unfiltered list is everything that went out, not everything that is live.\
\
A project with no releases is a legitimate answer, not an error. Nothing ages off this list: unlike the \
board it has no archive window, so it is capped by `limit` instead and reports `total`.";
}

#[async_trait::async_trait]
impl McpToolCall<ReleasesListInput, ReleasesListResponse> for ReleasesListHandler {
    async fn execute_tool_call(
        &self,
        model: ReleasesListInput,
    ) -> Result<ReleasesListResponse, String> {
        let board = self.app.board.read();
        let project = crate::scripts::resolve_project_by_prefix(&board, &model.project)?;

        let limit = match model.limit {
            None => DEFAULT_RELEASES,
            Some(limit) if (1..=MAX_RELEASES).contains(&limit) => limit,
            Some(limit) => {
                return Err(format!(
                    "limit {limit} is out of range — pass 1 to {MAX_RELEASES}, or omit it for {DEFAULT_RELEASES}"
                ));
            }
        };

        let of_goal = model
            .goal
            .as_deref()
            .map(str::trim)
            .filter(|itm| !itm.is_empty());

        // Both sources hand back the same order, so the filter below and the cut after it mean the same
        // thing whichever one the caller came through.
        let releases = match of_goal {
            Some(goal) => {
                let number = crate::scripts::resolve_goal_reference(&board, &project, goal)?;

                match board.get_goal(&project.id, number) {
                    Some(goal) => board.releases_of_goal(&goal),
                    None => Vec::new(),
                }
            }
            None => board.releases_of_project(&project.id),
        };

        let microservice_id = model
            .microservice_id
            .as_deref()
            .map(str::trim)
            .filter(|itm| !itm.is_empty());

        let matched: Vec<_> = releases
            .iter()
            .filter(|release| match microservice_id {
                Some(wanted) => release
                    .services
                    .iter()
                    .any(|itm| itm.microservice_id == wanted),
                None => true,
            })
            .filter(|release| match model.released_on_prod {
                Some(wanted) => release.is_released_on_prod() == wanted,
                None => true,
            })
            .collect();

        let releases: Vec<ReleaseView> = matched
            .iter()
            .take(limit as usize)
            .map(|release| ReleaseView::from_model(release, &project, &board))
            .collect();

        Ok(ReleasesListResponse {
            amount: releases.len() as i32,
            total: matched.len() as i32,
            releases,
        })
    }
}

// ------------------------------------------------------------------------------------------ create

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleasesCreateInput {
    #[property(description = "Which project the release belongs to, by project prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "What went out, in one line — the feature or the fix, not a version number: the versions are in `services`, one per microservice"
    )]
    pub title: String,
    #[property(
        description = "What this release is, as Markdown — the context around it. Omit when the goal it ships already says it"
    )]
    pub description: Option<String>,
    #[property(
        description = "What changed for whoever is on the receiving end, as Markdown — the part meant to be repeated to the people affected. Settings changes do NOT go here: each one belongs in `settings_update_note` on the service it concerns"
    )]
    pub release_notes: Option<String>,
    #[property(
        description = "The date of the release: `2026-10-07`, or a date and time like `2026-10-07T14:30:00Z`. A zone offset such as `+03:00` is honoured; no zone means UTC. Omit for now — right when you are recording it as it goes out, WRONG when you are writing down something that shipped earlier, since this is what the list is ordered by"
    )]
    pub date: Option<String>,
    #[property(
        description = "One entry per microservice this release touched: its `microservice_id`, the `version` that went out, the `git_hash` that version was built from, and optionally `datetime`, `settings_update_note` and `description`. A release of one feature usually touches several services — list them all here rather than recording a release each. May be omitted and filled in with releases_update as the services go out"
    )]
    pub services: Option<Vec<ServiceReleaseInput>>,
    #[property(
        description = "The goal this release ships, by goal id — `RMS-G7`. Attached in this same call, so the goal lists the release from the moment it exists. The goal is normally the description of the very feature being released; it may be open or already closed. Omit only when the release belongs to no goal"
    )]
    pub goal: Option<String>,
    #[property(
        description = "Pass true ONLY if this release is already out on production as you record it. Omit otherwise — a release normally goes to a test stand first, and the mark is put on later with releases_update, when production has actually been rolled"
    )]
    pub released_on_prod: Option<bool>,
}

pub struct ReleasesCreateHandler {
    app: Arc<AppContext>,
}

impl ReleasesCreateHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for ReleasesCreateHandler {
    const FUNC_NAME: &'static str = "releases_create";
    const DESCRIPTION: &'static str = "Record a release — the fact that a feature went out, and in \
what. ONE RELEASE IS ONE FEATURE, ACROSS HOWEVER MANY MICROSERVICES IT TOUCHED: the release says what \
changed (`title`, `description`, `release_notes`), and each entry in `services` says what was deployed — \
the microservice, its version, the commit that version was built from, and when.\
\
NAME THE GOAL IT SHIPS. Pass `goal` and the release is attached in the same call: the goal is the \
description of the feature, the release is the record of it going out, and from then on the goal lists \
every release it went out in. A release with no goal is allowed and is still a release of its project.\
\
SETTINGS CHANGES HAVE THEIR OWN FIELD, PER SERVICE. When rolling a service out needs its settings changed \
— a key to add, a value to change, a secret to supply — write that in that service's \
`settings_update_note` and nowhere else. It is separate from `description` on purpose: it is the one part \
of a release somebody has to ACT on, and the board marks the services that carry one.\
\
Read releases_list first: this records a release unconditionally. To add a service to a release that is \
already there, or to correct one, use releases_update — not a second release.";
}

#[async_trait::async_trait]
impl McpToolCall<ReleasesCreateInput, ReleaseWriteResponse> for ReleasesCreateHandler {
    async fn execute_tool_call(
        &self,
        model: ReleasesCreateInput,
    ) -> Result<ReleaseWriteResponse, String> {
        let handle = crate::scripts::create_release(
            &self.app,
            NewRelease {
                project_prefix: model.project,
                title: model.title,
                description: model.description.unwrap_or_default(),
                release_notes: model.release_notes.unwrap_or_default(),
                date: model.date,
                services: ServiceReleaseInput::into_new(model.services),
                goal: model.goal,
                released_on_prod: model.released_on_prod.unwrap_or(false),
            },
        )
        .await?;

        read_back(&self.app, &handle)
    }
}

// ------------------------------------------------------------------------------------------ update

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleasesUpdateInput {
    #[property(description = "Which release to change, by id, e.g. `RMS-R12`")]
    pub id: String,
    #[property(description = "Rename it. Omit to leave the title alone")]
    pub title: Option<String>,
    #[property(description = "Rewrite what the release is, as Markdown. Omit to leave it alone")]
    pub description: Option<String>,
    #[property(
        description = "Rewrite the release notes, as Markdown. The whole text is replaced. Omit to leave them alone"
    )]
    pub release_notes: Option<String>,
    #[property(
        description = "Re-date it: `2026-10-07`, or `2026-10-07T14:30:00Z`. The release moves in the list at once, since the list is ordered by this. Omit to leave it alone"
    )]
    pub date: Option<String>,
    #[property(
        description = "Services to add to the release, or to CORRECT: an entry whose `microservice_id` the release already has rewrites that entry in place — its version and git_hash are replaced, and its `datetime`, `settings_update_note` and `description` only if you pass them. Anything else is added. This is how a release grows as its services go out one by one"
    )]
    pub add_services: Option<Vec<ServiceReleaseInput>>,
    #[property(
        description = "Services to take out of the release, by `microservice_id`. Applied after add_services. For one that was listed by mistake — a service whose rollout was reverted is better left in, with the fact in its `description`"
    )]
    pub remove_services: Option<Vec<String>>,
    #[property(
        description = "Pass true to MARK THE RELEASE AS OUT ON PRODUCTION — when the rollout to prod has actually happened. The moment is stamped for you, and marking one that is already marked changes nothing. Pass false to take the mark off, for a release that was pulled back from production. Omit to leave it alone. Worth a `comment` in the same call saying how the rollout went"
    )]
    pub released_on_prod: Option<bool>,
    #[property(
        description = "Pass false to UNDELETE a release somebody removed, which also puts it back on every goal that listed it. Pass true to delete it, which releases_delete also does. Omit to leave it alone"
    )]
    pub deleted: Option<bool>,
    #[property(
        description = "A note for the release's thread as part of this same change, as Markdown — how the rollout went, what was checked, why it was pulled back. Optional"
    )]
    pub comment: Option<String>,
    #[property(
        description = "Who the comment is from: an email, or the literal `AI`. Required whenever `comment` is passed"
    )]
    pub comment_by: Option<String>,
}

pub struct ReleasesUpdateHandler {
    app: Arc<AppContext>,
}

impl ReleasesUpdateHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for ReleasesUpdateHandler {
    const FUNC_NAME: &'static str = "releases_update";
    const DESCRIPTION: &'static str = "Change a release: rename it, rewrite its notes, re-date it, \
change which microservices are in it, or mark it as out on production. Only the fields you pass change.\
\
REACHING PRODUCTION IS A MARK ON THE RELEASE, NOT A SECOND RELEASE. A release is recorded when it ships \
somewhere, usually a test stand; when the same versions go out to prod, pass `released_on_prod: true` \
here rather than recording them again. That mark is what tells 'went out' from 'is live', and \
releases_list filters by it.\
\
A RELEASE NAMES A MICROSERVICE ONCE. `add_services` with an id the release already has corrects that \
entry instead of adding a second one, which is how a mistyped version or a missing settings note is \
fixed — and how a release recorded early is filled in as the rest of its services go out.\
\
WHICH GOAL A RELEASE BELONGS TO IS NOT CHANGED HERE. The goal lists its releases, so that is \
goals_update with add_releases or remove_releases.\
\
`deleted: false` brings a deleted release back. Deleting is for a release that was recorded by mistake; \
one that went out and was rolled back DID happen, and is better kept, unmarked and commented.";
}

#[async_trait::async_trait]
impl McpToolCall<ReleasesUpdateInput, ReleaseWriteResponse> for ReleasesUpdateHandler {
    async fn execute_tool_call(
        &self,
        model: ReleasesUpdateInput,
    ) -> Result<ReleaseWriteResponse, String> {
        let handle = crate::scripts::update_release(
            &self.app,
            &model.id,
            ReleasePatch {
                title: model.title,
                description: model.description,
                release_notes: model.release_notes,
                date: model.date,
                services: ServicesPatch {
                    add: ServiceReleaseInput::into_new(model.add_services),
                    remove: model.remove_services.unwrap_or_default(),
                },
                released_on_prod: model.released_on_prod,
                deleted: model.deleted,
                comment: model.comment,
                comment_by: model.comment_by,
            },
        )
        .await?;

        read_back(&self.app, &handle)
    }
}

// ---------------------------------------------------------------------------------------- comments

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleasesAddCommentInput {
    #[property(description = "Which release to add to, by id, e.g. `RMS-R12`")]
    pub id: String,
    #[property(
        description = "Who it is from: an email from users_list, or the literal `AI` when it is an agent's own note"
    )]
    pub who: String,
    #[property(
        description = "The note, as Markdown. What happened with this release: how the rollout went, what was verified, what broke, why it was pulled back"
    )]
    pub text: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleasesGetCommentsInput {
    #[property(description = "Which release's thread to read, by id, e.g. `RMS-R12`")]
    pub id: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleaseCommentsResponse {
    #[property(description = "The release the thread belongs to, by id")]
    pub id: String,
    #[property(description = "The notes, oldest first")]
    pub comments: Vec<CommentView>,
    #[property(description = "Number of rows in `comments`")]
    pub amount: i32,
}

pub struct ReleasesAddCommentHandler {
    app: Arc<AppContext>,
}

impl ReleasesAddCommentHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for ReleasesAddCommentHandler {
    const FUNC_NAME: &'static str = "releases_add_comment";
    const DESCRIPTION: &'static str = "Put a note on a release's thread. `release_notes` say what \
CHANGED; the thread says what HAPPENED — the rollout went clean, a setting was missed and added by hand, \
errors showed up an hour in, it was pulled back and why. Write it as it happens: this is what somebody \
reads months later to find out whether a version was ever really live and what it took.\
\
A comment does not move the release's `updated`.";
}

#[async_trait::async_trait]
impl McpToolCall<ReleasesAddCommentInput, ReleaseCommentsResponse> for ReleasesAddCommentHandler {
    async fn execute_tool_call(
        &self,
        model: ReleasesAddCommentInput,
    ) -> Result<ReleaseCommentsResponse, String> {
        let handle =
            crate::scripts::add_release_comment(&self.app, &model.id, &model.who, &model.text)
                .await?;

        read_comments(&self.app, &handle)
    }
}

pub struct ReleasesGetCommentsHandler {
    app: Arc<AppContext>,
}

impl ReleasesGetCommentsHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for ReleasesGetCommentsHandler {
    const FUNC_NAME: &'static str = "releases_get_comments";
    const DESCRIPTION: &'static str = "Read a release's thread, oldest first. Worth reading whenever \
`comments_amount` is not zero and you are about to roll the same versions out somewhere else, or to \
answer what became of a release: how it actually went lives here rather than in its notes.";
}

#[async_trait::async_trait]
impl McpToolCall<ReleasesGetCommentsInput, ReleaseCommentsResponse> for ReleasesGetCommentsHandler {
    async fn execute_tool_call(
        &self,
        model: ReleasesGetCommentsInput,
    ) -> Result<ReleaseCommentsResponse, String> {
        read_comments(&self.app, &model.id)
    }
}

fn read_comments(app: &AppContext, handle: &str) -> Result<ReleaseCommentsResponse, String> {
    let board = app.board.read();
    let resolved = crate::scripts::resolve_release_by_handle(&board, handle)?;

    let comments: Vec<CommentView> = resolved
        .release
        .comments
        .iter()
        .map(|itm| CommentView {
            moment_unix_seconds: itm.moment.unix_microseconds / 1_000_000,
            who: itm.who.clone(),
            text: itm.text.clone(),
        })
        .collect();

    Ok(ReleaseCommentsResponse {
        id: crate::board::compose_release_handle(&resolved.project.prefix, resolved.release.number),
        amount: comments.len() as i32,
        comments,
    })
}

// ------------------------------------------------------------------------------------------ delete

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleasesDeleteInput {
    #[property(description = "Which release to delete, by id — `RMS-R12`")]
    pub id: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleasesDeleteResponse {
    #[property(description = "The id of the release that was deleted")]
    pub id: String,
}

pub struct ReleasesDeleteHandler {
    app: Arc<AppContext>,
}

impl ReleasesDeleteHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for ReleasesDeleteHandler {
    const FUNC_NAME: &'static str = "releases_delete";
    const DESCRIPTION: &'static str = "Delete a release that should never have been recorded — a \
duplicate, or one written against the wrong project.\
\
NOT FOR A RELEASE THAT WAS ROLLED BACK. That one happened: it went out, and then it was taken back, and \
both halves are worth knowing months later. Keep it, take its production mark off with releases_update \
if it had one, and say what became of it on its thread.\
\
IT IS A FLAG, NOT A REMOVAL. The release leaves releases_list and every goal that listed it, and stays \
reachable by its id, which reports it as deleted. The goals are not edited — they simply stop showing it \
— so undoing this with releases_update and `deleted: false` puts it back on them as well. Its number is \
never reused either way.";
}

#[async_trait::async_trait]
impl McpToolCall<ReleasesDeleteInput, ReleasesDeleteResponse> for ReleasesDeleteHandler {
    async fn execute_tool_call(
        &self,
        model: ReleasesDeleteInput,
    ) -> Result<ReleasesDeleteResponse, String> {
        let id = crate::scripts::delete_release(&self.app, &model.id).await?;

        Ok(ReleasesDeleteResponse { id })
    }
}
