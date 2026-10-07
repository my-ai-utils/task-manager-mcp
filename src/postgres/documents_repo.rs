use std::sync::Arc;
use std::time::Duration;

use service_sdk::my_postgres::sql_where::{NoneWhereModel, StaticLineWhereModel};
use service_sdk::my_postgres::{MyPostgres, UpdateConflictType};

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "documents";
pub const PK_NAME: &str = "documents_pk";

pub const HISTORY_TABLE_NAME: &str = "documents_history";
pub const HISTORY_PK_NAME: &str = "documents_history_pk";

pub const TRASH_TABLE_NAME: &str = "documents_trash";
pub const TRASH_PK_NAME: &str = "documents_trash_pk";

pub const BRIEFS_TABLE_NAME: &str = "document_briefs";
pub const BRIEFS_PK_NAME: &str = "document_briefs_pk";

// One document, as it currently stands.
//
// **The primary key is `id` alone, and it is a `SortableId`** — unlike a task or a goal, which are
// `(project_id, number)` out of the project's counter. Three things follow from that, and all three are the
// point of the design:
//
// * a document has no handle of the `RMS-7` shape, so it is never renamed by a prefix moving between
//   projects and there is nothing to compose on read;
// * a reference to a document — from a task, from a goal — is just this string, so it resolves without
//   knowing which project it came from;
// * and because the id is minted once and never reused, the whole history of one document is stitched
//   together by it, however many times its path or its text changed.
//
// `doc_path` rather than `path` because `path` is a geometric TYPE name in Postgres and the schema
// generator does not quote identifiers — the same defensiveness `task_text` was named with. It carries the
// document's name AND its position: folders are derived from it and stored nowhere, which is why moving a
// document is one write to one column.
//
// The `(project_id, doc_path)` index is UNIQUE, and it is the backstop under the path-is-the-key rule: an
// upload finds the document by its path, so two rows at one path would make "which document is at
// docs/a.md" a question with two answers. The application checks first; this is what stops two writes
// racing past that check.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct DocumentDto {
    #[primary_key(0)]
    pub id: String,
    #[db_index(id: 0, index_name: "documents_path_idx", is_unique: true, order: "ASC")]
    pub project_id: String,
    #[db_index(id: 1, index_name: "documents_path_idx", is_unique: true, order: "ASC")]
    pub doc_path: String,
    // What the payload IS — a MIME type: `text/markdown`, `application/pdf`. Metadata rather than a
    // discriminator: which of the two payload columns is filled is what decides whether this document is
    // text or binary, and this says how to render or download it.
    //
    // NULLABLE for the reason every column added to an existing table here is: the generator derives
    // nullability from the Rust type, and a non-Option field emits `not null`, which Postgres refuses on a
    // populated table. `None` reads as `text/markdown`, which is what every document written before this
    // column existed was.
    pub content_type: Option<String>,
    // The text half. `None` for a binary document.
    //
    // Nullable now, where it used to be NOT NULL — my-postgres can relax NOT NULL to NULL, which is the one
    // direction it can do (it cannot tighten it, and cannot change a type).
    pub content: Option<String>,
    // The binary half. `None` for a text document.
    //
    // **EXACTLY ONE of `content` and `binary_content` is set.** Not an enum in the row because a column
    // cannot be one; the invariant is established on the write path — see `crate::scripts::DocumentBody` —
    // and every read decides which kind of document it is holding by asking which one is `Some`.
    //
    // Real bytes in `bytea`, NOT base64. base64 exists on this feature only because JSON cannot carry
    // bytes, so it is what an MCP argument and an HTTP response carry; storing it would cost a third more
    // space on every version and leave the column unreadable for nothing.
    pub binary_content: Option<Vec<u8>>,
    // How big the payload is, in BYTES — UTF-8 bytes for text, real bytes for binary. One unit, so a list of
    // documents is comparable whichever kind each of them is.
    //
    // **Stored rather than computed, and that is the point of it.** The index wants a size, and computing one
    // means reading the payload — which for a PDF is the whole blob, for every document in the project, on
    // every open of the Documents screen. A number written once on upload costs nothing to read.
    pub content_size: Option<i64>,
    // sha256 of the payload, lowercase hex — what this document's BRIEF is filed under. See
    // `crate::documents::content_hash`.
    //
    // Stored rather than computed for the same reason `content_size` is, and it matters more here: the
    // index has no payload, so "which documents have not been briefed yet" would otherwise be a read of
    // every document in the project. Written on every write that changes the payload; carried unchanged by
    // a move, which does not.
    //
    // NULL means one of two things and neither needs telling apart: a binary document, which is never
    // briefed, or a row written before this column existed, which `backfill_content_hashes` fills at the
    // next start. Both read as "no brief yet", which is the truth.
    pub content_hash: Option<String>,
    // Which version this row is. Starts at 1 and moves on every write of any kind — a rewrite, a move, a
    // delete, a restore — so it doubles as the count of history rows this document has.
    pub version: i64,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
    #[sql_type("timestamp")]
    pub updated: DateTimeAsMicroseconds,
    // Who wrote this version — an email or the literal `AI`, unvalidated for the same reason a comment's
    // author is: MCP has no session to derive one from, and an author whose user row was later removed
    // still has to render.
    pub updated_by: String,
}

