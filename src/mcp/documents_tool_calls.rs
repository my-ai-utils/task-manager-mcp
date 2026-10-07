use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::{
    DeletedDocumentView, DocumentContentView, DocumentVersionView, DocumentView,
    TrashedDocumentView,
};

/// The prefix of the project a document belongs to.
///
/// A document row stores the project **id**, but every id a caller sees on this surface is a prefix — so the
/// translation happens once, here. A project that has vanished between the read and this call falls back to
/// the raw id rather than failing: the document is what was asked for, and naming its board oddly is better
/// than refusing to hand it over.
/// The board's CURRENT prefix, from whichever prefix the caller named it by.
///
/// A project keeps every prefix it has ever had, so a caller working from an older conversation can name a
/// board by one it no longer carries — and a reference built with that would name a board that is not
/// there. Falls back to what was written when nothing resolves, which is the case where the call itself is
/// about to fail anyway.
pub(super) fn project_prefix_named(app: &AppContext, named: &str) -> String {
    let board = app.board.read();

    crate::scripts::resolve_project_by_prefix(&board, named)
        .map(|itm| itm.prefix.clone())
        .unwrap_or_else(|_| named.to_string())
}

pub(super) fn project_prefix_of(app: &AppContext, project_id: &str) -> String {
    app.board
        .read()
        .get_project(project_id)
        .map(|itm| itm.prefix.clone())
        .unwrap_or_else(|| project_id.to_string())
}

/// What arrived in the two payload arguments, as the domain wants it.
///
/// **Exactly one of the two, and saying so is the whole job.** Both would be a caller who does not know what
/// they are uploading; neither would be a document with nothing in it. base64 is decoded HERE — the boundary
/// JSON forces it through — so that nothing past this point knows the encoding exists.
fn read_payload(
    content: Option<String>,
    binary_base64: Option<String>,
    content_type: Option<String>,
) -> Result<crate::scripts::NewDocumentContent, String> {
    use rust_extensions::base64::FromBase64;

    let binary = binary_base64
        .as_deref()
        .map(str::trim)
        .filter(|itm| !itm.is_empty());

    let body = match (content, binary) {
        (Some(_), Some(_)) => {
            return Err(
                "pass `content` for a text document or `binary_base64` for a file — not both. A document is one or the other"
                    .to_string(),
            );
        }
        (None, None) => {
            return Err(
                "nothing to upload: pass `content` for text, or `binary_base64` for a file".to_string(),
            );
        }
        (Some(text), None) => crate::scripts::DocumentBody::Text(text),
        (None, Some(encoded)) => crate::scripts::DocumentBody::Binary(
            encoded
                .from_base64()
                .map_err(|err| format!("`binary_base64` is not valid base64: {err}"))?,
        ),
    };

    Ok(crate::scripts::NewDocumentContent { body, content_type })
}

// -------------------------------------------------------------------------------------------- list

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsListInput {
    #[property(description = "Which project's documents to index, by prefix, e.g. `RMS`")]
    pub project: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsListResponse {
    #[property(
        description = "Every live document of the project, by path. WITHOUT the texts — read one with documents_get. Sorted by path, so everything under one folder is contiguous"
    )]
    pub documents: Vec<DocumentView>,
    #[property(description = "How many there are")]
    pub amount: i32,
    #[property(
        description = "How many DISTINCT CONTENTS on this board nobody has briefed yet — the size of the reading list, not a count of rows: two copies of one text are one piece of work, and files (a PDF, an image) are never counted because there is no text in them to brief. A number above zero is an invitation rather than an error: documents_next_without_brief hands them over one at a time, and a board whose briefs are written answers 'which document covers X' from this listing alone"
    )]
    pub without_brief: i32,
}

pub struct DocumentsListHandler {
    app: Arc<AppContext>,
}

