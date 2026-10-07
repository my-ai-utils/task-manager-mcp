use std::io::Read;

use task_manager_shared::documents::{content_type_for_path, normalise_document_path};

use crate::app::AppContext;
use crate::postgres::DocumentDto;

use super::{DocumentBody, MAX_BINARY_LEN, MAX_CONTENT_LEN, NewDocumentContent};

/// The most files one archive may write.
///
/// Not a technical ceiling — it is the point past which "I dropped a folder in" has stopped being what
/// happened. Every entry is a row, a version row and an index entry, all written one after another inside a
/// single request; a zip of somebody's whole home directory would sit there for minutes and then be a mess
/// nobody can undo, because there is no "undo an upload" on this board.
pub const MAX_ARCHIVE_ENTRIES: usize = 500;

/// The most an archive may unpack to, in bytes, added up across every entry.
///
/// The archive itself is bounded by what a document may be, and that bound says nothing about what comes out
/// of it: a few hundred KB of zeroes compresses to nothing and unpacks to gigabytes. So the total is counted
/// as entries are read and the read stops when it is reached, rather than being trusted from the headers —
/// the headers are written by whoever made the archive.
pub const MAX_ARCHIVE_UNPACKED: u64 = 64 * 1024 * 1024;

/// One file out of an archive: where it goes and what it holds.
pub struct ArchiveEntry {
    /// The entry's path INSIDE the archive, normalised. Not yet the document's path — the folder it is being
    /// unpacked into is still to be prepended.
    pub path: String,
    pub bytes: Vec<u8>,
}

/// An entry that was not written, and why. Reported rather than raised: see [`unpack_archive`].
pub struct SkippedEntry {
    pub name: String,
    pub reason: String,
}

impl SkippedEntry {
    fn new(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            reason: reason.into(),
        }
    }
}

/// Unpack a ZIP into a project's documents, one document per file inside it.
///
/// **The archive is a transport and is never stored.** What lands is the files, at `folder` + each entry's own
/// path — so the tree inside the zip becomes the tree in the project, and re-uploading a zip after editing one
/// file in it writes a new version of exactly that document, by the same path-is-the-key rule every upload
/// follows. See [`super::upload_document`].
///
/// **Partial by design.** A zip made out of a real folder carries things that are not documents, and refusing
/// the whole upload over a `.DS_Store` would make the feature useless. Every entry that can be written is
/// written and the rest come back as skips with a reason each — which the caller shows, so nobody is left
/// believing a file arrived when it did not.
pub async fn upload_archive(
    app: &AppContext,
    project_prefix: &str,
    folder: Option<&str>,
    archive: &[u8],
    who: &str,
) -> Result<(Vec<DocumentDto>, Vec<SkippedEntry>), String> {
    // Before anything is read: the project has to exist, and the caller has to be somebody. Both are checked
    // again per entry by `upload_document` — this is only so an archive is not unpacked to find out.
    {
        let board = app.board.read();
        super::resolve_project_by_prefix(&board, project_prefix)?;
    }

    let (entries, mut skipped) = unpack_archive(archive)?;

    let mut written = Vec::with_capacity(entries.len());

    for entry in entries {
        let path = join_folder(folder, &entry.path);

        // The body is decided from the entry's own path, not from the archive: a `.md` inside a zip is text
        // and has to be readable and diffable, exactly as one written through MCP is. See [`body_for_entry`].
        let content = NewDocumentContent {
            body: body_for_entry(&path, entry.bytes),
            content_type: None,
        };

        match super::upload_document(app, project_prefix, &path, content, who).await {
            Ok(row) => written.push(row),
            // One entry's problem is one entry's problem. The rest of the archive still arrives.
            Err(err) => skipped.push(SkippedEntry::new(entry.path, err)),
        }
    }

    Ok((written, skipped))
}

