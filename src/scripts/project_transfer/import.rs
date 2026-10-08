use std::io::Read;

use ahash::{AHashMap, AHashSet};
use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;
use task_manager_shared::documents::{
    DocumentReference, mirror_document_reference, own_document_reference, read_document_reference,
};
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::priority::Priority;

use crate::app::AppContext;
use crate::board::{
    Board, CommentModel, GhActionModel, GoalModel, ProjectModel, ReleaseModel, ServiceReleaseModel,
    SubtaskModel, TaskModel, parse_goal_handle, parse_release_handle, parse_task_handle,
};
use crate::postgres::{GoalDto, ReleaseDto, TaskDto};

use super::models::*;

/// The most one import may write, counted in goals, tasks and releases together.
///
/// Not a technical ceiling — it is the point past which one HTTP request is the wrong shape for the job.
/// Every card is a Postgres upsert, written one after another inside a single request, and there is no undo:
/// an import that ran for four minutes and then timed out would leave a board half-poured with nothing to
/// roll it back. A real board is a fraction of this.
///
/// **A release counts, though nobody would call it a card**, because what this caps is the upserts and not
/// the kind of thing behind them: a release is one more row written in that same request, and one more
/// number out of the same reservation. Left out, a project with a long release history would be the one
/// import this does not bound.
pub const MAX_IMPORT_CARDS: usize = 2_000;

/// The most an import archive may be, decoded.
///
/// Matched to what an export can produce, so a file this product wrote is a file this product can read back.
pub const MAX_IMPORT_BYTES: usize = 512 * 1024 * 1024;

/// One thing in the file that was not written, and why.
pub struct SkippedImport {
    pub name: String,
    pub reason: String,
}

impl SkippedImport {
    fn new(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            reason: reason.into(),
        }
    }
}

/// What an import did.
pub struct ImportOutcome {
    pub goals: usize,
    pub tasks: usize,
    pub comments: usize,
    pub documents: usize,
    pub releases: usize,
    pub skipped: Vec<SkippedImport>,
    pub notes: Vec<String>,
}

/// Pour an export into an existing project.
///
/// **Numbers are re-issued, and that is the one thing this cannot preserve.** A task is identified by
/// `(project, number)` out of the receiving project's own counter, so `TM-42` from the file lands as whatever
/// this board's counter hands out next. Every reference inside the file is therefore remapped: a task's goal,
/// its dependencies, the releases a goal lists, and the target of every comment. That is why the file spells
/// them as handles — a bare number would be indistinguishable from one this board already uses.
///
/// **Everything else is kept as it was**: text, status, priority, kind, assignee, labels, checklists, build
/// links, the created / updated / closed / deleted moments, and every comment with its own author and moment.
/// A comment is not re-signed by whoever pressed Import — the thread is a record of who said what, and
/// rewriting it would be a lie about the past. A release is kept whole for the same reason: its date and the
/// moment each service went out are what somebody SAID happened, and neither is restamped with today.
///
/// **Nothing on this board is touched except its settings.** Import only ever adds — cards, and the releases
/// they went out in; the tasks already here keep their numbers and are not looked at. The settings ARE
/// replaced, because the statuses in the file are the source board's column ids and mean nothing unless this
/// project follows the same template — see [`super::models::ProjectFile`].
///
/// **Partial by design**, like every archive on this API: an entry that cannot be written comes back in
/// `skipped` with a reason, and the rest still arrives.
pub async fn import_project(
    app: &AppContext,
    project_prefix: &str,
    archive: &[u8],
    who: &str,
) -> Result<ImportOutcome, String> {
    if archive.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "that archive is {} bytes — the limit is {MAX_IMPORT_BYTES}",
            archive.len()
        ));
    }

    let project = {
        let board = app.board.read();
        super::super::resolve_project_by_prefix(&board, project_prefix)?
            .as_ref()
            .clone()
    };

    let mut archive = ImportArchive::open(archive)?;

    let project_file = archive.read_yaml::<ProjectFile>(PROJECT_FILE)?;

    if project_file.format != FORMAT {
        return Err(format!(
            "this archive says it is '{}', and this build reads '{FORMAT}'",
            project_file.format
        ));
    }

    let source_prefix = project_file.project.prefix.trim().to_uppercase();

    if source_prefix.is_empty() {
        return Err(format!("{PROJECT_FILE} does not say which project it came from"));
    }

    let goals_file = archive.read_yaml_or_default::<GoalsFile>(GOALS_FILE)?;
    let tasks_file = archive.read_yaml_or_default::<TasksFile>(TASKS_FILE)?;
    let comments_file = archive.read_yaml_or_default::<CommentsFile>(COMMENTS_FILE)?;
    // Missing from every archive made before releases existed, and that has to read as a board with none
    // rather than as a file that is not an export.
    let releases_file = archive.read_yaml_or_default::<ReleasesFile>(RELEASES_FILE)?;

    let cards = goals_file.goals.len() + tasks_file.tasks.len() + releases_file.releases.len();

    if cards > MAX_IMPORT_CARDS {
        return Err(format!(
            "this archive holds {cards} goals, tasks and releases, and the limit for one import is {MAX_IMPORT_CARDS}"
        ));
    }

    let mut skipped: Vec<SkippedImport> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // The briefs ahead of the documents they describe. They are filed by the hash of a text, so they need
    // nothing that has not happened yet — and filed first, no document is ever on this board without the
    // brief it travelled with, however briefly: a listing read halfway through the loop below would
    // otherwise show every arriving document as one nobody has read.
    //
    // **This call was missing from the day the function was written**: `briefs.yaml` went out in every
    // export and was read by nothing, so a board always arrived with its documents unbriefed.
    import_briefs(app, &mut archive, who, &mut skipped).await;

    // Documents next, because the cards point at them: a reference on a task names a document by the id
    // it had on the other board, and that id has to be ON this board before a card can be built pointing
    // at it.
    let documents = write_documents(app, &mut archive, &project, who, &mut skipped).await?;

    let targets = DocumentTargets {
        arrived: documents.values().cloned().collect(),
        source_prefix: source_prefix.clone(),
        target_prefix: project.prefix.clone(),
    };

    // Then the numbers — every one of them, before a single card is built. A task's goal and its dependencies
    // can name anything in the file, including something further down it, so the whole map has to exist first.
    let mut numbers = Numbering::reserve(
        &app.board,
        &project,
        &goals_file,
        &tasks_file,
        &releases_file,
    )?;

    let mut comments_by_target = group_comments(&comments_file, &mut skipped);

    // Before the goals, which list them: by the time a goal is built the numbering holds only the releases
    // that are really arriving.
    let releases = build_releases(
        &project,
        &releases_file,
        &mut numbers,
        &mut comments_by_target,
        &mut skipped,
    );

    let mut goals: Vec<GoalModel> = Vec::with_capacity(goals_file.goals.len());

    for goal in &goals_file.goals {
        match build_goal(
            &project,
            goal,
            &numbers,
            &mut comments_by_target,
            &targets,
            &mut skipped,
        ) {
            Ok(model) => goals.push(model),
            Err(err) => skipped.push(SkippedImport::new(goal.id.clone(), err)),
        }
    }

    let mut tasks: Vec<TaskModel> = Vec::with_capacity(tasks_file.tasks.len());

    for task in &tasks_file.tasks {
        match build_task(
            &project,
            task,
            &numbers,
            &mut comments_by_target,
            &targets,
            &mut skipped,
        ) {
            Ok(model) => tasks.push(model),
            Err(err) => skipped.push(SkippedImport::new(task.id.clone(), err)),
        }
    }

    // Every comment left over is one whose target never made it into this board — a handle naming a card that
    // is not in the file, or a card that was itself skipped. Said out loud rather than dropped: a thread that
    // silently loses half its notes is worse than one that says which.
    for (target, orphans) in comments_by_target.iter() {
        skipped.push(SkippedImport::new(
            target.clone(),
            format!(
                "{} comment(s) are on something this archive does not carry",
                orphans.len()
            ),
        ));
    }

    let comments = goals
        .iter()
        .map(|itm| itm.comments.len())
        .chain(tasks.iter().map(|itm| itm.comments.len()))
        .sum();

    // Postgres before memory, as everywhere else in this service: a row that failed to write must not be on
    // a board that says it is there.
    let telemetry = MyTelemetryContext::create_empty();

    // Releases ahead of the goals that list them — the order `create_release` writes its two rows in, and
    // for its reason. Nothing wraps these loops in a transaction, so a request that dies part of the way
    // through leaves whatever it had reached: a release no goal lists yet is a legitimate thing to find on
    // a board, where the other order would leave goals pointing at numbers that name nothing.
    for release in &releases {
        let dto: ReleaseDto = release.into();
        app.releases_repo.upsert(&dto, &telemetry).await;
    }

    for goal in &goals {
        let dto: GoalDto = goal.into();
        app.goals_repo.upsert(&dto, &telemetry).await;
    }

    for task in &tasks {
        let dto: TaskDto = task.into();
        app.tasks_repo.upsert(&dto, &telemetry).await;
    }

    // The settings, replaced — and the counter, which moved when the numbers were reserved. One project row,
    // written once, carrying both.
    apply_settings(app, &project, &project_file, &telemetry, &mut notes).await;

    notes.extend(read_notes(app, &project.id, &tasks));

    let goals_written = goals.len();
    let tasks_written = tasks.len();
    let releases_written = releases.len();

    // One snapshot swap for the whole import, rather than one per card.
    app.board
        .upsert_goals_releases_and_tasks(goals, releases, tasks);
    app.notify_project_changed(&project.id).await;

    Ok(ImportOutcome {
        goals: goals_written,
        tasks: tasks_written,
        releases: releases_written,
        comments,
        documents: documents.len(),
        skipped,
        notes,
    })
}

/// The zip, and the two ways this feature reads out of it.
struct ImportArchive<'s> {
    zip: zip::ZipArchive<std::io::Cursor<&'s [u8]>>,
}

impl<'s> ImportArchive<'s> {
    fn open(archive: &'s [u8]) -> Result<Self, String> {
        if archive.is_empty() {
            return Err("that archive is empty".to_string());
        }

        let zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
            .map_err(|err| format!("that file is not a zip we can read: {err}"))?;

        Ok(Self { zip })
    }

    fn read_yaml<TModel: serde::de::DeserializeOwned>(
        &mut self,
        name: &str,
    ) -> Result<TModel, String> {
        let mut entry = self
            .zip
            .by_name(name)
            .map_err(|_| format!("this archive has no {name} — it is not a project export"))?;

        let mut text = String::new();

        entry
            .read_to_string(&mut text)
            .map_err(|err| format!("{name} could not be read: {err}"))?;

        serde_yaml::from_str(&text).map_err(|err| format!("{name} could not be understood: {err}"))
    }

    /// The same, for the files an archive may legitimately be without — a project with no goals writes an
    /// empty `goals.yaml`, but a hand-made archive is entitled to leave it out, and one made before releases
    /// existed has no `releases.yaml` to leave out.
    fn read_yaml_or_default<TModel: serde::de::DeserializeOwned + Default>(
        &mut self,
        name: &str,
    ) -> Result<TModel, String> {
        if self.zip.index_for_name(name).is_none() {
            return Ok(TModel::default());
        }

        self.read_yaml(name)
    }

    /// Every entry under `documents/`, as a path within the project and its bytes.
    ///
    /// Read one at a time by the caller rather than collected: a document can be sixteen megabytes and there
    /// can be hundreds of them, so this hands back the names and the caller comes back for each payload.
    fn document_names(&self) -> Vec<String> {
        self.zip
            .file_names()
            .filter(|name| name.starts_with(DOCUMENTS_FOLDER) && !name.ends_with('/'))
            .map(|itm| itm.to_string())
            .collect()
    }

    fn read_entry(&mut self, name: &str) -> Result<Vec<u8>, String> {
        let mut entry = self
            .zip
            .by_name(name)
            .map_err(|err| format!("it could not be found in the archive: {err}"))?;

        let mut bytes = Vec::with_capacity(entry.size() as usize);

        entry
            .read_to_end(&mut bytes)
            .map_err(|err| format!("it did not decompress: {err}"))?;

        Ok(bytes)
    }
}