impl DocumentsListHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsListHandler {
    const FUNC_NAME: &'static str = "documents_list";
    const DESCRIPTION: &'static str = "The index of a project's documents: what exists, where it \
lives and how big it is. CALL THIS BEFORE UPLOADING — a path that is already taken gets overwritten, \
which is the right behaviour when you meant to edit that document and a lost text when you did not.\
\
It deliberately does NOT return the texts. A document can be a whole specification, and an agent that \
pulled all of them in to find one would have spent the context it needed for the work. Read the paths, \
pick one, then documents_get.\
\
IT DOES RETURN THE BRIEFS, AND THAT IS WHAT MAKES THE PATHS USABLE. Each row carries a few sentences \
saying what is in that document, written by whoever read it last. Scan those before opening anything — \
on a board where they are written, this one call answers 'which document covers X'. `without_brief` \
says how many contents nobody has read for you yet; documents_next_without_brief works through them.\
\
Folders in the paths are not real: there is no such thing as a folder here, they are read off the paths \
of the documents in them. So an empty folder cannot exist, and moving every document out of one is what \
makes it disappear.\
\
PATHS UNDER `github/` ARE NOT THIS PROJECT'S DOCUMENTS. They are files in a GitHub repository somebody \
connected, and they are listed beside the real ones because that is what makes reference material \
reachable. What is behind them is a real clone on this server's disk, so reading one is a file read that \
costs nothing and needs no key — documents_get takes such a path exactly as it takes any other.\
\
THEY ARE READ-ONLY, ALL OF THEM. documents_upload, documents_edit, documents_delete, \
documents_delete_folder and documents_update_path refuse a `github/` path and say why. The folder behind \
it is a clone this service REPLACES: refreshing a connection clones the repository again and swaps the \
new copy in for the old one, so anything written there would be gone at the next refresh with nothing to \
say it had been there. To change one of these files, change it in the repository and refresh the \
connection. To have a copy this board owns and keeps versions of, sync it into the project's own \
documents.\
\
THEY ALSO HAVE NO VERSIONS — every one reports version 0, and documents_history, documents_diff and \
documents_restore refuse them, because their history is the repository's and `git log`, through \
github_git, is how it is read.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsListInput, DocumentsListResponse> for DocumentsListHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsListInput,
    ) -> Result<DocumentsListResponse, String> {
        // No `await`: the index is in memory, which is the one part of a document that is cached.
        let rows = crate::scripts::list_documents(&self.app, &model.project)?;

        // The brief is joined here, out of memory, for every row at once — a lookup per row against a map
        // rather than a query per row against Postgres, which is what would turn the cheapest call on this
        // surface into a few hundred round trips.
        let documents: Vec<DocumentView> = rows
            .iter()
            .map(|row| {
                DocumentView::from_entry(
                    row,
                    &project_prefix_of(&self.app, &row.project_id),
                    self.app.briefs.text_of(row.content_hash.as_deref()),
                )
            })
            .collect();

        let without_brief = self.app.briefs.without_brief(&rows) as i32;

        Ok(DocumentsListResponse {
            amount: documents.len() as i32,
            documents,
            without_brief,
        })
    }
}

// --------------------------------------------------------------------------------------------- get

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsGetInput {
    #[property(
        description = "Which document. A reference url — `raw/{project}/document/{id}` or `raw/{project}/github/{repository}/{path}` — is what a task, a goal or somebody's notes hand you, and it is taken here as it is written; so is the bare `id` documents_list reports. Either this or `project` + `path`"
    )]
    pub id: Option<String>,
    #[property(
        description = "Which project, by prefix. Only needed when reading by `path` instead of by id"
    )]
    pub project: Option<String>,
    #[property(
        description = "Which document, by where it lives — e.g. `docs/design/system.md`. Needs `project` beside it. Use this when a person named a document rather than handing you an id"
    )]
    pub path: Option<String>,
    #[property(
        description = "Read an OLD version instead of the current one, by its version number from documents_history. Requires `id` — a path is where a document lives now, and says nothing about where it lived then. Omit for the current text"
    )]
    pub version: Option<i64>,
    #[property(
        description = "Start reading at this line, 1-based. Use it with `to_line` to read ONE SECTION of a large document — documents_outline reports exactly those two numbers per heading. Text only; a file has no lines"
    )]
    pub from_line: Option<i64>,
    #[property(
        description = "Stop after this line, 1-based and INCLUSIVE. Asking past the end of the document is fine and gives you the rest of it, which is how 'from here on' is written when you do not know the length"
    )]
    pub to_line: Option<i64>,
    #[property(
        description = "Return at most this many bytes of text, cut at a line boundary. A safety net rather than a way to navigate — reach for `from_line` / `to_line` when you know what you want. The response says `truncated` and where it stopped, so you can read on from `to_line + 1`"
    )]
    pub max_bytes: Option<i64>,
}