// One version of one document, for ever.
//
// **A separate table, and the one place in this service where two tables are written for one change.**
// Everywhere else state rides on its own row precisely to avoid that — a comment lives inside the task's
// jsonb so appending one is a single atomic upsert. History cannot: it grows without bound and would turn
// every document read into a read of every version it ever had.
//
// So the write order is fixed and load-bearing: **history first, then the working table.** If the second
// write fails, the history says a version exists that nothing is serving — one row too many, which the next
// attempt overwrites, because history is upserted rather than inserted. The other order would lose a
// version outright on the same failure, and a history with a hole in it is not a history.
//
// `content` is stored whole per version rather than as a diff. Documents are read one at a time out of
// Postgres, so nothing here has to walk a chain to answer a question — and a chain of diffs is a structure
// whose failure mode is "the oldest version is unreadable", which is the one thing this table exists to
// prevent.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct DocumentHistoryDto {
    #[primary_key(0)]
    pub document_id: String,
    #[primary_key(1)]
    pub version: i64,
    pub project_id: String,
    // The path the document had at THIS version — which is why a move is a version of its own. Without it,
    // the history could say what a document said but never when it moved or who moved it.
    pub doc_path: String,
    // The payload as it was at THIS version — same split, same invariant as the working row. A version of a
    // PDF is a PDF.
    pub content_type: Option<String>,
    pub content: Option<String>,
    pub binary_content: Option<Vec<u8>>,
    pub content_size: Option<i64>,
    pub who: String,
    // What happened: see `crate::scripts::DocumentEvent`. A plain string with no constraint, the same
    // leniency every open vocabulary in this service gets — an unrecognised value renders as itself rather
    // than failing a read of the history.
    pub event: String,
    #[sql_type("timestamp")]
    pub moment: DateTimeAsMicroseconds,
}

// A deleted document.
//
// **A second table rather than a flag on the first.** With a flag, every read of the working set — the
// index, the path lookup, the uniqueness check — would have to remember to exclude the deleted, and the one
// that forgot would be a bug nobody notices until a path that looks free refuses to take a document. Moving
// the row out means the working table contains exactly the live documents and no read has a filter to get
// wrong.
//
// The trash is FLAT: it keeps `doc_path` as the last path the document had, and nothing draws a folder tree
// out of it. It is not a place you browse — it is a list you ask for when something needs restoring.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct DocumentTrashDto {
    #[primary_key(0)]
    pub id: String,
    pub project_id: String,
    // The path it had when it was deleted. Where a restore puts it back, unless the caller names another
    // one — and where the refusal comes from when something else has taken that path since.
    pub doc_path: String,
    pub content_type: Option<String>,
    pub content: Option<String>,
    pub binary_content: Option<Vec<u8>>,
    pub content_size: Option<i64>,
    // Carried across so the version sequence continues rather than restarting: a restored document's next
    // version is one past the version it was deleted at, and its history stays one unbroken run.
    pub version: i64,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
    #[sql_type("timestamp")]
    pub deleted: DateTimeAsMicroseconds,
    pub deleted_by: String,
}

// One document WITHOUT its payload — the shape every listing reads.
//
// **The whole reason this type exists is that a document can now be a PDF.** `SelectDbEntity` builds its
// column list from the struct's fields, so leaving the two payload columns out means the query never touches
// them: drawing the Documents tree for a project costs a few hundred bytes a row instead of every blob in it.
// `content_size` is a stored column precisely so this shape can still report a size.
//
// The first version of this feature selected whole rows for the index and said in a comment that the day
// that stopped being true, this was the function to split. Binary payloads are that day.
#[derive(SelectDbEntity, Debug)]
pub struct DocumentIndexDto {
    pub id: String,
    pub project_id: String,
    pub doc_path: String,
    pub content_type: Option<String>,
    pub content_size: Option<i64>,
    pub content_hash: Option<String>,
    pub version: i64,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
    #[sql_type("timestamp")]
    pub updated: DateTimeAsMicroseconds,
    pub updated_by: String,
}

