use std::io::Write;
use std::path::{Path, PathBuf};

use rust_extensions::AsStr;
use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;

use crate::app::AppContext;
use crate::board::{
    GoalModel, ProjectModel, ReleaseModel, TaskModel, compose_goal_handle, compose_release_handle,
    compose_task_handle,
};

use super::models::*;

/// The most one export may carry, added up across the raw bytes of its documents.
///
/// A ceiling on the temp file and on the time somebody waits for it, not a judgement about the board. It is
/// generous — a board of Markdown is a few megabytes, and this is two orders of magnitude past that — so
/// reaching it means somebody has a project full of binaries, and the export says so instead of filling the
/// container's disk and then failing on a write.
pub const MAX_EXPORT_DOCUMENT_BYTES: u64 = 512 * 1024 * 1024;

/// A finished export, sitting on disk.
///
/// **The file rather than the bytes, and that is the whole shape of this feature.** A project's documents are
/// its real payload — PDFs, images, whatever somebody uploaded — and holding an archive of them in memory to
/// hand to a response would mean the service's footprint is set by the largest board anybody exports. So the
/// zip is built into the temp directory as it is read out of Postgres, one document at a time, and the
/// response streams it back off disk. Nothing bigger than one document is ever in memory at once.
///
/// The file is the caller's to delete — see `stream_and_remove` in the action, which removes it whether the
/// download finished or the reader closed the tab half way through it.
pub struct ExportedFile {
    pub path: PathBuf,
    /// What the browser saves it as.
    pub file_name: String,
    pub size: u64,
}

/// Write one project into a zip in the temp directory.
///
/// The archive holds one YAML file per kind of thing and a `documents/` folder — see [`super::models`] for the
/// shape of each. What goes in is the whole board: every goal, every task, every comment on either, every
/// release, and every document the project owns.
///
/// **Deleted work is exported too.** A deleted task is hidden from every screen and kept so that searching
/// for its id still finds it; leaving it out would make an export a quiet way of losing the record, and a
/// hand-editable `tasks.yaml` is a better answer for anybody who wants it gone than a decision made here.
/// A deleted release has one reason more: the goals that listed it still do, so that bringing it back puts
/// it on them again, and that only holds on the other board if the release is there to bring back.
///
/// **A connected repository is not.** Documents under the reserved `github/` root are files in a working copy
/// on disk, mirrored from a repository that is still there — they are not this project's to carry, and the
/// receiving board gets them by connecting the same repository. They are not in the documents index, so
/// nothing here has to exclude them; the check below is a backstop, not a filter.
pub async fn export_project(app: &AppContext, project_prefix: &str) -> Result<ExportedFile, String> {
    // Everything read off the board happens here, in one block: `parking_lot`'s guard is `!Send`, so a read
    // held across the awaits below would not compile — which is the compiler enforcing what we want anyway.
    let (project, goals, tasks, releases) = {
        let board = app.board.read();
        let project = super::super::resolve_project_by_prefix(&board, project_prefix)?;

        // Deleted work included — see the note on this function. By number on both, which is the order the
        // file reads best in and is also oldest-first, since the counter only goes up.
        let mut goals: Vec<GoalModel> = board
            .goals_of_project_including_deleted(&project.id)
            .iter()
            .map(|itm| itm.as_ref().clone())
            .collect();

        goals.sort_by_key(|itm| itm.number);

        let mut tasks: Vec<TaskModel> = board
            .tasks_of_project(&project.id)
            .iter()
            .map(|itm| itm.as_ref().clone())
            .collect();

        tasks.sort_by_key(|itm| itm.number);

        // By number here too, and for a release that is more than how the file reads. Every list of
        // releases is newest first by their DATE, and the number is what orders two of one date — the
        // later number is the one written down later. The import hands numbers out in file order, so the
        // file has to be in the order the numbers were: written the way those lists read, two releases of
        // one day would arrive the other way round.
        let mut releases: Vec<ReleaseModel> = board
            .releases_of_project_including_deleted(&project.id)
            .iter()
            .map(|itm| itm.as_ref().clone())
            .collect();

        releases.sort_by_key(|itm| itm.number);

        (project.as_ref().clone(), goals, tasks, releases)
    };

    // The documents to carry, resolved from the in-memory index — paths and ids, no payloads. The payloads
    // are read one at a time inside the loop below, which is the point of building onto disk.
    let documents: Vec<crate::documents::DocumentIndexEntry> = app
        .documents_index
        .of_project(&project.id)
        .into_iter()
        .filter(|itm| !task_manager_shared::github::is_github_path(&itm.path))
        .collect();

    let now = DateTimeAsMicroseconds::now();
    let path = temp_file_path(&project.prefix, now);

    // Built inside so one `?` can clean up after itself: a half-written archive left in the temp directory
    // would sit there until the container is replaced.
    match build(app, &path, &project, &goals, &tasks, &releases, &documents).await {
        Ok(size) => Ok(ExportedFile {
            path,
            file_name: file_name_for(&project.prefix, now),
            size,
        }),
        Err(err) => {
            let _ = std::fs::remove_file(&path);
            Err(err)
        }
    }
}