pub struct DocumentsGetHandler {
    app: Arc<AppContext>,
}

impl DocumentsGetHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsGetHandler {
    const FUNC_NAME: &'static str = "documents_get";
    const DESCRIPTION: &'static str = "Read one document, text included. Name it by `id` — which \
never changes — or by `project` and `path`, which is what to use when a person named the document \
rather than handing you an id.\
\
A LARGE DOCUMENT IS READ IN PIECES, NOT WHOLE. Pass `from_line` and `to_line` for one section, or \
`max_bytes` as a ceiling. documents_outline gives you exactly those line numbers per heading, so the \
pair is the way to work: outline once for a kilobyte of structure, then read the one section you need. \
Reading a whole specification to change a sentence spends the context the work needed — and a document \
big enough not to fit in a tool result is one you cannot read at all without this.\
\
Check `truncated` on the way out. It is true whenever what you got is not the whole document, and \
neither an edit nor a conclusion of the form \"it does not mention X\" is safe from a truncated read.\
\
Pass `version` to read an older text: every version a document has ever had is kept, and \
documents_history lists them. That is what the stable id is FOR — a document that was rewritten and \
moved three times is still one document, and its whole past is reachable through it. To see what \
CHANGED between two versions, documents_diff answers without either text.\
\
A path under `github/` is a file in a connected repository and reads exactly like anything else here — \
but it is READ-ONLY, and it has no versions, so `version`, documents_history and documents_diff all \
refuse it and say so. What you read is the WORKING COPY on this server, not the branch: it is fetched on \
a ten-minute timer, and the fast-forward after the fetch is skipped for as long as the working tree has \
anything uncommitted in it, so it can sit behind the branch for longer than ten minutes. Pressing \
Refresh on the connection is what settles that — it clones the repository again and swaps the new copy \
in for the old one, with the old one readable until the moment it does. `git log` through github_git \
says what has actually arrived here.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsGetInput, DocumentContentView> for DocumentsGetHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsGetInput,
    ) -> Result<DocumentContentView, String> {
        let id = model.id.as_deref().map(str::trim).filter(|itm| !itm.is_empty());

        // Asked for an old version but named the document by where it lives. Refused rather than guessed at:
        // a path says where a document is NOW, and version 3 may well have been somewhere else — reading
        // "version 3 of whatever is at this path today" is a question nobody means to ask.
        if model.version.is_some() && id.is_none() {
            return Err(
                "reading an old version needs `id` — a path is where the document lives now, which says nothing about where that version lived. Get the id from documents_list, then pass `version`"
                    .to_string(),
            );
        }

        let view = if let Some(version) = model.version {
            let id = id.expect("checked just above");
            let row = crate::scripts::document_version(&self.app, id, version).await?;

            DocumentContentView::from_history(
                &row,
                &project_prefix_of(&self.app, &row.project_id),
                |hash| self.app.briefs.text_of(Some(hash)),
            )
        } else {
            let row = crate::scripts::resolve_document(
                &self.app,
                model.id.as_deref(),
                model.project.as_deref(),
                model.path.as_deref(),
            )
            .await?;

            DocumentContentView::from_dto(
                &row,
                &project_prefix_of(&self.app, &row.project_id),
                self.app.briefs.text_of(row.content_hash.as_deref()),
            )
        };

        // Last, and applied to the built view rather than to the row: an old version and the current one
        // are sliced identically, which is the whole reason they share a shape.
        view.into_slice(model.from_line, model.to_line, model.max_bytes)
    }
}

