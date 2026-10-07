use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;
use task_manager_shared::documents::{
    DEFAULT_BINARY_CONTENT_TYPE, DEFAULT_TEXT_CONTENT_TYPE, DocumentReference,
    content_type_for_path, mirror_document_reference, normalise_document_path,
    own_document_reference,
};

use crate::app::AppContext;
use crate::documents::DocumentIndexEntry;
use crate::postgres::{
    DocumentDto, DocumentHistoryDto, DocumentTrashDto, DocumentTrashIndexDto, DocumentVersionDto,
};

use super::resolve_project_by_prefix;

/// The longest a TEXT document may be, in characters.
///
/// A constant rather than a setting: a setting needs a deployment to change and a template to carry it, and
/// nobody has an opinion about this number until a write is refused — at which point the message says what
/// the limit is. One MiB of prose is a long specification; a text that does not fit is two documents.
pub const MAX_CONTENT_LEN: usize = 1_000_000;

/// The largest BINARY document, in bytes.
///
/// Higher than the text limit and still deliberately modest, for three reasons that compound: **every version
/// is kept whole**, so a 16 MiB file rewritten five times is 80 MiB of history that nothing prunes; the
/// payload crosses the MCP boundary base64-encoded, which inflates it by a third; and the whole thing is held
/// in memory on both sides of a single `INSERT` — there is no streaming here.
///
/// If real files start bouncing off this, the answer is not a bigger number: it is storing blobs outside
/// Postgres and keeping only a key in the row.
pub const MAX_BINARY_LEN: usize = 16 * 1024 * 1024;

/// What a document actually holds. **Exactly one of the two, never both and never neither.**
///
/// An enum rather than two optional fields threaded through every function, because the invariant is the
/// point: a document is text or it is bytes, and everything downstream — how it is drawn, whether it can be
/// searched, whether a line in it could ever be referenced — follows from which. A row cannot hold an enum,
/// so the two nullable columns are the storage and this is the truth that writes them.
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentBody {
    /// Markdown, or any other text. The kind a person reads in the browser and an agent can diff.
    Text(String),
    /// A file — a PDF, an image, anything whose bytes are not text.
    Binary(Vec<u8>),
}

impl DocumentBody {
    pub fn is_binary(&self) -> bool {
        matches!(self, Self::Binary(_))
    }

    /// The size in BYTES either way — UTF-8 bytes for text, real bytes for a file.
    ///
    /// One unit on purpose: a listing shows sizes of both kinds down one column, and two units would make it
    /// a lie half the time.
    pub fn size_bytes(&self) -> i64 {
        match self {
            Self::Text(text) => text.len() as i64,
            Self::Binary(bytes) => bytes.len() as i64,
        }
    }

    /// Refuse a payload that is too big, or one a text column cannot physically hold.
    ///
    /// The `NUL` check is the one that looks pedantic and is not: Postgres rejects `\0` in a `text` column,
    /// and a JSON string may legally contain one. Without this the write fails inside the driver, which
    /// reaches the caller as "it did not save" with no reason attached — and the fix they would reach for is
    /// to try again. Bytes containing NUL are not a text document; they are a binary one.
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Text(text) => {
                let length = text.chars().count();

                if length > MAX_CONTENT_LEN {
                    return Err(format!(
                        "that document is {length} characters — the limit is {MAX_CONTENT_LEN}. A text that does not fit is two documents"
                    ));
                }

                if text.contains('\0') {
                    return Err(
                        "that text contains a NUL byte, which Postgres cannot store in a text column — upload it as binary if it is a file"
                            .to_string(),
                    );
                }
            }
            Self::Binary(bytes) => {
                if bytes.is_empty() {
                    return Err("that file is empty".to_string());
                }

                if bytes.len() > MAX_BINARY_LEN {
                    return Err(format!(
                        "that file is {} bytes — the limit is {MAX_BINARY_LEN}. Every version is kept whole, so a large file is a large history",
                        bytes.len()
                    ));
                }
            }
        }

        Ok(())
    }

    /// The two columns this payload writes: `(content, binary_content)`.
    fn into_columns(self) -> (Option<String>, Option<Vec<u8>>) {
        match self {
            Self::Text(text) => (Some(text), None),
            Self::Binary(bytes) => (None, Some(bytes)),
        }
    }

    /// Read a payload back out of the two columns.
    ///
    /// `binary_content` wins if both are somehow set, and a row with neither reads as empty text. Lenient
    /// rather than fatal on purpose: the invariant is enforced where rows are written, and one odd row must
    /// not make a document unreadable — the same leniency an unknown status or colour gets everywhere else.
    pub fn from_columns(content: Option<String>, binary_content: Option<Vec<u8>>) -> Self {
        match binary_content {
            Some(bytes) => Self::Binary(bytes),
            None => Self::Text(content.unwrap_or_default()),
        }
    }

    /// The text, for a text document; `None` for a file.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text.as_str()),
            Self::Binary(_) => None,
        }
    }

    /// What this payload's brief is filed under, or `None` for a file.
    ///
    /// **`None` for binary is a statement, not a gap.** A brief is prose about a text somebody could read;
    /// a PDF or a PNG has none this product can produce, so a binary document is never counted as waiting
    /// for one. See `crate::documents::content_hash`.
    pub fn content_hash(&self) -> Option<String> {
        match self {
            Self::Text(text) => Some(crate::documents::content_hash(text.as_bytes())),
            Self::Binary(_) => None,
        }
    }

    /// The payload as bytes, whichever kind it is — what a filesystem takes.
    ///
    /// Beside `into_columns`, which is the same payload for the other destination: a document of the
    /// project's own goes into two Postgres columns, and a file of a connected repository goes onto a
    /// disk, where the distinction between text and bytes stops existing.
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Text(text) => text.into_bytes(),
            Self::Binary(bytes) => bytes,
        }
    }
}

/// A document as a caller hands it over: what it holds, and what it is.
pub struct NewDocumentContent {
    pub body: DocumentBody,
    /// The MIME type. `None` is worked out from the path — see `content_type_for_path` — which is what makes
    /// `docs/spec.pdf` arrive as `application/pdf` without anybody saying so.
    pub content_type: Option<String>,
}

impl NewDocumentContent {
    /// The content type to store: what the caller said, else what the path implies, else a default that
    /// depends on which kind of payload this is.
    fn resolve_content_type(&self, path: &str) -> String {
        if let Some(declared) = self
            .content_type
            .as_deref()
            .map(str::trim)
            .filter(|itm| !itm.is_empty())
        {
            return declared.to_lowercase();
        }

        if let Some(guessed) = content_type_for_path(path) {
            return guessed.to_string();
        }

        // Nothing said and nothing to guess from. The fallbacks differ because the payloads do: an unknown
        // text is still text, and unknown bytes are a file to be downloaded rather than shown.
        if self.body.is_binary() {
            DEFAULT_BINARY_CONTENT_TYPE.to_string()
        } else {
            DEFAULT_TEXT_CONTENT_TYPE.to_string()
        }
    }
}

/// What a version of a document records having happened to it.
///
/// Five values rather than a bool, because "the payload changed" and "it moved" are the two questions a
/// history is asked and a single flag answers neither. Stored as the string these produce, with no constraint
/// in Postgres — the same leniency every open vocabulary here gets, so an unrecognised value renders as
/// itself instead of failing a read of the history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DocumentEvent {
    /// The first version. There is exactly one of these per document, for ever.
    Created,
    /// The payload was rewritten at the same path.
    Updated,
    /// The path changed; the payload did not.
    Moved,
    /// It went to the trash. The version records the path and payload it had when it went.
    Deleted,
    /// It came back out of the trash, at the path this version records.
    Restored,
}

impl DocumentEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Moved => "moved",
            Self::Deleted => "deleted",
            Self::Restored => "restored",
        }
    }
}

/// Upload a document: create one at this path, or overwrite the one already there.
///
/// **The path is the key, and the id is the identity.** An upload never takes an id: it looks the path up
/// within the project, and either writes a new version of what it found or creates something new. That is
/// what makes "just upload the file again" the whole of the editing story.
///
/// Which is also why moving is a *different* call. If an upload could carry a new path together with a new
/// payload, the history could not tell "somebody rewrote it" from "somebody moved it", and those are the two
/// things a history is for. See [`update_document_path`].
///
/// **A rewrite may change the KIND of a document**, text to binary or back, and that is allowed: it is one
/// document at one path whose payload was replaced, and the history records what each version held. Refusing
/// it would only push somebody into deleting and re-uploading, which loses the thread.
pub async fn upload_document(
    app: &AppContext,
    project_prefix: &str,
    path: &str,
    content: NewDocumentContent,
    who: &str,
) -> Result<DocumentDto, String> {
    write_document(app, project_prefix, path, content, who, None).await
}

/// The same write, with the id said rather than minted — the one caller being an import.
///
/// **A document's id crosses instances, and an import is where it has to.** A reference on a task names a
/// document by id, so a board that arrived with fresh ids would arrive with every reference pointing at
/// nothing. Carrying the id over is what makes the receiving board the same board rather than a copy that
/// lost its links, and it is safe to do because a `SortableId` is `{unix_micros}-{uuid}`: unique across
/// instances, not merely within one.
///
/// It applies to a document being CREATED and to nothing else. Uploading onto a path that is taken still
/// writes a version of whatever is there, keeping the id it already has — an import into a project that
/// already holds that path is a rewrite of that document, and giving it a second id is not a thing the
/// table can express.
pub async fn import_document(
    app: &AppContext,
    project_prefix: &str,
    path: &str,
    content: NewDocumentContent,
    who: &str,
    id: &str,
) -> Result<DocumentDto, String> {
    write_document(app, project_prefix, path, content, who, Some(id)).await
}

