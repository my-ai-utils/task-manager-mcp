use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;

use super::documents_tool_calls::project_prefix_of;
use super::{DocumentContentView, DocumentHeadingView};

// ---------------------------------------------------------------- documents_set_brief

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsSetBriefInput {
    #[property(
        description = "The content hash the brief is filed under — the `content_hash` of the document you just read, 64 hex characters. NOT an id and not a path: a brief describes a TEXT, so filing it by content is what makes every copy of that text briefed at once, and what makes an edited document unbriefed again by itself"
    )]
    pub hash: String,
    #[property(
        description = "WHAT IS IN THE DOCUMENT, written so that the next reader can decide whether to open it WITHOUT opening it. Say what it is (a specification, a runbook, a meeting note, an API contract), what it covers and — where it is not obvious — what it does NOT, the systems, services, tables, endpoints, people and decisions named in it, and the questions somebody could answer from it. Concrete nouns beat adjectives: `settlement-engine retry policy, the four failure classes, and why B-book fills are exempt` is worth ten times `describes retry behaviour`. A few sentences up to 1500 characters — write to the ceiling when the document earns it, because this text is all a future reader gets before choosing"
    )]
    pub brief: String,
    #[property(description = "Who wrote it: an email, or `AI`")]
    pub who: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsSetBriefResponse {
    #[property(description = "The hash it was filed under, normalised to lowercase")]
    pub hash: String,
    #[property(description = "The brief as stored, trimmed")]
    pub brief: String,
}

pub struct DocumentsSetBriefHandler {
    app: Arc<AppContext>,
}

impl DocumentsSetBriefHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsSetBriefHandler {
    const FUNC_NAME: &'static str = "documents_set_brief";
    const DESCRIPTION: &'static str = "Record what a document says, so nobody has to read it to find \
out. The brief comes back on every documents_list row and every documents_get, and scanning briefs \
instead of opening documents is how a board of two hundred specifications stays usable.\
\
IT IS FILED BY CONTENT, NOT BY DOCUMENT. The key is the sha256 of the text — `content_hash` on \
anything you read. Three consequences, and all three are the point: the same text stored on two boards \
or living in a connected repository AND synced into a project is briefed once and found by both; an \
edit changes the hash, so the document goes back to being unbriefed and nobody is left reading a brief \
that describes the old text; and a document restored from the trash finds its brief again, because the \
bytes came back with it.\
\
WRITE IT RIGHT AFTER YOU READ SOMETHING. documents_upload and documents_edit hand you the new \
`content_hash` in their response — that is the moment: you have just read or written the text, and it \
costs you nothing to say what is in it. A board where this is habit answers 'which document covers the \
settlement retries' from one documents_list.\
\
WHAT MAKES A BRIEF WORTH HAVING is that it is specific enough to rule a document IN or OUT. Name the \
systems, the endpoints, the tables, the decisions and the people in it. Say what it does not cover when \
somebody would reasonably assume it does. Do not write 'documentation about the API' — write which API, \
which operations, and what a reader would come to it for.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsSetBriefInput, DocumentsSetBriefResponse> for DocumentsSetBriefHandler {
    async fn execute_tool_call(
        &self,
        model: DocumentsSetBriefInput,
    ) -> Result<DocumentsSetBriefResponse, String> {
        let brief =
            crate::scripts::set_brief(&self.app, &model.hash, &model.brief, &model.who).await?;

        Ok(DocumentsSetBriefResponse {
            hash: crate::documents::normalise_content_hash(&model.hash)?,
            brief: brief.text,
        })
    }
}

