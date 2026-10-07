//! The four tools that let a LARGE document be worked on rather than merely retrieved.
//!
//! `documents_list`, `documents_get` and `documents_upload` are complete: everything a document can be is
//! reachable through them. They are complete at one size, though — the size where reproducing a document
//! verbatim to change a line is affordable, and where reading one to find out whether it mentions something
//! is affordable. Past that size the surface quietly becomes read-only in practice, and the way an agent
//! finds anything is to pull documents in one at a time until the context is gone.
//!
//! * **edit** — send the two hundred bytes that change instead of the seventy-six kilobytes that do not;
//! * **search** — ask the whole project a question instead of asking every document in turn;
//! * **outline** — see the structure of a document for a kilobyte, and slice out the one section wanted;
//! * **diff** — find out what a write actually did, without reading either version.
//!
//! They compose: outline to find the section, get with `from_line`/`to_line` to read it, edit to change it,
//! diff to check that the change is the one that was meant.

use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::{DocumentHeadingView, DocumentSearchHitView, DocumentView};

use super::documents_tool_calls::{project_prefix_named, project_prefix_of};

// -------------------------------------------------------------------------------------------- edit

/// One replacement, as the tool takes it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentEditInput {
    #[property(
        description = "The text to find, EXACTLY as it stands in the document right now — whitespace, indentation, the kind of dash and all. Not a pattern and not a line number: a literal piece of the current text. Include enough of the lines around it to make it unique, which is usually one line of context either side"
    )]
    pub old_string: String,
    #[property(
        description = "What to put there instead. An empty string DELETES the matched text, which is how a paragraph is removed. To insert, match the line it goes next to and write that line plus the new one"
    )]
    pub new_string: String,
    #[property(
        description = "Change every occurrence instead of refusing an ambiguous one. Leave it off unless you mean it: with it off, text that appears twice is a refusal that names the count, and that refusal is the guard rail. Use it for a rename that really is meant to sweep the document"
    )]
    pub replace_all: Option<bool>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsEditInput {
    #[property(
        description = "Which document, by id — from documents_list, documents_search, or the `documents` list on a task or a goal. Either this or `project` + `path`"
    )]
    pub id: Option<String>,
    #[property(description = "Which project, by prefix. Only needed when naming the document by `path`")]
    pub project: Option<String>,
    #[property(description = "Which document, by where it lives — `docs/design/system.md`. Needs `project` beside it")]
    pub path: Option<String>,
    #[property(
        description = "The edits, applied IN ORDER and each one against the result of the last. So a later edit may match text an earlier one wrote — and an edit whose `old_string` an earlier one destroyed is an error rather than a skip. They land as ONE new version, not one per edit"
    )]
    pub edits: Vec<DocumentEditInput>,
    #[property(
        description = "The version you read, as an optimistic lock. If the document has moved on since, the call is REFUSED and nothing is written. Pass it whenever you read the document earlier in the conversation rather than in the call just before this one: the edits would very likely still apply, and would splice your change into a text you have never seen"
    )]
    pub expected_version: Option<i64>,
    #[property(description = "Who is writing it: an email, or the literal `AI` when it is you")]
    pub who: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsEditResponse {
    #[property(
        description = "The document as it now stands, at its new version. Check `version` — that is what to pass as `expected_version` on the next edit of the same document"
    )]
    pub document: DocumentView,
    #[property(
        description = "How many occurrences each edit changed, in the order you sent them. Everything is 1 unless `replace_all` was used — worth reading when it was, because that is the number you were guessing at"
    )]
    pub replacements: Vec<i32>,
}

pub struct DocumentsEditHandler {
    app: Arc<AppContext>,
}

