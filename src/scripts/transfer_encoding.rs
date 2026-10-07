//! How a transfer file spells the things a person wrote.
//!
//! Shared by the two export formats this product has — a whole project (`project_transfer`) and the
//! templates a board is configured from (`templates_transfer`) — and shared deliberately: two files that
//! encode prose differently would be two formats to learn, and the second one to be written would be the
//! one that got it subtly wrong.

use rust_extensions::base64::{FromBase64, IntoBase64};
use rust_extensions::date_time::DateTimeAsMicroseconds;

/// Prose, carried as base64.
///
/// **Every field a person wrote goes through this, and nothing else does.** A task's text is Markdown
/// written by a person or an agent: it holds newlines, colons, leading dashes, `#`, quotes and tabs —
/// every character YAML gives a meaning to. Encoding it means the file cannot be mis-parsed by a reader
/// with a different idea of block scalars, and cannot be silently re-indented by a hand edit. Ids,
/// statuses, labels, emails and timestamps stay legible: they are vocabulary, not prose, and none of them
/// can carry a newline.
///
/// The `_base64` suffix on every such field is the other half — the file says what it is doing.
pub fn encode_text(src: &str) -> String {
    src.as_bytes().into_base64()
}

/// The other direction. A field that is not valid base64 is a corrupt file rather than an empty string, so
/// this says so instead of guessing.
pub fn decode_text(src: &str, what: &str) -> Result<String, String> {
    let bytes = src
        .from_base64()
        .map_err(|err| format!("{what} is not valid base64: {err}"))?;

    String::from_utf8(bytes).map_err(|_| format!("{what} did not decode as UTF-8"))
}

/// A moment on the wire: RFC 3339 in UTC, fixed to microseconds — `2026-08-06T09:00:00.000000Z`.
///
/// `to_rfc3339_utc` rather than `to_rfc3339` because the width is fixed, which makes the file sort the way
/// it reads. It is parsed back by `DateTimeAsMicroseconds::from_str`, which accepts exactly this spelling.
pub fn encode_moment(src: DateTimeAsMicroseconds) -> String {
    src.to_rfc3339_utc()
}

pub fn decode_moment(src: &str, what: &str) -> Result<DateTimeAsMicroseconds, String> {
    DateTimeAsMicroseconds::from_str(src).ok_or_else(|| format!("{what} is not a date: '{src}'"))
}

/// The date part of a moment, for naming a downloaded file: `2026-08-06`.
///
/// A folder of exports then sorts by name into the order they were taken. Two on one day land on one name,
/// which the browser resolves by adding its own `(1)` — the behaviour everybody already expects.
pub fn file_name_date(now: DateTimeAsMicroseconds) -> String {
    let stamp = now.to_rfc3339_utc();

    stamp
        .split('T')
        .next()
        .unwrap_or("export")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prose survives the round trip with every character YAML would otherwise have an opinion about.
    #[test]
    fn prose_round_trips_through_base64() {
        let text = "# Heading\n\n- item: with a colon\n  \"quoted\"\n\tTabbed\n";

        assert_eq!(decode_text(&encode_text(text), "x").unwrap(), text);
    }

    #[test]
    fn a_moment_round_trips_at_a_fixed_width() {
        let moment = DateTimeAsMicroseconds::from_str("2026-08-06T09:15:00.000000Z").unwrap();

        assert_eq!(encode_moment(moment), "2026-08-06T09:15:00.000000Z");
        assert_eq!(
            decode_moment(&encode_moment(moment), "x").unwrap().unix_microseconds,
            moment.unix_microseconds
        );
        assert_eq!(file_name_date(moment), "2026-08-06");
    }

    #[test]
    fn what_is_not_base64_is_refused_rather_than_read_as_empty() {
        assert!(decode_text("not base64 at all!!!", "a name").is_err());
        assert!(decode_moment("yesterday", "a moment").is_err());
    }
}