async fn write_document(
    app: &AppContext,
    project_prefix: &str,
    path: &str,
    content: NewDocumentContent,
    who: &str,
    given_id: Option<&str>,
) -> Result<DocumentDto, String> {
    let project_id = {
        let board = app.board.read();
        resolve_project_by_prefix(&board, project_prefix)?.id.clone()
    };

    let path = normalise_document_path(path)?;
    let who = require_author(who)?;

    content.body.validate()?;

    // **A path under the reserved root is somebody else's file, and this board only reads those.** The
    // folder behind it is a clone that Refresh deletes and makes again, so a write there is work that
    // disappears at the next refresh without anybody being told. Checked after normalisation, so a path
    // spelled `/github/a.md` is caught as readily as `github/a.md`.
    if task_manager_shared::github::is_github_path(&path) {
        return Err(super::refuse_mirror_write(&path, "written"));
    }

    let content_type = content.resolve_content_type(&path);
    let size = content.body.size_bytes();
    // Before `into_columns` MOVES the payload, beside the size and for the same reason: this is the last
    // line at which the whole body is in one piece. Afterwards it is two Option columns and hashing it
    // would mean two branches that can disagree.
    let content_hash = content.body.content_hash();
    let (text, binary) = content.body.into_columns();

    let ctx = MyTelemetryContext::create_empty();
    let now = DateTimeAsMicroseconds::now();

    let existing = app
        .documents_repo
        .get_by_path(&project_id, &path, &ctx)
        .await;

    let row = match existing {
        Some(existing) => DocumentDto {
            // The id of what was already there. This is the line the whole feature turns on: overwriting a
            // path does not mint a new document, it adds a version to the one that lives there.
            id: existing.id,
            project_id,
            doc_path: path,
            content_type: Some(content_type),
            content: text,
            binary_content: binary,
            content_size: Some(size),
            content_hash,
            version: existing.version + 1,
            // Kept, not restamped: a document was created once, and that is a different fact from when it
            // was last written.
            created: existing.created,
            updated: now,
            updated_by: who.clone(),
        },
        None => DocumentDto {
            // Minted here and nowhere else, unless an import said which id this document already has —
            // see `import_document`. Sortable, so a listing by id is a listing by age.
            id: given_id
                .map(str::to_string)
                .unwrap_or_else(|| rust_extensions::SortableId::generate().to_string()),
            project_id,
            doc_path: path,
            content_type: Some(content_type),
            content: text,
            binary_content: binary,
            content_size: Some(size),
            content_hash,
            version: 1,
            created: now,
            updated: now,
            updated_by: who.clone(),
        },
    };

    let event = if row.version == 1 {
        DocumentEvent::Created
    } else {
        DocumentEvent::Updated
    };

    write_version(app, &row, &who, event, &ctx).await;
    app.documents_index.upsert(&row);

    Ok(row)
}

/// Move a document to another path. The payload is untouched and the id does not change.
///
/// Its own call rather than a field on the upload, so the history says which of the two happened — see
/// [`upload_document`]. It is also how a document is renamed, and how a "folder" is renamed: there is no
/// folder to rename, so moving every document under a prefix is the operation, one call each.
///
/// **The content type is left exactly as it was**, even when the new path implies another one. A guess from
/// an extension is how a type is chosen when nobody said; it is not a reason to overwrite what was said.
pub async fn update_document_path(
    app: &AppContext,
    id: &str,
    path: &str,
    who: &str,
) -> Result<DocumentDto, String> {
    let id = &unwrap_document_reference(id);
    let path = normalise_document_path(path)?;
    let who = require_author(who)?;

    // **Both ends are refused, for two different reasons.** A file of a connected repository cannot be
    // renamed because nothing here writes into one; a document of the project's own cannot be moved INTO
    // a connection because that would change what it is rather than where it is — see
    // `refuse_move_into_mirror`.
    if let Some((_, from_path)) = super::parse_mirror_document_id(id) {
        return Err(super::refuse_mirror_write(from_path, "renamed"));
    }

    super::refuse_move_into_mirror(&path)?;

    let ctx = MyTelemetryContext::create_empty();
    let existing = require_live(app, id, &ctx).await?;

    if existing.doc_path == path {
        return Err(format!(
            "'{path}' is already where that document is — nothing to move"
        ));
    }

    if let Some(taken) = app
        .documents_repo
        .get_by_path(&existing.project_id, &path, &ctx)
        .await
    {
        return Err(format!(
            "'{path}' is taken by document {} — pick another path, or upload over that one if replacing it is what you meant",
            taken.id
        ));
    }

    let row = DocumentDto {
        id: existing.id,
        project_id: existing.project_id,
        doc_path: path,
        content_type: existing.content_type,
        // Carried, exactly as the content type is: a move changes where a document is, and a hash is about
        // what is IN it. The fallback covers a row written before the column existed and not yet reached by
        // the startup backfill — the payload is in hand here, so filling it in costs nothing.
        content_hash: existing
            .content_hash
            .or_else(|| existing.content.as_deref().map(|itm| crate::documents::content_hash(itm.as_bytes()))),
        content: existing.content,
        binary_content: existing.binary_content,
        content_size: existing.content_size,
        version: existing.version + 1,
        created: existing.created,
        updated: DateTimeAsMicroseconds::now(),
        updated_by: who.clone(),
    };

    write_version(app, &row, &who, DocumentEvent::Moved, &ctx).await;
    app.documents_index.upsert(&row);

    Ok(row)
}

/// Put a document in the trash.
///
/// Not a deletion: the row moves to a second table, keeping its id, its payload and the last path it had.
/// Which is why nothing prunes the references to it on tasks and goals — restoring is one call away, and a
/// reference quietly removed here would not come back with the document.
///
/// Returns the path it had, which is what a caller wants echoed back: it is what the document was called.
pub async fn delete_document(app: &AppContext, id: &str, who: &str) -> Result<String, String> {
    let id = &unwrap_document_reference(id);

    // A file of a connected repository is not this board's to delete: what is on the disk is a clone, and
    // removing a file from it would change nothing in the repository and be undone by the next refresh.
    if let Some((_, mirror_path)) = super::parse_mirror_document_id(id) {
        return Err(super::refuse_mirror_write(mirror_path, "deleted"));
    }

    let who = require_author(who)?;

    let ctx = MyTelemetryContext::create_empty();
    let existing = require_live(app, id, &ctx).await?;

    let now = DateTimeAsMicroseconds::now();
    let version = existing.version + 1;

    // The version is written first, as always — see `write_version`. Recorded with the path and payload it
    // had when it went, so the history answers "what was in it when it was thrown away".
    app.documents_repo
        .upsert_history(
            &DocumentHistoryDto {
                document_id: existing.id.clone(),
                version,
                project_id: existing.project_id.clone(),
                doc_path: existing.doc_path.clone(),
                content_type: existing.content_type.clone(),
                content: existing.content.clone(),
                binary_content: existing.binary_content.clone(),
                content_size: existing.content_size,
                who: who.clone(),
                event: DocumentEvent::Deleted.as_str().to_string(),
                moment: now,
            },
            &ctx,
        )
        .await;

    // Then the trash, and only then the removal from the working table. That order cannot lose the
    // document: if the second write fails it is briefly in both places, which reads as still live and is
    // undone by deleting it again. The other order would drop it from the working table with nowhere to
    // restore it from.
    app.documents_repo
        .upsert_trash(
            &DocumentTrashDto {
                id: existing.id.clone(),
                project_id: existing.project_id.clone(),
                doc_path: existing.doc_path.clone(),
                content_type: existing.content_type,
                content: existing.content,
                binary_content: existing.binary_content,
                content_size: existing.content_size,
                version,
                created: existing.created,
                deleted: now,
                deleted_by: who,
            },
            &ctx,
        )
        .await;

    app.documents_repo.delete_row(&existing.id, &ctx).await;

    app.documents_index
        .remove(&existing.project_id, &existing.id);

    Ok(existing.doc_path)
}

/// One document that a folder deletion took, as the caller wants it echoed back.
///
/// The id first, because that is the half a restore needs and the half nobody can reconstruct: the path is
/// free again the moment it goes, and a folder deletion is exactly the operation after which somebody
/// discovers one file in it mattered.
pub struct DeletedDocument {
    pub id: String,
    pub path: String,
}

/// Put every document under one folder in the trash — the folder and everything below it.
///
/// **There is no folder to delete, which is why this exists.** A folder in this product is read off the
/// paths of the documents in it, so removing one is removing every document whose path goes through it —
/// one call each, and a folder five levels deep is dozens of them. Done by hand that is dozens of round
/// trips, and the failure mode is the interesting one: an agent that gets bored half way leaves a folder
/// that still exists because three files nobody meant to keep are still in it.
///
/// **Each document goes exactly as [`delete_document`] sends it**: its own history entry, its own trash
/// row, its own id kept. So this is not a bigger kind of deletion, it is the same one repeated — and every
/// one of them is restorable on its own.
///
/// It takes the SUBTREE. `docs` takes `docs/design/system.md` as readily as `docs/notes.md`, because that
/// is what deleting a folder means to anyone who has ever deleted one. What it does not take is a document
/// CALLED `docs` — a file that shares a name with a folder is not in it.
///
/// There is no transaction across the documents, and there could not be one: each is two tables of its own.
/// A failure part way through is reported as itself — how many went, which one stopped it — rather than
/// rolled back, because the ones that went are in the trash and are the caller's to restore.
pub async fn delete_folder(
    app: &AppContext,
    project_prefix: &str,
    folder: &str,
    who: &str,
) -> Result<Vec<DeletedDocument>, String> {
    let folder = normalise_folder(folder)?;

    // A folder under the reserved root is a folder in somebody else's repository, and this is the most
    // destructive shape the refusal has to cover: one word would otherwise empty a whole working copy.
    if task_manager_shared::github::is_github_path(&folder) {
        return Err(super::refuse_mirror_write(&folder, "deleted"));
    }

    let who = require_author(who)?;

    let project_id = {
        let board = app.board.read();
        resolve_project_by_prefix(&board, project_prefix)?
            .id
            .clone()
    };

    // Off the index rather than out of Postgres: it is the same list `documents_list` answers with, so what
    // this deletes is exactly what the caller was looking at. Mirrored files are not in it at all — they
    // are added to that answer separately — which is the second reason nothing under `github/` can be hit.
    let targets = documents_in_folder(&app.documents_index.of_project(&project_id), &folder);

    if targets.is_empty() {
        return Err(format!(
            "nothing is under '{folder}' — no documents to delete. Folders are read off the paths of the documents in them, so one with nothing in it does not exist; check documents_list for the spelling, which is case-sensitive"
        ));
    }

    let total = targets.len();
    let mut deleted: Vec<DeletedDocument> = Vec::with_capacity(total);

    for (id, path) in targets {
        match delete_document(app, &id, &who).await {
            Ok(path) => deleted.push(DeletedDocument { id, path }),
            // Said with the count rather than as a bare failure: the ones already in the trash are a fact
            // the caller has to know about, and re-running this call finishes the job.
            Err(err) => {
                return Err(format!(
                    "{} of {total} under '{folder}' were put in the trash, then '{path}' stopped it: {err}",
                    deleted.len()
                ));
            }
        }
    }

    Ok(deleted)
}