impl DocumentsEditHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsEditHandler {
    const FUNC_NAME: &'static str = "documents_edit";
    const DESCRIPTION: &'static str = "Change parts of a document's text without sending the rest of \
it. THIS IS HOW A LARGE DOCUMENT IS EDITED — documents_upload replaces the whole text, which for a \
specification means reproducing tens of kilobytes verbatim to change a sentence, and every one of those \
reproductions is a chance to drop a section nobody notices is gone.\
\
EACH EDIT MATCHES EXACT TEXT, AND AN AMBIGUOUS ONE IS REFUSED. If `old_string` appears more than once \
and you did not pass `replace_all`, the call fails and tells you how many times it was found. That is \
deliberate and it is the point of the tool: silently changing all seven occurrences of a word in a 76 KB \
architecture document is exactly how one goes quietly wrong in a place nobody reads again. Give the \
match more surrounding text until it is unique.\
\
ALL OF THE EDITS OR NONE OF THEM. If any one fails — not found, found twice, empty — nothing is \
written at all, and the document is exactly as it was. So a batch is safe to send whole rather than one \
call at a time; and when it is refused, re-read before retrying, because the failure usually means the \
document is not what you think it is.\
\
ONE NEW VERSION PER CALL, not one per edit. Seven edits are one entry in the history, because seven \
entries would describe seven documents nobody ever intended.\
\
PASS `expected_version` WHENEVER YOU READ THE DOCUMENT EARLIER IN THE CONVERSATION. A person can upload \
over a document from the browser at any moment; if that happened, your edits would be spliced into text \
you have not seen, and they would very likely still apply. The refusal is what stops that, and \
documents_diff then says what they changed.\
\
A file — a PDF, an image — has no text to match against and is refused. Replace one with \
documents_upload.\
\
THE RESPONSE CARRIES THE NEW `content_hash`, AND THE OLD BRIEF NO LONGER APPLIES. A brief is filed \
against the content it describes, so an edit leaves the document unbriefed on purpose — write a fresh \
one with documents_set_brief while the change is still in front of you.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsEditInput, DocumentsEditResponse> for DocumentsEditHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsEditInput,
    ) -> Result<DocumentsEditResponse, String> {
        let edits: Vec<crate::scripts::DocumentEdit> = model
            .edits
            .into_iter()
            .map(|itm| crate::scripts::DocumentEdit {
                old_string: itm.old_string,
                new_string: itm.new_string,
                replace_all: itm.replace_all.unwrap_or(false),
            })
            .collect();

        let (row, replacements) = crate::scripts::edit_document(
            &self.app,
            model.id.as_deref(),
            model.project.as_deref(),
            model.path.as_deref(),
            &edits,
            model.expected_version,
            &model.who,
        )
        .await?;

        Ok(DocumentsEditResponse {
            document: DocumentView::from_dto(
                &row,
                &project_prefix_of(&self.app, &row.project_id),
                self.app.briefs.text_of(row.content_hash.as_deref()),
            ),
            replacements,
        })
    }
}