/// Read a zip into the entries worth writing, and the reasons the others are not.
///
/// Separate from the write loop above so it can be tested against a real archive without a database, which is
/// where the rules that matter live: what is a directory, what is noise, what is a path this product accepts.
fn unpack_archive(archive: &[u8]) -> Result<(Vec<ArchiveEntry>, Vec<SkippedEntry>), String> {
    if archive.is_empty() {
        return Err("that archive is empty".to_string());
    }

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|err| format!("that file is not a zip we can read: {err}"))?;

    let mut entries: Vec<ArchiveEntry> = Vec::new();
    let mut skipped: Vec<SkippedEntry> = Vec::new();
    let mut unpacked: u64 = 0;

    for index in 0..zip.len() {
        let mut file = zip
            .by_index(index)
            .map_err(|err| format!("that archive could not be read at entry {index}: {err}"))?;

        let name = file.name().to_string();

        // A folder is not a document — folders are derived from the paths of the documents in them, so an
        // entry that only names one carries nothing to write. Not a skip either: there is nothing missing.
        if file.is_dir() || name.ends_with('/') {
            continue;
        }

        if is_noise(&name) {
            continue;
        }

        if entries.len() >= MAX_ARCHIVE_ENTRIES {
            skipped.push(SkippedEntry::new(
                name,
                format!("this archive holds more than {MAX_ARCHIVE_ENTRIES} files, which is the limit"),
            ));
            continue;
        }

        // The zip's own idea of the path, put through the same rule every document path goes through — which
        // is also the sanitiser: `..` is refused there, and a leading slash or a backslash separator is
        // normalised rather than being allowed to name somewhere else.
        let path = match normalise_document_path(&name) {
            Ok(path) => path,
            Err(problem) => {
                skipped.push(SkippedEntry::new(name, problem));
                continue;
            }
        };

        let declared = file.size();

        if declared > MAX_BINARY_LEN as u64 {
            skipped.push(SkippedEntry::new(
                name,
                format!("it is {declared} bytes — the limit for one document is {MAX_BINARY_LEN}"),
            ));
            continue;
        }

        if unpacked + declared > MAX_ARCHIVE_UNPACKED {
            skipped.push(SkippedEntry::new(
                name,
                format!(
                    "the archive unpacks to more than {MAX_ARCHIVE_UNPACKED} bytes, which is the limit"
                ),
            ));
            continue;
        }

        let mut bytes = Vec::with_capacity(declared as usize);

        if let Err(err) = file.read_to_end(&mut bytes) {
            skipped.push(SkippedEntry::new(name, format!("it did not decompress: {err}")));
            continue;
        }

        // An empty file is a real thing in a folder and not a document: a payload has to hold something, and
        // `upload_document` would refuse it one line later with a worse message.
        if bytes.is_empty() {
            skipped.push(SkippedEntry::new(name, "it is empty"));
            continue;
        }

        unpacked += bytes.len() as u64;
        entries.push(ArchiveEntry { path, bytes });
    }

    if entries.is_empty() && skipped.is_empty() {
        return Err("that archive holds no files".to_string());
    }

    Ok((entries, skipped))
}

/// What a zip made on a real machine carries that nobody meant to upload.
///
/// Dropped silently rather than reported: a skip is a line the reader has to read, and "your archive contained
/// a `.DS_Store`" is not news to anybody. Everything else that cannot be written IS reported.
fn is_noise(name: &str) -> bool {
    // macOS puts a whole shadow tree beside the files when it zips a folder in Finder.
    if name.starts_with("__MACOSX/") || name.contains("/__MACOSX/") {
        return true;
    }

    matches!(
        task_manager_shared::documents::document_file_name(name),
        ".DS_Store" | "Thumbs.db" | "desktop.ini"
    )
}

/// A destination folder and an entry's own path into the document's path.
///
/// The entry keeps its whole path, which is the point: a zip of a folder tree unpacks as that tree rather than
/// as a heap of files with their names collided together in one folder.
fn join_folder(folder: Option<&str>, entry_path: &str) -> String {
    let folder = folder.unwrap_or_default().trim().trim_matches('/');

    if folder.is_empty() {
        entry_path.to_string()
    } else {
        format!("{folder}/{entry_path}")
    }
}

/// Whether an entry is stored as TEXT or as BYTES, decided from its path and then confirmed against its
/// content.
///
/// **This is the one place the product guesses, and the guess is narrow on purpose.** A browser upload stores
/// bytes always and says why: it is handed a file, and deciding it is "really text" would mean guessing an
/// encoding. Here there is more to go on — the extension table both sides already trust to say what a document
/// IS — and much more to lose: a zip of a docs folder is the whole use case, and storing forty Markdown files
/// as binary makes every one of them a download rather than a page, undiffable and unsearchable.
///
/// So text is chosen only when the extension says text AND the bytes really are UTF-8 with no NUL in them.
/// Anything else — including a `.md` that turns out not to be UTF-8 — is stored as what it verifiably is.
pub fn body_for_entry(path: &str, bytes: Vec<u8>) -> DocumentBody {
    if !is_text_content_type(content_type_for_path(path)) {
        return DocumentBody::Binary(bytes);
    }

    match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') && text.chars().count() <= MAX_CONTENT_LEN => {
            DocumentBody::Text(text)
        }
        // Not utf-8, or too long for a text column. It is still a file, and a file it stays.
        Ok(text) => DocumentBody::Binary(text.into_bytes()),
        Err(err) => DocumentBody::Binary(err.into_bytes()),
    }
}