/// Where the archive is built.
///
/// `std::env::temp_dir()` — `/tmp` in the container, and whatever the OS says elsewhere. The name carries a
/// UUID rather than only the prefix and the moment, because two people exporting the same board in the same
/// second must not write the same file: the second one would truncate the first mid-download.
fn temp_file_path(prefix: &str, now: DateTimeAsMicroseconds) -> PathBuf {
    std::env::temp_dir().join(format!(
        "task-manager-export-{prefix}-{}-{}.zip",
        now.unix_microseconds,
        uuid::Uuid::new_v4()
    ))
}

/// What the reader's browser saves it as: `TM-2026-08-06.zip`.
///
/// The date and not the time: an export is a thing somebody keeps beside other exports, and a folder of them
/// sorts by name into the order they were taken. Two on one day land on one name, which the browser resolves
/// by adding its own `(1)` — the behaviour everybody already expects of a download.
fn file_name_for(prefix: &str, now: DateTimeAsMicroseconds) -> String {
    let date = now.to_rfc3339_utc();
    let date = date.split('T').next().unwrap_or("export");

    format!("{prefix}-{date}.zip")
}

async fn build(
    app: &AppContext,
    path: &Path,
    project: &ProjectModel,
    goals: &[GoalModel],
    tasks: &[TaskModel],
    releases: &[ReleaseModel],
    documents: &[crate::documents::DocumentIndexEntry],
) -> Result<u64, String> {
    let comments = comments_file(project, goals, tasks, releases);

    let project_file = ProjectFile {
        format: FORMAT.to_string(),
        exported: encode_moment(DateTimeAsMicroseconds::now()),
        project: ProjectFileProject {
            prefix: project.prefix.clone(),
            name_base64: encode_text(&project.name),
            description_base64: encode_text(&project.description),
            column_template_id: project.column_template_id.clone(),
            kind_template_id: project.kind_template_id.clone(),
            archive_days: project.archive_days,
        },
        contents: ProjectFileContents {
            goals: goals.len(),
            tasks: tasks.len(),
            comments: comments.comments.len(),
            documents: documents.len(),
            releases: releases.len(),
        },
    };

    let file = std::fs::File::create(path)
        .map_err(|err| format!("the export could not be started: {err}"))?;

    let mut writer = zip::ZipWriter::new(std::io::BufWriter::new(file));

    write_yaml(&mut writer, PROJECT_FILE, &project_file)?;
    write_yaml(
        &mut writer,
        GOALS_FILE,
        &GoalsFile {
            goals: goals.iter().map(|itm| goal_to_file(project, itm)).collect(),
        },
    )?;
    write_yaml(
        &mut writer,
        TASKS_FILE,
        &TasksFile {
            tasks: tasks.iter().map(|itm| task_to_file(project, itm)).collect(),
        },
    )?;
    write_yaml(&mut writer, COMMENTS_FILE, &comments)?;

    // Written whether or not there is a release to put in it, as the lists above are and `briefs.yaml`
    // below is not: `releases: []` says this board had none, where no file at all says the archive was
    // made before a board could have one.
    write_yaml(
        &mut writer,
        RELEASES_FILE,
        &ReleasesFile {
            releases: releases
                .iter()
                .map(|itm| release_to_file(project, itm))
                .collect(),
        },
    )?;

    // Beside the bytes rather than in place of them: `documents/` stays a plain folder of the project's
    // files, openable by anything, and this says what each of those files IS — above all which id it has,
    // which is what every reference on every card names it by.
    write_yaml(
        &mut writer,
        DOCUMENTS_FILE,
        &DocumentsFile {
            documents: documents
                .iter()
                .map(|itm| DocumentFileModel {
                    id: itm.id.clone(),
                    path: itm.path.clone(),
                    content_type: Some(itm.content_type.clone()),
                })
                .collect(),
        },
    )?;

    // The briefs of exactly the documents being carried, by content hash. The texts travel in
    // `documents/`, so the receiving board hashes them back to these same keys — what would not survive
    // otherwise is the reading somebody did to write them, and a board that arrives with every document
    // reading as unread is a board somebody has to read again.
    let briefs: Vec<BriefFileModel> = documents
        .iter()
        .filter_map(|itm| itm.content_hash.as_deref())
        .collect::<std::collections::BTreeSet<&str>>()
        .into_iter()
        .filter_map(|hash| {
            app.briefs.get(hash).map(|brief| BriefFileModel {
                content_hash: hash.to_string(),
                brief: brief.text.clone(),
                updated_by: Some(brief.updated_by.clone()),
            })
        })
        .collect();

    if !briefs.is_empty() {
        write_yaml(&mut writer, BRIEFS_FILE, &BriefsFile { briefs })?;
    }

    let telemetry = MyTelemetryContext::create_empty();
    let mut written: u64 = 0;

    for entry in documents {
        // One document in memory at a time, and never two: read, write, drop. This is the loop the whole
        // build-onto-disk arrangement exists for.
        let Some(row) = app.documents_repo.get_by_id(&entry.id, &telemetry).await else {
            // The index listed it and the database does not have it — almost always because somebody deleted
            // it in the seconds this export has been running. Refused rather than skipped, and refused for
            // the whole archive: an export quietly missing a document is a file somebody trusts and should
            // not, and asking again is one click.
            return Err(format!(
                "'{}' was deleted while the export was being built — nothing was written, try again",
                entry.path
            ));
        };

        let bytes = super::super::body_of(&row).into_bytes();

        written += bytes.len() as u64;

        if written > MAX_EXPORT_DOCUMENT_BYTES {
            return Err(format!(
                "this project's documents add up to more than {MAX_EXPORT_DOCUMENT_BYTES} bytes, which is the limit for one export"
            ));
        }

        let name = format!("{DOCUMENTS_FOLDER}{}", row.doc_path);

        start_entry(&mut writer, &name)?;

        writer
            .write_all(&bytes)
            .map_err(|err| format!("'{}' could not be written: {err}", row.doc_path))?;
    }

    let mut inner = writer
        .finish()
        .map_err(|err| format!("the export could not be finished: {err}"))?;

    // The `BufWriter`'s own Drop would flush too and swallow whatever it hit — which for a full disk is the
    // difference between an error and a truncated zip somebody downloads.
    inner
        .flush()
        .map_err(|err| format!("the export could not be flushed: {err}"))?;

    let size = std::fs::metadata(path)
        .map_err(|err| format!("the export could not be measured: {err}"))?
        .len();

    Ok(size)
}

