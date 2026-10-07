use std::rc::Rc;

use dioxus::prelude::*;
use task_manager_shared::column_templates::ColumnTemplateResponse;
use task_manager_shared::documents::UploadArchiveResponse;
use task_manager_shared::kind_templates::KindTemplateResponse;
use task_manager_shared::goals::GoalResponse;
use task_manager_shared::project_transfer::ImportProjectResponse;
use task_manager_shared::templates_transfer::ImportTemplatesResponse;
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::tasks::FindTaskResponse;

mod checklist;
pub use checklist::*;
mod dialog_template;
pub use dialog_template::*;
mod edit_column_template;
pub use edit_column_template::*;
mod edit_kind_template;
pub use edit_kind_template::*;
mod edit_members;
pub use edit_members::*;
mod edit_project;
pub use edit_project::*;
mod github_connections;
pub use github_connections::*;
mod github_key;
pub use github_key::*;
mod import_project;
pub use import_project::*;
mod import_templates;
pub use import_templates::*;
mod sync_github;
pub use sync_github::*;
mod land_task;
pub use land_task::*;
mod md;
pub use md::*;
mod message;
pub use message::*;
mod refresh_github;
pub use refresh_github::*;
mod release_details;
pub use release_details::*;
mod upload_document;
pub use upload_document::*;
mod view_document;
pub use view_document::*;
mod view_goal;
pub use view_goal::*;
mod view_task;
pub use view_task::*;