/// Which of the types the extension table can produce are text that belongs in the text column.
///
/// `text/html` is one of them, deliberately: html is stored as text everywhere in this product — it is
/// diffable and searchable — and it is the viewer that decides to frame it.
fn is_text_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        // Nothing to go on. The table knows the names that are text by convention — `Makefile`,
        // `LICENSE`, `.gitignore` — so what reaches here is a suffix nobody has a type for, and a guess
        // with no evidence behind it is the guess the browser upload refuses to make.
        return false;
    };

    content_type.starts_with("text/")
        || matches!(content_type, "application/json" | "application/yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a zip in memory, so the tests read real archives rather than a stand-in for one.
    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        use zip::write::SimpleFileOptions;

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));

        for (name, bytes) in files {
            writer.start_file(*name, SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut writer, bytes).unwrap();
        }

        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn every_file_in_the_archive_comes_out_at_its_own_path() {
        let archive = zip_of(&[
            ("readme.md", b"# hello"),
            ("design/system.md", b"# system"),
        ]);

        let (entries, skipped) = unpack_archive(&archive).unwrap();

        assert!(skipped.is_empty());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "readme.md");
        assert_eq!(entries[1].path, "design/system.md");
        assert_eq!(entries[1].bytes, b"# system");
    }

    /// The tree inside the zip becomes the tree in the project, hung under the folder that was chosen.
    #[test]
    fn the_folder_is_prepended_to_the_whole_path() {
        assert_eq!(join_folder(Some("docs"), "a/b.md"), "docs/a/b.md");
        assert_eq!(join_folder(Some("/docs/"), "b.md"), "docs/b.md");
        assert_eq!(join_folder(Some("  "), "b.md"), "b.md");
        assert_eq!(join_folder(None, "a/b.md"), "a/b.md");
    }

    /// What Finder puts in a zip is not what the person meant to upload, and telling them so is not news.
    #[test]
    fn the_noise_a_real_folder_carries_is_dropped_without_a_word() {
        let archive = zip_of(&[
            ("__MACOSX/._readme.md", b"junk"),
            ("docs/.DS_Store", b"junk"),
            ("readme.md", b"# hello"),
        ]);

        let (entries, skipped) = unpack_archive(&archive).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "readme.md");
        assert!(skipped.is_empty(), "noise is dropped, not reported");
    }

    /// One bad entry must not cost the other forty. Both halves of the answer are returned.
    #[test]
    fn an_entry_that_cannot_be_written_is_reported_and_the_rest_still_arrive() {
        let archive = zip_of(&[
            ("../escape.md", b"nope"),
            ("empty.md", b""),
            ("readme.md", b"# hello"),
        ]);

        let (entries, skipped) = unpack_archive(&archive).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "readme.md");

        assert_eq!(skipped.len(), 2);
        assert_eq!(skipped[0].name, "../escape.md");
        assert_eq!(skipped[1].reason, "it is empty");
    }

    #[test]
    fn what_is_not_a_zip_is_refused_outright() {
        assert!(unpack_archive(b"").is_err());
        assert!(unpack_archive(b"this is not a zip at all").is_err());
    }

    /// The whole reason this decides text at all: a zip of a docs folder has to arrive as documents that
    /// render, not as forty downloads.
    #[test]
    fn a_markdown_entry_is_stored_as_text_and_a_png_as_bytes() {
        assert_eq!(
            body_for_entry("docs/a.md", b"# hello".to_vec()),
            DocumentBody::Text("# hello".to_string())
        );

        let png = vec![0x89, 0x50, 0x4E, 0x47];
        assert_eq!(
            body_for_entry("logo.png", png.clone()),
            DocumentBody::Binary(png)
        );

        // A name the table knows IS evidence, even with no extension on it — and the bytes are still
        // checked, which is what keeps this from being a guess.
        assert_eq!(
            body_for_entry("Makefile", b"all:".to_vec()),
            DocumentBody::Text("all:".to_string())
        );

        // A name it does not know, on the other hand, is nothing to go on and stays a file.
        assert!(body_for_entry("thing.unknownext", b"all:".to_vec()).is_binary());
    }

    /// The confirmation half. An extension is a claim, and bytes that are not utf-8 are the counter-evidence —
    /// storing them as text would either fail in Postgres or store mojibake.
    #[test]
    fn a_text_extension_over_bytes_that_are_not_text_stays_bytes() {
        let not_utf8 = vec![0xFF, 0xFE, 0x00, 0x01];
        assert!(body_for_entry("a.md", not_utf8.clone()).is_binary());

        let with_nul = b"hello\0world".to_vec();
        assert!(body_for_entry("a.txt", with_nul).is_binary());
    }
}
