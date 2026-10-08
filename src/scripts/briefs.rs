//! Writing a brief, and finding the next document that has none.
//!
//! **The loop this exists for is deliberately dull.** An agent asks for the next document nobody has read
//! for it, gets the document and the hash of its content, reads it, writes three sentences, files them
//! under that hash, and asks again. Nothing here decides what a brief should say — that is the reader's
//! job and the tool descriptions are where it is spelled out; what is here is only the bookkeeping that
//! makes the loop finish: what counts as unbriefed, in which order, and how a document that cannot be
//! briefed gets out of the way.

use rust_extensions::date_time::DateTimeAsMicroseconds;

use crate::app::AppContext;
use crate::documents::{Brief, content_hash, normalise_brief, normalise_content_hash};
use crate::postgres::{DocumentBriefDto, DocumentDto};

use super::{body_of, list_documents, require_author, resolve_document};

/// How many candidates one call will open before giving up on this pass.
///
/// **A ceiling on a loop that reads files, not a limit on the work.** Every candidate that turns out not
/// to be text costs a read, and a project that somehow held hundreds of them would otherwise make one
/// "give me the next one" call walk the whole board. Whatever is left is still there on the next call, and
/// the count says so.
const MAX_CANDIDATES_PER_CALL: usize = 20;

/// One document waiting to be read, and how many are waiting behind it.
pub struct NextWithoutBrief {
    /// `None` when everything readable has been briefed.
    pub document: Option<DocumentDto>,
    /// The hash the brief goes under. Always set when there is a document — it is computed from the bytes
    /// that were just read, even when the listing did not have one.
    pub content_hash: String,
    /// How many CONTENTS are still unbriefed, this one included. Deduplicated, so it is a number of briefs
    /// left to write rather than of rows.
    pub remaining: usize,
}

/// File a brief under the hash of the content it describes.
///
/// **It takes a hash rather than a document, and that is the feature rather than an awkwardness.** The
/// brief describes a text; whichever document the reader happened to open it through, every other copy of
/// that text is briefed by the same call — and when somebody edits one of them, the edit produces other
/// bytes and the document goes back to being unbriefed by itself.
pub async fn set_brief(
    app: &AppContext,
    content_hash: &str,
    brief: &str,
    who: &str,
) -> Result<Brief, String> {
    let content_hash = normalise_content_hash(content_hash)?;
    let text = normalise_brief(brief)?;
    let who = require_author(who)?;

    let now = DateTimeAsMicroseconds::now();

    // Kept from the brief that is there, so the date on it means "when this content was first read for
    // us" rather than "when somebody last rephrased it".
    let created = app
        .briefs
        .get(&content_hash)
        .map(|itm| itm.created)
        .unwrap_or(now);

    let row = DocumentBriefDto {
        content_hash: content_hash.clone(),
        brief: text.clone(),
        updated_by: who.clone(),
        created,
        updated: now,
    };

    let ctx = service_sdk::my_telemetry::MyTelemetryContext::create_empty();

    // Postgres first, memory second: the opposite order would show a brief on a listing that a restart
    // would then lose.
    app.documents_repo.upsert_brief(&row, &ctx).await;

    let brief = Brief {
        text,
        updated_by: who,
        created,
    };

    app.briefs.put(&content_hash, brief.clone());

    Ok(brief)
}

/// The next document of a project that nobody has written a brief for, with the text to write it from.
///
/// **Both kinds, in one order.** `list_documents` already answers with the project's own documents and the
/// files of its connected repositories sorted together by path, so the loop walks a board the way a person
/// would read it rather than doing all of one kind and then all of the other.
///
/// **A candidate that cannot be briefed is stepped over rather than handed back.** A row whose bytes turn
/// out not to be text — a `.md` that is really a binary, a listing made before hashing existed — would
/// otherwise come back as base64 with nothing in it to summarise, and the caller would have no way to
/// move past it. `skip` is the caller's half of the same thing: a hash it has decided it cannot brief this
/// round.
pub async fn next_without_brief(
    app: &AppContext,
    project_prefix: &str,
    skip: &[String],
) -> Result<NextWithoutBrief, String> {
    let entries = list_documents(app, project_prefix)?;

    // Counted over the whole listing rather than over what is walked below, so the number is the work
    // left on the board and not the work left in this call.
    let remaining = app.briefs.without_brief(&entries);

    let skip: ahash::AHashSet<&str> = skip.iter().map(|itm| itm.as_str()).collect();

    let mut opened = 0;

    for entry in entries.iter().filter(|itm| app.briefs.needs_brief(itm)) {
        if let Some(hash) = entry.content_hash.as_deref() {
            if skip.contains(hash) {
                continue;
            }
        }

        if opened >= MAX_CANDIDATES_PER_CALL {
            break;
        }

        opened += 1;

        // Read by id, which routes a file of a connected repository to its clone and everything else to
        // Postgres. A document that vanished between the listing and here is one to step over, not a
        // failure: the listing is a snapshot and this is a loop that will come round again.
        let Ok(row) = resolve_document(app, Some(&entry.id), None, None).await else {
            continue;
        };

        let Some(text) = body_of(&row).as_text().map(str::to_string) else {
            continue;
        };

        // The hash of what was actually read. The row usually carries it; when it does not — a legacy
        // document the backfill has not reached — this is the moment the bytes are in hand, so it is
        // computed here and written back rather than left to be rediscovered on every call.
        let hash = match row.content_hash.clone() {
            Some(hash) => hash,
            None => {
                let hash = content_hash(text.as_bytes());

                remember_hash(app, &row, &hash).await;

                hash
            }
        };

        if skip.contains(hash.as_str()) {
            continue;
        }

        return Ok(NextWithoutBrief {
            document: Some(row),
            content_hash: hash,
            remaining,
        });
    }

    Ok(NextWithoutBrief {
        document: None,
        content_hash: String::new(),
        remaining,
    })
}

/// Write a hash onto a document that had none, without touching anything else about it.
///
/// Only for a document of the project's own: a file of a connected repository has no row, and its hash
/// comes back on the next listing walk.
async fn remember_hash(app: &AppContext, row: &DocumentDto, hash: &str) {
    if task_manager_shared::github::is_github_path(&row.doc_path) {
        return;
    }

    let ctx = service_sdk::my_telemetry::MyTelemetryContext::create_empty();

    app.documents_repo
        .set_content_hash(&row.id, hash, &ctx)
        .await;

    // And in memory, or the listing keeps saying this document has no hash until the next restart.
    app.documents_index.set_content_hash(&row.id, hash);
}