// ------------------------------------------------------------------------------------------ upload

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsUploadInput {
    #[property(description = "Which project to put it on, by prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "Where it goes — `notes.md`, or `docs/design/system.md` for one inside folders. A leading slash and backslashes are accepted and normalised, so `/docs/a.md` is the same document as `docs/a.md`. IF THIS PATH IS TAKEN, THE DOCUMENT THERE IS OVERWRITTEN with a new version, keeping its id and its whole history — which is how a document is edited. Call documents_list first if you are not certain the path is free"
    )]
    pub path: String,
    #[property(
        description = "The document as TEXT — Markdown usually. Sent whole every time: there is no partial write, and what arrives becomes the current version in one go. The previous text is not lost, it stays as the previous version. Pass this OR `binary_base64`, never both"
    )]
    pub content: Option<String>,
    #[property(
        description = "The document as a FILE, base64-encoded — a PDF, an image. base64 because JSON cannot carry bytes; it is decoded here and stored as real bytes, so nothing downstream pays for the encoding. Pass this OR `content`, never both. Remember base64 is a third larger than the file"
    )]
    pub binary_base64: Option<String>,
    #[property(
        description = "The MIME type — `application/pdf`, `image/png`. OMIT IT and the path decides: `docs/spec.pdf` is a PDF without being told. Worth passing only when the extension is missing or lies"
    )]
    pub content_type: Option<String>,
    #[property(
        description = "Who is writing it: an email, or the literal `AI` when it is you. Recorded on the version, which is what makes the history answer 'who changed this'"
    )]
    pub who: String,
}

pub struct DocumentsUploadHandler {
    app: Arc<AppContext>,
}

impl DocumentsUploadHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsUploadHandler {
    const FUNC_NAME: &'static str = "documents_upload";
    const DESCRIPTION: &'static str = "Put a document into the system: create one at this path, or \
write a new version of the one already there. This is the ONLY way a document is written — there is no \
editing it in the browser, exactly as with everything else on this board.\
\
THE PATH IS THE KEY AND THE ID IS THE IDENTITY. Uploading to a taken path does not make a second \
document: it adds a version to the one that lives there, keeping its id and everything that references \
it. That is what makes 'upload the file again' the whole of the editing story. It is also why moving is \
a different call — documents_update_path — because a history that could not tell a rewrite from a move \
would not answer the question it exists for.\
\
Read documents_list first when you are not sure the path is free. An upload to a path you did not mean \
to touch replaces what a person put there, and while the old text is still in the history, nobody knows \
to go looking for it.\
\
THE RESPONSE CARRIES `content_hash` — BRIEF IT NOW. You have just written this text and know exactly \
what is in it; documents_set_brief with that hash costs one call and saves the next reader the whole \
document. The hash is new, so whatever brief the old text had does not apply and the document reads as \
unbriefed until you do.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsUploadInput, DocumentView> for DocumentsUploadHandler {
    async fn execute_tool_call(&self, model: DocumentsUploadInput) -> Result<DocumentView, String> {
        let row = crate::scripts::upload_document(
            &self.app,
            &model.project,
            &model.path,
            read_payload(model.content, model.binary_base64, model.content_type)?,
            &model.who,
        )
        .await?;

        Ok(DocumentView::from_dto(
            &row,
            &project_prefix_of(&self.app, &row.project_id),
            self.app.briefs.text_of(row.content_hash.as_deref()),
        ))
    }
}

// ------------------------------------------------------------------------------------- update_path

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsUpdatePathInput {
    #[property(description = "Which document to move, by id")]
    pub id: String,
    #[property(
        description = "Where it goes. Refused if another document is already there — pick a free path, or upload over that one if replacing it is what you meant"
    )]
    pub path: String,
    #[property(description = "Who is moving it: an email, or `AI`")]
    pub who: String,
}

pub struct DocumentsUpdatePathHandler {
    app: Arc<AppContext>,
}

