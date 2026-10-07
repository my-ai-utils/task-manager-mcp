use std::sync::Arc;

use ahash::AHashMap;
use arc_swap::ArcSwap;
use rust_extensions::date_time::DateTimeAsMicroseconds;

use crate::postgres::{DocumentDto, DocumentIndexDto};

/// One document as the index knows it: everything about it EXCEPT its payload.
///
/// **This is the whole of what documents put in memory, and the exclusion is the design.** Tasks, goals and
/// projects are held whole because they are small and read constantly. A document can be a PDF, so the split
/// is not an optimisation: caching payloads would put every file on the board into the process, and the board
/// is pushed whole down a WebSocket on every change.
///
/// What is here is what a screen needs to draw a tree and what a card needs to count: a path, a size, a kind
/// and an id. The payload is read from Postgres when somebody opens the document.
#[derive(Debug, Clone)]
pub struct DocumentIndexEntry {
    pub id: String,
    pub project_id: String,
    /// Where it lives — its name AND its position. Folders are derived from this and stored nowhere, which is
    /// why an empty folder cannot exist.
    pub path: String,
    pub content_type: String,
    /// Whether the payload is bytes rather than text. **The discriminator**, and not derived from
    /// `content_type`: which storage column is filled is the fact, and the MIME type is metadata beside it.
    pub is_binary: bool,
    /// Size in bytes, for text and binary alike.
    pub size: i64,
    /// What this document's BRIEF is filed under — sha256 of the payload, lowercase hex.
    ///
    /// `None` for a file, which is never briefed, and for a row written before the column existed, which
    /// `backfill_content_hashes` fills at the next start. It rides on the index entry rather than being
    /// read from the payload because the question it answers — "does this one still need reading for
    /// me?" — is asked of a whole project's listing at once, and reading every payload to answer it is
    /// exactly what a brief exists to avoid.
    pub content_hash: Option<String>,
    pub version: i64,
    pub created: DateTimeAsMicroseconds,
    pub updated: DateTimeAsMicroseconds,
    pub updated_by: String,
}

impl DocumentIndexEntry {
    /// From the blob-free row the index query returns.
    ///
    /// `is_binary` cannot be read from this shape — the payload columns are exactly what it does not select —
    /// so it comes from the content type, which is the one thing the row does carry. That is the single place
    /// in the product where the MIME type decides the kind rather than describing it, and it is why every
    /// write stores a type that agrees with the payload it wrote.
    pub fn from_index_row(src: &DocumentIndexDto) -> Self {
        let content_type = crate::scripts::content_type_of(src.content_type.as_deref(), &src.doc_path);

        Self {
            id: src.id.clone(),
            project_id: src.project_id.clone(),
            path: src.doc_path.clone(),
            is_binary: !is_text_content_type(&content_type),
            content_type,
            size: src.content_size.unwrap_or(0),
            content_hash: src.content_hash.clone(),
            version: src.version,
            created: src.created,
            updated: src.updated,
            updated_by: src.updated_by.clone(),
        }
    }

    /// From a whole row, which a write has just produced. Here `is_binary` is the truth rather than a
    /// reading of the type: the row says which column was filled.
    pub fn from_row(src: &DocumentDto) -> Self {
        Self {
            id: src.id.clone(),
            project_id: src.project_id.clone(),
            path: src.doc_path.clone(),
            content_type: crate::scripts::content_type_of(src.content_type.as_deref(), &src.doc_path),
            is_binary: src.binary_content.is_some(),
            size: src.content_size.unwrap_or(0),
            content_hash: src.content_hash.clone(),
            version: src.version,
            created: src.created,
            updated: src.updated,
            updated_by: src.updated_by.clone(),
        }
    }
}

/// Whether a MIME type describes text.
///
/// `text/*` plus the handful of `application/*` types that are text in practice. Anything else is treated as
/// bytes, which is the safe direction: a file offered for download when it could have been shown is a click
/// wasted, where a PDF rendered as text is a screen of noise.
pub fn is_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type == "application/json"
        || content_type == "application/yaml"
        || content_type == "application/xml"
}

/// Every live document's index entry, in memory.
///
/// Its own type rather than a collection inside `Board`, and deliberately: `Board` is what the WebSocket
/// pushes whole to every open screen, and a document index has no business travelling with a board. Nothing
/// here is ever part of a snapshot — a screen asks for the index over HTTP, and gets it from memory.
///
/// Same machinery as `Board` for the same reason: read constantly, written at agent rate. `ArcSwap` means a
/// reader takes no lock at all, and a writer clones, mutates and swaps. The `Mutex<()>` only stops two
/// writers losing one of two changes; readers never touch it.
pub struct DocumentsIndex {
    inner: ArcSwap<DocumentsIndexInner>,
    write_lock: parking_lot::Mutex<()>,
}