fn start_entry<TWrite: std::io::Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<TWrite>,
    name: &str,
) -> Result<(), String> {
    writer
        .start_file(name, zip::write::SimpleFileOptions::default())
        .map_err(|err| format!("'{name}' could not be added to the archive: {err}"))
}

fn write_yaml<TWrite: std::io::Write + std::io::Seek, TModel: serde::Serialize>(
    writer: &mut zip::ZipWriter<TWrite>,
    name: &str,
    model: &TModel,
) -> Result<(), String> {
    let yaml = serde_yaml::to_string(model)
        .map_err(|err| format!("'{name}' could not be written: {err}"))?;

    start_entry(writer, name)?;

    writer
        .write_all(yaml.as_bytes())
        .map_err(|err| format!("'{name}' could not be written: {err}"))
}

/// Every comment on the board, oldest first, whichever kind of thing it is on.
///
/// One thread rather than one per card: `comments.yaml` read on its own is the project's conversation in the
/// order it happened, which is the file somebody actually opens. Sorted by moment, with the handle as the
/// tiebreaker so two comments stamped in the same microsecond come out in the same order every time — an
/// export that reshuffles between two runs would diff as a change that did not happen.
// `pub(super)` for the import's round-trip test of a release's thread, for the reason `goal_to_file`
// below is: the test has to start from what THIS writes.
pub(super) fn comments_file(
    project: &ProjectModel,
    goals: &[GoalModel],
    tasks: &[TaskModel],
    releases: &[ReleaseModel],
) -> CommentsFile {
    let mut comments: Vec<CommentFileModel> = Vec::new();

    for release in releases {
        let handle = compose_release_handle(&project.prefix, release.number);

        for comment in &release.comments {
            comments.push(CommentFileModel {
                target: handle.clone(),
                moment: encode_moment(comment.moment),
                who: comment.who.clone(),
                text_base64: encode_text(&comment.text),
            });
        }
    }

    for goal in goals {
        let handle = compose_goal_handle(&project.prefix, goal.number);

        for comment in &goal.comments {
            comments.push(CommentFileModel {
                target: handle.clone(),
                moment: encode_moment(comment.moment),
                who: comment.who.clone(),
                text_base64: encode_text(&comment.text),
            });
        }
    }

    for task in tasks {
        let handle = compose_task_handle(&project.prefix, task.number);

        for comment in &task.comments {
            comments.push(CommentFileModel {
                target: handle.clone(),
                moment: encode_moment(comment.moment),
                who: comment.who.clone(),
                text_base64: encode_text(&comment.text),
            });
        }
    }

    comments.sort_by(|left, right| {
        left.moment
            .cmp(&right.moment)
            .then_with(|| left.target.cmp(&right.target))
    });

    CommentsFile { comments }
}

