use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::{CommentView, ReleaseView, ServiceReleaseInput};
use crate::scripts::{EnvsPatch, NewRelease, ReleasePatch, ServicesPatch};

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
        description = "Only the releases that are out on this environment, by its label — `Prod`. Case does not matter. With `microservice_id` and the default order, `Prod` answers 'which version of this service is live' in one row. The labels a project uses come back in `envs` on every listing. Omit for releases wherever they are"
    )]
    pub env: Option<String>,
    #[property(
        description = "Only the releases that are NOT out on this environment, by its label. `Prod` here is what has been recorded and has not reached production yet — the list of what is still to be rolled out. May be combined with `env`: `env: Dev` and `not_on_env: Prod` is what sits on Dev waiting. Omit for releases wherever they are"
    )]
    pub not_on_env: Option<String>,
    #[property(
        description = "Pass false for ONLY the releases that are still going out — not closed yet, the ones somebody still has to do something about. Pass true for only the closed ones. Omit for both"
    )]
    pub done: Option<bool>,
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
    #[property(
        description = "Every environment label the releases of this project carry, once each — the project's vocabulary, whatever the filters. REUSE THESE SPELLINGS when you put a release on an environment: a label that matches one in any case is stored in the spelling shown here, and only one the project has never used is taken as you wrote it. Empty means no release of the project says where it is out"
    )]
    pub envs: Vec<String>,
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
service, or by `env` for what is out on one environment — `Prod` for what has actually reached \
production. A release is recorded when it ships anywhere, so the unfiltered list is everything that went \
out, not everything that is live; each release says where it is in its own `envs`, and `not_on_env` \
turns the question round to what has not got there yet. `done: false` is the releases whose rollout is \
not over — the short list of what still needs somebody.\
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

        let env = model
            .env
            .as_deref()
            .map(str::trim)
            .filter(|itm| !itm.is_empty());

        let not_on_env = model
            .not_on_env
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
            .filter(|release| match env {
                Some(wanted) => release.is_on_env(wanted),
                None => true,
            })
            .filter(|release| match not_on_env {
                Some(unwanted) => !release.is_on_env(unwanted),
                None => true,
            })
            .filter(|release| match model.done {
                Some(wanted) => release.is_done() == wanted,
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
            // The project's, not the filtered list's: it is the vocabulary to write with, and a caller
            // who filtered down to one environment still needs the names of the others.
            envs: board.envs_of_project(&project.id),
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
        description = "One entry per microservice this release touched: its `microservice_id`, the `version` that went out, the `git_hash` that version was built from, and optionally `release_link`, `datetime`, `settings_update_note` and `description`. A release of one feature usually touches several services — list them all here rather than recording a release each. May be omitted and filled in with releases_update as the services go out"
    )]
    pub services: Option<Vec<ServiceReleaseInput>>,
    #[property(
        description = "The goal this release ships, by goal id — `RMS-G7`. Attached in this same call, so the goal lists the release from the moment it exists. The goal is normally the description of the very feature being released; it may be open or already closed. Omit only when the release belongs to no goal"
    )]
    pub goal: Option<String>,
    #[property(
        description = "The environments this release is ALREADY out on as you record it, as labels — normally the one stand it has just been rolled to, like `Dev`. One word each. Spell them as `envs` on releases_list reports the project's: a label that matches one of those in any case is stored in that spelling. Omit when it has not gone anywhere yet; the rest are added with add_envs on releases_update as the release reaches them — production last, and only once it has actually been rolled"
    )]
    pub envs: Option<Vec<String>>,
    #[property(
        description = "Pass true ONLY if the rollout is already over as you record it — the release is on every environment it is going to. That is the case for one written down after the fact. Omit otherwise: a release normally has somewhere still to go, and is closed later with `done: true` on releases_update"
    )]
    pub done: Option<bool>,
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
the microservice, its version, the commit that version was built from, and when. When CI built what \
went out, give each service its `release_link` — the url of the GitHub release or of the workflow run \
that built the image — so the build is one click from the record of it.\
\
SAY WHERE IT IS OUT. `envs` are the environments the release is on, as labels — `Dev`, `Prod`. Pass the \
one it has just been rolled to; reaching the next is a label added to this same release with \
releases_update, never a second release.\
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
                envs: model.envs.unwrap_or_default(),
                done: model.done.unwrap_or(false),
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
        description = "Services to add to the release, or to CORRECT: an entry whose `microservice_id` the release already has rewrites that entry in place — its version and git_hash are replaced, and its `release_link`, `datetime`, `settings_update_note` and `description` only if you pass them. Anything else is added. This is how a release grows as its services go out one by one"
    )]
    pub add_services: Option<Vec<ServiceReleaseInput>>,
    #[property(
        description = "Services to take out of the release, by `microservice_id`. Applied after add_services. For one that was listed by mistake — a service whose rollout was reverted is better left in, with the fact in its `description`"
    )]
    pub remove_services: Option<Vec<String>>,
    #[property(
        description = "Environments the release has REACHED, as labels to put on it — `Prod` when the rollout to production has actually happened. One word each. Spell them as `envs` on releases_list reports the project's; a label that matches one of those in any case is stored in that spelling, and one the release already carries changes nothing. Worth a `comment` in the same call saying how the rollout went"
    )]
    pub add_envs: Option<Vec<String>>,
    #[property(
        description = "Environments to take the release OFF, by label, in any case — for one that was pulled back from there. Applied after add_envs, so a label passed to both ends up removed. A label the release does not carry is not an error"
    )]
    pub remove_envs: Option<Vec<String>>,
    #[property(
        description = "Pass true to CLOSE the release: its rollout is over — it has reached every environment it is going to. The moment is stamped for you, and closing one that is already closed changes nothing. Pass false to reopen it, for one that turned out to have somewhere still to go. Omit to leave it alone. Normally passed in the same call as the last `add_envs`, with a `comment` saying how the rollout went"
    )]
    pub done: Option<bool>,
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
change which microservices are in it, say which environments it is out on, or close it. Only the fields \
you pass change.\
\
REACHING AN ENVIRONMENT IS A LABEL ON THE RELEASE, NOT A SECOND RELEASE. A release is recorded when it \
ships somewhere, usually a test stand; when the same versions go out to production, pass `Prod` in \
`add_envs` here rather than recording them again. Its `envs` are what tell 'went out' from 'is live', \
and releases_list filters by them. `remove_envs` takes a label off, for a release pulled back \
from an environment.\
\
CLOSE A RELEASE WHEN ITS ROLLOUT IS OVER. `done: true` says it has reached every environment it is going \
to and nothing more will happen to it; that is what takes it off the list of releases somebody still has \
to do something about. It is your statement — nothing here knows which environments a project has, so it \
is not worked out from `envs` — and `done: false` reopens one that turned out to have somewhere still to \
go.\
\
A RELEASE NAMES A MICROSERVICE ONCE. `add_services` with an id the release already has corrects that \
entry instead of adding a second one, which is how a mistyped version or a missing settings note is \
fixed — and how a release recorded early is filled in as the rest of its services go out.\
\
WHICH GOAL A RELEASE BELONGS TO IS NOT CHANGED HERE. The goal lists its releases, so that is \
goals_update with add_releases or remove_releases.\
\
`deleted: false` brings a deleted release back. Deleting is for a release that was recorded by mistake; \
one that went out and was rolled back DID happen, and is better kept, taken off the environment it was \
pulled back from and commented.";
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
                envs: EnvsPatch {
                    add: model.add_envs.unwrap_or_default(),
                    remove: model.remove_envs.unwrap_or_default(),
                },
                done: model.done,
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
both halves are worth knowing months later. Keep it, take it off the environments it was pulled back \
from with remove_envs on releases_update, and say what became of it on its thread.\
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