/// Which documents are inside a folder, by id and path, in path order.
///
/// Separate and pure because it is the whole rule: `docs/` as a prefix — the slash being what makes `docs`
/// not match `docs-old/a.md` — and nothing clever about the tree, since a subtree is just a longer prefix.
fn documents_in_folder(entries: &[DocumentIndexEntry], folder: &str) -> Vec<(String, String)> {
    let prefix = format!("{folder}/");

    let mut found: Vec<(String, String)> = entries
        .iter()
        .filter(|itm| itm.path.starts_with(&prefix))
        .map(|itm| (itm.id.clone(), itm.path.clone()))
        .collect();

    // In path order, so the report reads like the tree it came from and everything under one subfolder is
    // contiguous — the same order `documents_list` answers in.
    found.sort_by(|left, right| left.1.cmp(&right.1));
    found
}

/// A folder path, or a refusal saying why it is not one.
///
/// The grammar of a document path with one difference: a trailing slash is how a person writes a folder, so
/// it is stripped rather than refused. Everything else — `..`, the length limit, case being significant —
/// is the same rule, checked by the same function, because a folder is nothing but the front of a path.
fn normalise_folder(src: &str) -> Result<String, String> {
    let src = src.trim().replace('\\', "/");
    let src = src.trim_matches('/');

    if src.is_empty() {
        return Err(
            "name the folder to delete, e.g. `docs` or `docs/design`. There is deliberately no way to say 'all of them': that is not a folder, it is the project's documents"
                .to_string(),
        );
    }

    normalise_document_path(src)
}

/// Take a document back out of the trash.
///
/// `path` is optional, and the default is the point: with nothing passed it goes back where it was, which is
/// what somebody undoing a mistake means. If that path has been taken since, the call is **refused and says
/// so** rather than picking a name — where a restored document lands is a decision, and guessing it is how a
/// document ends up somewhere nobody looks.
pub async fn restore_document(
    app: &AppContext,
    id: &str,
    path: Option<&str>,
    who: &str,
) -> Result<DocumentDto, String> {
    let id = &unwrap_document_reference(id);

    // Both ends, because neither has anything to do with this product's trash: a file of a connected
    // repository never went into it, and a trashed document restored INTO a working copy would be a
    // commit to somebody's repository dressed up as an undo. The refusal names the git command that
    // really does undo the first one.
    super::refuse_reserved_restore(id)?;

    if let Some(path) = path {
        super::refuse_reserved_restore(path)?;
    }

    let who = require_author(who)?;

    let ctx = MyTelemetryContext::create_empty();

    let Some(trashed) = app.documents_repo.get_trashed(id, &ctx).await else {
        // Told apart from a document that is simply live, because "it is not in the trash" is a different
        // thing to hear depending on which.
        return Err(match app.documents_repo.get_by_id(id, &ctx).await {
            Some(live) => format!(
                "document {id} is not in the trash — it is live, at '{}'",
                live.doc_path
            ),
            None => format!("no document {id}, in the trash or out of it"),
        });
    };

    let path = match path.map(str::trim).filter(|itm| !itm.is_empty()) {
        Some(path) => normalise_document_path(path)?,
        None => trashed.doc_path.clone(),
    };

    if let Some(taken) = app
        .documents_repo
        .get_by_path(&trashed.project_id, &path, &ctx)
        .await
    {
        return Err(format!(
            "'{path}' is taken by document {} — pass `path` to restore this one somewhere else",
            taken.id
        ));
    }

    let row = DocumentDto {
        id: trashed.id.clone(),
        project_id: trashed.project_id.clone(),
        doc_path: path,
        content_type: trashed.content_type.clone(),
        // Recomputed rather than carried: the trash row has no hash column, and it needs none — the
        // payload came back with it, and hashing text is cheaper than a second column that could be wrong.
        // A restored document therefore finds the brief it had before, because the bytes are the same ones.
        content_hash: trashed
            .content
            .as_deref()
            .map(|itm| crate::documents::content_hash(itm.as_bytes())),
        content: trashed.content.clone(),
        binary_content: trashed.binary_content.clone(),
        content_size: trashed.content_size,
        // One past the version it was deleted at, so its history is a single unbroken run rather than two.
        version: trashed.version + 1,
        created: trashed.created,
        updated: DateTimeAsMicroseconds::now(),
        updated_by: who.clone(),
    };

    write_version(app, &row, &who, DocumentEvent::Restored, &ctx).await;

    // Last, for the same reason the working row is removed last on the way in: while both exist the document
    // reads as live, which is the harmless half of the two failure modes.
    app.documents_repo.delete_trash_row(&trashed.id, &ctx).await;

    app.documents_index.upsert(&row);

    Ok(row)
}

/// One live document by id, payload included.
pub async fn read_document(app: &AppContext, id: &str) -> Result<DocumentDto, String> {
    let id = &unwrap_document_reference(id);
    let ctx = MyTelemetryContext::create_empty();
    require_live(app, id, &ctx).await
}

/// One live document by where it lives.
///
/// Offered beside the by-id read because a path is what a person says: an agent asked to "read the design
/// doc" has a path and no id, and making it list the project first to translate one into the other would be
/// a round trip for nothing.
pub async fn read_document_by_path(
    app: &AppContext,
    project_prefix: &str,
    path: &str,
) -> Result<DocumentDto, String> {
    let project_id = {
        let board = app.board.read();
        resolve_project_by_prefix(&board, project_prefix)?.id.clone()
    };

    let path = normalise_document_path(path)?;

    let ctx = MyTelemetryContext::create_empty();

    app.documents_repo
        .get_by_path(&project_id, &path, &ctx)
        .await
        .ok_or_else(|| format!("no document at '{path}' on {}", project_prefix.to_uppercase()))
}

/// One live document, named whichever of the two ways a caller happens to hold it.
///
/// Every read-and-then-act tool takes both — an id, or a project and a path — because those are the two
/// things a caller actually has: an id when it came out of a previous call, a path when a person said the
/// name of a document out loud. Resolving that here rather than in each tool is what keeps the refusals
/// identical, and the refusals are the useful part: "a path is only unique within one board" is the whole
/// reason `project` is not optional beside one.
pub async fn resolve_document(
    app: &AppContext,
    id: Option<&str>,
    project_prefix: Option<&str>,
    path: Option<&str>,
) -> Result<DocumentDto, String> {
    let id = unwrap_named_reference(id, path);
    let id = id.as_deref();
    let path = path.map(str::trim).filter(|itm| !itm.is_empty());

    match (id, path) {
        // An id wins when both arrive: it is the identity, where a path is only where the document is
        // sitting today.
        (Some(id), _) => {
            // A mirrored file is named by an id of its own shape and is never looked for in Postgres —
            // there is no row. See `task_manager_shared::github::MIRROR_ID_PREFIX`.
            if let Some((project_prefix, mirror_path)) = super::parse_mirror_document_id(id) {
                return super::read_mirror_document(app, project_prefix, mirror_path).await;
            }

            read_document(app, id).await
        }
        (None, Some(path)) => {
            let project = project_prefix
                .map(str::trim)
                .filter(|itm| !itm.is_empty())
                .ok_or_else(|| {
                    "reading by `path` needs `project` beside it — a path is only unique within one board"
                        .to_string()
                })?;

            // The same interception by the other door: a path under the reserved root is a file in a
            // connected repository, and the documents table has never heard of it.
            if task_manager_shared::github::is_github_path(path) {
                return super::read_mirror_document(app, project, path).await;
            }

            read_document_by_path(app, project, path).await
        }
        (None, None) => Err(
            "pass `id`, or `project` and `path` — documents_list reports both for every document"
                .to_string(),
        ),
    }
}