impl DocumentsUpdatePathHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsUpdatePathHandler {
    const FUNC_NAME: &'static str = "documents_update_path";
    const DESCRIPTION: &'static str = "Move or rename a document. The text is untouched and the id \
does not change, so every task and goal that references it still does.\
\
Its own call rather than a field on the upload, so the history says which of the two happened: 'somebody \
rewrote it' and 'somebody moved it' are the two questions a history is asked, and one field could not \
answer either.\
\
RENAMING A FOLDER IS THIS CALL, ONCE PER DOCUMENT. There is no folder to rename — folders are read off \
the paths — so moving everything under one prefix is the operation, and each move is its own entry in \
its document's history.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsUpdatePathInput, DocumentView> for DocumentsUpdatePathHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsUpdatePathInput,
    ) -> Result<DocumentView, String> {
        let row =
            crate::scripts::update_document_path(&self.app, &model.id, &model.path, &model.who)
                .await?;

        Ok(DocumentView::from_dto(
            &row,
            &project_prefix_of(&self.app, &row.project_id),
            self.app.briefs.text_of(row.content_hash.as_deref()),
        ))
    }
}

// ----------------------------------------------------------------------------------------- history

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsHistoryInput {
    #[property(description = "Which document, by id — including one that is in the trash")]
    pub id: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsHistoryResponse {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — the url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, and every tool here takes it wherever it takes an id"
    )]
    pub reference: String,
    #[property(description = "Which document this is the history of")]
    pub id: String,
    #[property(
        description = "Every version, oldest first, WITHOUT the texts — read one with documents_get and its `version`. Each entry says what was done, where the document was at the time, and by whom"
    )]
    pub versions: Vec<DocumentVersionView>,
}

pub struct DocumentsHistoryHandler {
    app: Arc<AppContext>,
}

impl DocumentsHistoryHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsHistoryHandler {
    const FUNC_NAME: &'static str = "documents_history";
    const DESCRIPTION: &'static str = "Every version a document has ever had: what happened, where it \
was at the time, who did it and when. Nothing is ever pruned, and the run has no gaps.\
\
This is what the stable id is for. A document that was rewritten five times and moved twice is one \
document with one history, and a `moved` entry is what answers 'when did this end up here'.\
\
It answers for a document in the trash as well as a live one — deleting is a version like any other, and \
the whole point of keeping the id is that throwing something away does not end its story. The texts are \
left out; documents_get with a `version` reads the one you want.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsHistoryInput, DocumentsHistoryResponse> for DocumentsHistoryHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsHistoryInput,
    ) -> Result<DocumentsHistoryResponse, String> {
        let rows = crate::scripts::document_history(&self.app, &model.id).await?;

        // After the history, because it errors first when the id names nothing at all — that is the
        // message a caller wants, rather than a reference to something that is not there.
        let reference = crate::scripts::reference_of(&self.app, &model.id).await;

        Ok(DocumentsHistoryResponse {
            reference,
            id: model.id,
            versions: rows.iter().map(DocumentVersionView::from_dto).collect(),
        })
    }
}

// ------------------------------------------------------------------------------------------ delete

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsDeleteInput {
    #[property(description = "Which document to put in the trash, by id")]
    pub id: String,
    #[property(description = "Who is deleting it: an email, or `AI`")]
    pub who: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsDeleteResponse {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — the url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, and every tool here takes it wherever it takes an id"
    )]
    pub reference: String,
    #[property(
        description = "The id, unchanged — it is what documents_restore takes, so it is worth keeping"
    )]
    pub id: String,
    #[property(
        description = "The path it had, which is where a restore puts it back by default"
    )]
    pub path: String,
}

pub struct DocumentsDeleteHandler {
    app: Arc<AppContext>,
}