/// Which dialog is open, if any.
///
/// A context signal of its own rather than a field of `AppState`. Dioxus subscribes per signal, not per
/// field: opening a dialog would re-run every reactive scope that reads `AppState`, and Home's board
/// re-read is one of them — it reads `board_revision` from there. A separate signal keeps opening a
/// dialog from refetching a board.
///
/// **Every dialog follows one pattern**: it is handed a model, it builds a new model as you edit, its Save
/// lights up only when the two differ, and pressing Save hands the new model out through an
/// `EventHandler`. The handler — owned by the page, not the dialog — makes the request and refreshes.
/// No dialog calls an API and none of them touch a page's state, which is why none of them can leave a
/// half-applied edit behind: nothing is sent until Save, and what is sent is the whole thing.
#[derive(Clone)]
pub enum DialogState {
    None,
    /// `project: None` is a new project. The form is the same either way; only the title and which API
    /// call runs differ, so it is one dialog rather than two nearly identical ones.
    EditProject {
        project: Option<Rc<ProjectResponse>>,
        on_saved: EventHandler<()>,
    },
    EditMembers {
        project: Rc<ProjectResponse>,
        on_saved: EventHandler<()>,
    },
    /// Columns and task types are configured here — once per template, under Settings — and never on a
    /// project, which only points at one of each. `template: None` creates.
    EditColumnTemplate {
        template: Option<Rc<ColumnTemplateResponse>>,
        on_saved: EventHandler<()>,
    },
    EditKindTemplate {
        template: Option<Rc<KindTemplateResponse>>,
        on_saved: EventHandler<()>,
    },
    /// One task, looked up by id. Read-only — it has no `on_saved` because there is nothing to save.
    ViewTask {
        found: FindTaskResponse,
    },
    /// Upload a file into a project's documents — the one dialog here that writes anything but a colour. It
    /// is handed the folders that already exist so it can offer them; creating one is typing a path, because
    /// a folder is not a thing that exists until a document is in it.
    ///
    /// `initial_folder` is where the reader currently is in the tree, so an upload starts there instead of at
    /// the root — the folder they were looking at is overwhelmingly the folder they mean, and it stays fully
    /// editable.
    UploadDocument {
        project: String,
        initial_folder: String,
        folders: Vec<String>,
        on_uploaded: EventHandler<()>,
    },
    /// Pour an exported project into this one — the other half of the Export link beside it on the setup
    /// screen. Carries the prefix and the name rather than the whole `ProjectResponse`: the dialog draws the
    /// name so the reader can see which board they are about to change, and sends the prefix.
    ImportProject {
        project: String,
        project_name: String,
        on_imported: EventHandler<()>,
    },
    /// Apply a templates YAML file — the other half of the Export link beside it on Settings. Carries
    /// nothing: templates are instance-wide, so there is nothing to scope it to.
    ImportTemplates {
        on_imported: EventHandler<()>,
    },
    /// The GitHub repositories connected to one project, listed and edited in one place.
    ///
    /// **`revision` is what makes the list refresh.** This dialog is the one that performs several
    /// different calls without closing — connect, detach, hand over a key — so after each one the router
    /// re-opens it with the number bumped, and the dialog reloads on the change. A dialog that closed
    /// after every act would make configuring three repositories nine gestures.
    GithubConnections {
        project: String,
        revision: usize,
        on_saved: EventHandler<()>,
    },
    /// Copy files out of a connected repository into the project's own documents. Handed the mirrors as
    /// the Documents screen already has them — the tree it draws IS the index it loaded, so the dialog
    /// costs no request and cannot disagree with what the reader is looking at.
    SyncGithub {
        project: String,
        mirrors: Vec<MirrorChoice>,
        folders: Vec<String>,
        initial_folder: String,
        on_synced: EventHandler<()>,
    },
    /// Hand the server a key for one connected repository, opened from the tree row that says it needs
    /// one — the fix belongs on the screen where the symptom is.
    GithubKey {
        project: String,
        connection: String,
        on_saved: EventHandler<()>,
    },
    /// Read connected repositories from GitHub again — one, or every one of a project's — and watch the
    /// reading to its end rather than closing on "it was asked for".
    ///
    /// **`runs` is what makes it a watch rather than a wish.** The dialog opens with `None`; pressing
    /// Refresh sends the router off to ask for a listing per connection, and each ask comes back with the
    /// number that connection's finished listings stood at. The router then re-opens this dialog carrying
    /// them, exactly as `GithubConnections` re-opens itself with a bumped revision, and the dialog polls
    /// until every one of those numbers has been passed.
    ///
    /// **`attempt` is why a second press is a second watch.** The dialog wakes on its props CHANGING, and
    /// two presses can hand back the same numbers: a refresh that was given up on and tried again reports
    /// the same baseline precisely because nothing has finished since. Without a number that always moves,
    /// that second press would start nothing and the dialog would sit there reading `Reading…` for ever.
    RefreshGithub {
        project: String,
        connections: Vec<String>,
        attempt: usize,
        runs: Option<Vec<GithubRefreshRun>>,
        on_finished: EventHandler<()>,
    },
    /// One goal, in full: its text and its thread. Handed the whole goal rather than an id, because the
    /// screen that opens it is already holding one — a goal arrives with the board on every push.
    ///
    /// `status` comes with it for the same reason and one more: a goal's status is derived from the TASKS
    /// under it, which the Goals screen holds and this dialog does not — see `goal_status` there.
    ViewGoal {
        goal: GoalResponse,
        status: String,
    },
    /// One document, opened from the task or the goal that references it.
    ///
    /// **`back` is the dialog to return to, carried whole.** Re-opening the card by id would be a request,
    /// and it would land on whatever that card looks like now rather than on what the reader was reading —
    /// which for a dialog somebody is halfway through is the wrong answer even when it is fresher. It is
    /// boxed because a `DialogState` that contained itself by value would have no size.
    ///
    /// `project` is the board of the card it was opened from, and it is the fallback rather than the
    /// answer: a reference names its own board, and only the legacy bare-id spelling names none.
    ViewDocument {
        project: String,
        reference: String,
        back: Box<DialogState>,
    },
    /// A card was dragged into Done and the server will refuse the move without a resolution. Carries the
    /// column it is being dropped into rather than assuming `done`: a project can only have one Done, but
    /// spelling it out keeps the dialog from knowing which id that is.
    LandTask {
        handle: String,
        status: String,
    },
    /// Something the board tried failed, said in the caller's own words. Not a toast: a refused move is a
    /// sentence worth reading — "RMS-42 is part of RMS-G7, which is closed" — and a message that fades is a
    /// message half the readers miss.
    Message {
        title: String,
        text: String,
    },
}