/// Write every document in the archive, and answer with source path -> new document id.
///
/// Each one goes through `upload_document`, which is the same call the Documents screen and every MCP write
/// make: a path already holding a document gets a NEW VERSION of it rather than a duplicate, with the whole
/// history kept. That is the right behaviour for an import run twice, and it is the reason nothing here has
/// to check what is already on the board.
async fn write_documents(
    app: &AppContext,
    archive: &mut ImportArchive<'_>,
    project: &ProjectModel,
    who: &str,
    skipped: &mut Vec<SkippedImport>,
) -> Result<AHashMap<String, String>, String> {
    // What each file IS, keyed by path — above all the id it had on the board it came from, which is what
    // the references on the cards name it by. An archive without this file, or with a path missing from
    // it, still imports: that document simply gets a fresh id, exactly as it did before the file existed.
    let declared: AHashMap<String, DocumentFileModel> = archive
        .read_yaml_or_default::<DocumentsFile>(DOCUMENTS_FILE)?
        .documents
        .into_iter()
        .map(|itm| (itm.path.trim().to_string(), itm))
        .collect();

    let mut written: AHashMap<String, String> = AHashMap::new();

    for name in archive.document_names() {
        let path = &name[DOCUMENTS_FOLDER.len()..];

        // The same rule every document path goes through, which is also the sanitiser: `..` is refused here
        // rather than allowed to name somewhere outside the project.
        let path = match task_manager_shared::documents::normalise_document_path(path) {
            Ok(path) => path,
            Err(problem) => {
                skipped.push(SkippedImport::new(name, problem));
                continue;
            }
        };

        // The reserved root is a connected repository's working copy on disk, not this project's to write:
        // an import that wrote there would edit a git checkout, and the repository is what puts files there.
        if task_manager_shared::github::is_github_path(&path) {
            skipped.push(SkippedImport::new(
                name,
                "it is under `github/`, which is a connected repository — connect the same repository instead",
            ));
            continue;
        }

        let bytes = match archive.read_entry(&name) {
            Ok(bytes) => bytes,
            Err(problem) => {
                skipped.push(SkippedImport::new(name, problem));
                continue;
            }
        };

        if bytes.is_empty() {
            skipped.push(SkippedImport::new(name, "it is empty"));
            continue;
        }

        let declared = declared.get(&path);

        // Text or bytes decided from the path and confirmed against the content — the same call the zip
        // upload makes, so a `.md` arrives as a document that renders and diffs rather than as a download.
        // The content type is the one the source DECLARED when it declared one, because re-deriving it here
        // would overwrite a deliberate answer with a guess from the extension.
        let content = super::super::NewDocumentContent {
            body: super::super::body_for_entry(&path, bytes),
            content_type: declared.and_then(|itm| itm.content_type.clone()),
        };

        // The id it had on the other board, so that every reference pointing at it keeps pointing at it.
        // A path the archive did not describe gets a fresh one — a hand-made archive is still an archive.
        let written_row = match declared
            .map(|itm| itm.id.trim())
            .filter(|itm| !itm.is_empty())
        {
            Some(id) => {
                super::super::import_document(app, &project.prefix, &path, content, who, id).await
            }
            None => super::super::upload_document(app, &project.prefix, &path, content, who).await,
        };

        match written_row {
            Ok(row) => {
                written.insert(path, row.id);
            }
            Err(err) => skipped.push(SkippedImport::new(name, err)),
        }
    }

    Ok(written)
}

/// Carry the briefs an archive brought with it.
///
/// **Keyed by content, so nothing here has to line up with anything else in the import.** Ids are
/// renumbered, paths can be pointed at another folder, projects change prefix — and none of that touches a
/// hash. A brief lands beside the text it describes because that text hashes to the same thing on both
/// boards, or it lands beside nothing at all and is simply never looked up.
///
/// A refused brief is skipped rather than failing the import: what it costs is one document reading as
/// unread on a board that has just gained everything else. **Skipped and SAID**, like every other entry
/// this import cannot write — the same goes for a `briefs.yaml` that will not parse, which is the one list
/// file here whose failure is not allowed to stop the import: the others are the board, and this one is
/// notes about it.
async fn import_briefs(
    app: &AppContext,
    archive: &mut ImportArchive<'_>,
    who: &str,
    skipped: &mut Vec<SkippedImport>,
) {
    let file = match archive.read_yaml_or_default::<BriefsFile>(BRIEFS_FILE) {
        Ok(file) => file,
        Err(err) => {
            skipped.push(SkippedImport::new(BRIEFS_FILE, err));
            return;
        }
    };

    let to_file = briefs_to_file(file, who, |hash| app.briefs.get(hash).is_some(), skipped);

    for brief in to_file {
        if let Err(err) =
            crate::scripts::set_brief(app, &brief.content_hash, &brief.text, &brief.who).await
        {
            skipped.push(SkippedImport::new(
                format!("{BRIEFS_FILE}: {}", brief.content_hash),
                err,
            ));
        }
    }
}

/// One brief out of an archive that this instance has no brief for yet.
struct BriefToFile {
    content_hash: String,
    text: String,
    who: String,
}

/// Which of an archive's briefs to file, and a line for each one that cannot be.
///
/// **A content this instance has already briefed keeps the brief it has.** An import only ever adds, and a
/// brief is the one thing in an archive that is not this project's alone: it is filed under the hash of a
/// text, so the same text anywhere on this server already answers to it. That is every brief in the file
/// when a board is copied on the instance the original still lives on — the common case for an import —
/// and writing them again would only restamp each one as rephrased today. On another instance it is the
/// rare text both hold, where what is here was written by somebody who read it here.
///
/// The check is a closure rather than the index itself so that this half — every decision the import
/// makes about a brief — runs in a test, where the rest of an `AppContext` is Postgres.
///
/// A hash written twice in one file is filed once, the first of them: the second would only overwrite it.
fn briefs_to_file(
    file: BriefsFile,
    who: &str,
    is_briefed: impl Fn(&str) -> bool,
    skipped: &mut Vec<SkippedImport>,
) -> Vec<BriefToFile> {
    let mut result: Vec<BriefToFile> = Vec::with_capacity(file.briefs.len());

    for row in file.briefs {
        // The spelling the index is keyed by. A hash that is not one names no text on any board.
        let content_hash = match crate::documents::normalise_content_hash(&row.content_hash) {
            Ok(content_hash) => content_hash,
            Err(err) => {
                skipped.push(SkippedImport::new(BRIEFS_FILE, err));
                continue;
            }
        };

        if is_briefed(&content_hash) || result.iter().any(|itm| itm.content_hash == content_hash) {
            continue;
        }

        // Whoever wrote it, when the file says — the import is not the reading. An archive with no
        // author on a brief is signed by whoever pressed Import, as a document with none is.
        let who = row
            .updated_by
            .as_deref()
            .map(str::trim)
            .filter(|itm| !itm.is_empty())
            .unwrap_or(who)
            .to_string();

        result.push(BriefToFile {
            content_hash,
            text: row.brief,
            who,
        });
    }

    result
}

/// Source handle -> the number it gets on this board.
///
/// Reserved in one go, before anything is built, because a task can name a goal or a dependency that is
/// further down the file than it is — and a goal lists releases, which are in another file altogether.
///
/// **A release draws from the same run of numbers as the cards, not from one of its own.** This board serves
/// tasks, goals and releases from a single counter, as the board the file left did, and that is what keeps
/// `NEW-41`, `NEW-G41` and `NEW-R41` from ever coexisting; a release numbered any other way would be the one
/// thing an import could land on top of a card. The three maps stay apart all the same, because a reference
/// says what KIND of thing it points at: a goal that lists `TM-42` among its releases has to find nothing
/// there, not the task.
struct Numbering {
    goals: AHashMap<String, i64>,
    tasks: AHashMap<String, i64>,
    releases: AHashMap<String, i64>,
}

impl Numbering {
    /// Takes the board rather than the `AppContext` it is reached through, because the board is all a
    /// reservation touches — and a board can be built in a test, where the rest of an `AppContext` is
    /// Postgres. This is the half of an import in which a miscount is a panic rather than a skipped entry.
    fn reserve(
        board: &Board,
        project: &ProjectModel,
        goals_file: &GoalsFile,
        tasks_file: &TasksFile,
        releases_file: &ReleasesFile,
    ) -> Result<Self, String> {
        let amount =
            (goals_file.goals.len() + tasks_file.tasks.len() + releases_file.releases.len()) as i64;

        if amount == 0 {
            return Ok(Self {
                goals: AHashMap::new(),
                tasks: AHashMap::new(),
                releases: AHashMap::new(),
            });
        }

        let reserved = board
            .reserve_task_numbers(&project.id, amount)
            .ok_or_else(|| {
                format!(
                    "project {} vanished while the import was reserving numbers",
                    project.prefix
                )
            })?;

        let mut numbers = reserved.into_iter();

        let mut goals = AHashMap::with_capacity(goals_file.goals.len());
        let mut tasks = AHashMap::with_capacity(tasks_file.tasks.len());
        let mut releases = AHashMap::with_capacity(releases_file.releases.len());

        // In file order, so the cards land on this board in the order they were written on the other one.
        //
        // **A repeated id is refused, and it has to be**: the map is what every reference in the file
        // resolves through, so two cards spelled `TM-42` would both be built against the second one's number
        // and the second would overwrite the first in Postgres — losing a card and silently re-pointing
        // whatever depended on it. An export cannot produce that; a hand-edited file can.
        for goal in &goals_file.goals {
            let number = numbers.next().expect("one number per card was reserved");

            if goals.insert(normalise_handle(&goal.id), number).is_some() {
                return Err(format!(
                    "'{}' is in {GOALS_FILE} more than once — an id names one goal",
                    goal.id
                ));
            }
        }

        for task in &tasks_file.tasks {
            let number = numbers.next().expect("one number per card was reserved");

            if tasks.insert(normalise_handle(&task.id), number).is_some() {
                return Err(format!(
                    "'{}' is in {TASKS_FILE} more than once — an id names one task",
                    task.id
                ));
            }
        }

        // File order matters here for more than tidiness. The export writes releases by number, and the
        // number is what orders two releases of one date in every list of them — so handing numbers out
        // in the order the file has them is what keeps a day's releases reading as they did.
        for release in &releases_file.releases {
            let number = numbers.next().expect("one number per release was reserved");

            if releases
                .insert(normalise_handle(&release.id), number)
                .is_some()
            {
                return Err(format!(
                    "'{}' is in {RELEASES_FILE} more than once — an id names one release",
                    release.id
                ));
            }
        }

        Ok(Self {
            goals,
            tasks,
            releases,
        })
    }
}

/// A handle as it is looked up: upper-cased and with the spaces off.
///
/// The prefix is NOT rewritten to the source's — a file naming `TM-7` in one place and `tm-7` in another is
/// one card either way, and a file whose handles name a third project is a file this import will not resolve,
/// which is exactly what should happen.
fn normalise_handle(src: &str) -> String {
    src.trim().to_uppercase()
}

/// Every comment in the file, grouped by what it hangs on.
///
/// Grouped rather than searched per card: a board can carry thousands of comments, and a scan per task would
/// be quadratic. What is left in the map when every card has been built is the orphans, which the caller
/// reports.
fn group_comments(
    file: &CommentsFile,
    skipped: &mut Vec<SkippedImport>,
) -> AHashMap<String, Vec<CommentModel>> {
    let mut grouped: AHashMap<String, Vec<CommentModel>> = AHashMap::new();

    for comment in &file.comments {
        let moment = match decode_moment(&comment.moment, "a comment's moment") {
            Ok(moment) => moment,
            Err(problem) => {
                skipped.push(SkippedImport::new(comment.target.clone(), problem));
                continue;
            }
        };

        let text = match decode_text(&comment.text_base64, "a comment's text") {
            Ok(text) => text,
            Err(problem) => {
                skipped.push(SkippedImport::new(comment.target.clone(), problem));
                continue;
            }
        };

        grouped
            .entry(normalise_handle(&comment.target))
            .or_default()
            .push(CommentModel {
                moment,
                // Kept exactly as it was. A thread says who said what, and re-signing it with whoever pressed
                // Import would make the record false.
                who: comment.who.clone(),
                text,
            });
    }

    // Oldest first within one card, which is the order a thread is read in and the order every other writer
    // in this service appends in.
    for comments in grouped.values_mut() {
        comments.sort_by_key(|itm| itm.moment.unix_microseconds);
    }

    grouped
}