impl DocumentsDeleteHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsDeleteHandler {
    const FUNC_NAME: &'static str = "documents_delete";
    const DESCRIPTION: &'static str = "Put a document in the trash. Its text, its path and its whole \
history are kept, and documents_restore brings it back — this is not destruction, it is taking something \
out of the tree.\
\
The path it was at becomes free immediately, so a document can be replaced by deleting it and uploading \
another there.\
\
REFERENCES TO IT ARE NOT REMOVED. A task or a goal pointing at a deleted document keeps pointing at it, \
on purpose: restoring is one call away, and a reference quietly dropped would not come back with the \
document. A reader who cannot resolve one is told it is in the trash.\
\
The trash is not shown in the browser — documents_trash is how you see what is in it.\
\
A PATH UNDER `github/` IS REFUSED. It is a file in a connected repository, and this board only reads \
those: what is on this disk is a clone that a refresh deletes and clones again, so removing a file from \
it would change nothing in the repository and be undone the next time anybody pressed Refresh. Delete it \
where it lives — in the repository — and refresh the connection.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsDeleteInput, DocumentsDeleteResponse> for DocumentsDeleteHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsDeleteInput,
    ) -> Result<DocumentsDeleteResponse, String> {
        // Read before the deletion rather than after: the row is about to leave the index for the trash,
        // and resolving it while it is still live is one lookup instead of two.
        let reference = crate::scripts::reference_of(&self.app, &model.id).await;

        let path = crate::scripts::delete_document(&self.app, &model.id, &model.who).await?;

        Ok(DocumentsDeleteResponse {
            reference,
            id: model.id,
            path,
        })
    }
}

// ----------------------------------------------------------------------------------- delete folder

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsDeleteFolderInput {
    #[property(description = "Which project the folder is in, by prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "Which folder to empty, e.g. `docs/design`. Everything BELOW it goes too. A trailing slash is fine, and the spelling is case-sensitive — it is the path the documents carry"
    )]
    pub folder: String,
    #[property(description = "Who is deleting them: an email, or `AI`")]
    pub who: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsDeleteFolderResponse {
    #[property(description = "The folder that was emptied, as it was read")]
    pub folder: String,
    #[property(
        description = "What went, in path order. Each one is in the trash under its own id and documents_restore takes them back one at a time — keep this list if any of them might be wanted back"
    )]
    pub documents: Vec<DeletedDocumentView>,
    #[property(description = "How many were deleted")]
    pub amount: i32,
}

pub struct DocumentsDeleteFolderHandler {
    app: Arc<AppContext>,
}

impl DocumentsDeleteFolderHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsDeleteFolderHandler {
    const FUNC_NAME: &'static str = "documents_delete_folder";
    const DESCRIPTION: &'static str = "Put EVERY document under one folder in the trash, in one call. \
This is how a folder is deleted — there is no folder to delete otherwise: folders are read off the paths \
of the documents in them, so a folder stops existing exactly when the last document under it does.\
\
IT TAKES THE SUBTREE. `docs` takes `docs/design/system.md` as readily as `docs/notes.md`. What it does \
NOT take is a document CALLED `docs` — a file sharing a folder's name is not inside it.\
\
Each of the project's own documents goes exactly as documents_delete sends it: its own history entry, its \
own trash row, its id kept. So nothing here is more destructive than one deletion — it is the same one \
repeated, and every document is restorable on its own with documents_restore. The response lists what \
went; keep it if anything in there might be wanted back, because the trash is flat and a folder of forty \
is forty rows in it.\
\
CALL documents_list FIRST when you are not certain what is under the folder. There is deliberately no way \
to say 'all of them' — an empty folder argument is refused, because that is not a folder, it is the \
project's documents.\
\
References on tasks and goals are NOT removed, on purpose, exactly as with a single deletion: restoring \
is one call away, and a reference quietly dropped would not come back with the document.\
\
A PATH UNDER `github/` IS REFUSED, WHOLE. That is a folder in a connected repository — somebody else's \
files, which this board only reads — and the refusal is worth having here more than anywhere: \
`github/<connection>` is one word that would otherwise empty a whole working copy, and emptying it would \
change nothing in the repository and be undone by the next refresh.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsDeleteFolderInput, DocumentsDeleteFolderResponse>
    for DocumentsDeleteFolderHandler
{
    async fn execute_tool_call(
        &self,
        model: DocumentsDeleteFolderInput,
    ) -> Result<DocumentsDeleteFolderResponse, String> {
        let deleted =
            crate::scripts::delete_folder(&self.app, &model.project, &model.folder, &model.who)
                .await?;

        let prefix = project_prefix_named(&self.app, &model.project);

        let documents: Vec<DeletedDocumentView> = deleted
            .iter()
            .map(|itm| DeletedDocumentView::from_dto(itm, &prefix))
            .collect();

        Ok(DocumentsDeleteFolderResponse {
            folder: model.folder,
            amount: documents.len() as i32,
            documents,
        })
    }
}

// ------------------------------------------------------------------------------------------- trash

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsTrashInput {
    #[property(description = "Which project's trash to look in, by prefix")]
    pub project: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsTrashResponse {
    #[property(
        description = "What is in the trash, most recently deleted first. FLAT — the trash has no folders, only the last path each document had"
    )]
    pub documents: Vec<TrashedDocumentView>,
    #[property(description = "How many there are")]
    pub amount: i32,
}

