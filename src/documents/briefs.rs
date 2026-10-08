//! What is known about what a document SAYS, held by content rather than by document.
//!
//! **The index is here for the same reason the documents index is: the question is asked of a whole
//! listing at once.** `documents_list` answers with every document of a project and every file of its
//! connected repositories, and each row carries its brief — so a per-row read of Postgres would turn the
//! cheapest call on this surface into a few hundred round trips. The whole table is a few hundred short
//! strings; it is loaded at startup and kept in step by the one call that writes it.

use std::sync::Arc;

use ahash::AHashMap;
use arc_swap::ArcSwap;
use rust_extensions::date_time::DateTimeAsMicroseconds;

use crate::postgres::DocumentBriefDto;

use super::{DocumentIndexEntry, MAX_BRIEF_LEN};

/// One brief, as everything above reads it.
#[derive(Debug, Clone)]
pub struct Brief {
    pub text: String,
    pub updated_by: String,
    // When this content was FIRST briefed, kept across a rewrite so that "somebody has read this" has a
    // date that does not move every time the wording improves.
    //
    // There is no `updated` beside it. The row in Postgres has one and it is written on every rephrase,
    // but nothing in this process ever asks when a brief was last reworded — and a field that is filled
    // in four places and read in none is how a struct comes to look like it promises something.
    pub created: DateTimeAsMicroseconds,
}

/// Every brief there is, by content hash.
///
/// Same machinery as the documents index and for the same reason — read on every listing, written when an
/// agent finishes reading something. `ArcSwap` so a reader takes no lock; the mutex only stops two writers
/// losing one of two briefs.
pub struct BriefsIndex {
    inner: ArcSwap<AHashMap<String, Arc<Brief>>>,
    write_lock: parking_lot::Mutex<()>,
}