// What one text SAYS, in a few sentences somebody wrote so that nobody has to read it to find out.
//
// **Keyed by the hash of the content and by nothing else — no project, no path, no document id.** That is
// the whole design in one column: the same specification stored as a document on one board, as a file in a
// connected repository on another, and as a copy somebody synced into a third is one text, and summarising
// it three times is three chances to describe it differently. It is also what makes a brief expire by
// itself: an edit produces other bytes, so the document is unbriefed again without anybody remembering to
// say so.
//
// The table is therefore GLOBAL rather than per project, and that is deliberate rather than sloppy: a
// brief describes a text, and a text does not belong to a board. Nothing in it is private — it is prose
// about content the reader can already open.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct DocumentBriefDto {
    #[primary_key(0)]
    pub content_hash: String,
    pub brief: String,
    // Who wrote it — an email or the literal `AI`, exactly as a document's `updated_by`. Unvalidated for
    // the same reason: MCP has no session to derive one from.
    pub updated_by: String,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
    #[sql_type("timestamp")]
    pub updated: DateTimeAsMicroseconds,
}

// One text document that has never been hashed, and the text to hash — the backfill's work list.
//
// Its own select model rather than reading whole rows, because the difference is the `bytea` column: a
// board with a few hundred PDFs in it would otherwise pull every one of them out of Postgres at startup to
// discover it has nothing to do with them.
#[derive(SelectDbEntity, Debug)]
pub struct DocumentHashBackfillDto {
    pub id: String,
    pub content: Option<String>,
}

// One document's hash, written on its own.
//
// **A partial update rather than an upsert of the whole row, and that is not an optimisation.** The
// backfill runs at startup over documents nobody asked about; writing whole rows back would rewrite every
// payload it touched, and any field this process read a moment before somebody else wrote would be put
// back as it was. This says the one thing it knows.
#[derive(UpdateDbEntity, Debug)]
pub struct DocumentHashUpdateDto {
    #[primary_key(0)]
    pub id: String,
    pub content_hash: Option<String>,
}

// One history entry WITHOUT its payload, for the same reason: a document rewritten twenty times would
// otherwise answer "show me its history" with twenty copies of itself.
#[derive(SelectDbEntity, Debug)]
pub struct DocumentVersionDto {
    pub version: i64,
    pub doc_path: String,
    pub content_type: Option<String>,
    pub content_size: Option<i64>,
    pub who: String,
    pub event: String,
    #[sql_type("timestamp")]
    pub moment: DateTimeAsMicroseconds,
}

// One document's TEXT and nothing else — the shape a project-wide search reads.
//
// **`binary_content` is deliberately absent from this struct, which is what makes the query safe to run over
// a whole project.** `SelectDbEntity` builds its column list from the fields, so a search never touches a
// single blob: a project holding forty PDFs costs the same as one holding none. A binary document comes back
// with `content: None` and is skipped by the caller, which is the honest answer — there is no text in it to
// search.
//
// This is the one read in the feature that pulls payloads in bulk, and it is bounded by the same thing that
// bounds a document: `MAX_CONTENT_LEN` per row, one project at a time. The alternative — pushing the match
// into SQL with `ILIKE` — would return less over the wire and cannot express what the tool offers (a regex,
// a line number, the lines around a hit), so the text has to come here either way.
#[derive(SelectDbEntity, Debug)]
pub struct DocumentTextDto {
    pub id: String,
    pub doc_path: String,
    pub content: Option<String>,
}

// One trashed document WITHOUT its payload. Restoring needs the payload and reads the full row; LISTING the
// trash does not.
#[derive(SelectDbEntity, Debug)]
pub struct DocumentTrashIndexDto {
    pub id: String,
    pub doc_path: String,
    pub content_type: Option<String>,
    pub content_size: Option<i64>,
    #[sql_type("timestamp")]
    pub deleted: DateTimeAsMicroseconds,
    pub deleted_by: String,
}

// Everything in one project, for the index and the trash list alike.
#[derive(WhereDbModel, Debug)]
pub struct ByProjectWhereModel<'s> {
    pub project_id: &'s str,
}

// One document by id. The whole reason the primary key is the id alone.
#[derive(WhereDbModel, Debug)]
pub struct ByIdWhereModel<'s> {
    pub id: &'s str,
}

// One document by where it lives. What an upload asks before it decides whether it is creating or
// overwriting.
#[derive(WhereDbModel, Debug)]
pub struct ByPathWhereModel<'s> {
    pub project_id: &'s str,
    pub doc_path: &'s str,
}