/// Rewrite pieces of a document's text in place, as one new version.
///
/// **This is the same write as an upload, reached differently, and that is the point.** A document is
/// edited by sending its whole text back, which for a large specification means reproducing tens of
/// kilobytes verbatim to change a sentence — and every one of those reproductions is a chance to drop a
/// section nobody notices is gone. Here the caller sends only what changes, the server does the splice
/// against the text it already holds, and the version that lands is the old text with exactly those pieces
/// different.
///
/// **One version per CALL, not per edit.** A batch of seven edits is one entry in the history, because
/// seven entries would describe seven documents that never existed as anybody's intention. It is also what
/// makes the batch meaningfully atomic from a reader's point of view: there is no moment at which the
/// document is half-edited.
///
/// **`expected_version` is an optimistic lock, and the only concurrency control in this feature.** A
/// caller that read version 4, thought about it, and now edits had better still be editing version 4: if a
/// person uploaded in between, the edits are being spliced into text the caller has never seen. Refusing is
/// the whole value — the edits themselves would very likely still *apply*, and land a document that mixes
/// two intentions.
///
/// A binary document is refused. There is no text in a PDF to match against, and the only honest way to
/// change one is to upload another.
pub async fn edit_document(
    app: &AppContext,
    id: Option<&str>,
    project_prefix: Option<&str>,
    path: Option<&str>,
    edits: &[super::DocumentEdit],
    expected_version: Option<i64>,
    who: &str,
) -> Result<(DocumentDto, Vec<i32>), String> {
    let who = require_author(who)?;

    // Before the read, so an empty batch does not cost a database round trip to be told it is empty.
    if edits.is_empty() {
        return Err(
            "no edits to make — pass at least one `{ old_string, new_string }` in `edits`".to_string(),
        );
    }

    // **A file of a connected repository is refused here rather than after it is read**, and by every
    // name it can be handed over under: an edit that read the file, spliced it and then discovered it
    // could not be written would have spent the round trip to say what the path already says.
    if let Some(mirror_path) = mirror_named_by(id, path) {
        return Err(super::refuse_mirror_write(&mirror_path, "edited"));
    }

    let existing = resolve_document(app, id, project_prefix, path).await?;

    if let Some(expected) = expected_version {
        if existing.version != expected {
            return Err(format!(
                "document {} is at version {}, not the {expected} you read — somebody wrote to it in \
                 between. NOTHING WAS WRITTEN. Read it again and rebase the edits onto the new text: \
                 retrying this call as it stands would splice your changes into a document you have not \
                 seen. documents_diff {} {expected} {} says what they changed",
                existing.id, existing.version, existing.id, existing.version
            ));
        }
    }

    let body = body_of(&existing);

    let Some(text) = body.as_text() else {
        return Err(format!(
            "document {} is a file ({}), so there is no text in it to match against — replace it with \
             documents_upload",
            existing.id,
            content_type_of(existing.content_type.as_deref(), &existing.doc_path)
        ));
    };

    let (edited, replacements) = super::apply_edits(text, edits)?;

    let body = DocumentBody::Text(edited);
    body.validate()?;

    let size = body.size_bytes();
    // The spliced text is a different document from the one that was read, so it is a different hash and
    // therefore a document with no brief again — which is the point: whatever was written about the old
    // text no longer describes this one.
    let content_hash = body.content_hash();
    let (content, binary_content) = body.into_columns();

    let row = DocumentDto {
        id: existing.id,
        project_id: existing.project_id,
        doc_path: existing.doc_path,
        // Left exactly as it was. An edit changes what a document SAYS, never what it is, and re-deriving
        // the type from the path here would quietly overwrite one somebody declared on purpose.
        content_type: existing.content_type,
        content,
        binary_content,
        content_size: Some(size),
        content_hash,
        version: existing.version + 1,
        created: existing.created,
        updated: DateTimeAsMicroseconds::now(),
        updated_by: who.clone(),
    };

    let ctx = MyTelemetryContext::create_empty();

    // `Updated`, indistinguishable in the history from an upload of the same text — because it IS the same
    // thing. How a version was produced is a property of the call, not of the document.
    write_version(app, &row, &who, DocumentEvent::Updated, &ctx).await;
    app.documents_index.upsert(&row);

    Ok((row, replacements))
}

/// The file of a connected repository a caller named, by any of the three names one has.
///
/// **The id wins when both arrive**, for the same reason it does in [`resolve_document`]: it is the
/// identity, where a path is only where the file is sitting today. What comes back is the
/// `github/<connection>/<file>` path, which is the half of the name worth putting in a refusal.
fn mirror_named_by(id: Option<&str>, path: Option<&str>) -> Option<String> {
    // A reference is the THIRD name one of these files has, and it is the one a person writes down. It
    // carries its own project, so it needs nothing beside it — which is the whole reason it is a url.
    if let Some(named) = unwrap_named_reference(id, path) {
        return super::parse_mirror_document_id(&named).map(|(_, path)| path.to_string());
    }

    let path = path.map(str::trim).filter(|itm| !itm.is_empty())?;

    // **No `project` is asked for, and that is the difference between this and reading one.** Which
    // connection a path belongs to only matters to somebody about to open the file; the answer here is
    // that nothing writes there, and it is the same answer on every board.
    task_manager_shared::github::is_github_path(path).then(|| path.to_string())
}

/// What a caller is looking for across a project's documents.
pub struct DocumentSearchQuery {
    pub query: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    /// Only documents whose path starts with this — `docs/design`. Folder-shaped rather than glob-shaped,
    /// because folders here are prefixes of paths and nothing else.
    pub path_prefix: Option<String>,
    pub context_lines: i64,
    pub max_matches_per_document: i64,
}

/// One document that matched.
pub struct DocumentSearchHit {
    pub id: String,
    pub path: String,
    pub matches: Vec<super::TextMatch>,
    /// How many lines matched in total, which may be more than were returned.
    pub matches_total: i32,
}

/// What a search found, and what it did not look at.
pub struct DocumentSearchOutcome {
    pub hits: Vec<DocumentSearchHit>,
    pub searched: i32,
    /// Documents with no text in them — files, mostly. Reported rather than hidden: "nothing matched" and
    /// "nothing was searched" are different answers, and a project of PDFs gives the second one.
    pub skipped: i32,
    pub matches_total: i32,
    /// True when a cap cut the answer short, at either level.
    pub truncated: bool,
}

/// Search every text document of one project, without returning any of them whole.
///
/// **The tool that stops "which documents mention X" costing the whole project.** Answering it by reading
/// documents one at a time spends the context on the ones that turn out to be irrelevant — and those are
/// most of them, so most of what is spent is spent learning nothing.
///
/// Texts are pulled from Postgres rather than from the index, because the index deliberately holds no
/// payloads; see [`crate::documents::DocumentsIndex`]. Blobs are never touched — see
/// [`crate::postgres::DocumentTextDto`].
pub async fn search_documents(
    app: &AppContext,
    project_prefix: &str,
    query: &DocumentSearchQuery,
) -> Result<DocumentSearchOutcome, String> {
    let project_id = {
        let board = app.board.read();
        resolve_project_by_prefix(&board, project_prefix)?.id.clone()
    };

    // Compiled before anything is read: a broken pattern must cost nothing, and it must be reported as a
    // broken pattern rather than as a project with no matches in it.
    let matcher = super::Matcher::new(&query.query, query.is_regex, query.case_sensitive)?;

    let context_lines = query
        .context_lines
        .clamp(0, super::MAX_CONTEXT_LINES) as usize;

    let max_per_document = query
        .max_matches_per_document
        .clamp(1, super::MAX_MATCHES_PER_DOCUMENT) as usize;

    // Folder-shaped, so a trailing slash is meaningless either way and both spellings mean the folder.
    let path_prefix = query
        .path_prefix
        .as_deref()
        .map(|itm| itm.trim().replace('\\', "/"))
        .map(|itm| itm.trim_start_matches('/').trim_end_matches('/').to_string())
        .filter(|itm| !itm.is_empty());

    let ctx = MyTelemetryContext::create_empty();

    let mut rows = app.documents_repo.get_texts_of_project(&project_id, &ctx).await;

    // By path, which is the order the tree is drawn in — and, because the total cap cuts the tail, the
    // order is also what makes the cut deterministic rather than whatever Postgres returned this time.
    rows.sort_by(|left, right| left.doc_path.cmp(&right.doc_path));

    let mut outcome = DocumentSearchOutcome {
        hits: Vec::new(),
        searched: 0,
        skipped: 0,
        matches_total: 0,
        truncated: false,
    };

    let mut reported = 0;

    for row in &rows {
        if let Some(prefix) = &path_prefix {
            if !row.doc_path.starts_with(prefix.as_str()) {
                continue;
            }
        }

        let Some(text) = row.content.as_deref() else {
            outcome.skipped += 1;
            continue;
        };

        // Stopped rather than carried on with the counters running. `searched` must mean "the matcher was
        // run over this", so a document past the cap is not one — and the difference between `searched` and
        // what documents_list reports is then the honest measure of how much of the project this answer
        // does not cover. `truncated` is what says to narrow the query.
        if reported >= super::MAX_MATCHES_TOTAL {
            outcome.truncated = true;
            break;
        }

        outcome.searched += 1;

        let room = max_per_document.min(super::MAX_MATCHES_TOTAL - reported);
        let (matches, matches_total) = super::search_text(&matcher, text, context_lines, room);

        if matches_total == 0 {
            continue;
        }

        outcome.matches_total += matches_total;

        if matches.len() < matches_total as usize {
            outcome.truncated = true;
        }

        reported += matches.len();

        outcome.hits.push(DocumentSearchHit {
            id: row.id.clone(),
            path: row.doc_path.clone(),
            matches,
            matches_total,
        });
    }

    Ok(outcome)
}

/// The headings of one document, with the span each one opens.
///
/// A 76 KB specification becomes a kilobyte of structure, and every entry carries the line range that reads
/// it back — which is what makes a large document navigable rather than merely retrievable. Pairs with
/// `from_line` / `to_line` on the read.
pub async fn document_outline(
    app: &AppContext,
    id: Option<&str>,
    project_prefix: Option<&str>,
    path: Option<&str>,
) -> Result<(DocumentDto, Vec<super::DocumentHeading>, i64), String> {
    let row = resolve_document(app, id, project_prefix, path).await?;
    let body = body_of(&row);

    let Some(text) = body.as_text() else {
        return Err(format!(
            "document {} is a file ({}), and a file has no headings to read",
            row.id,
            content_type_of(row.content_type.as_deref(), &row.doc_path)
        ));
    };

    let lines_total = text.lines().count() as i64;

    Ok((row, super::outline_of(text), lines_total))
}

/// A diff between two versions of one document.
pub struct DocumentDiff {
    pub id: String,
    pub project_id: String,
    pub from_version: i64,
    pub to_version: i64,
    /// Where the document was at each version. They differ when it moved in between, which is exactly the
    /// case a diff of the texts alone would not show.
    pub from_path: String,
    pub to_path: String,
    pub diff: String,
    pub truncated: bool,
}