// ------------------------------------------------------------------------------------------ search

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsSearchInput {
    #[property(description = "Which project's documents to search, by prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "What to look for. Literal text by default — punctuation, brackets and dots mean themselves. Matching is per LINE, like grep, so a query cannot span a newline"
    )]
    pub query: String,
    #[property(
        description = "Read `query` as a regular expression instead: `\\bfoo\\b`, `min_(statistics|values)`, `^## `. Rust regex syntax. An invalid pattern is refused with the reason rather than silently finding nothing"
    )]
    pub is_regex: Option<bool>,
    #[property(
        description = "Match case exactly. OFF by default, which is the safe direction for a search: case-insensitive finds a superset, so it cannot hide the one hit you were looking for. Turn it on for an identifier where the case is the point"
    )]
    pub case_sensitive: Option<bool>,
    #[property(
        description = "Only documents whose path starts with this — `docs/design`. A folder prefix, not a glob: folders here are prefixes of paths and nothing else, and a trailing slash makes no difference"
    )]
    pub path_prefix: Option<String>,
    #[property(
        description = "How many lines to return either side of each match. Two by default, which is enough to see what a line is talking about; zero for the bare lines when you are counting rather than reading. Ten is the ceiling — past that you wanted documents_get with a slice"
    )]
    pub context_lines: Option<i64>,
    #[property(
        description = "How many matching lines one document may return. Twenty by default, a hundred at most. `matches_total` still reports the real count per document, so a capped result tells you it was capped"
    )]
    pub max_matches_per_document: Option<i64>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsSearchResponse {
    #[property(
        description = "The documents that matched — never their texts. Each carries the `reference` that names it: hand that to documents_get, using a match's `line_number` as `from_line`, or to add_documents to put the document in front of a piece of work"
    )]
    pub documents: Vec<DocumentSearchHitView>,
    #[property(description = "How many documents matched")]
    pub documents_amount: i32,
    #[property(
        description = "How many lines matched across the documents that were READ. Complete — including matches the per-document cap left out — UNLESS `truncated` is true, which means the project-wide cap stopped the scan early and documents after it contributed nothing to this number. So it is authoritative for 'how often is this mentioned' exactly when `truncated` is false"
    )]
    pub matches_total: i32,
    #[property(
        description = "How many documents were actually read. Compare it with documents_list: the difference is what `path_prefix` excluded, what had no text in it, and — when `truncated` is true — what the project-wide cap never got to"
    )]
    pub searched_amount: i32,
    #[property(
        description = "How many documents had no text to search — files, mostly. A PDF cannot be searched here at all, so a project of them gives you zero matches for a reason that is not 'it is not mentioned'"
    )]
    pub skipped_amount: i32,
    #[property(
        description = "True when a cap cut this short, at either level. `matches_total` is still the real count; what was cut is lines you did not get to see, so narrow the query before concluding anything from the absence of one"
    )]
    pub truncated: bool,
}

pub struct DocumentsSearchHandler {
    app: Arc<AppContext>,
}

impl DocumentsSearchHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsSearchHandler {
    const FUNC_NAME: &'static str = "documents_search";
    const DESCRIPTION: &'static str = "Ask a whole project's documents a question at once: which of \
them mention this, and on which lines. NEVER returns a text — only the matching lines and a couple of \
lines around each.\
\
THIS IS THE TOOL FOR 'WHERE IS X WRITTEN DOWN'. The alternative is reading documents one at a time \
until you find it, and most of what that costs is spent on the ones that turn out to be irrelevant — \
two documents of 24 KB and 33 KB read in full to learn that neither says anything is the context the \
work needed, gone. One call answers it for the whole project.\
\
IT IS ALSO HOW YOU FIND OUT WHETHER SOMETHING IS MENTIONED AT ALL — but read `truncated` before you \
conclude anything from a number. False, and `matches_total` is the real count for the whole project \
even where the returned LINES were capped, so zero means it is genuinely not there. True, and a cap \
stopped the scan early: documents past that point were never read and contributed nothing to the \
total, so narrow it with `path_prefix` before believing what it says. The other blind spot the \
response names for you is `skipped_amount` — a file has no text, so this cannot see into one at all.\
\
Each match carries a `line_number` you can hand straight to documents_get as `from_line`, which is how \
a search turns into a read of the right section rather than of the whole document.\
\
Matching is LINE-ORIENTED, exactly like grep: a query cannot span a newline. To search the BOARD — \
tasks, goals and their comment threads — use tasks_search instead.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsSearchInput, DocumentsSearchResponse> for DocumentsSearchHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsSearchInput,
    ) -> Result<DocumentsSearchResponse, String> {
        let query = crate::scripts::DocumentSearchQuery {
            query: model.query,
            is_regex: model.is_regex.unwrap_or(false),
            case_sensitive: model.case_sensitive.unwrap_or(false),
            path_prefix: model.path_prefix,
            context_lines: model
                .context_lines
                .unwrap_or(crate::scripts::DEFAULT_CONTEXT_LINES),
            max_matches_per_document: model
                .max_matches_per_document
                .unwrap_or(crate::scripts::DEFAULT_MAX_MATCHES_PER_DOCUMENT),
        };

        let outcome = crate::scripts::search_documents(&self.app, &model.project, &query).await?;

        // After the search, which is what refuses an unknown project — and the board's CURRENT prefix
        // rather than the one that was written, since a project answers to every prefix it has ever had
        // and a reference must name the one it carries now.
        let prefix = project_prefix_named(&self.app, &model.project);

        Ok(DocumentsSearchResponse {
            documents_amount: outcome.hits.len() as i32,
            matches_total: outcome.matches_total,
            searched_amount: outcome.searched,
            skipped_amount: outcome.skipped,
            truncated: outcome.truncated,
            documents: outcome
                .hits
                .into_iter()
                .map(|itm| DocumentSearchHitView::from_hit(itm, &prefix))
                .collect(),
        })
    }
}