#[derive(Default, Clone)]
struct DocumentsIndexInner {
    /// By id, which is how a reference from a task resolves — the lookup this exists for.
    by_id: AHashMap<String, Arc<DocumentIndexEntry>>,
    /// By project, sorted by path, which is how the tree is drawn. A second view of the same entries rather
    /// than a filter over the first: drawing a tree must not walk every document of every project.
    by_project: AHashMap<String, Vec<Arc<DocumentIndexEntry>>>,
}

impl DocumentsIndex {
    pub fn new() -> Self {
        Self {
            inner: ArcSwap::from_pointee(DocumentsIndexInner::default()),
            write_lock: parking_lot::Mutex::new(()),
        }
    }

    fn mutate<TMutation: FnOnce(&mut DocumentsIndexInner)>(&self, mutation: TMutation) {
        let _guard = self.write_lock.lock();

        let mut next = self.inner.load().as_ref().clone();
        mutation(&mut next);

        self.inner.store(Arc::new(next));
    }

    /// Install everything read from Postgres at startup, replacing whatever is there.
    pub fn replace_all(&self, rows: &[DocumentIndexDto]) {
        let mut inner = DocumentsIndexInner::default();

        for row in rows {
            let entry = Arc::new(DocumentIndexEntry::from_index_row(row));
            inner.by_id.insert(entry.id.clone(), entry.clone());
            inner
                .by_project
                .entry(entry.project_id.clone())
                .or_default()
                .push(entry);
        }

        for entries in inner.by_project.values_mut() {
            entries.sort_by(|left, right| left.path.cmp(&right.path));
        }

        let _guard = self.write_lock.lock();
        self.inner.store(Arc::new(inner));
    }

    /// One document's entry, by id — how a reference on a task is resolved and validated.
    pub fn get(&self, id: &str) -> Option<Arc<DocumentIndexEntry>> {
        self.inner.load().by_id.get(id).cloned()
    }

    /// One project's documents, by path. What the tree is built from.
    pub fn of_project(&self, project_id: &str) -> Vec<DocumentIndexEntry> {
        self.inner
            .load()
            .by_project
            .get(project_id)
            .map(|entries| entries.iter().map(|itm| itm.as_ref().clone()).collect())
            .unwrap_or_default()
    }

    /// Add or replace one document's entry.
    ///
    /// Called after the Postgres write, like every other memory update in this service — see `scripts/`.
    /// Record a hash on a document that was indexed without one, changing nothing else about it.
    ///
    /// **Its own method rather than an upsert of the whole entry**, because the caller that needs it — the
    /// briefing loop, meeting a row written before the column existed — has read the payload but has no
    /// business restating a document's size, version or author from it.
    pub fn set_content_hash(&self, id: &str, content_hash: &str) {
        let id = id.to_string();
        let content_hash = content_hash.to_string();

        self.mutate(move |inner| {
            let Some(existing) = inner.by_id.get(&id) else {
                return;
            };

            let mut updated = (**existing).clone();
            updated.content_hash = Some(content_hash);

            let updated = Arc::new(updated);

            // Both views hold the same entry, so both have to be given the new one — a project's list is
            // not a filter over the ids, it is a second map.
            if let Some(entries) = inner.by_project.get_mut(&updated.project_id) {
                if let Some(slot) = entries.iter_mut().find(|itm| itm.id == id) {
                    *slot = updated.clone();
                }
            }

            inner.by_id.insert(id, updated);
        });
    }

    pub fn upsert(&self, row: &DocumentDto) {
        let entry = Arc::new(DocumentIndexEntry::from_row(row));

        self.mutate(move |inner| {
            // Removed from wherever it was first: a MOVE changes the path, and leaving the old entry in the
            // project's list would draw the document twice — once at each path.
            if let Some(previous) = inner.by_id.get(&entry.id) {
                let previous_project = previous.project_id.clone();

                if let Some(entries) = inner.by_project.get_mut(&previous_project) {
                    entries.retain(|itm| itm.id != entry.id);
                }
            }

            inner.by_id.insert(entry.id.clone(), entry.clone());

            let entries = inner
                .by_project
                .entry(entry.project_id.clone())
                .or_default();

            entries.push(entry);
            entries.sort_by(|left, right| left.path.cmp(&right.path));
        });
    }