/// What changed between two versions of a document.
///
/// **The answer to "did that write do what I meant", and it needs neither text in full.** `documents_history`
/// says a version happened and who did it; it cannot say what is different, and finding out by reading two
/// 76 KB texts costs more than the change is worth.
///
/// `to_version` defaults to the newest version there is, taken from the history rather than from the
/// document row — so it answers for a document in the trash exactly as it does for a live one, which is the
/// same rule `documents_history` follows and for the same reason.
pub async fn document_diff(
    app: &AppContext,
    id: &str,
    from_version: i64,
    to_version: Option<i64>,
    context_lines: i64,
) -> Result<DocumentDiff, String> {
    let id = &unwrap_document_reference(id);

    // Errors when the id names nothing at all, which is the message a caller wants before any talk of
    // version numbers.
    let versions = document_history(app, id).await?;

    let newest = versions
        .iter()
        .map(|itm| itm.version)
        .max()
        .expect("a history with no versions cannot exist — document_history refuses one");

    let to_version = to_version.unwrap_or(newest);

    for wanted in [from_version, to_version] {
        if !versions.iter().any(|itm| itm.version == wanted) {
            return Err(format!(
                "document {id} has no version {wanted} — it has versions 1 to {newest}"
            ));
        }
    }

    if from_version == to_version {
        return Err(format!(
            "`from_version` and `to_version` are both {from_version} — a version does not differ from \
             itself. The version before it is {}",
            (from_version - 1).max(1)
        ));
    }

    let before = document_version(app, id, from_version).await?;
    let after = document_version(app, id, to_version).await?;

    let before_body = body_of_version(&before);
    let after_body = body_of_version(&after);

    // Named rather than lumped together: "which of the two is the file" is what the caller has to know to
    // do anything about it, and a document may well have been text at one version and bytes at another.
    let (Some(before_text), Some(after_text)) = (before_body.as_text(), after_body.as_text()) else {
        let which = match (before_body.is_binary(), after_body.is_binary()) {
            (true, true) => format!("versions {from_version} and {to_version} are both files"),
            (true, false) => format!("version {from_version} is a file"),
            _ => format!("version {to_version} is a file"),
        };

        return Err(format!(
            "document {id}: {which}, and bytes do not diff into anything a reader can use. \
             documents_history reports the size and content type of every version"
        ));
    };

    let (diff, truncated) = super::unified_diff(
        before_text,
        after_text,
        &format!("{}@v{from_version}", before.doc_path),
        &format!("{}@v{to_version}", after.doc_path),
        context_lines.clamp(0, super::MAX_DIFF_CONTEXT) as usize,
    );

    Ok(DocumentDiff {
        id: id.to_string(),
        project_id: after.project_id.clone(),
        from_version,
        to_version,
        from_path: before.doc_path,
        to_path: after.doc_path,
        diff,
        truncated,
    })
}

/// Every live document of one project, by path, WITHOUT the payloads.
///
/// **Served from memory.** The index is the one part of a document that is cached — see
/// `crate::documents::DocumentsIndex` — because it is what the tree is drawn from and what a card counts.
/// The payloads are not, which is the whole point of splitting them.
pub fn list_documents(
    app: &AppContext,
    project_prefix: &str,
) -> Result<Vec<DocumentIndexEntry>, String> {
    let project = {
        let board = app.board.read();
        resolve_project_by_prefix(&board, project_prefix)?
    };

    let mut rows = app.documents_index.of_project(&project.id);

    // The connected repositories, in the same list and the same shape. That is what "served like your own
    // folders" means in practice: an agent asks one question and gets the project's own documents and the
    // reference material beside them, told apart by the `github/` root on the paths.
    rows.extend(super::mirror_index_entries(app, &project));

    // Re-sorted rather than appended, because the contract this answers is "sorted by path, so everything
    // under one folder is contiguous" — and two already-sorted lists concatenated are not one sorted list.
    rows.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(rows)
}

/// One project's trash, most recently deleted first, WITHOUT the payloads.
///
/// From Postgres rather than from memory, unlike the index: the trash is not drawn anywhere and is asked for
/// rarely, so caching it would be a second copy to keep in step for nothing.
pub async fn list_trash(
    app: &AppContext,
    project_prefix: &str,
) -> Result<Vec<DocumentTrashIndexDto>, String> {
    let project_id = {
        let board = app.board.read();
        resolve_project_by_prefix(&board, project_prefix)?.id.clone()
    };

    let ctx = MyTelemetryContext::create_empty();

    let mut rows = app
        .documents_repo
        .get_trash_of_project(&project_id, &ctx)
        .await;

    rows.sort_by(|left, right| {
        right
            .deleted
            .unix_microseconds
            .cmp(&left.deleted.unix_microseconds)
    });

    Ok(rows)
}

/// Every version of one document, oldest first, WITHOUT the payloads.
///
/// Works for a trashed document as well as a live one, and that is deliberate: the history is the reason the
/// id is stable, so it must not stop answering the moment somebody deletes the thing.
pub async fn document_history(
    app: &AppContext,
    id: &str,
) -> Result<Vec<DocumentVersionDto>, String> {
    let id = &unwrap_document_reference(id);

    // A mirrored file has no history HERE, which is a different sentence from "no such document" — its
    // history is in the repository it came from. `document_diff` comes through this function, so refusing
    // once covers both.
    super::refuse_reserved_history(id)?;

    let ctx = MyTelemetryContext::create_empty();

    let mut rows = app.documents_repo.get_history(id, &ctx).await;

    if rows.is_empty() {
        return Err(format!(
            "no document {id} — nothing has ever been written under that id"
        ));
    }

    rows.sort_by_key(|itm| itm.version);
    Ok(rows)
}

/// One version of one document, payload included — how an old text or an old file is read back.
pub async fn document_version(
    app: &AppContext,
    id: &str,
    version: i64,
) -> Result<DocumentHistoryDto, String> {
    let id = &unwrap_document_reference(id);

    super::refuse_reserved_history(id)?;

    let ctx = MyTelemetryContext::create_empty();

    match app.documents_repo.get_version(id, version, &ctx).await {
        Some(row) => Ok(row),
        None => {
            // The versions that DO exist, named. A caller off by one otherwise has to guess.
            let existing = app.documents_repo.get_history(id, &ctx).await;

            Err(match existing.iter().map(|itm| itm.version).max() {
                Some(highest) => format!(
                    "document {id} has no version {version} — it has versions 1 to {highest}"
                ),
                None => format!("no document {id}"),
            })
        }
    }
}

/// Fill in `content_type` and `content_size` on rows written before those columns existed.
///
/// **A one-time backfill, run at startup, and the reason it exists is that the fallbacks cannot answer for
/// everything.** A missing content type can be recovered from the path — see [`content_type_of`] — but a
/// missing SIZE cannot be recovered from anything except the payload, and the index deliberately never reads
/// one. So every such document showed as `0 B`, which is not a size, it is a gap dressed as a fact.
///
/// It writes NO history row and does not touch `version`: nothing about the document changed. What was always
/// true about it is merely now written down, which is what makes running this twice a no-op.
///
/// Reads the full row — payload included — for the affected documents only. That is the one place in this
/// feature that pulls payloads in bulk, it happens once per deploy over a handful of rows, and it stops being
/// work at all as soon as there are none left.
/// Give every text document written before this column existed the hash its brief will be filed under.
///
/// **Its own pass rather than a branch in [`backfill_document_columns`], because the two write
/// differently.** That one rewrites whole rows, which is what filling in a size and a type needs; this one
/// writes a single column with `set_content_hash`, so a board of a thousand documents is a thousand small
/// updates rather than a thousand payloads sent back to Postgres to be stored again exactly as they were.
///
/// **It reads text and never bytes.** The work list is selected with `content_hash IS NULL AND
/// binary_content IS NULL`, so a board full of PDFs costs one query and no blobs — and a binary document
/// is left with a NULL hash for ever, which is correct: it is never briefed.
///
/// No history entry and no version bump. Nothing about the document changed; what changed is that this
/// product now knows something it could always have worked out.
pub async fn backfill_content_hashes(app: &AppContext) {
    let ctx = MyTelemetryContext::create_empty();

    let stale = app.documents_repo.get_text_rows_without_hash(&ctx).await;

    if stale.is_empty() {
        return;
    }

    println!(
        "documents: hashing {} text document(s) written before briefs existed",
        stale.len()
    );

    for row in stale {
        let Some(content) = row.content else {
            // Neither text nor binary — an empty row nothing can be said about. Left alone rather than
            // hashed as an empty string, which would file a brief under the hash of nothing.
            continue;
        };

        let hash = crate::documents::content_hash(content.as_bytes());

        app.documents_repo.set_content_hash(&row.id, &hash, &ctx).await;
    }
}

pub async fn backfill_document_columns(app: &AppContext) {
    let ctx = MyTelemetryContext::create_empty();

    let stale: Vec<String> = app
        .documents_repo
        .get_all_indexed(&ctx)
        .await
        .into_iter()
        .filter(|row| row.content_size.is_none() || row.content_type.is_none())
        .map(|row| row.id)
        .collect();

    if stale.is_empty() {
        return;
    }

    println!(
        "documents: filling in content_type and content_size for {} row(s) written before those columns",
        stale.len()
    );

    for id in stale {
        let Some(row) = app.documents_repo.get_by_id(&id, &ctx).await else {
            continue;
        };

        let body = DocumentBody::from_columns(row.content.clone(), row.binary_content.clone());

        let filled = DocumentDto {
            content_type: Some(content_type_of(row.content_type.as_deref(), &row.doc_path)),
            content_size: Some(body.size_bytes()),
            ..row
        };

        app.documents_repo.upsert(&filled, &ctx).await;
    }
}

/// The payload of a live document row.
pub fn body_of(row: &DocumentDto) -> DocumentBody {
    DocumentBody::from_columns(row.content.clone(), row.binary_content.clone())
}

/// The payload of one history row.
pub fn body_of_version(row: &DocumentHistoryDto) -> DocumentBody {
    DocumentBody::from_columns(row.content.clone(), row.binary_content.clone())
}

/// The content type to report for a row whose stored one may be missing, or may be an old guess.
///
/// **The path is consulted before the default, and that is not a nicety — it is the fix for a real bug.** Rows
/// written before this column existed have `NULL` in it, and a flat `text/markdown` fallback made every one of
/// them Markdown: `prototype_draft.html` came back as `text/markdown`, so the viewer did not frame it and drew
/// a web page as a wall of markup. The path knew the answer all along.
///
/// Order: what was stored, then what the path implies, then **bytes**.
///
/// **The last step used to be Markdown, and that was the second half of the same bug.** An extension the table
/// did not know became `text/markdown` — so a mirrored `design-system.css` was served as Markdown, and every
/// browser refused to apply it as a stylesheet, leaving a page that renders bare with two lines in a console
/// nobody has open. Claiming a type we do not know is worse than admitting it: `application/octet-stream`
/// makes the browser offer a download, which is visible, rather than quietly discard the file.
///
/// **A stored `text/markdown` on a path that says otherwise is treated as that old guess rather than as a
/// declaration.** Nothing declared it — it is what every unknown extension became before the table grew, and
/// it is sitting in the rows those builds wrote, on files that are plainly a stylesheet or a script. The
/// narrow reading is deliberate: only the one legacy value, and only when the path has a specific answer of
/// its own, so a `.md` document and anything a caller actually named are both untouched.
pub fn content_type_of(stored: Option<&str>, path: &str) -> String {
    let from_path = content_type_for_path(path);

    if let Some(declared) = stored.map(str::trim).filter(|itm| !itm.is_empty()) {
        let declared = declared.to_lowercase();

        if declared == DEFAULT_TEXT_CONTENT_TYPE
            && let Some(guessed) = from_path.filter(|itm| *itm != DEFAULT_TEXT_CONTENT_TYPE)
        {
            return guessed.to_string();
        }

        return declared;
    }

    from_path.unwrap_or(DEFAULT_BINARY_CONTENT_TYPE).to_string()
}