// ----------------------------------------------------------------------------------------- outline

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsOutlineInput {
    #[property(description = "Which document, by id. Either this or `project` + `path`")]
    pub id: Option<String>,
    #[property(description = "Which project, by prefix. Only needed when naming the document by `path`")]
    pub project: Option<String>,
    #[property(description = "Which document, by where it lives. Needs `project` beside it")]
    pub path: Option<String>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsOutlineResponse {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — the url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, and every tool here takes it wherever it takes an id"
    )]
    pub reference: String,
    #[property(description = "The document's id")]
    pub id: String,
    #[property(description = "The prefix of the project it belongs to")]
    pub project: String,
    #[property(description = "Where it lives")]
    pub path: String,
    #[property(description = "Which version this is the structure of — the current one")]
    pub version: i64,
    #[property(description = "How big the whole document is, in bytes")]
    pub size: i64,
    #[property(
        description = "How many lines it has. The bound on `from_line` and `to_line` when you read a section back"
    )]
    pub lines_total: i64,
    #[property(
        description = "The headings in document order, each with the line range of the section it opens. EMPTY is a real answer and means the document has no Markdown headings — read it with documents_get and `max_bytes`, or search it"
    )]
    pub headings: Vec<DocumentHeadingView>,
}

pub struct DocumentsOutlineHandler {
    app: Arc<AppContext>,
}

impl DocumentsOutlineHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsOutlineHandler {
    const FUNC_NAME: &'static str = "documents_outline";
    const DESCRIPTION: &'static str = "The heading structure of one document, with the line range each \
section spans. A 76 KB specification becomes about a kilobyte — enough to see what is in it and decide \
what to read.\
\
CALL THIS BEFORE READING A LARGE DOCUMENT. Every entry carries `line` and `end_line`, which go straight \
into documents_get as `from_line` and `to_line`, so the pair is the whole workflow: outline once, then \
read the one section you actually need. Check `size` on the entry first — a section can be most of the \
document.\
\
A SECTION CONTAINS ITS SUBSECTIONS. `end_line` runs to the next heading at the same level or shallower, \
so the spans of nested headings overlap on purpose — slicing an entry gives you the whole section \
rather than its first paragraph.\
\
Headings inside fenced code blocks are NOT reported, which is why this is worth calling rather than \
grepping for `^#`: a document full of shell examples has a `# comment` on nearly every page, and an \
outline of those would name sections that do not exist.\
\
Markdown `#` headings only. A document with none comes back with an empty list, which is a real answer.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsOutlineInput, DocumentsOutlineResponse> for DocumentsOutlineHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsOutlineInput,
    ) -> Result<DocumentsOutlineResponse, String> {
        let (row, headings, lines_total) = crate::scripts::document_outline(
            &self.app,
            model.id.as_deref(),
            model.project.as_deref(),
            model.path.as_deref(),
        )
        .await?;

        let project = project_prefix_of(&self.app, &row.project_id);

        Ok(DocumentsOutlineResponse {
            reference: task_manager_shared::documents::canonical_document_reference(
                &project, &row.id,
            ),
            id: row.id.clone(),
            project,
            path: row.doc_path.clone(),
            version: row.version,
            size: row.content_size.unwrap_or(0),
            lines_total,
            headings: headings
                .into_iter()
                .map(|itm| DocumentHeadingView {
                    level: itm.level,
                    title: itm.title,
                    line: itm.line,
                    end_line: itm.end_line,
                    size: itm.size,
                })
                .collect(),
        })
    }
}