impl Default for BriefsIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl BriefsIndex {
    pub fn new() -> Self {
        Self {
            inner: ArcSwap::from_pointee(AHashMap::new()),
            write_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Replace the lot, which is what startup does.
    pub fn replace_all(&self, rows: &[DocumentBriefDto]) {
        let map: AHashMap<String, Arc<Brief>> = rows
            .iter()
            .map(|row| {
                (
                    row.content_hash.clone(),
                    Arc::new(Brief {
                        text: row.brief.clone(),
                        updated_by: row.updated_by.clone(),
                        created: row.created,
                    }),
                )
            })
            .collect();

        let _guard = self.write_lock.lock();
        self.inner.store(Arc::new(map));
    }

    pub fn put(&self, content_hash: &str, brief: Brief) {
        let _guard = self.write_lock.lock();

        let mut next = self.inner.load().as_ref().clone();
        next.insert(content_hash.to_string(), Arc::new(brief));

        self.inner.store(Arc::new(next));
    }

    pub fn get(&self, content_hash: &str) -> Option<Arc<Brief>> {
        self.inner.load().get(content_hash).cloned()
    }

    /// The brief for a document that may not have a hash, as a listing wants it: a string, empty for none.
    ///
    /// One function so that "no hash" and "a hash nobody has briefed" come out the same, which is the
    /// truth from a reader's point of view — nobody has read this for you either way.
    pub fn text_of(&self, content_hash: Option<&str>) -> String {
        content_hash
            .and_then(|hash| self.get(hash))
            .map(|brief| brief.text.clone())
            .unwrap_or_default()
    }

    /// How many contents are briefed. For the tests alone, which is why it is compiled for them alone:
    /// everything else asks about one hash at a time, and a public count nobody calls is a warning in
    /// every build but the one that uses it.
    #[cfg(test)]
    pub fn amount(&self) -> usize {
        self.inner.load().len()
    }

    /// Whether this document is one somebody still has to read.
    ///
    /// **A file is never waiting for a brief**, and that is not a limitation being apologised for: a brief
    /// is prose about a text, and there is no text in a PNG for this product to produce it from. Counting
    /// binaries as unbriefed would make the number a to-do list nobody can finish.
    pub fn needs_brief(&self, entry: &DocumentIndexEntry) -> bool {
        if entry.is_binary {
            return false;
        }

        match entry.content_hash.as_deref() {
            Some(hash) => self.get(hash).is_none(),
            // No hash at all: a legacy row the backfill has not reached, or a repository file whose bytes
            // turned out not to be text. Either way nobody has briefed it.
            None => true,
        }
    }

    /// How many documents in a listing are waiting to be read, counting one CONTENT once.
    ///
    /// **Deduplicated by hash, because the work is per text and not per row.** A repository with the same
    /// licence file in nine folders is one brief away from being fully briefed, and a count of nine would
    /// send an agent round a loop that finishes in one.
    pub fn without_brief(&self, entries: &[DocumentIndexEntry]) -> usize {
        self.without_brief_of(
            entries
                .iter()
                .map(|itm| (itm.is_binary, itm.content_hash.as_deref())),
        )
    }

    /// The same count over anything that knows those two things about itself.
    ///
    /// **Two shapes ask this and neither should have to become the other.** A listing asks it of index
    /// entries; a connection that has just been re-cloned asks it of the files in its mirror, which are
    /// not documents and have no ids. What the count actually needs is what every one of them has: whether
    /// it is a file, and what its content hashes to.
    pub fn without_brief_of<'s>(
        &self,
        items: impl IntoIterator<Item = (bool, Option<&'s str>)>,
    ) -> usize {
        let mut seen: ahash::AHashSet<&str> = ahash::AHashSet::new();
        let mut amount = 0;

        for (_, content_hash) in items.into_iter().filter(|(is_binary, content_hash)| {
            !is_binary
                && match content_hash {
                    Some(hash) => self.get(hash).is_none(),
                    None => true,
                }
        }) {
            match content_hash {
                // Two rows with one hash are one piece of work.
                Some(hash) => {
                    if seen.insert(hash) {
                        amount += 1;
                    }
                }
                // Nothing to deduplicate by — it is counted on its own, because until it is read there is
                // no way to know what it is a copy of.
                None => amount += 1,
            }
        }

        amount
    }
}

/// Read a brief somebody wrote, or say what is wrong with it.
///
/// **The cap is the whole of the validation, and it is what keeps a brief a brief.** Everything else is
/// prose written by a reader for a reader; the one thing this side has an opinion about is that it must
/// stay short enough for three hundred of them to travel in one listing.
pub fn normalise_brief(src: &str) -> Result<String, String> {
    let brief = src.trim();

    if brief.is_empty() {
        return Err(
            "an empty brief says nothing — write what the document is, what it covers, and what somebody would come to it for"
                .to_string(),
        );
    }

    // The same refusal a document's text gets, for the same reason: Postgres will not put a NUL in a text
    // column, and the failure without this check reaches the caller as "it did not save".
    if brief.contains('\0') {
        return Err("a brief cannot contain a NUL character".to_string());
    }

    // Counted in CHARACTERS rather than bytes: a brief written in Ukrainian is not two thirds of a brief
    // written in English.
    let length = brief.chars().count();

    if length > MAX_BRIEF_LEN {
        return Err(format!(
            "that brief is {length} characters — the limit is {MAX_BRIEF_LEN}. A brief says what the document is and what is in it; the document itself is one documents_get away"
        ));
    }

    Ok(brief.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, hash: Option<&str>, is_binary: bool) -> DocumentIndexEntry {
        DocumentIndexEntry {
            id: path.to_string(),
            project_id: "P".to_string(),
            path: path.to_string(),
            content_type: "text/markdown".to_string(),
            is_binary,
            size: 1,
            content_hash: hash.map(str::to_string),
            version: 1,
            created: DateTimeAsMicroseconds::new(0),
            updated: DateTimeAsMicroseconds::new(0),
            updated_by: "AI".to_string(),
        }
    }

    fn briefed(hashes: &[&str]) -> BriefsIndex {
        let index = BriefsIndex::new();

        for hash in hashes {
            index.put(
                hash,
                Brief {
                    text: "what it says".to_string(),
                    updated_by: "AI".to_string(),
                    created: DateTimeAsMicroseconds::new(0),
                },
            );
        }

        index
    }

    #[test]
    fn a_document_needs_a_brief_until_its_content_has_one() {
        let index = briefed(&["aaa"]);

        assert!(!index.needs_brief(&entry("done.md", Some("aaa"), false)));
        assert!(index.needs_brief(&entry("new.md", Some("bbb"), false)));

        // No hash yet — a legacy row, or a file whose bytes were not text. Nobody has read it either way.
        assert!(index.needs_brief(&entry("old.md", None, false)));

        // A file is never waiting: there is no text in it to write a brief from.
        assert!(!index.needs_brief(&entry("logo.png", Some("ccc"), true)));
        assert!(!index.needs_brief(&entry("scan.pdf", None, true)));
    }

    /// The count is of WORK, not of rows — which is the difference between a loop that ends and one that
    /// asks nine times about one licence file.
    #[test]
    fn the_count_is_of_contents_and_not_of_documents() {
        let index = briefed(&["aaa"]);

        let entries = vec![
            entry("a.md", Some("aaa"), false),    // briefed
            entry("b.md", Some("bbb"), false),    // one piece of work…
            entry("c.md", Some("bbb"), false),    // …the same one
            entry("d.md", Some("ccc"), false),    // another
            entry("logo.png", Some("ddd"), true), // never counted
        ];

        assert_eq!(index.without_brief(&entries), 2);

        // A row with no hash cannot be deduplicated against anything, so two of them are two.
        let unknown = vec![entry("x.md", None, false), entry("y.md", None, false)];

        assert_eq!(index.without_brief(&unknown), 2);

        assert_eq!(index.without_brief(&[]), 0);
    }

    #[test]
    fn a_brief_is_trimmed_and_bounded_and_never_empty() {
        assert_eq!(
            normalise_brief("  what it says  "),
            Ok("what it says".to_string())
        );

        assert!(normalise_brief("").is_err());
        assert!(normalise_brief("   \n ").is_err());
        assert!(normalise_brief("a\0b").is_err());

        // The cap counts characters: a brief in Cyrillic is not worth two thirds of one in English.
        let cyrillic: String = "я".repeat(MAX_BRIEF_LEN);

        assert!(normalise_brief(&cyrillic).is_ok());
        assert!(normalise_brief(&"a".repeat(MAX_BRIEF_LEN + 1)).is_err());
    }

    /// What a listing asks for: one string, and the same answer for "not briefed" whichever way it got
    /// there.
    #[test]
    fn a_listing_reads_a_missing_brief_as_an_empty_one() {
        let index = briefed(&["aaa"]);

        assert_eq!(index.text_of(Some("aaa")), "what it says");
        assert_eq!(index.text_of(Some("bbb")), "");
        assert_eq!(index.text_of(None), "");
    }

    /// A second reader of the same text replaces what the first wrote rather than being refused: the
    /// content cannot have changed, because the content is the key.
    #[test]
    fn writing_a_brief_twice_keeps_the_second() {
        let index = briefed(&["aaa"]);

        index.put(
            "aaa",
            Brief {
                text: "a better one".to_string(),
                updated_by: "yuri@mxtm.ai".to_string(),
                created: DateTimeAsMicroseconds::new(0),
            },
        );

        assert_eq!(index.text_of(Some("aaa")), "a better one");
        assert_eq!(index.amount(), 1);
    }
}