fn build_goal(
    project: &ProjectModel,
    src: &GoalFileModel,
    numbers: &Numbering,
    comments: &mut AHashMap<String, Vec<CommentModel>>,
    documents: &DocumentTargets,
    skipped: &mut Vec<SkippedImport>,
) -> Result<GoalModel, String> {
    let handle = normalise_handle(&src.id);

    if parse_goal_handle(&handle).is_none() {
        return Err(format!(
            "'{}' is not a goal id — expected something like TM-G1",
            src.id
        ));
    }

    let number = *numbers
        .goals
        .get(&handle)
        .ok_or_else(|| format!("'{}' has no number reserved for it", src.id))?;

    // The leniency a task's dependencies get, and one line per lost edge for the same reason: a release
    // that is not arriving cannot be listed here. Usually that is one the archive does not carry. The
    // number in its handle came out of another counter — the source board's, or some third project's when
    // the prefix is not even the export's own — and on this board it names something else or nothing. It
    // is equally one the archive does carry that would not build, which `build_releases` has taken out of
    // the map by now. The goal is real either way, so it arrives without the edge and says so.
    //
    // **A deleted release is not one of those.** It is in the archive, so the goal goes on listing it, and
    // bringing it back on this board puts it on the goal again exactly as it would have on the other.
    let mut releases = Vec::with_capacity(src.releases.len());

    for release in &src.releases {
        match numbers.releases.get(&normalise_handle(release)) {
            // In the order the goal lists them and once each — the shape `ReleasesPatch` leaves this list
            // in. Not sorted, as a task's dependencies are: the order here is the order they were attached.
            Some(release_number) => {
                if !releases.contains(release_number) {
                    releases.push(*release_number);
                }
            }
            None => skipped.push(SkippedImport::new(
                src.id.clone(),
                format!(
                    "it lists release '{release}', which did not arrive — the link was dropped"
                ),
            )),
        }
    }

    Ok(GoalModel {
        project_id: project.id.clone(),
        number,
        name: decode_text(&src.name_base64, "a goal's name")?,
        description: decode_text(&src.description_base64, "a goal's description")?,
        // An unrecognised colour reads as the default swatch, exactly as one stored by a build that knew a
        // colour this one does not — the same leniency, and for the same reason.
        color: KindColor::parse_or_default(&src.color),
        priority: Priority::parse_or_default(&src.priority),
        subtasks: build_subtasks(&src.subtasks)?,
        documents: resolve_documents(&src.documents, documents),
        releases,
        comments: comments.remove(&handle).unwrap_or_default(),
        created: decode_moment(&src.created, "a goal's created")?,
        updated: decode_moment(&src.updated, "a goal's updated")?,
        close_moment: decode_optional_moment(src.closed.as_deref(), "a goal's closed")?,
        deleted_moment: decode_optional_moment(src.deleted.as_deref(), "a goal's deleted")?,
    })
}

/// Every release in the file that can be built — and the numbering told about each one that cannot.
///
/// **A release that does not build comes out of the map**, which is what this does that the loops building
/// the cards do not. Its number is not given back: the counter has moved, and nothing is ever handed that
/// number again. But no goal built afterwards can resolve to it either, so a goal that listed it reports
/// the lost edge like any other — instead of arriving with a number that names nothing on this board and
/// never will, and not a line anywhere to say so.
///
/// It costs nothing here because of the order. A release points at nothing, so every one of them is settled
/// before the first goal is built — where a task can depend on a task further down the file.
fn build_releases(
    project: &ProjectModel,
    file: &ReleasesFile,
    numbers: &mut Numbering,
    comments: &mut AHashMap<String, Vec<CommentModel>>,
    skipped: &mut Vec<SkippedImport>,
) -> Vec<ReleaseModel> {
    let mut releases = Vec::with_capacity(file.releases.len());

    for release in &file.releases {
        match build_release(project, release, numbers, comments) {
            Ok(model) => releases.push(model),
            Err(err) => {
                numbers.releases.remove(&normalise_handle(&release.id));
                skipped.push(SkippedImport::new(release.id.clone(), err));
            }
        }
    }

    releases
}

fn build_release(
    project: &ProjectModel,
    src: &ReleaseFileModel,
    numbers: &Numbering,
    comments: &mut AHashMap<String, Vec<CommentModel>>,
) -> Result<ReleaseModel, String> {
    let handle = normalise_handle(&src.id);

    if parse_release_handle(&handle).is_none() {
        return Err(format!(
            "'{}' is not a release id — expected something like TM-R1",
            src.id
        ));
    }

    let number = *numbers
        .releases
        .get(&handle)
        .ok_or_else(|| format!("'{}' has no number reserved for it", src.id))?;

    let mut services = Vec::with_capacity(src.services.len());

    for service in &src.services {
        services.push(ServiceReleaseModel {
            // Spelled the way `ServicesPatch` stores them. An export wrote them that way already, so this
            // only ever changes a file somebody edited — and there it matters: the id is what a later
            // correction finds this entry by and what one service's history is filtered on, both compared
            // exactly, and a commit is lower-cased so that one commit is one spelling.
            microservice_id: service.microservice_id.trim().to_string(),
            version: service.version.trim().to_string(),
            git_hash: service.git_hash.trim().to_lowercase(),
            release_link: release_link_of_file(&service.release_link),
            // The moment somebody SAID this service went out, so the file's and never now — an import is
            // not a rollout.
            datetime: decode_moment(&service.datetime, "a service's datetime")?,
            settings_update_note: decode_text(
                &service.settings_update_note_base64,
                "a service's settings note",
            )?,
            description: decode_text(&service.description_base64, "a service's description")?,
        });
    }

    Ok(ReleaseModel {
        project_id: project.id.clone(),
        number,
        title: decode_text(&src.title_base64, "a release's title")?,
        description: decode_text(&src.description_base64, "a release's description")?,
        release_notes: decode_text(&src.release_notes_base64, "a release's notes")?,
        date: decode_moment(&src.date, "a release's date")?,
        services,
        envs: envs_of_file(src),
        // Its thread out of the same map the cards take theirs from: `comments.yaml` names what a comment
        // is on by handle, and a release's handle is as unambiguous as a task's.
        comments: comments.remove(&handle).unwrap_or_default(),
        created: decode_moment(&src.created, "a release's created")?,
        updated: decode_moment(&src.updated, "a release's updated")?,
        deleted_moment: decode_optional_moment(src.deleted.as_deref(), "a release's deleted")?,
    })
}

/// Where a release in the file is out.
///
/// The labels it carries, tidied the way `EnvsPatch` would have stored them — trimmed, blanks dropped, and
/// each environment once however it is cased — which only ever changes a file somebody edited.
///
/// **They arrive spelled as the SOURCE board spelled them.** Nothing here looks at what the receiving
/// project calls its environments: an import carries a board across, it does not merge two vocabularies,
/// and a `prod` that became `Prod` on the way would be the importer editing the record.
///
/// An archive from 0.2.0 has no labels at all and may say `released_on_prod` instead. That was the only
/// statement that build could make about where a release was out, so it is honoured — as the label the
/// mark became everywhere else. Only whether it is THERE is read: the moment it carried has nowhere to go.
fn envs_of_file(src: &ReleaseFileModel) -> Vec<String> {
    if src.envs.is_empty() {
        return ReleaseModel::envs_of_prod_mark(src.released_on_prod.is_some());
    }

    let mut envs: Vec<String> = Vec::with_capacity(src.envs.len());

    for env in &src.envs {
        let env = env.trim();

        if env.is_empty()
            || envs
                .iter()
                .any(|itm| task_manager_shared::releases::same_env(itm, env))
        {
            continue;
        }

        envs.push(env.to_string());
    }

    envs
}

/// A service's link as the board will hold it: what the file says when that is a link, and nothing when
/// it is not.
///
/// The write path refuses a `release_link` that is not a url, because whatever is stored is drawn as an
/// anchor. A file somebody edited is the one way around that check, so it is applied here too — and the
/// service still arrives: a bad link is not a reason to lose the record of what was deployed.
fn release_link_of_file(src: &str) -> String {
    let link = src.trim();

    let is_link = (link.starts_with("https://") || link.starts_with("http://"))
        && !link.chars().any(char::is_whitespace);

    if is_link {
        link.to_string()
    } else {
        String::new()
    }
}

fn build_task(
    project: &ProjectModel,
    src: &TaskFileModel,
    numbers: &Numbering,
    comments: &mut AHashMap<String, Vec<CommentModel>>,
    documents: &DocumentTargets,
    skipped: &mut Vec<SkippedImport>,
) -> Result<TaskModel, String> {
    let handle = normalise_handle(&src.id);

    if parse_task_handle(&handle).is_none() {
        return Err(format!(
            "'{}' is not a task id — expected something like TM-42",
            src.id
        ));
    }

    let number = *numbers
        .tasks
        .get(&handle)
        .ok_or_else(|| format!("'{}' has no number reserved for it", src.id))?;

    // A goal naming something the archive does not carry leaves the task standalone rather than refusing it:
    // the work is real and the grouping is not worth losing it over. Reported, so nobody has to notice.
    let goal_number = match src.goal.as_deref().map(str::trim).filter(|itm| !itm.is_empty()) {
        None => None,
        Some(goal) => {
            let goal_handle = normalise_handle(goal);

            match numbers.goals.get(&goal_handle) {
                Some(number) => Some(*number),
                None => {
                    skipped.push(SkippedImport::new(
                        src.id.clone(),
                        format!("its goal '{goal}' is not in this archive — imported standalone"),
                    ));
                    None
                }
            }
        }
    };

    // Same leniency, same reason, and one line per lost edge rather than one refusal: a dependency on
    // something that is not in the file cannot be expressed on this board.
    let mut depends_on = Vec::with_capacity(src.depends_on.len());

    for dependency in &src.depends_on {
        let dependency_handle = normalise_handle(dependency);

        match numbers.tasks.get(&dependency_handle) {
            Some(number) => depends_on.push(*number),
            None => skipped.push(SkippedImport::new(
                src.id.clone(),
                format!("it depends on '{dependency}', which is not in this archive — the dependency was dropped"),
            )),
        }
    }

    depends_on.sort_unstable();
    depends_on.dedup();

    let mut gh_actions = Vec::with_capacity(src.gh_actions.len());

    for action in &src.gh_actions {
        gh_actions.push(GhActionModel {
            url: action.url.clone(),
            title: decode_text(&action.title_base64, "a build link's title")?,
            moment: decode_moment(&action.moment, "a build link's moment")?,
        });
    }

    Ok(TaskModel {
        project_id: project.id.clone(),
        number,
        text: decode_text(&src.text_base64, "a task's text")?,
        // Kept verbatim, even when this project's template has no such column. `effective_status` reads an
        // unknown one as Todo and leaves the stored value alone, so pointing the project at the right
        // template afterwards brings every one of these tasks to where it belongs — which rewriting them
        // here would have made impossible. The count is reported.
        status: src.status.trim().to_lowercase(),
        priority: Priority::parse_or_default(&src.priority),
        kind: src
            .kind
            .as_deref()
            .map(|itm| itm.trim().to_lowercase())
            .filter(|itm| !itm.is_empty()),
        goal_number,
        assignee: src
            .assignee
            .as_deref()
            .map(str::trim)
            .filter(|itm| !itm.is_empty())
            .map(super::super::normalise_actor),
        labels: normalise_labels(&src.labels),
        depends_on,
        subtasks: build_subtasks(&src.subtasks)?,
        documents: resolve_documents(&src.documents, documents),
        gh_actions,
        comments: comments.remove(&handle).unwrap_or_default(),
        created: decode_moment(&src.created, "a task's created")?,
        updated: decode_moment(&src.updated, "a task's updated")?,
        close_moment: decode_optional_moment(src.closed.as_deref(), "a task's closed")?,
        deleted_moment: decode_optional_moment(src.deleted.as_deref(), "a task's deleted")?,
    })
}