/// Mounted once, at the top of the signed-in shell, so a dialog overlays whatever screen opened it.
///
/// This is also where a dialog's result is turned into a request: the submit handlers live here so the
/// dialogs stay pure and every caller gets the same behaviour — save, close on success, report on failure.
#[component]
pub fn RenderDialog() -> Element {
    let state = consume_context::<Signal<DialogState>>().read().clone();

    match state {
        DialogState::None => rsx! {},
        DialogState::EditProject { project, on_saved } => rsx! {
            EditProjectDialog { project, on_saved }
        },
        DialogState::EditMembers { project, on_saved } => rsx! {
            EditMembersDialog { project, on_saved }
        },
        DialogState::ViewTask { found } => rsx! {
            ViewTaskDialog { found }
        },
        DialogState::ViewGoal { goal, status } => rsx! {
            ViewGoalDialog { goal, status }
        },
        DialogState::ViewDocument {
            project,
            reference,
            back,
        } => {
            // Cloned per call rather than moved: an `EventHandler` is callable more than once, and a
            // reader who goes back, follows a second reference and goes back again is the ordinary way
            // this dialog is used.
            let back = *back;

            rsx! {
                ViewDocumentDialog {
                    project,
                    reference,
                    on_back: EventHandler::new(move |_| open(back.clone())),
                }
            }
        }
        DialogState::UploadDocument {
            project,
            initial_folder,
            folders,
            on_uploaded,
        } => rsx! {
            UploadDocumentDialog {
                project,
                initial_folder,
                folders,
                on_submit: move |submit: UploadSubmit| {
                    begin_submit();
                    spawn(async move {
                        match submit {
                            UploadSubmit::Document { project, path, bytes, content_type } => {
                                match crate::api::upload_document(&project, &path, &bytes, content_type)
                                    .await
                                {
                                    Ok(_) => {
                                        on_uploaded.call(());
                                        close();
                                    }
                                    Err(err) => submit_failed(err.message),
                                }
                            }
                            UploadSubmit::Archive { project, folder, bytes } => {
                                match crate::api::upload_archive(&project, &folder, &bytes).await {
                                    Ok(response) => {
                                        on_uploaded.call(());

                                        // An archive can half-succeed — a `.DS_Store`, an entry over the size
                                        // limit — and a dialog that just closed would say every file arrived.
                                        // So the skips replace it with a report; a clean unpack closes as any
                                        // other save does.
                                        match archive_report(&response) {
                                            Some(text) => {
                                                open(DialogState::Message {
                                                    title: "Unpacked".to_string(),
                                                    text,
                                                });
                                            }
                                            None => close(),
                                        }
                                    }
                                    Err(err) => submit_failed(err.message),
                                }
                            }
                        }
                    });
                },
            }
        },
        DialogState::ImportProject {
            project,
            project_name,
            on_imported,
        } => rsx! {
            ImportProjectDialog {
                project,
                project_name,
                on_submit: move |submit: ImportSubmit| {
                    begin_submit();
                    spawn(async move {
                        match crate::api::import_project(&submit.project, submit.bytes).await {
                            Ok(response) => {
                                on_imported.call(());

                                // Always a report, never a silent close — unlike every other save here. An
                                // import is the one act on this screen whose result nobody can see by
                                // looking: the board it changed is not the screen it was pressed on, and
                                // "42 tasks arrived" is the only way to know it did what was meant.
                                open(DialogState::Message {
                                    title: "Imported".to_string(),
                                    text: import_report(&response),
                                });
                            }
                            Err(err) => submit_failed(err.message),
                        }
                    });
                },
            }
        },
        DialogState::ImportTemplates { on_imported } => rsx! {
            ImportTemplatesDialog {
                on_submit: move |bytes: Vec<u8>| {
                    begin_submit();
                    spawn(async move {
                        match crate::api::import_templates(bytes).await {
                            Ok(response) => {
                                on_imported.call(());

                                // Always a report, never a silent close — same reason the project import
                                // gives one: what changed is configuration every board follows, and the
                                // counts are the only place "it replaced two templates in use" is said.
                                open(DialogState::Message {
                                    title: "Templates imported".to_string(),
                                    text: templates_report(&response),
                                });
                            }
                            Err(err) => submit_failed(err.message),
                        }
                    });
                },
            }
        },
        DialogState::GithubConnections {
            project,
            revision,
            on_saved,
        } => rsx! {
            GithubConnectionsDialog {
                project: project.clone(),
                revision,
                on_submit: move |submit: GithubSubmit| {
                    let project = project.clone();
                    begin_submit();
                    spawn(async move {
                        let result = match submit {
                            GithubSubmit::Save { name, url, branch, path, key } => {
                                crate::api::set_github_connection(
                                        &project,
                                        &name,
                                        &url,
                                        &branch,
                                        &path,
                                        key,
                                    )
                                    .await
                            }
                            GithubSubmit::Delete { name } => {
                                crate::api::delete_github_connection(&project, &name).await
                            }
                            GithubSubmit::SetKey { name, key } => {
                                crate::api::set_github_key(&project, &name, &key).await
                            }
                        };
                        match result {
                            Ok(()) => {
                                on_saved.call(());
                                // Re-opened rather than closed: configuring repositories is several acts
                                // in a row, and the bumped revision is what makes the list show the one
                                // just performed. Connecting one and handing over a key both start a
                                // listing, so a row that says `reading…` here is that, landing.
                                open(DialogState::GithubConnections {
                                    project,
                                    revision: revision + 1,
                                    on_saved,
                                });
                            }
                            Err(err) => submit_failed(err.message),
                        }
                    });
                },
            }
        },
        DialogState::SyncGithub {
            project,
            mirrors,
            folders,
            initial_folder,
            on_synced,
        } => rsx! {
            SyncGithubDialog {
                project,
                mirrors,
                folders,
                initial_folder,
                on_submit: move |submit: SyncGithubSubmit| {
                    begin_submit();
                    spawn(async move {
                        match crate::api::sync_github(
                                &submit.project,
                                &submit.connection,
                                submit.paths,
                                &submit.folder,
                                submit.override_existing,
                            )
                            .await
                        {
                            Ok(response) => {
                                on_synced.call(());
                                // A sync half-succeeds in the ordinary case rather than the exceptional
                                // one — without Override, everything already there is skipped, and that
                                // IS the outcome the reader asked for. Closing silently would say every
                                // file arrived.
                                match sync_report(&response) {
                                    Some(text) => {
                                        open(DialogState::Message {
                                            title: "Synced".to_string(),
                                            text,
                                        });
                                    }
                                    None => close(),
                                }
                            }
                            Err(err) => submit_failed(err.message),
                        }
                    });
                },
            }
        },
        DialogState::GithubKey {
            project,
            connection,
            on_saved,
        } => rsx! {
            GithubKeyDialog {
                connection: connection.clone(),
                on_submit: move |key: String| {
                    let project = project.clone();
                    let connection = connection.clone();
                    begin_submit();
                    spawn(async move {
                        match crate::api::set_github_key(&project, &connection, &key).await {
                            // The server starts reading the repository the moment it has the key, so
                            // this closes on "it was accepted" rather than on "the files are here" —
                            // `on_saved` re-reads the connection so the row moves to `reading…`.
                            Ok(()) => {
                                on_saved.call(());
                                close();
                            }
                            Err(err) => submit_failed(err.message),
                        }
                    });
                },
            }
        },
        DialogState::RefreshGithub {
            project,
            connections,
            attempt,
            runs,
            on_finished,
        } => rsx! {
            RefreshGithubDialog {
                project: project.clone(),
                connections: connections.clone(),
                attempt,
                runs,
                on_finished,
                on_submit: move |_| {
                    let project = project.clone();
                    let connections = connections.clone();
                    begin_submit();
                    spawn(async move {
                        // One ask per connection, each one's receipt kept beside its name. Sequential
                        // because the receipts have to line up with the rows and a refusal has to stop
                        // the rest — the listings themselves are started on the server and overlap there
                        // whatever order these went out in.
                        let mut runs = Vec::with_capacity(connections.len());
                        for connection in connections.iter() {
                            match crate::api::pull_github_connection(&project, connection).await {
                                Ok(response) => {
                                    runs.push(GithubRefreshRun {
                                        connection: connection.clone(),
                                        pull_no: response.pull_no,
                                    });
                                }
                                // Nothing was started for this one, and what was started for the ones
                                // before it goes on running — the dialog stays where it is with the
                                // reason on it, rather than watching a set it cannot describe.
                                Err(err) => return submit_failed(err.message),
                            }
                        }
                        // Re-opened rather than closed, and this is the whole gesture: the listing is
                        // running, and the dialog re-opens holding what to watch it by. `open` clears the
                        // feedback, so the button comes back out of `Asking…` by itself.
                        open(DialogState::RefreshGithub {
                            project,
                            connections,
                            attempt: attempt + 1,
                            runs: Some(runs),
                            on_finished,
                        });
                    });
                },
            }
        },
        DialogState::Message { title, text } => rsx! {
            MessageDialog { title, text }
        },
        DialogState::LandTask { handle, status } => rsx! {
            LandTaskDialog {
                handle: handle.clone(),
                on_submit: move |comment: String| {
                    let handle = handle.clone();
                    let status = status.clone();
                    begin_submit();
                    spawn(async move {
                        match crate::api::move_task(&handle, &status, Some(&comment)).await {
                            // Nothing to refresh: the move comes back as a WebSocket push carrying the whole
                            // board, which is also why this does not write the task back by hand.
                            Ok(()) => close(),
                            Err(err) => submit_failed(err.message),
                        }
                    });
                },
            }
        },
        DialogState::EditKindTemplate { template, on_saved } => rsx! {
            EditKindTemplateDialog {
                template,
                on_submit: move |submit: KindTemplateSubmit| {
                    begin_submit();
                    spawn(async move {
                        match crate::api::save_kind_template(
                                &submit.id,
                                &submit.name,
                                &submit.description,
                                submit.kinds,
                            )
                            .await
                        {
                            Ok(()) => {
                                on_saved.call(());
                                close();
                            }
                            Err(err) => submit_failed(err.message),
                        }
                    });
                },
            }
        },
        DialogState::EditColumnTemplate { template, on_saved } => rsx! {
            EditColumnTemplateDialog {
                template,
                on_submit: move |submit: ColumnTemplateSubmit| {
                    begin_submit();
                    spawn(async move {
                        match crate::api::save_column_template(
                                &submit.id,
                                &submit.name,
                                &submit.description,
                                submit.columns,
                            )
                            .await
                        {
                            Ok(()) => {
                                on_saved.call(());
                                close();
                            }
                            Err(err) => submit_failed(err.message),
                        }
                    });
                },
            }
        },
    }
}