/// What a caller wants to change about the documents a task or a goal references.
///
/// Add and remove rather than "here is the new list", the same shape labels use and for the same reason: a
/// caller attaching one document must not have to know — or resend — the four that are already there.
///
/// **Unlike a label, a reference that names nothing is refused.** A label is a word and removing one that is
/// not there is harmless; a reference is supposed to point at something a reader can open, so one that
/// resolves to nothing means the caller is working from a stale read or has invented it, and storing a dead
/// link is worse than refusing.
#[derive(Default)]
pub struct DocumentsPatch {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

/// The canonical spelling of a reference, WITHOUT checking that it points at anything.
///
/// Its own function because removal needs it and validation must not run there: a reference to a document
/// that has since been deleted, or to a repository that has since been disconnected, is exactly the
/// reference somebody is trying to take off a card. Comparing canonical spellings is also what lets a
/// caller detach with the name they attached with, whichever of the four spellings that was — and what
/// lets a legacy bare id already on a card be removed by naming its url.
///
/// The reading itself is in `shared`, because the browser does it too — see
/// [`task_manager_shared::documents::read_document_reference`] for the four spellings it accepts.
fn canonical_reference(project_prefix: &str, src: &str) -> String {
    task_manager_shared::documents::canonical_document_reference(project_prefix, src)
}

impl DocumentsPatch {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }

    /// Apply the patch to one reference list, or refuse it entirely.
    ///
    /// `project_id` is the project the task or goal is on: every id added is checked to be a live document of
    /// **that** project. References do not cross projects — a document is owned by one board, and a
    /// reference to one somebody cannot see would read as a broken link to them and as a working one to
    /// whoever wrote it.
    ///
    /// **Checked against the in-memory index**, not Postgres: the index holds every live document's
    /// reference, which is exactly what this question needs, and it is the reason the index exists. Only the
    /// "is it in the trash" half — the branch that produces a better message — goes to the database.
    ///
    /// Removal happens after addition, so an id passed to both ends up removed — the order labels use.
    pub async fn apply(
        &self,
        app: &AppContext,
        project_id: &str,
        ids: &mut Vec<String>,
        owner: &str,
    ) -> Result<(), String> {
        // The board is read once, before any await: `parking_lot`'s guard is `!Send`, and every reference
        // below needs the same two things — what this project is CALLED, since a reference carries a
        // prefix, and which repositories are connected to it.
        let (project_prefix, connections) = {
            let board = app.board.read();
            let project = super::projects::load(&board, project_id)?;

            (project.prefix.clone(), project.github_connections.clone())
        };

        for reference in &self.add {
            let reference = reference.trim();

            if reference.is_empty() {
                return Err("a document reference needs a document to point at".to_string());
            }

            // The two kinds are checked by different things and could not share one branch: a document of
            // the project's own is a row, so the question is whether it is live, and a file in a
            // repository is a working copy, so the question is whether the mirror holds it.
            let named =
                task_manager_shared::documents::read_document_reference(&project_prefix, reference);

            let canonical = match named {
                DocumentReference::Own { project, id } => {
                    require_own_document(app, project_id, &project, &project_prefix, &id, owner)
                        .await?;

                    own_document_reference(&project_prefix, &id)
                }
                DocumentReference::Mirror { project, path } => {
                    require_mirror_file(
                        app,
                        project_id,
                        &project,
                        &project_prefix,
                        &connections,
                        &path,
                        owner,
                    )?;

                    mirror_document_reference(&project_prefix, &path)
                }
            };

            if !ids.iter().any(|itm| itm == &canonical) {
                ids.push(canonical);
            }
        }

        for reference in &self.remove {
            // Compared as canonical spellings rather than as strings, so a caller detaches with whichever
            // of the spellings they have — including a bare id put on the card before references were
            // urls. Unlike an add, removing one that is not there is NOT refused: the caller's intent,
            // "this reference should not be here", is already true and there is nothing for them to fix.
            let canonical = canonical_reference(&project_prefix, reference);

            ids.retain(|itm| canonical_reference(&project_prefix, itm) != canonical);
        }

        // Sorted so that two callers attaching the same documents in different orders end up with
        // identical lists, and nothing compares unequal for a reason nobody can see.
        ids.sort();
        ids.dedup();

        Ok(())
    }
}

/// A reference url turned into the name the documents tools already take, or the string untouched.
///
/// **This is what makes a reference something you can WRITE DOWN.** A reference is a url, so it survives
/// being pasted into a `CLAUDE.md`, an issue or a chat message — and what comes back the other way is
/// somebody handing that url to `documents_get` and expecting the document. Unwrapped at the top of every
/// tool that takes a document's name, so every one of them accepts it: read it, edit it, move it, delete
/// it, diff it, outline it.
///
/// The two forms unwrap to the two names this service already had, which is why nothing downstream had to
/// learn anything:
///
/// ```text
/// raw/TM/document/01K2C4…        →  01K2C4…                        an id, looked up in Postgres
/// raw/TM/github/specs/a.md       →  github:TM:github/specs/a.md    a mirrored file, read off disk
/// ```
///
/// Anything that is not a reference comes back as itself, trimmed — which is every id and every path that
/// was ever passed to these tools, so this can be put in front of all of them without a branch at the
/// call site.
pub fn unwrap_document_reference(src: &str) -> String {
    match task_manager_shared::documents::parse_document_reference(src) {
        Some(DocumentReference::Own { id, .. }) => id,
        Some(DocumentReference::Mirror { project, path }) => {
            task_manager_shared::github::mirror_document_id(&project, &path)
        }
        None => src.trim().to_string(),
    }
}

/// The id slot of a tool that takes `id`, `project` and `path`, with a reference unwrapped out of either.
///
/// **A person who writes a reference down does not know which of the three arguments this product calls
/// it**, and both readings are right: a reference IS an identity, and it also looks like an address. So one
/// arriving as a `path` is moved into the id slot, where identity belongs — otherwise it would be looked
/// for as a document literally called `raw/TM/github/…`, and the miss would say the path is not there
/// rather than that it was read in the wrong slot.
fn unwrap_named_reference(id: Option<&str>, path: Option<&str>) -> Option<String> {
    let named = |src: Option<&str>| {
        src.map(str::trim)
            .filter(|itm| task_manager_shared::documents::parse_document_reference(itm).is_some())
            .map(unwrap_document_reference)
    };

    named(id).or_else(|| named(path)).or_else(|| {
        id.map(str::trim)
            .filter(|itm| !itm.is_empty())
            .map(str::to_string)
    })
}

/// The reference for a document the caller NAMED, whatever they named it by.
///
/// **For the tools that answer about a document they were handed rather than one they read out** — a
/// history, a deletion — so that the answer speaks the vocabulary the question could have been asked in.
/// Everywhere else the row is in hand and the reference is built straight off it.
///
/// A mirrored file needs nothing looked up: its id carries its project. A document of the project's own is
/// found in the index, and then in the trash — which is not a fallback but the common case here, since a
/// history is most often asked for about something that has just been deleted.
pub async fn reference_of(app: &AppContext, named: &str) -> String {
    let id = unwrap_document_reference(named);

    if let Some((project, path)) = task_manager_shared::github::parse_mirror_document_id(&id) {
        return mirror_document_reference(project, path);
    }

    let project_id = match app.documents_index.get(&id) {
        Some(entry) => Some(entry.project_id.clone()),
        None => {
            let ctx = MyTelemetryContext::create_empty();

            app.documents_repo
                .get_trashed(&id, &ctx)
                .await
                .map(|itm| itm.project_id)
        }
    };

    let prefix = project_id.and_then(|project_id| {
        app.board
            .read()
            .get_project(&project_id)
            .map(|itm| itm.prefix.clone())
    });

    match prefix {
        Some(prefix) => own_document_reference(&prefix, &id),
        // Nothing knows where it is — which, for a caller who has just been answered about it, means the
        // row moved between two reads. The id on its own is still a name this product answers to.
        None => id,
    }
}

/// Refuse a reference unless it names a live document of THIS board.
///
/// **Checked against the in-memory index**, not Postgres: the index holds every live document's reference,
/// which is exactly what this question needs, and it is the reason the index exists. Only the "is it in the
/// trash" half — the branch that produces a better message — goes to the database.
async fn require_own_document(
    app: &AppContext,
    project_id: &str,
    named_prefix: &str,
    project_prefix: &str,
    id: &str,
    owner: &str,
) -> Result<(), String> {
    // A reference that spelled out a board has to have spelled out this one. Checked before the lookup so
    // that naming another project's document reads as crossing a boundary rather than as a missing id.
    if !named_prefix.eq_ignore_ascii_case(project_prefix) {
        return Err(format!(
            "that reference is on project {named_prefix} and {owner} is on {project_prefix} — a reference does not cross boards"
        ));
    }

    let Some(entry) = app.documents_index.get(id) else {
        // A trashed document is named as trashed rather than as missing: attaching one is still refused —
        // a reference should point at something a reader can open — but the fix is different, and it is
        // one call away.
        let ctx = MyTelemetryContext::create_empty();

        return Err(match app.documents_repo.get_trashed(id, &ctx).await {
            Some(trashed) => format!(
                "document {id} is in the trash (it was at '{}') — restore it before attaching it to {owner}",
                trashed.doc_path
            ),
            None => format!(
                "no document {id} — list the project's documents and use the `id` each entry reports"
            ),
        });
    };

    if entry.project_id != project_id {
        return Err(format!(
            "document {id} belongs to another project, and a reference does not cross projects — {owner} can only point at documents of its own board"
        ));
    }

    Ok(())
}