// -------------------------------------------------------------------------------------------- diff

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsDiffInput {
    #[property(
        description = "Which document, by id. Only an id: a path is where the document lives now and says nothing about where a past version lived"
    )]
    pub id: String,
    #[property(description = "The older version to compare from, by its number from documents_history")]
    pub from_version: i64,
    #[property(
        description = "The newer version to compare to. OMIT for the current one, which is what checking your own write means"
    )]
    pub to_version: Option<i64>,
    #[property(
        description = "How many unchanged lines to show around each change. Three by default; zero for the changes alone, more when you need to see what a hunk sits in. Twenty is the ceiling — past that the hunks merge and the diff is the document twice"
    )]
    pub context_lines: Option<i64>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsDiffResponse {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — the url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, and every tool here takes it wherever it takes an id"
    )]
    pub reference: String,
    #[property(description = "The document's id")]
    pub id: String,
    #[property(description = "The prefix of the project it belongs to")]
    pub project: String,
    #[property(description = "The version compared from")]
    pub from_version: i64,
    #[property(description = "The version compared to — the current one when you did not name it")]
    pub to_version: i64,
    #[property(
        description = "Where the document was at the older version. Compare it with `to_path`: when they differ the document MOVED in between, which the texts alone would not show"
    )]
    pub from_path: String,
    #[property(description = "Where it was at the newer version")]
    pub to_path: String,
    #[property(
        description = "The unified diff — `@@` hunk headers, `-` for what went, `+` for what arrived. EMPTY OF HUNKS when the two texts are identical, which is a real answer: it means that version changed the path rather than the text, or rewrote it to the same thing"
    )]
    pub diff: String,
    #[property(
        description = "True when the diff was too long to return whole and was cut. Narrow it with fewer `context_lines`, or compare adjacent versions rather than a version far back"
    )]
    pub truncated: bool,
}

pub struct DocumentsDiffHandler {
    app: Arc<AppContext>,
}

impl DocumentsDiffHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsDiffHandler {
    const FUNC_NAME: &'static str = "documents_diff";
    const DESCRIPTION: &'static str = "What actually changed between two versions of a document, as a \
unified diff. Neither text is returned, which is the point: two 76 KB versions compare to a few hundred \
bytes when a paragraph moved.\
\
TWO USES, AND THE FIRST IS THE ONE TO REACH FOR REFLEXIVELY. After writing — with documents_edit or \
documents_upload — diff the version you just made against the one before it and read what you did. It \
is the only check on a write that does not involve reading the whole document back, and it catches the \
edit that matched in a place you did not mean.\
\
The second: reading history. documents_history says a version exists, who wrote it and when — it \
cannot say what is IN it, and finding out by fetching two full texts costs more than the change is \
worth. This is also how you find out what somebody else changed under you after documents_edit refused \
on `expected_version`.\
\
`to_version` defaults to the current version. It answers for a document in the TRASH exactly as it does \
for a live one, since the id outliving the deletion is the whole point of it.\
\
Two versions that are both FILES, or one of each, cannot be diffed — bytes do not diff into anything a \
reader can use. documents_history reports the size and type of every version.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsDiffInput, DocumentsDiffResponse> for DocumentsDiffHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsDiffInput,
    ) -> Result<DocumentsDiffResponse, String> {
        let diff = crate::scripts::document_diff(
            &self.app,
            &model.id,
            model.from_version,
            model.to_version,
            model
                .context_lines
                .unwrap_or(crate::scripts::DEFAULT_DIFF_CONTEXT),
        )
        .await?;

        let project = project_prefix_of(&self.app, &diff.project_id);

        Ok(DocumentsDiffResponse {
            reference: task_manager_shared::documents::canonical_document_reference(
                &project, &diff.id,
            ),
            project,
            id: diff.id,
            from_version: diff.from_version,
            to_version: diff.to_version,
            from_path: diff.from_path,
            to_path: diff.to_path,
            diff: diff.diff,
            truncated: diff.truncated,
        })
    }
}