/// What to tell the reader after an archive was unpacked, or `None` when everything in it landed.
///
/// **Silence on a clean unpack, a sentence otherwise.** An upload that half-worked is the one outcome this
/// surface could get wrong without anybody noticing: the tree refreshes either way, and a file that was never
/// written looks exactly like a file nobody put in the zip. So a skip is said out loud, with the reason the
/// server gave, and named by what the entry was called INSIDE the archive — the path it would have had is
/// precisely what does not exist.
///
/// One paragraph rather than lines: the dialog that shows it renders text as text, and a newline there is a
/// space.
fn archive_report(response: &UploadArchiveResponse) -> Option<String> {
    if response.skipped.is_empty() {
        return None;
    }

    let skipped: Vec<String> = response
        .skipped
        .iter()
        .map(|itm| format!("{} — {}", itm.name, itm.reason))
        .collect();

    Some(format!(
        "{} written. {} skipped: {}.",
        count_of(response.documents.len(), "document"),
        skipped.len(),
        skipped.join("; ")
    ))
}

/// What to tell the reader after a sync, or `None` when everything chosen landed.
///
/// **Skips are the normal outcome here, not the exceptional one.** With Override off, every file already
/// in the project is skipped on purpose — which is exactly what was asked for, and exactly what would
/// look like a silent failure if the dialog just closed. So the count is always said when there is one,
/// and the reasons are said for the first few: forty identical "already there" lines are not forty
/// pieces of information.
fn sync_report(response: &UploadArchiveResponse) -> Option<String> {
    if response.skipped.is_empty() {
        return None;
    }

    const REASONS_SHOWN: usize = 5;

    let reasons: Vec<String> = response
        .skipped
        .iter()
        .take(REASONS_SHOWN)
        .map(|itm| format!("{} — {}", itm.name, itm.reason))
        .collect();

    let rest = response.skipped.len().saturating_sub(reasons.len());

    let tail = if rest > 0 {
        format!("; and {rest} more")
    } else {
        String::new()
    };

    Some(format!(
        "{} written. {} skipped: {}{tail}.",
        count_of(response.documents.len(), "document"),
        response.skipped.len(),
        reasons.join("; ")
    ))
}