/// Refuse a reference unless it names a file this board's mirrors actually hold.
///
/// **The listing is what it is checked against, and that is the same list `documents_list` answered with.**
/// So what can be attached is exactly what the caller was offered — no directory walk, no request to
/// GitHub, and no chance of storing a link to a file that only exists in a stale read.
///
/// Synchronous, unlike its counterpart above: there is no row anywhere to ask about. A mirror is memory and
/// a working copy on disk, so every question this asks is answered without leaving the process.
///
/// **The three ways it can miss say three different things**, because the fix is different for each: the
/// repository is not connected to this board at all, the connection has never been read, or the file is not
/// in what was read. Only the last one means the caller got the path wrong.
fn require_mirror_file(
    app: &AppContext,
    project_id: &str,
    named_prefix: &str,
    project_prefix: &str,
    connections: &[crate::board::GithubConnectionModel],
    path: &str,
    owner: &str,
) -> Result<(), String> {
    if !named_prefix.eq_ignore_ascii_case(project_prefix) {
        return Err(format!(
            "'{path}' is a file on project {named_prefix} and {owner} is on {project_prefix} — a connected repository belongs to the board it was connected to, and a reference does not cross boards"
        ));
    }

    let Some((connection_name, relative)) =
        task_manager_shared::github::parse_github_mirror_path(path)
    else {
        return Err(format!(
            "'{path}' is not a file in a connected repository — those are `{}/<repository>/<file>`, exactly as documents_list reports them",
            task_manager_shared::github::GITHUB_ROOT
        ));
    };

    if !connections.iter().any(|itm| itm.name == connection_name) {
        return Err(format!(
            "this project has no connected repository called '{connection_name}' — projects_list and the Documents screen both show which ones it has"
        ));
    }

    let mirror = app.github.get_or_pending(project_id, connection_name);

    if mirror.entries.iter().any(|itm| itm.path == relative) {
        return Ok(());
    }

    // Nothing has been read yet, so this is not the caller being wrong — it is a repository that has not
    // been pulled since the service started, which for a private one means a key nobody has typed back in.
    if mirror.entries.is_empty()
        && mirror.state != task_manager_shared::github::GithubMirrorState::READY
    {
        return Err(format!(
            "'{connection_name}' has not been read yet — its state is '{}'{}, so there is nothing to check '{relative}' against. Attach it once the repository has been pulled",
            mirror.state,
            match mirror.error.is_empty() {
                true => String::new(),
                false => format!(": {}", mirror.error),
            }
        ));
    }

    Err(format!(
        "'{relative}' is not in '{connection_name}' — list the project's documents and use the `path` each entry reports"
    ))
}

/// The author of a write, or a refusal.
///
/// Required for the same reason a comment's author is: MCP has no session, so the caller passes one, and a
/// history of anonymous versions would not answer the question it exists to answer.
pub(crate) fn require_author(who: &str) -> Result<String, String> {
    let who = who.trim();

    if who.is_empty() {
        return Err(
            "a document write needs an author — pass `who` as an email, or `AI`".to_string(),
        );
    }

    Ok(super::normalise_actor(who))
}

/// One live document, or a refusal that says which of the two reasons it is.
async fn require_live(
    app: &AppContext,
    id: &str,
    ctx: &MyTelemetryContext,
) -> Result<DocumentDto, String> {
    if let Some(row) = app.documents_repo.get_by_id(id, ctx).await {
        return Ok(row);
    }

    Err(match app.documents_repo.get_trashed(id, ctx).await {
        Some(trashed) => format!(
            "document {id} is in the trash (it was at '{}') — restore it first",
            trashed.doc_path
        ),
        None => format!("no document {id}"),
    })
}