/// Where documents_get should carry on from, or `None` when it cannot.
///
/// **Two different things are `None` here and telling them apart is the caller's job, not ours.** A
/// document that arrived whole has nothing to continue; a document whose FIRST LINE is longer than the
/// whole budget cannot be continued by line at all — `slice_text` reports `to_line` as `from_line - 1`
/// there, because what was handed over is part of a line no number claims, and "read on from `to_line`
/// + 1" would ask for the same bytes for ever. A minified bundle or a one-line JSON is that document,
/// and it is one to describe as what it is rather than to page through.
fn read_on_from_line(truncated: bool, from_line: Option<i64>, to_line: Option<i64>) -> Option<i64> {
    if !truncated {
        return None;
    }

    let from = from_line.unwrap_or(1);
    let to = to_line?;

    match to >= from {
        true => Some(to + 1),
        false => None,
    }
}

// ---------------------------------------------------------------- documents_next_without_brief

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsNextWithoutBriefInput {
    #[property(description = "Which board to work through, by prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "Hashes to step over this round — the ones you have already decided you cannot brief. Without it, a document you skip is the one you are handed again on the next call, and the loop never ends. Add each hash you gave up on and pass the growing list back"
    )]
    pub skip: Option<Vec<String>>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentsNextWithoutBriefResponse {
    #[property(
        description = "The document to brief, with its text — absent when there is nothing left to read. It is the same shape documents_get returns, cut to the first 64 KB when it is longer: read `truncated`, and when it is true and the opening was not enough, documents_get with `from_line` gives you the rest"
    )]
    pub document: Option<DocumentContentView>,
    #[property(
        description = "The hash to hand to documents_set_brief with what you learned. Empty when there was nothing left"
    )]
    pub content_hash: String,
    #[property(
        description = "THE MAP OF THE WHOLE DOCUMENT when `document.content` is only part of it — every heading with the lines its section spans, which is what documents_outline returns. Empty when the text above IS the whole document. It is here so that a brief of a long document describes the DOCUMENT rather than its opening: the headings say what it covers end to end, and `documents_get` with `from_line` / `to_line` reads any section that matters before you write"
    )]
    pub outline: Vec<DocumentHeadingView>,
    #[property(
        description = "The line to continue from with documents_get, when there is more and it can be reached — `null` when you have the whole document, and ALSO null when the rest cannot be paged by line because a single line is longer than the whole budget (a minified bundle, a one-line JSON). That second case is not a document to brief section by section: say what it is, or skip it"
    )]
    pub read_on_from_line: Option<i64>,
    #[property(
        description = "How many distinct contents on this board are still unbriefed, INCLUDING this one. It is the reading list left, deduplicated — two copies of one text are one item"
    )]
    pub remaining: i32,
    #[property(
        description = "True when everything readable on the board has a brief. The loop stops here"
    )]
    pub none_left: bool,
}

pub struct DocumentsNextWithoutBriefHandler {
    app: Arc<AppContext>,
}

impl DocumentsNextWithoutBriefHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for DocumentsNextWithoutBriefHandler {
    const FUNC_NAME: &'static str = "documents_next_without_brief";
    const DESCRIPTION: &'static str = "Hand me the next document nobody has written a brief for, with \
its text. THIS IS A LOOP: call it, read what comes back, call documents_set_brief with the \
`content_hash` and what you learned, call this again. Stop when `none_left` is true.\
\
IT COVERS BOTH KINDS OF DOCUMENT — the project's own and the files of its connected repositories — in \
path order, because a reader works through a board the way it is drawn rather than doing all of one \
kind first.\
\
`remaining` IS THE WORK LEFT, deduplicated by content: two copies of one text are one brief. It does \
not count files (a PDF, an image), which have no text to brief and are never offered here.\
\
A DOCUMENT TOO LONG TO HAND OVER WHOLE ARRIVES AS ITS FIRST 64 KB PLUS ITS OUTLINE, and the brief is \
still about the whole document. `outline` is every heading with the lines it spans; `read_on_from_line` \
is where documents_get continues. So the shape of the work is: read the opening, read the outline, pull \
the two or three sections that decide what this document IS, then write. A brief that describes only \
the first pages of a long specification is worse than none — it tells the next reader the document is \
about what it happens to open with.\
\
IF YOU CANNOT BRIEF WHAT YOU ARE GIVEN — it is generated noise, a lock file, a minified bundle — put \
its hash in `skip` on the next call rather than writing a brief that says nothing. A brief of 'this is \
a generated file' is worth writing once; a brief of 'unclear' is worth nothing to the reader who finds \
it.\
\
WHERE TO START: documents_list reports `without_brief` for the board, and github_refresh reports it \
for a repository it has just re-cloned. Either number above zero is what this tool is for.";
}