/// What to tell the reader after an import.
///
/// **Always something, even when everything landed** — which is what makes this different from the archive
/// report above. An unpacked zip can be seen in the tree the reader is looking at; an import changes a board
/// that is not on this screen, so the counts ARE the result. The skips and the notes come after them, capped
/// the way a sync's are: forty identical lines are not forty pieces of information.
fn import_report(response: &ImportProjectResponse) -> String {
    const REASONS_SHOWN: usize = 5;

    let mut text = format!(
        "{}, {}, {}, {} and {} imported.",
        count_of(response.goals as usize, "goal"),
        count_of(response.tasks as usize, "task"),
        count_of(response.comments as usize, "comment"),
        count_of(response.releases as usize, "release"),
        count_of(response.documents as usize, "document"),
    );

    // The notes first: they are about what landed, and they are the half somebody has to act on — a column
    // this project has no template for is a setting to fix, not an entry to re-send.
    for note in response.notes.iter() {
        text.push_str(&format!(" {note}."));
    }

    if !response.skipped.is_empty() {
        let reasons: Vec<String> = response
            .skipped
            .iter()
            .take(REASONS_SHOWN)
            .map(|itm| format!("{} — {}", itm.name, itm.reason))
            .collect();

        let rest = response.skipped.len().saturating_sub(reasons.len());

        let tail = if rest > 0 {
            format!("; and {rest} more")
        } else {
            String::new()
        };

        text.push_str(&format!(
            " {} skipped: {}{tail}.",
            response.skipped.len(),
            reasons.join("; ")
        ));
    }

    text
}