    /// Drop one document's entry — a deletion, which moves the row to the trash. The trash is not indexed:
    /// nothing draws it, and MCP reads it from Postgres when somebody asks.
    pub fn remove(&self, project_id: &str, id: &str) {
        let project_id = project_id.to_string();
        let id = id.to_string();

        self.mutate(move |inner| {
            inner.by_id.remove(&id);

            if let Some(entries) = inner.by_project.get_mut(&project_id) {
                entries.retain(|itm| itm.id != id);
            }
        });
    }
}

impl Default for DocumentsIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, project: &str, path: &str, binary: bool) -> DocumentDto {
        DocumentDto {
            id: id.to_string(),
            project_id: project.to_string(),
            doc_path: path.to_string(),
            content_type: Some(if binary {
                "application/pdf".to_string()
            } else {
                "text/markdown".to_string()
            }),
            content: if binary {
                None
            } else {
                Some("text".to_string())
            },
            binary_content: if binary { Some(vec![1, 2]) } else { None },
            content_size: Some(4),
            content_hash: match binary {
                true => None,
                false => Some(crate::documents::content_hash(b"text")),
            },
            version: 1,
            created: DateTimeAsMicroseconds::new(0),
            updated: DateTimeAsMicroseconds::new(0),
            updated_by: "AI".to_string(),
        }
    }

    #[test]
    fn a_document_is_found_by_id_and_listed_under_its_project() {
        let index = DocumentsIndex::new();
        index.upsert(&row("d1", "p1", "docs/a.md", false));

        assert!(index.get("d1").is_some());
        assert_eq!(index.of_project("p1").len(), 1);
        assert!(index.of_project("p2").is_empty());
    }

    /// The case a naive upsert gets wrong: after a move the document must appear once, at its new path.
    #[test]
    fn a_move_does_not_leave_the_document_at_its_old_path() {
        let index = DocumentsIndex::new();
        index.upsert(&row("d1", "p1", "docs/a.md", false));
        index.upsert(&row("d1", "p1", "docs/b.md", false));

        let entries = index.of_project("p1");
        assert_eq!(entries.len(), 1, "one document, not two");
        assert_eq!(entries[0].path, "docs/b.md");
    }

    #[test]
    fn a_project_list_is_sorted_by_path() {
        let index = DocumentsIndex::new();
        index.upsert(&row("d2", "p1", "docs/z.md", false));
        index.upsert(&row("d1", "p1", "docs/a.md", false));

        let paths: Vec<String> = index
            .of_project("p1")
            .into_iter()
            .map(|itm| itm.path)
            .collect();

        assert_eq!(paths, vec!["docs/a.md", "docs/z.md"]);
    }

    #[test]
    fn removing_takes_it_out_of_both_views() {
        let index = DocumentsIndex::new();
        index.upsert(&row("d1", "p1", "docs/a.md", false));
        index.remove("p1", "d1");

        assert!(index.get("d1").is_none());
        assert!(index.of_project("p1").is_empty());
    }

    /// From a whole row the kind is the truth — which column was filled — rather than a reading of the MIME
    /// type.
    #[test]
    fn the_kind_comes_from_the_payload_on_a_write() {
        let index = DocumentsIndex::new();

        index.upsert(&row("text", "p1", "a.md", false));
        index.upsert(&row("file", "p1", "b.pdf", true));

        assert!(!index.get("text").unwrap().is_binary);
        assert!(index.get("file").unwrap().is_binary);
    }

    /// From an INDEX row there is no payload to look at, so the type decides. Worth pinning: it is the one
    /// place the two can disagree, and the safe direction is to treat the unknown as bytes.
    #[test]
    fn the_kind_comes_from_the_type_when_the_payload_was_not_read() {
        for (content_type, expected_binary) in [
            ("text/markdown", false),
            ("text/plain", false),
            ("application/json", false),
            ("application/pdf", true),
            ("image/png", true),
            ("application/octet-stream", true),
        ] {
            let entry = DocumentIndexEntry::from_index_row(&DocumentIndexDto {
                id: "d1".to_string(),
                project_id: "p1".to_string(),
                doc_path: "a".to_string(),
                content_type: Some(content_type.to_string()),
                content_size: Some(1),
                content_hash: None,
                version: 1,
                created: DateTimeAsMicroseconds::new(0),
                updated: DateTimeAsMicroseconds::new(0),
                updated_by: "AI".to_string(),
            });

            assert_eq!(
                entry.is_binary, expected_binary,
                "{content_type} should be binary: {expected_binary}"
            );
        }
    }
}
