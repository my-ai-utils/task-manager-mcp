//! What a document's content is called when the thing being named is the CONTENT rather than the
//! document.
//!
//! **This exists because a brief belongs to bytes, not to a row.** The same specification can be a
//! document of the project's own on one board, a file in a connected repository on another, and a copy
//! somebody synced into a third — three ids, three paths, three histories, one text. Summarising it three
//! times is three chances to say three different things about one file, and the third of them is written
//! by an agent that has no way of knowing the other two exist. Keyed by the hash of the bytes, it is
//! written once and found by all of them.
//!
//! It also gives the two questions the briefing loop is made of exactly one answer each: "has this been
//! read for me?" is a lookup, and "has it CHANGED since it was read?" is the same lookup — an edit
//! produces different bytes, therefore a different key, therefore a document with no brief again.

use sha2::{Digest, Sha256};

/// The most characters a brief may hold.
///
/// **A brief is read in bulk and a document is not, which is the whole of the number.** Every row of
/// `documents_list` can carry one, so a board of three hundred documents is three hundred of these in one
/// answer — at this cap that is a few hundred kilobytes in the worst case and a page or two in practice.
/// Generous enough for what a brief is for (what the document covers, what it does not, the names in it,
/// the questions it answers) and far too small to be a summary anybody would read instead of the document.
pub const MAX_BRIEF_LEN: usize = 1500;

/// The most of a document that is handed over to be briefed in one go.
///
/// A brief describes the whole document, but nothing is served by shipping a 900 KB specification into a
/// context window to produce three sentences: the shape of a document — its title, its headings, its
/// opening — is in the first few pages, and what is past this cap is reachable with `documents_get` when
/// the first pages are not enough. The read that hits it says so, so nobody has to guess whether they saw
/// all of it.
pub const BRIEF_READ_MAX_BYTES: usize = 64 * 1024;

/// The hash of a document's content: sha256 over the raw bytes, lowercase hex, 64 characters.
///
/// **The bytes, and nothing about where they were.** Not the path, not the id, not the content type —
/// two files with the same text hash alike whether one is a row in Postgres and the other a file in
/// somebody's repository, and that is exactly the property the brief table is built on.
///
/// Written against `&[u8]` rather than `&str` for the same reason: a text document is hashed as its UTF-8
/// bytes and a file off a clone as what is on the disk, so one function covers both and cannot disagree
/// with itself.
pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);

    // Not `format!("{:x}")`: sha2 0.11 hands back a `hybrid_array::Array`, which — unlike the
    // `GenericArray` every remembered snippet was written against — implements no `LowerHex` at all. This
    // is a compile error rather than a wrong answer, but it costs an afternoon to rediscover.
    rust_extensions::hex::utils::array_of_bytes_to_hex(digest.as_slice())
}

/// Read a hash a caller handed over, or say what is wrong with it.
///
/// **Case is normalised and everything else is refused.** The key is compared, never parsed, so one
/// spelling of it has to win — and a caller that pasted a hash with a `0x` on the front, or the first
/// sixteen characters of one, has made a mistake that must not become a brief nobody can find again.
pub fn normalise_content_hash(src: &str) -> Result<String, String> {
    let hash = src.trim().to_lowercase();

    if hash.is_empty() {
        return Err(
            "no hash — a brief is filed under the hash of the content it describes, which every read reports as `content_hash`"
                .to_string(),
        );
    }

    if hash.len() != 64 || !hash.chars().all(|itm| itm.is_ascii_hexdigit()) {
        return Err(format!(
            "'{src}' is not a content hash — those are 64 hex characters, exactly as `content_hash` reports them on documents_get, documents_list and documents_upload"
        ));
    }

    Ok(hash)
}

/// Whether these bytes are text this product will serve as text.
///
/// **The same rule the document reads already apply, in one place so a hash cannot disagree with a
/// read.** A `.md` that is not valid UTF-8, or one with a NUL in it, is served as bytes — so it has no
/// text to brief, and offering it to the briefing loop would hand back a document nobody can summarise.
/// Postgres is the second reason for the NUL: a text column will not take one.
pub fn is_text_bytes(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(text) => !text.contains('\0'),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two vectors everybody knows, so a change of crate, of feature or of encoding is caught here
    /// rather than by a table of briefs that quietly stops matching anything.
    #[test]
    fn the_hash_is_sha256_in_lowercase_hex() {
        assert_eq!(
            content_hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            content_hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let hash = content_hash("специфікація".as_bytes());

        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|itm| itm.is_ascii_hexdigit() && !itm.is_uppercase())
        );
    }

    /// The property the whole brief table rests on: the key is the bytes and nothing else.
    #[test]
    fn the_same_bytes_hash_alike_wherever_they_came_from() {
        let text = "# Design\n\nOne paragraph.\n";

        assert_eq!(
            content_hash(text.as_bytes()),
            content_hash(&text.as_bytes().to_vec())
        );
        assert_ne!(
            content_hash(text.as_bytes()),
            content_hash(b"# Design\n\nOne paragraph.")
        );
    }

    #[test]
    fn a_hash_is_read_in_one_spelling_and_anything_else_is_refused() {
        let hash = content_hash(b"abc");

        assert_eq!(
            normalise_content_hash(&hash.to_uppercase()),
            Ok(hash.clone())
        );
        assert_eq!(normalise_content_hash(&format!("  {hash}  ")), Ok(hash));

        for bad in [
            "",
            "   ",
            "ba7816bf",
            "0xba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015az",
        ] {
            assert!(normalise_content_hash(bad).is_err(), "{bad}");
        }
    }

    /// A file with a text extension whose bytes are not text is a file, and the briefing loop must not be
    /// offered it — it would come back as base64 with nothing in it to summarise.
    #[test]
    fn text_is_utf8_without_a_nul_and_nothing_else_counts() {
        assert!(is_text_bytes(b"# Design"));
        assert!(is_text_bytes("проєкт".as_bytes()));
        assert!(is_text_bytes(b""));

        assert!(!is_text_bytes(&[0xff, 0xfe, 0x00]));
        assert!(!is_text_bytes(b"text with a \0 in it"));
    }
}