/// Checklist items, with fresh ids.
///
/// The id is minted here rather than carried: it is a `SortableId` nobody ever sees, and two boards holding
/// the same one for two items that are only coincidentally the same is a collision waiting to be found by
/// whatever ticks one of them.
fn build_subtasks(src: &[SubtaskFileModel]) -> Result<Vec<SubtaskModel>, String> {
    let mut result = Vec::with_capacity(src.len());

    for item in src {
        result.push(SubtaskModel {
            id: rust_extensions::SortableId::generate().to_string(),
            title: decode_text(&item.title_base64, "a checklist item's title")?,
            text: decode_text(&item.text_base64, "a checklist item's text")?,
            done: item.done,
        });
    }

    Ok(result)
}

/// What a reference on an imported card is rewritten against.
struct DocumentTargets {
    /// The ids that actually landed on this board.
    arrived: AHashSet<String>,
    /// The prefix the archive's references were written with.
    source_prefix: String,
    /// The prefix they have to read as now.
    target_prefix: String,
}

/// Document references, as the receiving board has to spell them.
///
/// **Only the project prefix changes, and that is the whole point of preserving ids.** A reference is
/// `raw/{project}/document/{id}` or `raw/{project}/github/{repository}/{path}`; the id is carried over by
/// `documents.yaml` and the repository path means the same thing wherever the repository is connected, so
/// the one part that is about WHICH BOARD is the one part rewritten.
///
/// A reference to a document that did not arrive — skipped as noise, refused, or simply not in the
/// archive — drops out. One that resolves to nothing would read as a broken link on the screen that draws
/// it, which is a worse answer than one fewer reference.
///
/// **A file in a connected repository is kept regardless**, because there is nothing here that could have
/// made it arrive: an export deliberately does not carry a repository, since the repository is still there
/// and the receiving board gets those files by connecting it. Dropping the reference would throw away the
/// only record of which file the work was done against.
fn resolve_documents(references: &[String], targets: &DocumentTargets) -> Vec<String> {
    let mut resolved: Vec<String> = references
        .iter()
        .filter_map(
            |reference| match read_document_reference(&targets.source_prefix, reference) {
                DocumentReference::Own { id, .. } => targets
                    .arrived
                    .contains(&id)
                    .then(|| own_document_reference(&targets.target_prefix, &id)),
                DocumentReference::Mirror { path, .. } => {
                    Some(mirror_document_reference(&targets.target_prefix, &path))
                }
            },
        )
        .collect();

    // Sorted and de-duplicated — the shape every other writer of this list leaves it in.
    resolved.sort();
    resolved.dedup();
    resolved
}

/// Lower-case, trim, drop blanks, de-duplicate, sort — the same normalisation a label gets on every other
/// write, so a board built by import compares equal to one built by hand.
fn normalise_labels(src: &[String]) -> Vec<String> {
    let mut labels: Vec<String> = src
        .iter()
        .map(|itm| itm.trim().to_lowercase())
        .filter(|itm| !itm.is_empty())
        .collect();

    labels.sort();
    labels.dedup();
    labels
}

fn decode_optional_moment(
    src: Option<&str>,
    what: &str,
) -> Result<Option<DateTimeAsMicroseconds>, String> {
    match src.map(str::trim).filter(|itm| !itm.is_empty()) {
        None => Ok(None),
        Some(src) => Ok(Some(decode_moment(src, what)?)),
    }
}

/// Replace the project's settings with the file's, and persist the counter in the same write.
///
/// Both halves have to land in one row: the counter moved when the numbers were reserved, and a project row
/// written from a copy captured before that would undo the reservation. So the project is re-read from
/// memory here, the settings are laid over it, and it is saved once.
async fn apply_settings(
    app: &AppContext,
    project: &ProjectModel,
    file: &ProjectFile,
    telemetry: &MyTelemetryContext,
    notes: &mut Vec<String>,
) {
    // Re-read, so the counter this writes is the one the reservation left behind.
    let Some(current) = app.board.read().get_project(&project.id) else {
        return;
    };

    let mut updated = current.as_ref().clone();

    if let Ok(name) = decode_text(&file.project.name_base64, "the project's name") {
        let name = name.trim().to_string();

        if !name.is_empty() {
            updated.name = name;
        }
    }

    if let Ok(description) = decode_text(&file.project.description_base64, "the project's description") {
        updated.description = description.trim().to_string();
    }

    updated.archive_days = file.project.archive_days;

    {
        let board = app.board.read();

        // A template id is taken only when a template with that id is on this instance. Pointing the project
        // at one that is not here would leave it looking configured while its board had nothing in the
        // middle — the same refusal `set_column_template` makes, for the same reason.
        match file.project.column_template_id.as_deref() {
            None => updated.column_template_id = None,
            Some(id) if board.get_column_template(id).is_some() => {
                updated.column_template_id = Some(id.to_string());
            }
            Some(id) => notes.push(format!(
                "the export follows column template '{id}', which is not on this instance — this project's columns were left as they were"
            )),
        }

        match file.project.kind_template_id.as_deref() {
            None => updated.kind_template_id = None,
            Some(id) if board.get_kind_template(id).is_some() => {
                updated.kind_template_id = Some(id.to_string());
            }
            Some(id) => notes.push(format!(
                "the export follows task-type template '{id}', which is not on this instance — this project's task types were left as they were"
            )),
        }

        // The prefix last, and only when nobody else holds it. Two projects cannot share one, and the common
        // case for this feature — copying a board on the instance the original still lives on — is exactly
        // the case where it is taken.
        let wanted = file.project.prefix.trim().to_uppercase();

        if wanted != updated.prefix {
            if board.is_prefix_free(&wanted, Some(&project.id)) {
                // The prefix being left behind goes into history, so an id written under it can still be
                // traced — the same bookkeeping a rename does.
                if !updated.prefix_history.contains(&updated.prefix) {
                    updated.prefix_history.push(updated.prefix.clone());
                }

                updated.prefix = wanted;
            } else {
                notes.push(format!(
                    "the export came from prefix '{wanted}', which another project holds — this one kept '{}'",
                    updated.prefix
                ));
            }
        }
    }

    let dto: crate::postgres::ProjectDto = (&updated).into();
    app.projects_repo.upsert(&dto, telemetry).await;
    app.board.upsert_project(updated);
}

/// What landed but reads differently here than it did on the board it came from.
///
/// Only the statuses and the kinds, because they are the only two open vocabularies a card carries: a
/// priority is a product-wide enum and an assignee is an email, neither of which can name something this
/// project does not have.
fn read_notes(app: &AppContext, project_id: &str, tasks: &[TaskModel]) -> Vec<String> {
    let Some(project) = app.board.read().get_project(project_id) else {
        return Vec::new();
    };

    let mut unknown_statuses: Vec<String> = tasks
        .iter()
        .map(|itm| itm.status.clone())
        .filter(|status| !project.has_column(status))
        .collect();

    unknown_statuses.sort();
    unknown_statuses.dedup();

    let mut unknown_kinds: Vec<String> = tasks
        .iter()
        .filter_map(|itm| itm.kind.clone())
        .filter(|kind| !project.has_kind(kind))
        .collect();

    unknown_kinds.sort();
    unknown_kinds.dedup();

    let mut notes = Vec::new();

    if !unknown_statuses.is_empty() {
        notes.push(format!(
            "{} has no column for: {} — those tasks kept the status they arrived with and read as Todo until the column exists",
            project.prefix,
            unknown_statuses.join(", ")
        ));
    }

    if !unknown_kinds.is_empty() {
        notes.push(format!(
            "{} has no task type for: {} — those tasks kept the type they arrived with and read as having none",
            project.prefix,
            unknown_kinds.join(", ")
        ));
    }

    notes
}

#[cfg(test)]
mod tests {
    use super::super::export::{goal_to_file, release_to_file};
    use super::*;

    /// A handle is matched however it was spelled — the file is hand-editable, and a lower-cased id in it is
    /// the same card as an upper-cased one everywhere else in this product.
    #[test]
    fn a_handle_is_matched_whatever_case_it_was_written_in() {
        assert_eq!(normalise_handle(" tm-42 "), "TM-42");
        assert_eq!(normalise_handle("TM-G7"), "TM-G7");
    }

    /// The ids that landed on this board, as the reference rewriter sees them.
    fn targets(arrived: &[&str]) -> DocumentTargets {
        DocumentTargets {
            arrived: arrived.iter().map(|itm| itm.to_string()).collect(),
            source_prefix: "TM".to_string(),
            target_prefix: "RMS".to_string(),
        }
    }

    /// **The id survives the crossing and the prefix does not** — which is the whole reason `documents.yaml`
    /// exists. A reference is rewritten onto the receiving board's prefix and otherwise left exactly as it
    /// was, so it goes on naming the same document it named before the move.
    #[test]
    fn a_reference_keeps_its_document_and_changes_its_board() {
        let resolved =
            resolve_documents(&["raw/TM/document/id-a".to_string()], &targets(&["id-a"]));

        assert_eq!(resolved, vec!["raw/RMS/document/id-a".to_string()]);
    }

    /// A reference to a document that did not arrive drops out rather than pointing at nothing — and what
    /// does arrive comes out sorted and unique, the shape every other writer of this list leaves it in.
    #[test]
    fn a_reference_to_a_document_that_did_not_arrive_is_dropped() {
        let resolved = resolve_documents(
            &[
                "raw/TM/document/id-b".to_string(),
                "raw/TM/document/id-missing".to_string(),
                "  raw/TM/document/id-a  ".to_string(),
                "raw/TM/document/id-b".to_string(),
            ],
            &targets(&["id-a", "id-b"]),
        );

        assert_eq!(
            resolved,
            vec![
                "raw/RMS/document/id-a".to_string(),
                "raw/RMS/document/id-b".to_string()
            ]
        );
    }

    /// **A file in a connected repository is kept even though nothing carried it**, because nothing could:
    /// an export does not include a repository, since the repository is still there and the receiving board
    /// gets those files by connecting it. Dropping the reference would throw away the only record of which
    /// file the work was done against.
    #[test]
    fn a_reference_into_a_repository_survives_with_nothing_to_resolve_it_against() {
        let resolved = resolve_documents(
            &["raw/TM/github/specs/design/system.md".to_string()],
            &targets(&[]),
        );

        assert_eq!(
            resolved,
            vec!["raw/RMS/github/specs/design/system.md".to_string()]
        );
    }

    /// Labels arrive in whatever shape the file has them and leave in the one every other write produces,
    /// or two boards holding the same tags would not compare equal.
    #[test]
    fn labels_are_normalised_the_way_every_other_write_normalises_them() {
        let labels = normalise_labels(&[
            " Backend ".to_string(),
            "backend".to_string(),
            "".to_string(),
            "API".to_string(),
        ]);

        assert_eq!(labels, vec!["api".to_string(), "backend".to_string()]);
    }

    /// An absent moment and an empty one are the same fact — a card that was never closed — and neither is
    /// the epoch.
    #[test]
    fn a_moment_that_is_not_there_reads_as_nothing() {
        assert!(decode_optional_moment(None, "x").unwrap().is_none());
        assert!(decode_optional_moment(Some("  "), "x").unwrap().is_none());
        assert!(decode_optional_moment(Some("nonsense"), "x").is_err());

        let moment = decode_optional_moment(Some("2026-08-06T09:15:00.000000Z"), "x")
            .unwrap()
            .expect("a moment");

        assert_eq!(moment.to_rfc3339_utc(), "2026-08-06T09:15:00.000000Z");
    }

    /// Prose survives the round trip with every character YAML would otherwise have an opinion about.
    #[test]
    fn prose_round_trips_through_base64() {
        let text = "# Heading\n\n- item: with a colon\n  \"quoted\"\n\tTabbed\n";

        assert_eq!(decode_text(&encode_text(text), "x").unwrap(), text);
    }