/// What to tell the reader after a templates import.
///
/// **Created and replaced are said apart**, because they are not the same event: creating a template
/// affects nothing, where replacing one changes every project that follows it. The `notes` that follow say
/// how many projects that was, per template — which is the sentence somebody needs before they go looking
/// at a board that suddenly has different columns.
fn templates_report(response: &ImportTemplatesResponse) -> String {
    const REASONS_SHOWN: usize = 5;

    let mut parts: Vec<String> = Vec::new();

    for (created, replaced, noun) in [
        (
            response.column_templates_created,
            response.column_templates_replaced,
            "column template",
        ),
        (
            response.kind_templates_created,
            response.kind_templates_replaced,
            "task-type template",
        ),
    ] {
        if created > 0 {
            parts.push(format!("{} created", count_of(created as usize, noun)));
        }

        if replaced > 0 {
            parts.push(format!("{} replaced", count_of(replaced as usize, noun)));
        }
    }

    let mut text = if parts.is_empty() {
        "Nothing was applied.".to_string()
    } else {
        format!("{}.", parts.join(", "))
    };

    for note in response.notes.iter() {
        text.push_str(&format!(" {note}."));
    }

    if !response.skipped.is_empty() {
        let reasons: Vec<String> = response
            .skipped
            .iter()
            .take(REASONS_SHOWN)
            .map(|itm| format!("{} — {}", itm.name, itm.reason))
            .collect();

        let rest = response.skipped.len().saturating_sub(reasons.len());

        let tail = if rest > 0 {
            format!("; and {rest} more")
        } else {
            String::new()
        };

        text.push_str(&format!(
            " {} skipped: {}{tail}.",
            response.skipped.len(),
            reasons.join("; ")
        ));
    }

    text
}

fn count_of(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// How a submit went, for the dialogs whose request is made by the router rather than by themselves.
///
/// Without this a failed save is invisible: the dialog has already disabled its Save button and has no way
/// to learn the request came back, so it sits there looking busy for ever. The router writes the outcome
/// here and the dialog reads it — which keeps the dialog free of API calls without making a failure
/// disappear.
#[derive(Clone, Default, PartialEq)]
pub struct DialogFeedback {
    pub saving: bool,
    pub error: String,
}

pub fn feedback() -> DialogFeedback {
    consume_context::<Signal<DialogFeedback>>().read().clone()
}

fn begin_submit() {
    consume_context::<Signal<DialogFeedback>>().set(DialogFeedback {
        saving: true,
        error: String::new(),
    });
}

fn submit_failed(message: String) {
    consume_context::<Signal<DialogFeedback>>().set(DialogFeedback {
        saving: false,
        error: message,
    });
}

/// Open a dialog from anywhere with a `Signal<DialogState>` in context.
pub fn open(state: DialogState) {
    consume_context::<Signal<DialogFeedback>>().set(DialogFeedback::default());
    consume_context::<Signal<DialogState>>().set(state);
}