/// Write one version, then the document itself.
///
/// **The order is the point, and it is the same on every path through this module.** History is a second
/// table — the only place in this service where one change touches two — so there is no transaction to hide
/// behind, and one of the two writes has to go first. History does, because the failure it leaves behind is
/// a version nobody is serving: one row too many, which the next attempt overwrites, since history is
/// upserted rather than inserted. The other order loses a version outright, and a history with a hole in it
/// is not a history.
///
/// Memory comes after both, in the callers — the same order `scripts/` writes everything else in: Postgres
/// first, then the copy that is served.
async fn write_version(
    app: &AppContext,
    row: &DocumentDto,
    who: &str,
    event: DocumentEvent,
    ctx: &MyTelemetryContext,
) {
    app.documents_repo
        .upsert_history(
            &DocumentHistoryDto {
                document_id: row.id.clone(),
                version: row.version,
                project_id: row.project_id.clone(),
                doc_path: row.doc_path.clone(),
                content_type: row.content_type.clone(),
                content: row.content.clone(),
                binary_content: row.binary_content.clone(),
                content_size: row.content_size,
                who: who.to_string(),
                event: event.as_str().to_string(),
                moment: row.updated,
            },
            ctx,
        )
        .await;

    app.documents_repo.upsert(row, ctx).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An agent holds whichever name the tool it last called handed back, so all four spellings of one
    /// file have to reach the same file — otherwise attaching a document becomes a question of which
    /// listing you read it from.
    #[test]
    fn every_spelling_of_one_mirrored_file_is_the_same_reference() {
        let expected = "raw/TM/github/specs/design/system.md";

        for spelling in [
            // The canonical reference, and the url it resolves to.
            "raw/TM/github/specs/design/system.md",
            "/raw/TM/github/specs/design/system.md",
            // The `id` a mirrored file carries in documents_list.
            "github:TM:github/specs/design/system.md",
            // The `path` from that same listing, which names a file only on the board it is read with.
            "github/specs/design/system.md",
        ] {
            assert_eq!(
                canonical_reference("TM", spelling),
                expected,
                "'{spelling}' did not read as the file it names"
            );
        }
    }

    /// The other kind, and the one that has an id: what is stored is the reference, and what a caller may
    /// have passed is the bare id every reference was before this vocabulary existed.
    #[test]
    fn a_document_of_the_projects_own_is_named_by_its_id_either_way() {
        let expected = "raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8";

        for spelling in [
            "01K2C4Q0S1T2U3V4W5X6Y7Z8",
            "  01K2C4Q0S1T2U3V4W5X6Y7Z8  ",
            "raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8",
            "/raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8",
        ] {
            assert_eq!(canonical_reference("TM", spelling), expected);
        }
    }

    /// A reference names its own board, and one that names another must NOT be quietly adopted by the card
    /// it was passed to — the check that refuses it needs the project the caller actually wrote.
    #[test]
    fn a_reference_keeps_the_board_it_names() {
        assert_eq!(
            canonical_reference("TM", "raw/RMS/github/specs/a.md"),
            "raw/RMS/github/specs/a.md"
        );
        assert_eq!(
            canonical_reference("TM", "github:RMS:github/specs/a.md"),
            "raw/RMS/github/specs/a.md"
        );

        // And a spelling that names no board takes the one the card is on, which is the only board it
        // could sensibly mean.
        assert_eq!(
            canonical_reference("TM", "github/specs/a.md"),
            "raw/TM/github/specs/a.md"
        );
    }

    /// What every documents tool takes: a reference in, the name that tool already understood out.
    #[test]
    fn a_reference_unwraps_to_the_name_the_tools_take() {
        assert_eq!(
            unwrap_document_reference("raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8"),
            "01K2C4Q0S1T2U3V4W5X6Y7Z8"
        );
        assert_eq!(
            unwrap_document_reference("raw/TM/github/specs/a.md"),
            "github:TM:github/specs/a.md"
        );

        // Not a reference: back as itself, which is what lets this sit in front of every tool without a
        // branch at the call site.
        assert_eq!(
            unwrap_document_reference("  01K2C4Q0S1T2U3V4W5X6Y7Z8  "),
            "01K2C4Q0S1T2U3V4W5X6Y7Z8"
        );
        assert_eq!(unwrap_document_reference("docs/a.md"), "docs/a.md");
    }

    /// A reference identifies a document outright, so it belongs in the id slot however it arrived —
    /// looked for as a path it would miss, and the miss would blame the path.
    #[test]
    fn a_reference_written_into_the_path_slot_is_still_an_identity() {
        assert_eq!(
            unwrap_named_reference(None, Some("raw/TM/github/specs/a.md")),
            Some("github:TM:github/specs/a.md".to_string())
        );

        // An id beside a path still wins, exactly as it does without any reference involved.
        assert_eq!(
            unwrap_named_reference(Some("01K2C4"), Some("docs/a.md")),
            Some("01K2C4".to_string())
        );

        // And an ordinary path is left in the path slot, where it names a document on the project beside it.
        assert_eq!(unwrap_named_reference(None, Some("docs/a.md")), None);
    }

    /// The five events have to be distinct strings, or a history cannot tell a rewrite from a move — which is
    /// the one distinction it exists for.
    #[test]
    fn every_event_has_its_own_name() {
        let names = [
            DocumentEvent::Created.as_str(),
            DocumentEvent::Updated.as_str(),
            DocumentEvent::Moved.as_str(),
            DocumentEvent::Deleted.as_str(),
            DocumentEvent::Restored.as_str(),
        ];

        let mut sorted = names.to_vec();
        sorted.sort();
        sorted.dedup();

        assert_eq!(sorted.len(), names.len(), "two events share a name");
    }

    #[test]
    fn an_author_is_required_and_normalised() {
        assert!(require_author("").is_err());
        assert!(require_author("   ").is_err());

        assert_eq!(require_author("  Yuri@MXTM.ai ").unwrap(), "yuri@mxtm.ai");
        // `AI` survives whatever case it arrived in, exactly as a comment's author does — the two must not
        // drift, or a filter on one spelling misses the other.
        assert_eq!(require_author("ai").unwrap(), "AI");
    }

    fn index_entry(id: &str, path: &str) -> DocumentIndexEntry {
        DocumentIndexEntry {
            id: id.to_string(),
            project_id: "P".to_string(),
            path: path.to_string(),
            content_type: "text/markdown".to_string(),
            is_binary: false,
            size: 0,
            content_hash: Some(crate::documents::content_hash(path.as_bytes())),
            version: 1,
            created: DateTimeAsMicroseconds::new(0),
            updated: DateTimeAsMicroseconds::new(0),
            updated_by: "AI".to_string(),
        }
    }

    fn paths_in(entries: &[DocumentIndexEntry], folder: &str) -> Vec<String> {
        documents_in_folder(entries, folder)
            .into_iter()
            .map(|(_, path)| path)
            .collect()
    }

    /// Deleting a folder takes the SUBTREE, which is what deleting a folder means everywhere else.
    #[test]
    fn a_folder_is_everything_under_it() {
        let entries = [
            index_entry("1", "docs/notes.md"),
            index_entry("2", "docs/design/system.md"),
            index_entry("3", "docs/design/api/v2.md"),
            index_entry("4", "readme.md"),
        ];

        assert_eq!(
            paths_in(&entries, "docs"),
            vec![
                "docs/design/api/v2.md",
                "docs/design/system.md",
                "docs/notes.md"
            ]
        );

        // A subfolder is just a longer prefix, and taking it leaves the rest of the tree alone.
        assert_eq!(
            paths_in(&entries, "docs/design"),
            vec!["docs/design/api/v2.md", "docs/design/system.md"]
        );
    }

    /// The slash in the prefix is the whole rule: a neighbour that merely starts with the same letters is
    /// not inside, and a DOCUMENT sharing a folder's name is not the folder.
    #[test]
    fn only_what_is_really_inside_is_taken() {
        let entries = [
            index_entry("1", "docs/notes.md"),
            index_entry("2", "docs-old/notes.md"),
            index_entry("3", "docs"),
            index_entry("4", "archive/docs/notes.md"),
        ];

        assert_eq!(paths_in(&entries, "docs"), vec!["docs/notes.md"]);
    }

    /// A folder nobody has anything in does not exist — there is nothing to delete and nothing to report,
    /// and the caller is told rather than answered with an empty success.
    #[test]
    fn a_folder_with_nothing_in_it_matches_nothing() {
        let entries = [index_entry("1", "docs/notes.md")];

        assert!(paths_in(&entries, "drafts").is_empty());
        // Case is significant in a path, so it is significant here.
        assert!(paths_in(&entries, "Docs").is_empty());
    }

    /// A trailing slash is how a person writes a folder, so it is taken rather than refused — everything
    /// else is the grammar of a path, checked by the one function that owns it.
    #[test]
    fn a_folder_is_written_the_way_people_write_folders() {
        assert_eq!(normalise_folder(" docs/design/ ").unwrap(), "docs/design");
        assert_eq!(normalise_folder("/docs/").unwrap(), "docs");
        assert_eq!(normalise_folder("docs\\design").unwrap(), "docs/design");

        // "everything" is not a folder, and there is deliberately no way to say it.
        assert!(normalise_folder("").is_err());
        assert!(normalise_folder("   ").is_err());
        assert!(normalise_folder("/").is_err());

        assert!(normalise_folder("../etc").is_err());
    }

    #[test]
    fn an_empty_patch_is_recognised() {
        assert!(DocumentsPatch::default().is_empty());

        assert!(
            !DocumentsPatch {
                add: vec!["some-id".to_string()],
                ..Default::default()
            }
            .is_empty()
        );

        assert!(
            !DocumentsPatch {
                remove: vec!["some-id".to_string()],
                ..Default::default()
            }
            .is_empty()
        );
    }

    /// Exactly one column each way. This is the invariant the two nullable columns cannot express on their
    /// own, so it is worth a test rather than a comment.
    #[test]
    fn a_payload_writes_exactly_one_column() {
        let (text, binary) = DocumentBody::Text("hello".to_string()).into_columns();
        assert_eq!(text.as_deref(), Some("hello"));
        assert!(binary.is_none());

        let (text, binary) = DocumentBody::Binary(vec![1, 2, 3]).into_columns();
        assert!(text.is_none());
        assert_eq!(binary, Some(vec![1, 2, 3]));
    }

    #[test]
    fn a_payload_round_trips_through_its_columns() {
        for body in [
            DocumentBody::Text("# spec".to_string()),
            DocumentBody::Binary(vec![0, 1, 2, 255]),
        ] {
            let (text, binary) = body.clone().into_columns();
            assert_eq!(DocumentBody::from_columns(text, binary), body);
        }
    }

    /// A row with neither column set must read as something rather than break: writes enforce the invariant,
    /// reads are lenient — the rule the whole service follows.
    #[test]
    fn a_row_with_no_payload_reads_as_empty_text() {
        assert_eq!(
            DocumentBody::from_columns(None, None),
            DocumentBody::Text(String::new())
        );
    }

    /// Bytes for both kinds. Two units would make a listing's size column mean two different things.
    #[test]
    fn size_is_bytes_whichever_kind_it_is() {
        // Two bytes in UTF-8, one character — which is exactly why the unit has to be said out loud.
        assert_eq!(DocumentBody::Text("é".to_string()).size_bytes(), 2);
        assert_eq!(DocumentBody::Binary(vec![0; 10]).size_bytes(), 10);
    }

    /// The check that stops a write failing inside the driver with no reason attached.
    #[test]
    fn a_nul_byte_is_refused_in_text() {
        assert!(DocumentBody::Text("a\0b".to_string()).validate().is_err());
        assert!(DocumentBody::Text("ab".to_string()).validate().is_ok());

        // In BINARY it is perfectly normal — every PDF has them.
        assert!(DocumentBody::Binary(vec![0, 1, 0]).validate().is_ok());
    }

    #[test]
    fn an_oversized_payload_is_refused() {
        let long = "a".repeat(MAX_CONTENT_LEN + 1);
        assert!(DocumentBody::Text(long).validate().is_err());

        let big = vec![0u8; MAX_BINARY_LEN + 1];
        assert!(DocumentBody::Binary(big).validate().is_err());
    }

    #[test]
    fn an_empty_file_is_refused_but_an_empty_text_is_not() {
        assert!(DocumentBody::Binary(Vec::new()).validate().is_err());
        // An empty text document is a real thing somebody created and will fill in.
        assert!(DocumentBody::Text(String::new()).validate().is_ok());
    }

    /// What the caller said wins; then the path; then a default that depends on the kind of payload.
    #[test]
    fn the_content_type_is_declared_then_guessed_then_defaulted() {
        let declared = NewDocumentContent {
            body: DocumentBody::Binary(vec![1]),
            content_type: Some("  Application/PDF ".to_string()),
        };
        assert_eq!(
            declared.resolve_content_type("whatever.bin"),
            "application/pdf",
            "a declared type is taken, lower-cased"
        );

        let guessed = NewDocumentContent {
            body: DocumentBody::Binary(vec![1]),
            content_type: None,
        };
        assert_eq!(
            guessed.resolve_content_type("docs/spec.pdf"),
            "application/pdf"
        );

        let binary_fallback = NewDocumentContent {
            body: DocumentBody::Binary(vec![1]),
            content_type: None,
        };
        assert_eq!(
            binary_fallback.resolve_content_type("docs/thing"),
            DEFAULT_BINARY_CONTENT_TYPE
        );

        let text_fallback = NewDocumentContent {
            body: DocumentBody::Text("x".to_string()),
            content_type: None,
        };
        assert_eq!(
            text_fallback.resolve_content_type("docs/thing"),
            DEFAULT_TEXT_CONTENT_TYPE,
            "unknown text is still text"
        );
    }

    /// A row written before these columns existed has to read as what its PATH says, not as Markdown.
    ///
    /// This is the regression test for the bug it fixes: `.html` with no stored type was coming back
    /// `text/markdown`, so the viewer refused to frame it and showed the markup.
    #[test]
    fn a_row_predating_the_column_falls_back_to_its_path() {
        assert_eq!(
            content_type_of(None, "docs/pbi-004_prototype_draft.html"),
            "text/html",
            "an html document must not read as Markdown just because nothing was stored"
        );
        assert_eq!(content_type_of(None, "docs/spec.pdf"), "application/pdf");
        assert_eq!(content_type_of(None, "docs/notes.md"), DEFAULT_TEXT_CONTENT_TYPE);

        // A name the table knows without an extension.
        assert_eq!(content_type_of(None, "Makefile"), "text/plain");
        assert_eq!(content_type_of(Some("  "), "a.html"), "text/html");

        // A stored type still wins over the path — it was said on purpose.
        assert_eq!(
            content_type_of(Some("application/pdf"), "a.md"),
            "application/pdf"
        );
    }

    /// **Nothing known, so nothing claimed.** The old fallback said `text/markdown` about every extension
    /// the table had not heard of, which is how a stylesheet came to be served as Markdown and silently
    /// dropped by the browser. Bytes are the honest answer: the browser offers a download, which somebody
    /// can see, instead of discarding the file behind a clean-looking page.
    #[test]
    fn an_extension_nobody_knows_is_bytes_rather_than_a_guess() {
        assert_eq!(
            content_type_of(None, "prototype.unknownext"),
            DEFAULT_BINARY_CONTENT_TYPE
        );
        assert_eq!(
            content_type_of(None, "docs/thing"),
            DEFAULT_BINARY_CONTENT_TYPE
        );
    }

    /// The rows the old fallback already wrote. A stored `text/markdown` on a path that plainly says
    /// otherwise is that guess, not a declaration — so the path wins and the file starts working without
    /// anybody re-uploading it.
    #[test]
    fn a_stored_markdown_guess_gives_way_to_what_the_path_says() {
        assert_eq!(
            content_type_of(Some("text/markdown"), "design-system/css/design-system.css"),
            "text/css"
        );
        assert_eq!(
            content_type_of(Some("text/markdown"), "js/design-system.js"),
            "text/javascript"
        );
        assert_eq!(
            content_type_of(Some("text/markdown"), "build.py"),
            "text/plain"
        );

        // A real Markdown document is untouched — the path agrees with what is stored.
        assert_eq!(
            content_type_of(Some("text/markdown"), "docs/notes.md"),
            DEFAULT_TEXT_CONTENT_TYPE
        );

        // And so is one whose path says nothing: there is no better answer to swap in.
        assert_eq!(
            content_type_of(Some("text/markdown"), "docs/thing"),
            DEFAULT_TEXT_CONTENT_TYPE
        );

        // Only that one legacy value gives way. Anything else a caller stored is a decision.
        assert_eq!(
            content_type_of(Some("text/plain"), "a.css"),
            "text/plain",
            "a declared type is not overridden"
        );
    }
}