#[async_trait::async_trait]
impl McpToolCall<DocumentsNextWithoutBriefInput, DocumentsNextWithoutBriefResponse>
    for DocumentsNextWithoutBriefHandler
{
    async fn execute_tool_call(
        &self,
        model: DocumentsNextWithoutBriefInput,
    ) -> Result<DocumentsNextWithoutBriefResponse, String> {
        let skip = model.skip.unwrap_or_default();

        let next = crate::scripts::next_without_brief(&self.app, &model.project, &skip).await?;

        let Some(row) = next.document else {
            return Ok(DocumentsNextWithoutBriefResponse {
                document: None,
                outline: Vec::new(),
                read_on_from_line: None,
                content_hash: next.content_hash,
                remaining: next.remaining as i32,
                none_left: true,
            });
        };

        // Kept before the slice takes it: the outline below is of the WHOLE document, which is the point
        // of returning one at all.
        let full_text = row.content.clone().unwrap_or_default();

        let view = DocumentContentView::from_dto(
            &row,
            &project_prefix_of(&self.app, &row.project_id),
            // Empty by construction — this is a document nobody has briefed, which is why it is being
            // handed over.
            String::new(),
        );

        // Cut to what a brief actually needs. Shipping a 900 KB specification whole to produce three
        // sentences spends the context the reading itself needs — and what is missing is made good by the
        // outline rather than left to be guessed at.
        let view = view.into_slice(
            None,
            None,
            Some(crate::documents::BRIEF_READ_MAX_BYTES as i64),
        )?;

        // Only when there is something the reader has NOT seen. Beside a whole document an outline is
        // noise: they are holding the headings already.
        let outline = match view.truncated {
            true => crate::scripts::outline_of(&full_text)
                .into_iter()
                .map(|itm| DocumentHeadingView {
                    level: itm.level,
                    title: itm.title,
                    line: itm.line,
                    end_line: itm.end_line,
                    size: itm.size,
                })
                .collect(),
            false => Vec::new(),
        };

        Ok(DocumentsNextWithoutBriefResponse {
            outline,
            read_on_from_line: read_on_from_line(view.truncated, view.from_line, view.to_line),
            document: Some(view),
            content_hash: next.content_hash,
            remaining: next.remaining as i32,
            none_left: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The trap this exists to close.** A document is paged by asking for the line after the last one
    /// that fit — which works until a single line is longer than the whole budget, when there IS no such
    /// line and the obvious arithmetic asks for the same bytes for ever. `slice_text` reports that as a
    /// `to_line` BEFORE `from_line`, and this is what turns it into "you cannot page this one".
    #[test]
    fn a_document_that_cannot_be_paged_by_line_says_so_rather_than_looping() {
        // The ordinary case: 812 lines fitted out of 9 000, so carry on at 813.
        assert_eq!(read_on_from_line(true, Some(1), Some(812)), Some(813));

        // Carrying on from the middle works the same way.
        assert_eq!(read_on_from_line(true, Some(813), Some(1_600)), Some(1_601));

        // A minified bundle: one line, longer than the budget, so `to_line` is `from_line - 1` and there
        // is no next line to ask for. Answering 1 here would hand back the same 64 KB on every call.
        assert_eq!(read_on_from_line(true, Some(1), Some(0)), None);

        // Nothing was cut — the reader is holding the whole document.
        assert_eq!(read_on_from_line(false, Some(1), Some(120)), None);

        // A file has no lines at all; it is never offered for briefing, and this is not a way in.
        assert_eq!(read_on_from_line(true, None, None), None);
    }
}