// `pub(super)` on this and on `release_to_file` for one caller: the import's round-trip test, which has to
// start from what THIS writes rather than from a file model it built for itself, or it would pass with a
// field the export forgot.
pub(super) fn goal_to_file(project: &ProjectModel, goal: &GoalModel) -> GoalFileModel {
    GoalFileModel {
        id: compose_goal_handle(&project.prefix, goal.number),
        name_base64: encode_text(&goal.name),
        description_base64: encode_text(&goal.description),
        color: goal.color.as_str().to_string(),
        priority: goal.priority.as_str().to_string(),
        subtasks: goal.subtasks.iter().map(subtask_to_file).collect(),
        documents: goal.documents.clone(),
        // The STORED list, in the order it was attached in, and not `releases_of_goal`: that one reads
        // past a deleted release, and a goal has to arrive still listing it — see `export_project`.
        releases: goal
            .releases
            .iter()
            .map(|number| compose_release_handle(&project.prefix, *number))
            .collect(),
        created: encode_moment(goal.created),
        updated: encode_moment(goal.updated),
        closed: goal.close_moment.map(encode_moment),
        deleted: goal.deleted_moment.map(encode_moment),
    }
}

fn task_to_file(project: &ProjectModel, task: &TaskModel) -> TaskFileModel {
    TaskFileModel {
        id: compose_task_handle(&project.prefix, task.number),
        text_base64: encode_text(&task.text),
        // The STORED status, not the effective one: a task parked in a column the project's template no
        // longer has must arrive on the other board still parked there, so that re-creating the column brings
        // it back — exactly as it would here. Reading it through `effective_status` would silently rewrite
        // every such task to Todo, permanently.
        status: task.status.clone(),
        priority: task.priority.as_str().to_string(),
        kind: task.kind.clone(),
        goal: task
            .goal_number
            .map(|number| compose_goal_handle(&project.prefix, number)),
        assignee: task.assignee.clone(),
        labels: task.labels.clone(),
        depends_on: task
            .depends_on
            .iter()
            .map(|number| compose_task_handle(&project.prefix, *number))
            .collect(),
        subtasks: task.subtasks.iter().map(subtask_to_file).collect(),
        documents: task.documents.clone(),
        gh_actions: task
            .gh_actions
            .iter()
            .map(|itm| GhActionFileModel {
                url: itm.url.clone(),
                title_base64: encode_text(&itm.title),
                moment: encode_moment(itm.moment),
            })
            .collect(),
        created: encode_moment(task.created),
        updated: encode_moment(task.updated),
        closed: task.close_moment.map(encode_moment),
        deleted: task.deleted_moment.map(encode_moment),
    }
}

pub(super) fn release_to_file(project: &ProjectModel, release: &ReleaseModel) -> ReleaseFileModel {
    ReleaseFileModel {
        id: compose_release_handle(&project.prefix, release.number),
        title_base64: encode_text(&release.title),
        description_base64: encode_text(&release.description),
        release_notes_base64: encode_text(&release.release_notes),
        date: encode_moment(release.date),
        services: release
            .services
            .iter()
            .map(|itm| ServiceReleaseFileModel {
                microservice_id: itm.microservice_id.clone(),
                version: itm.version.clone(),
                git_hash: itm.git_hash.clone(),
                datetime: encode_moment(itm.datetime),
                settings_update_note_base64: encode_text(&itm.settings_update_note),
                description_base64: encode_text(&itm.description),
            })
            .collect(),
        released_on_prod: release.released_on_prod_moment.map(encode_moment),
        created: encode_moment(release.created),
        updated: encode_moment(release.updated),
        deleted: release.deleted_moment.map(encode_moment),
    }
}

fn subtask_to_file(src: &crate::board::SubtaskModel) -> SubtaskFileModel {
    SubtaskFileModel {
        title_base64: encode_text(&src.title),
        text_base64: encode_text(&src.text),
        done: src.done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of exports has to sort into the order they were taken, which is what the date-first name
    /// buys — and the prefix has to be in there, because an export of one board is not an export of another.
    #[test]
    fn an_export_is_named_by_its_board_and_the_day() {
        let moment = DateTimeAsMicroseconds::from_str("2026-08-06T09:15:00.000000Z")
            .expect("a valid moment");

        assert_eq!(file_name_for("TM", moment), "TM-2026-08-06.zip");
    }

    /// Two exports of one board in one second must not be one file — the second would truncate the first
    /// while somebody was downloading it.
    #[test]
    fn two_exports_of_one_board_do_not_collide() {
        let moment = DateTimeAsMicroseconds::now();

        assert_ne!(temp_file_path("TM", moment), temp_file_path("TM", moment));
    }
}