pub struct DocumentsTrashHandler {
    app: Arc<AppContext>,
}

impl DocumentsTrashHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsTrashHandler {
    const FUNC_NAME: &'static str = "documents_trash";
    const DESCRIPTION: &'static str = "What has been deleted from a project's documents. THE ONLY WAY \
TO SEE THE TRASH — it is deliberately not drawn in the browser: it is not a place to browse, it is a \
list you look at when something needs restoring.\
\
Flat, and newest first: no folders, just the last path each document had, because what needs restoring \
is almost always what just went in.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsTrashInput, DocumentsTrashResponse> for DocumentsTrashHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsTrashInput,
    ) -> Result<DocumentsTrashResponse, String> {
        let rows = crate::scripts::list_trash(&self.app, &model.project).await?;

        let prefix = project_prefix_named(&self.app, &model.project);

        let documents: Vec<TrashedDocumentView> = rows
            .iter()
            .map(|itm| TrashedDocumentView::from_dto(itm, &prefix))
            .collect();

        Ok(DocumentsTrashResponse {
            amount: documents.len() as i32,
            documents,
        })
    }
}

// ----------------------------------------------------------------------------------------- restore

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsRestoreInput {
    #[property(description = "Which document to bring back, by id — from documents_trash")]
    pub id: String,
    #[property(
        description = "Where to put it back. OMIT to restore it where it was, which is what undoing a mistake means. If that path has been taken since, the call is refused and names what is there — pass a path then, rather than guessing at one"
    )]
    pub path: Option<String>,
    #[property(description = "Who is restoring it: an email, or `AI`")]
    pub who: String,
}

pub struct DocumentsRestoreHandler {
    app: Arc<AppContext>,
}

impl DocumentsRestoreHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsRestoreHandler {
    const FUNC_NAME: &'static str = "documents_restore";
    const DESCRIPTION: &'static str = "Take a document back out of the trash, with its text, its id \
and its history intact — so every task and goal that referenced it works again.\
\
With no `path` it goes back exactly where it was, which is what undoing a deletion means. If something \
has taken that path since, the call is REFUSED and says what is there: where a restored document lands \
is a decision, and a guessed path is how a document ends up somewhere nobody looks.\
\
Restoring is only possible from here. There is no way to do it in the browser, which does not show the \
trash at all.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsRestoreInput, DocumentView> for DocumentsRestoreHandler {
    async fn execute_tool_call(&self, model: DocumentsRestoreInput) -> Result<DocumentView, String> {
        let row = crate::scripts::restore_document(
            &self.app,
            &model.id,
            model.path.as_deref(),
            &model.who,
        )
        .await?;

        Ok(DocumentView::from_dto(
            &row,
            &project_prefix_of(&self.app, &row.project_id),
            self.app.briefs.text_of(row.content_hash.as_deref()),
        ))
    }
}