// Every version of one document.
#[derive(WhereDbModel, Debug)]
pub struct ByDocumentWhereModel<'s> {
    pub document_id: &'s str,
}

// One version of one document.
#[derive(WhereDbModel, Debug)]
pub struct ByVersionWhereModel<'s> {
    pub document_id: &'s str,
    pub version: i64,
}

/// The three tables documents live in, behind one type.
///
/// One repo rather than three because they are one thing: no caller ever wants the history of a document
/// without the document, or the trash without knowing what is live. Keeping them together is also what
/// keeps the write order — history first — in one place instead of at every call site.
///
/// **Unlike every other repo here, this one is on the read path.** Projects, tasks and goals are loaded once
/// at startup and served from `Board` afterwards; documents are not held in memory at all and every read
/// comes here. That is deliberate: a document is a text somebody opens occasionally, and the board is
/// pushed whole down a WebSocket on every change — putting documents in it would ship every text to every
/// screen on every write.
pub struct DocumentsRepo {
    postgres: MyPostgres,
}

impl DocumentsRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<DocumentDto>(TABLE_NAME, Some(PK_NAME.into()))
            .with_table_schema_verification::<DocumentHistoryDto>(
                HISTORY_TABLE_NAME,
                Some(HISTORY_PK_NAME.into()),
            )
            .with_table_schema_verification::<DocumentTrashDto>(
                TRASH_TABLE_NAME,
                Some(TRASH_PK_NAME.into()),
            )
            // The fourth table on this connection, beside the three that already share it: briefs are read
            // and written with documents, and a table of a few hundred short strings does not earn a
            // Postgres connection of its own.
            .with_table_schema_verification::<DocumentBriefDto>(
                BRIEFS_TABLE_NAME,
                Some(BRIEFS_PK_NAME.into()),
            )
            .build()
            .await;

        Self { postgres }
    }

    /// Every live document of EVERY project, WITHOUT the payloads. Called once at startup to fill the index.
    pub async fn get_all_indexed(&self, ctx: &MyTelemetryContext) -> Vec<DocumentIndexDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("documents: query_rows get_all_indexed failed")
    }

    /// Every brief there is. Called once at startup, exactly as the documents index is filled.
    ///
    /// All of them rather than a project's worth: they are keyed by content and a listing joins them by
    /// hash, so there is no project to filter by — and the whole table is a few hundred short strings.
    pub async fn get_all_briefs(&self, ctx: &MyTelemetryContext) -> Vec<DocumentBriefDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(BRIEFS_TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("document_briefs: query_rows get_all_briefs failed")
    }

    /// Write one brief, replacing whatever was filed under that hash.
    ///
    /// Replacing rather than refusing: a second reader of the same text who writes a better brief is
    /// improving it, and there is nothing here worth a conflict — the content it describes cannot have
    /// changed, because the content is the key.
    pub async fn upsert_brief(&self, row: &DocumentBriefDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                BRIEFS_TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(BRIEFS_PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("document_briefs: insert_or_update_db_entity failed");
    }

    /// Every text document with no content hash yet, with its text — what the startup backfill works
    /// through.
    ///
    /// **The filter is a static line rather than a where-model, and that is load-bearing.** A derived
    /// `WhereDbModel` whose only field is an `Option` set to `None` renders NO `WHERE` clause at all —
    /// the model reports it has no conditions and the builder leaves the clause off — so the "documents
    /// with no hash" query would quietly become "every document, with its text". `Option<Vec<u8>>` cannot
    /// be a where field at all, which settles the binary half the same way.
    pub async fn get_text_rows_without_hash(
        &self,
        ctx: &MyTelemetryContext,
    ) -> Vec<DocumentHashBackfillDto> {
        let where_model =
            StaticLineWhereModel::new("content_hash IS NULL AND binary_content IS NULL");

        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, Some(&where_model), Some(ctx))
            .await
            .expect("documents: query_rows get_text_rows_without_hash failed")
    }

    /// Write one document's content hash, touching nothing else on the row.
    ///
    /// No history entry and no version bump: what this records is a fact about the payload that was always
    /// true, not a change somebody made to it.
    pub async fn set_content_hash(&self, id: &str, content_hash: &str, ctx: &MyTelemetryContext) {
        let row = DocumentHashUpdateDto {
            id: id.to_string(),
            content_hash: Some(content_hash.to_string()),
        };

        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .update_db_entity(&row, TABLE_NAME, Some(ctx))
            .await
            .expect("documents: update_db_entity set_content_hash failed");
    }

    /// One live document by id, or `None` — which means it is in the trash or was never there. The caller
    /// tells those apart by asking the trash; see `get_trashed`.
    pub async fn get_by_id(&self, id: &str, ctx: &MyTelemetryContext) -> Option<DocumentDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_single_row(TABLE_NAME, Some(&ByIdWhereModel { id }), Some(ctx))
            .await
            .expect("documents: query_single_row get_by_id failed")
    }

    /// The live document at a path, if a path is taken. What decides whether an upload creates or overwrites.
    pub async fn get_by_path(
        &self,
        project_id: &str,
        doc_path: &str,
        ctx: &MyTelemetryContext,
    ) -> Option<DocumentDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_single_row(
                TABLE_NAME,
                Some(&ByPathWhereModel {
                    project_id,
                    doc_path,
                }),
                Some(ctx),
            )
            .await
            .expect("documents: query_single_row get_by_path failed")
    }

    /// The TEXT of every live document of one project, and nothing else — what a search reads.
    ///
    /// See [`DocumentTextDto`] for why this is safe to run over a whole project: the blob column is not in
    /// the select list at all.
    pub async fn get_texts_of_project(
        &self,
        project_id: &str,
        ctx: &MyTelemetryContext,
    ) -> Vec<DocumentTextDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(
                TABLE_NAME,
                Some(&ByProjectWhereModel { project_id }),
                Some(ctx),
            )
            .await
            .expect("documents: query_rows get_texts_of_project failed")
    }

    /// Write the current state of a document.
    ///
    /// Always AFTER the matching history row — see the note on [`DocumentHistoryDto`].
    pub async fn upsert(&self, row: &DocumentDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("documents: insert_or_update_db_entity failed");
    }

    /// Take a document out of the working table. Only ever paired with a write to the trash — a document is
    /// never removed from both.
    pub async fn delete_row(&self, id: &str, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .delete(TABLE_NAME, &ByIdWhereModel { id }, Some(ctx))
            .await
            .expect("documents: delete failed");
    }

    /// Append a version.
    ///
    /// Upserted rather than inserted, and that is not laziness: if a previous attempt wrote the history row
    /// and then failed on the working table, the same version number comes round again — an insert would
    /// refuse it and the document would be stuck for good.
    pub async fn upsert_history(&self, row: &DocumentHistoryDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                HISTORY_TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(HISTORY_PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("documents: insert_or_update history failed");
    }

    /// Every version of one document, WITHOUT the payloads, in whatever order Postgres returns them — the
    /// caller sorts. One version's payload is read by [`Self::get_version`].
    pub async fn get_history(
        &self,
        document_id: &str,
        ctx: &MyTelemetryContext,
    ) -> Vec<DocumentVersionDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(
                HISTORY_TABLE_NAME,
                Some(&ByDocumentWhereModel { document_id }),
                Some(ctx),
            )
            .await
            .expect("documents: query_rows get_history failed")
    }

    /// One version of one document, content included.
    pub async fn get_version(
        &self,
        document_id: &str,
        version: i64,
        ctx: &MyTelemetryContext,
    ) -> Option<DocumentHistoryDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_single_row(
                HISTORY_TABLE_NAME,
                Some(&ByVersionWhereModel {
                    document_id,
                    version,
                }),
                Some(ctx),
            )
            .await
            .expect("documents: query_single_row get_version failed")
    }

    /// Put a document in the trash.
    pub async fn upsert_trash(&self, row: &DocumentTrashDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TRASH_TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(TRASH_PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("documents: insert_or_update trash failed");
    }

    /// One trashed document by id. How a reference to a deleted document is told from a reference to nothing.
    pub async fn get_trashed(&self, id: &str, ctx: &MyTelemetryContext) -> Option<DocumentTrashDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_single_row(TRASH_TABLE_NAME, Some(&ByIdWhereModel { id }), Some(ctx))
            .await
            .expect("documents: query_single_row get_trashed failed")
    }

    /// Everything in one project's trash, WITHOUT the payloads. Restoring reads the full row.
    pub async fn get_trash_of_project(
        &self,
        project_id: &str,
        ctx: &MyTelemetryContext,
    ) -> Vec<DocumentTrashIndexDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(
                TRASH_TABLE_NAME,
                Some(&ByProjectWhereModel { project_id }),
                Some(ctx),
            )
            .await
            .expect("documents: query_rows get_trash_of_project failed")
    }

    /// Take a document out of the trash — only ever paired with a write back to the working table.
    pub async fn delete_trash_row(&self, id: &str, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .delete(TRASH_TABLE_NAME, &ByIdWhereModel { id }, Some(ctx))
            .await
            .expect("documents: delete trash failed");
    }
}