    /// Builds an archive the way the export builds one, so these tests read a real zip rather than a
    /// stand-in for one.
    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));

        for (name, bytes) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }

        writer.finish().unwrap().into_inner()
    }

    fn a_project_file() -> Vec<u8> {
        let model = ProjectFile {
            format: FORMAT.to_string(),
            exported: "2026-08-06T09:00:00.000000Z".to_string(),
            project: ProjectFileProject {
                prefix: "TM".to_string(),
                name_base64: encode_text("Task manager"),
                description_base64: encode_text("The board"),
                column_template_id: Some("default".to_string()),
                kind_template_id: None,
                archive_days: Some(14),
            },
            contents: ProjectFileContents {
                goals: 1,
                tasks: 1,
                comments: 1,
                documents: 1,
                releases: 2,
            },
        };

        serde_yaml::to_string(&model).unwrap().into_bytes()
    }

    /// **The format's own round trip.** Everything the export writes has to come back as what it was — this
    /// is the one test that reads the files as a set, and it is what would catch a field renamed on one
    /// side only.
    #[test]
    fn what_the_export_writes_is_what_the_import_reads() {
        // **The releases and the goal that lists them are written by the export itself.** They start as the
        // models the source board holds and go through `release_to_file` and `goal_to_file`, so a field the
        // export forgot arrives empty and fails below. A file model filled in by hand, as the task under
        // this is, can only prove that the import reads what the test wrote.
        let source = a_source_project();

        let shipped = a_release(12);
        let recalled = ReleaseModel {
            title: "Recorded by mistake".to_string(),
            deleted_moment: Some(moment("2026-10-10T09:00:00.000000Z")),
            ..a_release(15)
        };

        let releases = ReleasesFile {
            releases: vec![
                release_to_file(&source, &shipped),
                release_to_file(&source, &recalled),
            ],
        };

        // The deleted one first: the list is in the order the releases were attached, which is not the
        // order of their numbers and has to survive as it is.
        let goals = GoalsFile {
            goals: vec![goal_to_file(&source, &a_goal(7, &[15, 12]))],
        };

        let tasks = TasksFile {
            tasks: vec![TaskFileModel {
                id: "TM-42".to_string(),
                text_base64: encode_text("Ship it:\n- properly\n"),
                status: "review".to_string(),
                priority: "high".to_string(),
                kind: Some("bug".to_string()),
                goal: Some("TM-G7".to_string()),
                assignee: Some("yuri@mxtm.ai".to_string()),
                labels: vec!["backend".to_string()],
                depends_on: vec!["TM-4".to_string()],
                subtasks: vec![SubtaskFileModel {
                    title_base64: encode_text("first"),
                    text_base64: encode_text(""),
                    done: true,
                }],
                documents: vec!["raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8".to_string()],
                gh_actions: vec![GhActionFileModel {
                    url: "https://github.com/o/r/actions/runs/1".to_string(),
                    title_base64: encode_text("build #1"),
                    moment: "2026-08-06T09:00:00.000000Z".to_string(),
                }],
                created: "2026-08-01T09:00:00.000000Z".to_string(),
                updated: "2026-08-05T09:00:00.000000Z".to_string(),
                closed: None,
                deleted: None,
            }],
        };

        let documents = DocumentsFile {
            documents: vec![DocumentFileModel {
                id: "01K2C4Q0S1T2U3V4W5X6Y7Z8".to_string(),
                path: "docs/a.md".to_string(),
                content_type: Some("text/markdown".to_string()),
            }],
        };

        let releases_yaml = serde_yaml::to_string(&releases).unwrap();

        let archive = zip_of(&[
            (PROJECT_FILE, &a_project_file()),
            (
                GOALS_FILE,
                serde_yaml::to_string(&goals).unwrap().as_bytes(),
            ),
            (
                TASKS_FILE,
                serde_yaml::to_string(&tasks).unwrap().as_bytes(),
            ),
            (RELEASES_FILE, releases_yaml.as_bytes()),
            (
                DOCUMENTS_FILE,
                serde_yaml::to_string(&documents).unwrap().as_bytes(),
            ),
            ("documents/docs/a.md", b"# hello"),
        ]);

        let mut read = ImportArchive::open(&archive).unwrap();

        let project = read.read_yaml::<ProjectFile>(PROJECT_FILE).unwrap();
        assert_eq!(project.format, FORMAT);
        assert_eq!(project.project.prefix, "TM");
        assert_eq!(project.project.archive_days, Some(14));
        assert_eq!(
            decode_text(&project.project.name_base64, "x").unwrap(),
            "Task manager"
        );

        let back = read.read_yaml::<TasksFile>(TASKS_FILE).unwrap();
        let task = &back.tasks[0];

        assert_eq!(task.id, "TM-42");
        assert_eq!(
            decode_text(&task.text_base64, "x").unwrap(),
            "Ship it:\n- properly\n"
        );
        assert_eq!(task.status, "review");
        assert_eq!(task.goal.as_deref(), Some("TM-G7"));
        assert_eq!(task.depends_on, vec!["TM-4".to_string()]);
        assert_eq!(
            task.documents,
            vec!["raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8".to_string()]
        );
        assert_eq!(task.gh_actions.len(), 1);

        assert_eq!(read.document_names(), vec!["documents/docs/a.md".to_string()]);
        assert_eq!(read.read_entry("documents/docs/a.md").unwrap(), b"# hello");

        // **The half that used to be missing.** The folder is keyed by path and carries no id, so without
        // this file the receiving board mints a fresh one for `docs/a.md` — and the reference on TM-42,
        // which names the document by id, arrives pointing at nothing.
        let listed = read.read_yaml::<DocumentsFile>(DOCUMENTS_FILE).unwrap();

        assert_eq!(listed.documents[0].id, "01K2C4Q0S1T2U3V4W5X6Y7Z8");
        assert_eq!(listed.documents[0].path, "docs/a.md");
        assert_eq!(
            listed.documents[0].content_type.as_deref(),
            Some("text/markdown")
        );

        // And the two halves meet: the reference on the card names the document the file describes.
        assert_eq!(
            resolve_documents(
                &task.documents,
                &targets(&[listed.documents[0].id.clone().as_str()])
            ),
            vec!["raw/RMS/document/01K2C4Q0S1T2U3V4W5X6Y7Z8".to_string()],
            "the reference has to survive the crossing and land on the receiving board's prefix"
        );

        assert_eq!(project.contents.releases, 2);

        // **`releases.yaml`, as a person opening it finds it**: the handle, the date and the three
        // identifiers of a service are there to be read, and none of the prose is.
        for legible in [
            "id: TM-R12",
            "id: TM-R15",
            "date: 2026-10-07T00:00:00.000000Z",
            "microservice_id: task-manager-rest-api",
            "version: 0.1.67",
            "git_hash: 099602e4c1a9b7d2f3e5a6b8c9d0e1f2a3b4c5d6",
        ] {
            assert!(
                releases_yaml.contains(legible),
                "'{legible}' should be legible in:\n{releases_yaml}"
            );
        }

        for prose in ["What went out", "the table and the tools", "enabled: true"] {
            assert!(
                !releases_yaml.contains(prose),
                "'{prose}' is prose and should travel encoded:\n{releases_yaml}"
            );
        }

        // **And what it means, once it is read back and built**: the same two releases, field for field,
        // under the numbers this board reserved for them. That comparison is also what holds the second
        // service's `1.10` and `1234567` to arriving as the text they are, and not as the numbers a YAML
        // reader would make of them given the chance.
        let numbers = numbering(&[("TM-G7", 41)], &[], &[("TM-R12", 51), ("TM-R15", 52)]);
        let target = a_target_project();

        let back = read
            .read_yaml_or_default::<ReleasesFile>(RELEASES_FILE)
            .unwrap();

        assert_eq!(back.releases.len(), 2);

        let landed =
            build_release(&target, &back.releases[0], &numbers, &mut AHashMap::new())
                .expect("the release should build");

        assert_eq!(landed.project_id, "p1", "on the board it arrived at");
        assert_eq!(landed.number, 51, "under this board's number, not R12");
        assert_eq!(landed.services.len(), 2);
        assert_arrived_as_it_left(&landed, &shipped);

        // The deleted one arrives too, and arrives deleted — at the moment it was, not at the import's.
        let landed_recalled =
            build_release(&target, &back.releases[1], &numbers, &mut AHashMap::new())
                .expect("the release should build");

        assert_eq!(landed_recalled.number, 52);
        assert!(landed_recalled.is_deleted());
        assert_arrived_as_it_left(&landed_recalled, &recalled);

        // The goal's half. In the file its releases are handles under the SOURCE prefix, in the order the
        // goal lists them.
        let goals_back = read.read_yaml::<GoalsFile>(GOALS_FILE).unwrap();

        assert_eq!(
            goals_back.goals[0].releases,
            vec!["TM-R15".to_string(), "TM-R12".to_string()]
        );

        // And the two halves meet: built, the goal lists the releases the file carries — the deleted one
        // among them, so that bringing it back here puts it on the goal as it would have there.
        let mut skipped = Vec::new();

        let goal = build_goal(
            &target,
            &goals_back.goals[0],
            &numbers,
            &mut AHashMap::new(),
            &targets(&[]),
            &mut skipped,
        )
        .expect("the goal should build");

        assert!(skipped.is_empty(), "nothing was lost");
        assert_eq!(goal.releases, vec![landed_recalled.number, landed.number]);
    }

    /// The four list files are the ones an archive may be without, and an absent one means "none of those"
    /// rather than "this is not an export". For `releases.yaml` that is not a courtesy to hand-made archives:
    /// it is every archive this product wrote before a board could have a release.
    fn a_brief(content_hash: &str, brief: &str, updated_by: Option<&str>) -> BriefFileModel {
        BriefFileModel {
            content_hash: content_hash.to_string(),
            brief: brief.to_string(),
            updated_by: updated_by.map(|itm| itm.to_string()),
        }
    }

    /// **`briefs.yaml` is read on the way in.** For as long as the file existed nothing opened it: the
    /// export wrote it and the import had the function and no call. This reads one as the export writes
    /// it — through the same model, into a zip — which is the half of that an `AppContext` is not needed
    /// for, and the half that would have failed first.
    #[test]
    fn the_briefs_an_export_wrote_are_read_back() {
        let hash = "0e299dc4".repeat(8);

        let written = BriefsFile {
            briefs: vec![a_brief(
                &hash,
                "What the board is for.\n\n- columns: who sets them\n- goals: \"epics\"",
                Some("yuri@example.com"),
            )],
        };

        let archive = zip_of(&[
            (PROJECT_FILE, &a_project_file()),
            (
                BRIEFS_FILE,
                serde_yaml::to_string(&written).unwrap().as_bytes(),
            ),
        ]);

        let mut read = ImportArchive::open(&archive).unwrap();
        let file = read.read_yaml_or_default::<BriefsFile>(BRIEFS_FILE).unwrap();

        let mut skipped = Vec::new();
        let to_file = briefs_to_file(file, "importer@example.com", |_| false, &mut skipped);

        assert!(skipped.is_empty());
        assert_eq!(to_file.len(), 1);
        assert_eq!(to_file[0].content_hash, hash);
        assert_eq!(to_file[0].text, written.briefs[0].brief, "prose and all");
        assert_eq!(to_file[0].who, "yuri@example.com");

        // And an archive without the file — every one made before briefs travelled — has none to file.
        let archive = zip_of(&[(PROJECT_FILE, &a_project_file())]);
        let mut read = ImportArchive::open(&archive).unwrap();

        assert!(
            read.read_yaml_or_default::<BriefsFile>(BRIEFS_FILE)
                .unwrap()
                .briefs
                .is_empty()
        );
    }

    /// An import only ever adds. A text this instance has already briefed keeps the brief it has — which
    /// on a copy made beside the original is every brief in the file, and must not restamp one of them.
    #[test]
    fn a_brief_is_filed_only_where_this_instance_has_none() {
        let already_here = "a".repeat(64);
        let new_here = "b".repeat(64);

        let file = BriefsFile {
            briefs: vec![
                a_brief(&already_here, "the archive's reading", Some("yuri@example.com")),
                // Written in capitals, as a hand-edited file might: it is the same hash, and the index
                // is keyed by the lower-case spelling.
                a_brief(&new_here.to_uppercase(), "what it covers", None),
                // The same text briefed twice in one file. The first stands.
                a_brief(&new_here, "a second opinion", Some("yuri@example.com")),
            ],
        };

        let mut skipped = Vec::new();

        let to_file = briefs_to_file(
            file,
            "importer@example.com",
            |hash| hash == already_here,
            &mut skipped,
        );

        assert!(skipped.is_empty(), "leaving a brief alone is not a failure to report");
        assert_eq!(to_file.len(), 1);
        assert_eq!(to_file[0].content_hash, new_here);
        assert_eq!(to_file[0].text, "what it covers");
        assert_eq!(
            to_file[0].who, "importer@example.com",
            "nobody signed it, so whoever imported it does"
        );
    }

    /// A brief filed under something that is not a hash would be a brief nothing could ever find. It is
    /// left out and said so, and it does not cost the briefs beside it.
    #[test]
    fn a_brief_under_something_that_is_not_a_hash_is_skipped_and_said() {
        let good = "c".repeat(64);

        let file = BriefsFile {
            briefs: vec![
                a_brief("docs/design/system.md", "filed by path, by hand", None),
                a_brief(&good, "what it covers", Some("  ")),
            ],
        };

        let mut skipped = Vec::new();
        let to_file = briefs_to_file(file, "importer@example.com", |_| false, &mut skipped);

        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, BRIEFS_FILE);
        assert!(skipped[0].reason.contains("docs/design/system.md"));

        assert_eq!(to_file.len(), 1);
        assert_eq!(to_file[0].content_hash, good);
        assert_eq!(to_file[0].who, "importer@example.com", "a blank author is no author");
    }

    #[test]
    fn the_list_files_may_be_absent() {
        let archive = zip_of(&[(PROJECT_FILE, &a_project_file())]);
        let mut read = ImportArchive::open(&archive).unwrap();

        assert!(read.read_yaml_or_default::<GoalsFile>(GOALS_FILE).unwrap().goals.is_empty());
        assert!(read.read_yaml_or_default::<TasksFile>(TASKS_FILE).unwrap().tasks.is_empty());
        assert!(
            read.read_yaml_or_default::<CommentsFile>(COMMENTS_FILE)
                .unwrap()
                .comments
                .is_empty()
        );
        assert!(
            read.read_yaml_or_default::<ReleasesFile>(RELEASES_FILE)
                .unwrap()
                .releases
                .is_empty()
        );
        assert!(read.document_names().is_empty());
    }

    /// **Why `FORMAT` is still `1`.** This is an archive as a build from before releases wrote it: no
    /// `releases.yaml`, no `releases` under a goal, no `releases` among the contents. It is spelled out as
    /// text because one made from today's models would carry all three and prove nothing. It has to read,
    /// and as a board with no releases — which is what it was.
    #[test]
    fn an_archive_from_before_releases_reads_as_a_board_with_none() {
        let project = [
            "format: task-manager-project/1",
            "exported: 2026-08-06T09:00:00.000000Z",
            "project:",
            "  prefix: TM",
            "  name_base64: VGFzayBtYW5hZ2Vy",
            "  description_base64: VGhlIGJvYXJk",
            "contents:",
            "  goals: 1",
            "  tasks: 0",
            "  comments: 0",
            "  documents: 0",
        ]
        .join("\n");

        let goals = [
            "goals:",
            "- id: TM-G7",
            "  name_base64: UmVsZWFzZXM=",
            "  description_base64: ''",
            "  color: blue",
            "  priority: high",
            "  subtasks: []",
            "  documents: []",
            "  created: 2026-08-01T09:00:00.000000Z",
            "  updated: 2026-08-05T09:00:00.000000Z",
        ]
        .join("\n");

        let archive = zip_of(&[
            (PROJECT_FILE, project.as_bytes()),
            (GOALS_FILE, goals.as_bytes()),
        ]);

        let mut read = ImportArchive::open(&archive).unwrap();

        let project_file = read.read_yaml::<ProjectFile>(PROJECT_FILE).unwrap();

        assert_eq!(
            project_file.format, FORMAT,
            "the format an old archive names has to be the one this build still reads"
        );
        assert_eq!(project_file.contents.releases, 0);

        assert!(
            read.read_yaml_or_default::<ReleasesFile>(RELEASES_FILE)
                .unwrap()
                .releases
                .is_empty()
        );

        let goals_file = read.read_yaml::<GoalsFile>(GOALS_FILE).unwrap();

        assert!(goals_file.goals[0].releases.is_empty());

        // And the goal builds, with nothing to report: an archive that never had a release has lost none.
        let mut skipped = Vec::new();

        let goal = build_goal(
            &a_target_project(),
            &goals_file.goals[0],
            &numbering(&[("TM-G7", 41)], &[], &[]),
            &mut AHashMap::new(),
            &targets(&[]),
            &mut skipped,
        )
        .expect("an old goal is still a goal");

        assert_eq!(goal.name, "Releases");
        assert!(goal.releases.is_empty());
        assert!(skipped.is_empty());
    }

    /// `project.yaml` is what makes an archive a project export. Without it there is nothing to check the
    /// format against, and the message has to say so rather than reporting an empty import.
    #[test]
    fn an_archive_that_is_not_an_export_is_refused_by_name() {
        let archive = zip_of(&[("readme.md", b"# not an export")]);
        let mut read = ImportArchive::open(&archive).unwrap();

        let problem = read.read_yaml::<ProjectFile>(PROJECT_FILE).unwrap_err();

        assert!(problem.contains(PROJECT_FILE), "{problem}");
        assert!(ImportArchive::open(b"").is_err());
        assert!(ImportArchive::open(b"not a zip at all").is_err());
    }

    /// The receiving board: a project that already has forty cards on it, so a number handed to an imported
    /// task can never be one the file asked for.
    fn a_target_project() -> ProjectModel {
        ProjectModel {
            id: "p1".to_string(),
            name: "Target".to_string(),
            description: String::new(),
            prefix: "NEW".to_string(),
            prefix_history: Vec::new(),
            column_template_id: None,
            columns: Vec::new(),
            kind_template_id: None,
            kinds: Vec::new(),
            members: std::collections::BTreeSet::new(),
            last_task_number: 40,
            archive_days: None,
            archived_moment: None,
            github_connections: Vec::new(),
            created: DateTimeAsMicroseconds::new(0),
        }
    }

    /// The board the archive was taken from. Only its prefix matters: it is what every handle in the file
    /// is spelled with.
    fn a_source_project() -> ProjectModel {
        ProjectModel {
            id: "p0".to_string(),
            name: "Source".to_string(),
            prefix: "TM".to_string(),
            ..a_target_project()
        }
    }

    /// The receiving board itself, holding nothing but its project — which is all a reservation reads.
    fn a_target_board() -> Board {
        let board = Board::new();
        board.upsert_project(a_target_project());
        board
    }

    /// The numbering a reservation would have produced, without a board to reserve from.
    fn numbering(
        goals: &[(&str, i64)],
        tasks: &[(&str, i64)],
        releases: &[(&str, i64)],
    ) -> Numbering {
        let map = |src: &[(&str, i64)]| -> AHashMap<String, i64> {
            src.iter()
                .map(|(handle, number)| (handle.to_string(), *number))
                .collect()
        };

        Numbering {
            goals: map(goals),
            tasks: map(tasks),
            releases: map(releases),
        }
    }

    fn moment(src: &str) -> DateTimeAsMicroseconds {
        DateTimeAsMicroseconds::from_str(src).expect("a valid moment")
    }

    /// A release as the source board holds it, with something in every field that can hold something.
    ///
    /// Two services, and they are not alike on purpose. The first changes its settings and says so in prose
    /// YAML would have opinions about. The second says nothing at all — an empty note is a fact too, it is
    /// how a release says the settings do NOT change — and its version and commit are ones YAML would read
    /// as numbers, which a legible field has to survive.
    fn a_release(number: i64) -> ReleaseModel {
        ReleaseModel {
            project_id: "p0".to_string(),
            number,
            title: "Releases".to_string(),
            description: "# What went out\n\n- the record: of a feature\n".to_string(),
            release_notes: "A goal lists the releases it shipped in.\n\t\"Quoted\", too."
                .to_string(),
            date: moment("2026-10-07T00:00:00.000000Z"),
            services: vec![
                ServiceReleaseModel {
                    microservice_id: "task-manager-rest-api".to_string(),
                    version: "0.1.67".to_string(),
                    git_hash: "099602e4c1a9b7d2f3e5a6b8c9d0e1f2a3b4c5d6".to_string(),
                    // Built by CI, so there is a build to point at. A url is what YAML has opinions about
                    // — a colon and a `#` in a plain scalar — which is the reason to carry one here.
                    release_link:
                        "https://github.com/my-ai-utils/task-manager-mcp/actions/runs/1785#summary"
                            .to_string(),
                    datetime: moment("2026-10-07T14:30:00.000000Z"),
                    settings_update_note: "add to settings:\n  releases:\n    enabled: true\n"
                        .to_string(),
                    description: "the table and the tools".to_string(),
                },
                ServiceReleaseModel {
                    microservice_id: "task-manager-ui".to_string(),
                    version: "1.10".to_string(),
                    git_hash: "1234567".to_string(),
                    // And one built by hand: no link is a fact too, and it has to arrive as none.
                    release_link: String::new(),
                    datetime: moment("2026-10-07T14:45:10.000000Z"),
                    settings_update_note: String::new(),
                    description: String::new(),
                },
            ],
            // On a test stand and then on production, in that order — the order is part of the record, and
            // neither label is spelled the way an importer would normalise it to.
            envs: vec!["dev-2".to_string(), "Prod".to_string()],
            // Left empty here: a thread does not travel in `releases.yaml`. The tests that are about it
            // put one in `comments.yaml`, which is where the export writes it.
            comments: Vec::new(),
            // Written down the day after it went out, and corrected the day after that: three moments
            // that are all different, so one arriving in another's place cannot pass.
            created: moment("2026-10-08T09:00:00.000000Z"),
            updated: moment("2026-10-09T09:00:00.123456Z"),
            deleted_moment: None,
        }
    }

    /// A release as the file holds it, for the tests that are about the file rather than the crossing.
    fn a_release_file_model(id: &str) -> ReleaseFileModel {
        ReleaseFileModel {
            id: id.to_string(),
            ..release_to_file(&a_source_project(), &a_release(1))
        }
    }

    /// Field for field, but for the two an import exists to change: the project it is on, and the number it
    /// has there.
    fn assert_arrived_as_it_left(landed: &ReleaseModel, left: &ReleaseModel) {
        assert_eq!(landed.title, left.title);
        assert_eq!(landed.description, left.description);
        assert_eq!(landed.release_notes, left.release_notes);
        assert_eq!(landed.date, left.date);
        // Every service, every field of each, and in the order they were added.
        assert_eq!(landed.services, left.services);
        // Where it is out, in the order it got there and spelled as the source board spelled it.
        assert_eq!(landed.envs, left.envs);
        assert_eq!(landed.created, left.created);
        assert_eq!(landed.updated, left.updated);
        assert_eq!(landed.deleted_moment, left.deleted_moment);
    }

    /// A goal as the source board holds it, listing these releases in this order.
    fn a_goal(number: i64, releases: &[i64]) -> GoalModel {
        GoalModel {
            project_id: "p0".to_string(),
            number,
            name: "Releases".to_string(),
            description: String::new(),
            color: KindColor::Blue,
            priority: Priority::High,
            subtasks: Vec::new(),
            documents: Vec::new(),
            releases: releases.to_vec(),
            comments: Vec::new(),
            created: moment("2026-08-01T09:00:00.000000Z"),
            updated: moment("2026-08-05T09:00:00.000000Z"),
            close_moment: None,
            deleted_moment: None,
        }
    }

    /// A goal as the file holds it, listing nothing until a test says what.
    fn a_goal_file_model(id: &str) -> GoalFileModel {
        GoalFileModel {
            id: id.to_string(),
            ..goal_to_file(&a_source_project(), &a_goal(1, &[]))
        }
    }

    fn a_task_file_model(id: &str) -> TaskFileModel {
        TaskFileModel {
            id: id.to_string(),
            text_base64: encode_text("do the thing"),
            status: "done".to_string(),
            priority: "high".to_string(),
            kind: Some("BUG".to_string()),
            goal: None,
            assignee: Some(" Yuri@MXTM.ai ".to_string()),
            labels: Vec::new(),
            depends_on: Vec::new(),
            subtasks: Vec::new(),
            documents: Vec::new(),
            gh_actions: Vec::new(),
            created: "2026-08-01T09:00:00.000000Z".to_string(),
            updated: "2026-08-05T09:00:00.000000Z".to_string(),
            closed: Some("2026-08-05T09:00:00.000000Z".to_string()),
            deleted: None,
        }
    }

    /// **The heart of an import.** Every reference in the file is a handle from the board it came from, and
    /// every one of them has to come out as a number on THIS board — a task's goal and its dependencies, the
    /// releases a goal lists, and nothing left pointing at what the file said.
    #[test]
    fn every_reference_is_remapped_onto_this_boards_numbers() {
        let project = a_target_project();
        let numbers = numbering(
            &[("TM-G7", 41)],
            &[("TM-42", 42), ("TM-4", 43)],
            &[("TM-R12", 44), ("TM-R15", 45)],
        );

        let mut src = a_task_file_model("TM-42");
        src.goal = Some("TM-G7".to_string());
        src.depends_on = vec!["TM-4".to_string()];

        let mut comments = AHashMap::new();
        let mut skipped = Vec::new();

        let task = build_task(
            &project,
            &src,
            &numbers,
            &mut comments,
            &targets(&[]),
            &mut skipped,
        )
        .expect("the task should build");

        assert!(skipped.is_empty(), "nothing was lost");
        assert_eq!(task.number, 42, "the number this board handed out");
        assert_eq!(task.goal_number, Some(41), "the goal's NEW number, not G7");
        assert_eq!(task.depends_on, vec![43], "the blocker's NEW number, not 4");
        assert_eq!(task.project_id, "p1");

        // The goal is the other thing in the file that points somewhere: at the releases it went out in,
        // however the handle was cased. They keep the order the goal listed them in — R15 was attached
        // first — and one written twice is listed once, which is the shape every other write leaves.
        let mut src = a_goal_file_model("TM-G7");
        src.releases = vec![
            "TM-R15".to_string(),
            " tm-r12 ".to_string(),
            "TM-R15".to_string(),
        ];

        let goal = build_goal(
            &project,
            &src,
            &numbers,
            &mut comments,
            &targets(&[]),
            &mut skipped,
        )
        .expect("the goal should build");

        assert!(skipped.is_empty(), "nothing was lost");
        assert_eq!(goal.number, 41, "the number this board handed out");
        assert_eq!(
            goal.releases,
            vec![45, 44],
            "the releases' NEW numbers, not R15 and R12"
        );
        assert_eq!(goal.project_id, "p1");
    }

    /// Everything that is not a reference arrives exactly as it left — including the moments, which is what
    /// makes an imported board read as the same history rather than as forty cards created today.
    #[test]
    fn what_is_not_a_reference_arrives_unchanged() {
        let project = a_target_project();
        let numbers = numbering(&[], &[("TM-42", 42)], &[]);

        let mut src = a_task_file_model("TM-42");
        src.labels = vec![" Backend ".to_string(), "backend".to_string()];

        let task = build_task(
            &project,
            &src,
            &numbers,
            &mut AHashMap::new(),
            &targets(&[]),
            &mut Vec::new(),
        )
        .expect("the task should build");

        assert_eq!(task.text, "do the thing");
        assert_eq!(task.status, "done");
        assert_eq!(rust_extensions::AsStr::as_str(&task.priority), "high");
        // Lower-cased, like every other write of one — a kind id is an open vocabulary, not free text.
        assert_eq!(task.kind.as_deref(), Some("bug"));
        assert_eq!(task.assignee.as_deref(), Some("yuri@mxtm.ai"));
        assert_eq!(task.labels, vec!["backend".to_string()]);
        assert_eq!(task.created.to_rfc3339_utc(), "2026-08-01T09:00:00.000000Z");
        assert_eq!(task.updated.to_rfc3339_utc(), "2026-08-05T09:00:00.000000Z");
        assert_eq!(
            task.close_moment.map(|itm| itm.to_rfc3339_utc()),
            Some("2026-08-05T09:00:00.000000Z".to_string())
        );
        assert!(task.deleted_moment.is_none());
    }

    /// A reference to something the archive does not carry cannot be expressed here — the work is still real,
    /// so it arrives without the edge and the loss is reported rather than swallowed.
    #[test]
    fn a_reference_to_something_outside_the_archive_is_dropped_and_said() {
        let project = a_target_project();
        let numbers = numbering(&[("TM-G7", 41)], &[("TM-42", 42)], &[("TM-R12", 44)]);

        let mut src = a_task_file_model("TM-42");
        src.goal = Some("TM-G9".to_string());
        src.depends_on = vec!["TM-4".to_string()];

        let mut skipped = Vec::new();

        let task = build_task(
            &project,
            &src,
            &numbers,
            &mut AHashMap::new(),
            &targets(&[]),
            &mut skipped,
        )
        .expect("the task itself is still worth having");

        assert_eq!(task.goal_number, None);
        assert!(task.depends_on.is_empty());
        assert_eq!(skipped.len(), 2, "the goal and the dependency, one line each");
        assert!(skipped.iter().all(|itm| itm.name == "TM-42"));

        // A goal is held to the same thing about the releases it lists, and three different ways of not
        // being in the archive all come to it. `TM-R99` is simply not there. `OTHER-R12` is the one that
        // matters most: the archive DOES carry a release numbered 12, and a handle under another board's
        // prefix must not be read as that one because the digits agree. And `TM-42` is in the archive, as
        // a task — one counter serves three kinds, so the marker is all that says which a handle names.
        let mut src = a_goal_file_model("TM-G7");
        src.releases = vec![
            "TM-R99".to_string(),
            "TM-R12".to_string(),
            "OTHER-R12".to_string(),
            "TM-42".to_string(),
        ];

        let mut skipped = Vec::new();

        let goal = build_goal(
            &project,
            &src,
            &numbers,
            &mut AHashMap::new(),
            &targets(&[]),
            &mut skipped,
        )
        .expect("the goal itself is still worth having");

        assert_eq!(
            goal.releases,
            vec![44],
            "the one the archive carries is kept"
        );
        assert_eq!(
            skipped.len(),
            3,
            "one line for each release it could not list"
        );
        assert!(skipped.iter().all(|itm| itm.name == "TM-G7"));

        // Each line names the release it is about, as the file spelled it — that is what somebody
        // reading the report goes looking for.
        for (line, lost) in skipped.iter().zip(["'TM-R99'", "'OTHER-R12'", "'TM-42'"]) {
            assert!(line.reason.contains(lost), "{}", line.reason);
        }
    }

    /// **A release that would not build is not one a goal can go on listing.** Its number was reserved and
    /// nothing will ever be written under it, so a goal resolving to it would point at nothing on this board
    /// for good, with no line to say so. It comes out of the numbering instead, and the goal reports the lost
    /// edge as it would for a release the archive never carried — beside the release's own line, which says
    /// what was wrong with it.
    #[test]
    fn a_goal_does_not_go_on_listing_a_release_that_was_skipped() {
        let project = a_target_project();
        let mut numbers = numbering(&[("TM-G7", 41)], &[], &[("TM-R12", 44), ("TM-R15", 45)]);

        let mut unreadable = a_release_file_model("TM-R15");
        unreadable.date = "the day after the 7th".to_string();

        let mut skipped = Vec::new();

        let releases = build_releases(
            &project,
            &ReleasesFile {
                releases: vec![a_release_file_model("TM-R12"), unreadable],
            },
            &mut numbers,
            &mut AHashMap::new(),
            &mut skipped,
        );

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].number, 44);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "TM-R15");
        assert!(skipped[0].reason.contains("date"), "{}", skipped[0].reason);

        let mut src = a_goal_file_model("TM-G7");
        src.releases = vec!["TM-R12".to_string(), "TM-R15".to_string()];

        let goal = build_goal(
            &project,
            &src,
            &numbers,
            &mut AHashMap::new(),
            &targets(&[]),
            &mut skipped,
        )
        .expect("the goal should build");

        assert_eq!(goal.releases, vec![44], "only the release that arrived");
        assert_eq!(
            skipped.len(),
            2,
            "the release's own line, then the goal's for the edge"
        );
        assert_eq!(skipped[1].name, "TM-G7");
        assert!(
            skipped[1].reason.contains("'TM-R15'"),
            "{}",
            skipped[1].reason
        );
    }

    /// **What an import builds is what the board then answers with.** Its last step puts the goals, the
    /// releases and the tasks into memory in one swap, and this reads them back the way a screen does. The
    /// goal shows the release that is live and reads past the one that is deleted — while still holding its
    /// number, which is what puts it back on the goal the day somebody brings that release back.
    #[test]
    fn an_imported_goal_finds_its_releases_on_the_board() {
        let board = a_target_board();
        let project = a_target_project();
        let source = a_source_project();

        let recalled = ReleaseModel {
            deleted_moment: Some(moment("2026-10-10T09:00:00.000000Z")),
            ..a_release(15)
        };

        let mut numbers = numbering(
            &[("TM-G7", 41)],
            &[("TM-42", 42)],
            &[("TM-R12", 44), ("TM-R15", 45)],
        );
        let mut skipped = Vec::new();

        let releases = build_releases(
            &project,
            &ReleasesFile {
                releases: vec![
                    release_to_file(&source, &a_release(12)),
                    release_to_file(&source, &recalled),
                ],
            },
            &mut numbers,
            &mut AHashMap::new(),
            &mut skipped,
        );

        let goal = build_goal(
            &project,
            &goal_to_file(&source, &a_goal(7, &[15, 12])),
            &numbers,
            &mut AHashMap::new(),
            &targets(&[]),
            &mut skipped,
        )
        .expect("the goal should build");

        let mut src = a_task_file_model("TM-42");
        src.goal = Some("TM-G7".to_string());

        let task = build_task(
            &project,
            &src,
            &numbers,
            &mut AHashMap::new(),
            &targets(&[]),
            &mut skipped,
        )
        .expect("the task should build");

        assert!(skipped.is_empty(), "nothing was lost");

        board.upsert_goals_releases_and_tasks(vec![goal], releases, vec![task]);

        let read = board.read();
        let goal = read.get_goal("p1", 41).expect("the goal is on the board");

        let shown: Vec<i64> = read
            .releases_of_goal(&goal)
            .iter()
            .map(|itm| itm.number)
            .collect();

        assert_eq!(shown, vec![44], "the live release, and not the deleted one");
        assert_eq!(goal.releases, vec![45, 44], "which the goal still lists");
        assert!(
            read.get_release_including_deleted("p1", 45)
                .is_some_and(|itm| itm.is_deleted()),
            "and which is there to be brought back"
        );

        // The edge reads from the release's side as well, and the task under the goal arrived with them.
        let shipped_by: Vec<i64> = read
            .goals_of_release("p1", 44)
            .iter()
            .map(|itm| itm.number)
            .collect();

        assert_eq!(shipped_by, vec![41]);
        assert_eq!(
            read.get_task("p1", 42).and_then(|itm| itm.goal_number),
            Some(41)
        );
    }

    /// **One counter, so one run of numbers.** A release is numbered out of the same reservation as the
    /// cards, after them and in the order the file has it — and the project's counter ends past all of
    /// them, so whatever is created on this board next, of any kind, cannot be handed one of these.
    #[test]
    fn releases_are_numbered_out_of_the_same_run_as_the_cards() {
        let board = a_target_board();
        let project = a_target_project();

        let numbers = Numbering::reserve(
            &board,
            &project,
            &GoalsFile {
                goals: vec![a_goal_file_model("TM-G7")],
            },
            &TasksFile {
                tasks: vec![a_task_file_model("TM-42"), a_task_file_model("TM-4")],
            },
            &ReleasesFile {
                releases: vec![
                    a_release_file_model("TM-R12"),
                    a_release_file_model("tm-r15"),
                ],
            },
        )
        .expect("the reservation should be made");

        assert_eq!(numbers.goals.get("TM-G7"), Some(&41));
        assert_eq!(numbers.tasks.get("TM-42"), Some(&42));
        assert_eq!(numbers.tasks.get("TM-4"), Some(&43));
        assert_eq!(numbers.releases.get("TM-R12"), Some(&44));
        assert_eq!(numbers.releases.get("TM-R15"), Some(&45));

        let counter = |board: &Board| board.read().get_project("p1").unwrap().last_task_number;

        assert_eq!(counter(&board), 45);

        // An archive holding releases and nothing else still reserves. Counted as cards only, it would
        // reserve nothing, and every release in it would be skipped for want of a number.
        let numbers = Numbering::reserve(
            &board,
            &project,
            &GoalsFile::default(),
            &TasksFile::default(),
            &ReleasesFile {
                releases: vec![a_release_file_model("TM-R12")],
            },
        )
        .expect("the reservation should be made");

        assert_eq!(numbers.releases.get("TM-R12"), Some(&46));
        assert_eq!(counter(&board), 46);
    }

    /// The same refusal a repeated card gets, and for its reason: the map is what a goal's list resolves
    /// through, so two releases under one id would both be built on the second one's number, and the
    /// second would overwrite the first.
    #[test]
    fn a_release_that_is_in_the_file_twice_is_refused_by_name() {
        let problem = Numbering::reserve(
            &a_target_board(),
            &a_target_project(),
            &GoalsFile::default(),
            &TasksFile::default(),
            &ReleasesFile {
                releases: vec![
                    a_release_file_model("TM-R12"),
                    a_release_file_model("tm-r12"),
                ],
            },
        )
        .err()
        .expect("a repeated id should be refused");

        assert!(problem.contains("tm-r12"), "{problem}");
        assert!(problem.contains(RELEASES_FILE), "{problem}");
    }

    /// A file somebody edited can hold a service's identifiers in a spelling no other write would have
    /// stored. They arrive in the one every other write uses, or the entry could not be found again: a
    /// later correction and the per-service history both look a service up by its id, compared exactly.
    #[test]
    fn a_services_identifiers_arrive_spelled_as_every_other_write_spells_them() {
        let mut src = a_release_file_model("TM-R12");
        src.services[0].microservice_id = " task-manager-rest-api ".to_string();
        src.services[0].version = " 0.1.67 ".to_string();
        src.services[0].git_hash = " 099602E ".to_string();

        let release = build_release(
            &a_target_project(),
            &src,
            &numbering(&[], &[], &[("TM-R12", 51)]),
            &mut AHashMap::new(),
        )
        .expect("the release should build");

        assert_eq!(release.services[0].microservice_id, "task-manager-rest-api");
        assert_eq!(release.services[0].version, "0.1.67");
        assert_eq!(release.services[0].git_hash, "099602e");
    }

    /// A card's thread comes off the map and comes off it ONCE — what is left when every card has been built
    /// is the orphans, and a card that took its comments twice would report the rest of the board as
    /// orphaned.
    #[test]
    fn a_card_takes_its_thread_out_of_the_map() {
        let project = a_target_project();
        let numbers = numbering(&[], &[("TM-42", 42)], &[]);

        let mut comments = AHashMap::new();
        comments.insert(
            "TM-42".to_string(),
            vec![CommentModel {
                moment: DateTimeAsMicroseconds::new(0),
                who: "AI".to_string(),
                text: "did it".to_string(),
            }],
        );
        comments.insert("TM-99".to_string(), Vec::new());

        let task = build_task(
            &project,
            &a_task_file_model("TM-42"),
            &numbers,
            &mut comments,
            &targets(&[]),
            &mut Vec::new(),
        )
        .expect("the task should build");

        assert_eq!(task.comments.len(), 1);
        assert_eq!(task.comments[0].who, "AI");
        assert!(!comments.contains_key("TM-42"), "its thread was taken");
        assert!(comments.contains_key("TM-99"), "somebody else's was not");
    }

    /// **A release's thread and its production mark cross with it, written by the export itself.** The
    /// thread is not in `releases.yaml` — it is in `comments.yaml` with every other, named by the
    /// release's handle — so this goes the whole way round: the model the source board holds, through
    /// the export's own `comments_file` and `release_to_file`, back through the grouping and the build.
    /// The mark arrives as the moment it was, not as the day of the import.
    #[test]
    fn a_release_arrives_with_its_thread_and_its_production_mark() {
        let source = a_source_project();

        let mut shipped = a_release(12);
        shipped.comments = vec![
            CommentModel {
                moment: moment("2026-10-07T15:00:00.000000Z"),
                who: "yuri@example.com".to_string(),
                text: "On the test stand.\n\n- `ttl`: added by hand".to_string(),
            },
            CommentModel {
                moment: moment("2026-10-09T16:25:00.000000Z"),
                who: "AI".to_string(),
                text: "Rolled out to production, no errors in the first hour.".to_string(),
            },
        ];

        // Somebody else's thread, to prove the release takes its own and only its own.
        let mut goal = a_goal(7, &[12]);
        goal.comments = vec![CommentModel {
            moment: moment("2026-10-01T09:00:00.000000Z"),
            who: "AI".to_string(),
            text: "Splitting this in two.".to_string(),
        }];

        let comments = super::super::export::comments_file(
            &source,
            std::slice::from_ref(&goal),
            &[],
            std::slice::from_ref(&shipped),
        );

        assert_eq!(comments.comments.len(), 3);
        assert_eq!(
            comments
                .comments
                .iter()
                .filter(|itm| itm.target == "TM-R12")
                .count(),
            2,
            "a release's comments are named by its handle, as a card's are"
        );

        let mut skipped = Vec::new();
        let mut threads = group_comments(&comments, &mut skipped);
        let mut numbers = numbering(&[("TM-G7", 41)], &[], &[("TM-R12", 51)]);

        let releases = build_releases(
            &a_target_project(),
            &ReleasesFile {
                releases: vec![release_to_file(&source, &shipped)],
            },
            &mut numbers,
            &mut threads,
            &mut skipped,
        );

        assert!(skipped.is_empty());
        assert_eq!(releases.len(), 1);

        let landed = &releases[0];

        assert_arrived_as_it_left(landed, &shipped);
        assert!(landed.is_on_env("prod"));
        assert_eq!(
            landed.envs,
            vec!["dev-2", "Prod"],
            "the environments it was on THERE, in the order it reached them"
        );
        assert_eq!(
            landed.services[0].release_link,
            "https://github.com/my-ai-utils/task-manager-mcp/actions/runs/1785#summary",
            "the link to the build, whole"
        );
        assert_eq!(landed.services[1].release_link, "", "and no link where there was none");

        assert_eq!(landed.comments.len(), 2);

        for (landed, left) in landed.comments.iter().zip(shipped.comments.iter()) {
            assert_eq!(landed.moment, left.moment);
            assert_eq!(landed.who, left.who, "not re-signed by whoever imported it");
            assert_eq!(landed.text, left.text);
        }

        assert!(!threads.contains_key("TM-R12"), "its thread was taken");
        assert!(threads.contains_key("TM-G7"), "the goal's is still there for the goal");
    }

    /// A release that is out nowhere says nothing about it in the file, and arrives the same way — the
    /// absence is the statement. Neither the list nor the field 0.2.0 wrote is spelled into the archive.
    #[test]
    fn a_release_out_nowhere_arrives_out_nowhere() {
        let mut staged = a_release(12);
        staged.envs.clear();

        let file = release_to_file(&a_source_project(), &staged);
        let written = serde_yaml::to_string(&file).unwrap();

        assert!(
            !written.contains("envs"),
            "an empty list is left out of the file rather than written as []: {written}"
        );
        assert!(
            !written.contains("released_on_prod"),
            "the field environments replaced is never written: {written}"
        );

        let landed = build_release(
            &a_target_project(),
            &file,
            &numbering(&[], &[], &[("TM-R12", 51)]),
            &mut AHashMap::new(),
        )
        .expect("the release should build");

        assert!(landed.envs.is_empty());
        assert!(landed.comments.is_empty());
    }

    /// An archive exported by 0.2.0 knows one thing about where a release is out: whether it had reached
    /// production. It was live there, so it arrives as live — as the label that mark became — and one
    /// that carries labels of its own is read from those alone.
    #[test]
    fn an_archive_from_before_environments_brings_its_production_mark_as_a_label() {
        let mut staged = a_release(12);
        staged.envs.clear();

        let written = serde_yaml::to_string(&release_to_file(&a_source_project(), &staged)).unwrap();

        let old: ReleaseFileModel = serde_yaml::from_str(&format!(
            "{written}released_on_prod: \"2026-10-09T16:20:00.000000Z\"\n"
        ))
        .expect("an archive with the old field still reads");

        let build = |file: &ReleaseFileModel| {
            build_release(
                &a_target_project(),
                file,
                &numbering(&[], &[], &[("TM-R12", 51)]),
                &mut AHashMap::new(),
            )
            .expect("the release should build")
        };

        assert_eq!(build(&old).envs, vec!["Prod"]);

        // Somebody added labels to that same file: they are what it says now.
        let mut relabelled = old;
        relabelled.envs = vec!["Dev".to_string()];

        assert_eq!(build(&relabelled).envs, vec!["Dev"]);
    }

    /// A file somebody edited is the one way a label or a link reaches the board without passing the
    /// checks the write path makes, so the same tidying is done on the way in: a label once however it
    /// is cased, and a link only when it is one — with the service arriving either way.
    #[test]
    fn an_edited_file_is_tidied_the_way_a_write_would_have_been() {
        let mut file = release_to_file(&a_source_project(), &a_release(12));

        file.envs = vec![
            "  Dev ".to_string(),
            "dev".to_string(),
            String::new(),
            "Prod".to_string(),
        ];
        file.services[0].release_link = "javascript:alert(1)".to_string();
        file.services[1].release_link = "  https://ci.example.com/job/45  ".to_string();

        let landed = build_release(
            &a_target_project(),
            &file,
            &numbering(&[], &[], &[("TM-R12", 51)]),
            &mut AHashMap::new(),
        )
        .expect("the release should build");

        assert_eq!(landed.envs, vec!["Dev", "Prod"]);
        assert_eq!(landed.services.len(), 2, "a bad link does not cost the service");
        assert_eq!(landed.services[0].release_link, "");
        assert_eq!(landed.services[1].release_link, "https://ci.example.com/job/45");
    }

    /// A goal handle where a task is expected — and the other way round — is a file somebody has edited into
    /// something this board cannot read, and it says so rather than building a card with a nonsense id.
    #[test]
    fn a_card_whose_id_is_not_a_handle_is_refused_by_name() {
        let project = a_target_project();
        let numbers = numbering(&[], &[("TM-42", 42)], &[("TM-12", 51), ("TM-G12", 52)]);

        for id in ["TM-G7", "not-a-handle", ""] {
            let problem = build_task(
                &project,
                &a_task_file_model(id),
                &numbers,
                &mut AHashMap::new(),
                &targets(&[]),
                &mut Vec::new(),
            )
            .unwrap_err();

            assert!(problem.contains(id) || id.is_empty(), "{problem}");
        }

        // A release is held to its own spelling as strictly. Both of these have a number waiting for
        // them, so it is the id that refuses them and not the lookup — a task's handle and a goal's each
        // name something else on the board the file came from.
        for id in ["TM-12", "TM-G12"] {
            let problem =
                build_release(&project, &a_release_file_model(id), &numbers, &mut AHashMap::new())
                    .unwrap_err();

            assert!(problem.contains(id), "{problem}");
        }
    }

    /// A comment lands on the card it names, and one naming nothing is left in the map for the caller to
    /// report — which is the whole reason this returns a map rather than attaching as it goes.
    #[test]
    fn comments_are_grouped_by_card_and_oldest_first() {
        let file = CommentsFile {
            comments: vec![
                CommentFileModel {
                    target: "TM-42".to_string(),
                    moment: "2026-08-06T10:00:00.000000Z".to_string(),
                    who: "yuri@mxtm.ai".to_string(),
                    text_base64: encode_text("second"),
                },
                CommentFileModel {
                    target: "tm-42".to_string(),
                    moment: "2026-08-06T09:00:00.000000Z".to_string(),
                    who: "AI".to_string(),
                    text_base64: encode_text("first"),
                },
                CommentFileModel {
                    target: "TM-G7".to_string(),
                    moment: "2026-08-06T09:30:00.000000Z".to_string(),
                    who: "AI".to_string(),
                    text_base64: encode_text("on the goal"),
                },
            ],
        };

        let mut skipped = Vec::new();
        let grouped = group_comments(&file, &mut skipped);

        assert!(skipped.is_empty());

        // Both spellings of the handle are one card, and the thread reads oldest first.
        let task = grouped.get("TM-42").expect("the task's thread");
        assert_eq!(task.len(), 2);
        assert_eq!(task[0].text, "first");
        assert_eq!(task[1].text, "second");
        // The author is kept, never re-signed by whoever pressed Import.
        assert_eq!(task[0].who, "AI");

        assert_eq!(grouped.get("TM-G7").expect("the goal's thread").len(), 1);
    }
}
