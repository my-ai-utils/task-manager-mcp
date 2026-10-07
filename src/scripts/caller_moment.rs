//! A moment a caller WROTE, read strictly.
//!
//! Every other moment on this board is stamped by the server as the thing happens. A release is the
//! exception: it is written down after the fact, so when it went out is something the caller says — and a
//! date typed by a person or an agent arrives in more spellings than a timestamp this service wrote itself.
//!
//! Not `DateTimeAsMicroseconds::from_str` directly, which is what reads the transfer files, for one reason
//! that matters: that parser reads the clock at fixed offsets and **ignores a zone offset entirely**, so
//! `2026-10-07T14:30:00+03:00` comes back as 14:30 UTC — three hours wrong, and silently. It is the right
//! parser for a file this service wrote, which is always UTC. It is the wrong one for a value somebody
//! copied out of `date -Iseconds` or a CI log. So the zone is split off and applied here, and only the part
//! that parser reads correctly is handed to it.

use rust_extensions::date_time::DateTimeAsMicroseconds;

/// Read `2026-10-07`, `2026-10-07T14:30`, `2026-10-07T14:30:00Z` or `2026-10-07T14:30:00+03:00`.
///
/// * A bare date is midnight UTC.
/// * No zone means UTC — the same reading everything else here gives a time.
/// * A zone offset is **honoured**: the result is the same instant, in UTC.
/// * Fractions of a second are accepted and dropped; nothing here is recorded finer than a second.
///
/// A unix timestamp is refused, deliberately. Every view reports moments as unix seconds, so it would be
/// natural to accept one back — but a bare number says nothing about its unit, and a guess that turned
/// milliseconds into a date in 1970 would be stored without complaint.
///
/// `what` names the field in the refusal, so a call that passed two dates is told which one is wrong.
pub fn parse_caller_moment(src: &str, what: &str) -> Result<DateTimeAsMicroseconds, String> {
    let src = src.trim();

    let refuse = || {
        format!(
            "{what} '{src}' is not a date — expected `2026-10-07`, or a date and time like `2026-10-07T14:30:00Z`. A zone offset such as `+03:00` is honoured, and no zone means UTC"
        )
    };

    // ASCII only, checked before anything is sliced by byte position.
    if src.len() < 10 || !src.is_ascii() {
        return Err(refuse());
    }

    let (date, rest) = src.split_at(10);

    if !is_date_shape(date) {
        return Err(refuse());
    }

    let (clock, offset_minutes) = if rest.is_empty() {
        ("00:00:00".to_string(), 0)
    } else {
        let mut chars = rest.chars();

        // `T` is the standard; a space is what a person types and what `date` prints.
        if !matches!(chars.next(), Some('T' | 't' | ' ')) {
            return Err(refuse());
        }

        split_clock(chars.as_str()).ok_or_else(refuse)?
    };

    // The calendar is checked by the library: the 30th of February and 25 o'clock both come back as
    // nothing, which is the validation worth not writing twice.
    let local =
        DateTimeAsMicroseconds::from_str(&format!("{date}T{clock}")).ok_or_else(refuse)?;

    // A local time is UTC plus its offset, so UTC is the local time minus it.
    Ok(DateTimeAsMicroseconds::new(
        local.unix_microseconds - offset_minutes * 60 * 1_000_000,
    ))
}

/// Whether ten ASCII bytes are shaped `YYYY-MM-DD`. Shape only — whether it is a real day is the
/// library's question.
fn is_date_shape(src: &str) -> bool {
    let bytes = src.as_bytes();

    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// The time half: the clock normalised to `HH:MM:SS`, and the zone as minutes east of UTC.
fn split_clock(src: &str) -> Option<(String, i64)> {
    let (clock, offset_minutes) =
        if let Some(clock) = src.strip_suffix('Z').or_else(|| src.strip_suffix('z')) {
            (clock, 0)
        } else if let Some(position) = src.find(['+', '-']) {
            let (clock, zone) = src.split_at(position);
            (clock, parse_offset(zone)?)
        } else {
            (src, 0)
        };

    let clock = match clock.split_once(['.', ',']) {
        Some((whole, fraction)) => {
            if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }

            whole
        }
        None => clock,
    };

    let parts: Vec<&str> = clock.split(':').collect();

    let all_two_digits = parts
        .iter()
        .all(|itm| itm.len() == 2 && itm.bytes().all(|byte| byte.is_ascii_digit()));

    if !all_two_digits || !(parts.len() == 2 || parts.len() == 3) {
        return None;
    }

    let seconds = parts.get(2).copied().unwrap_or("00");

    Some((
        format!("{}:{}:{seconds}", parts[0], parts[1]),
        offset_minutes,
    ))
}

/// `+03:00`, `+0300` or `+03`, as minutes east of UTC. Negative for a zone west of it.
fn parse_offset(src: &str) -> Option<i64> {
    let (sign, digits) = match src.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-", digits) => (-1, digits),
        _ => return None,
    };

    let (hours, minutes) = match digits.len() {
        2 => (digits, "00"),
        4 => digits.split_at(2),
        5 if digits.as_bytes()[2] == b':' => (&digits[..2], &digits[3..]),
        _ => return None,
    };

    let read = |itm: &str| -> Option<i64> {
        if itm.len() == 2 && itm.bytes().all(|byte| byte.is_ascii_digit()) {
            itm.parse().ok()
        } else {
            None
        }
    };

    let (hours, minutes) = (read(hours)?, read(minutes)?);

    // The widest zone in use is +14:00. Anything past that is a typo rather than a place.
    if hours > 14 || minutes > 59 {
        return None;
    }

    Some(sign * (hours * 60 + minutes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(src: &str) -> i64 {
        DateTimeAsMicroseconds::from_str(src)
            .unwrap()
            .unix_microseconds
    }

    fn read(src: &str) -> i64 {
        parse_caller_moment(src, "the date").unwrap().unix_microseconds
    }

    #[test]
    fn a_bare_date_is_midnight_utc() {
        assert_eq!(read("2026-10-07"), utc("2026-10-07T00:00:00.000000Z"));
        assert_eq!(read("  2026-10-07  "), utc("2026-10-07T00:00:00.000000Z"));
    }

    #[test]
    fn a_time_with_no_zone_is_utc() {
        let expected = utc("2026-10-07T14:30:00.000000Z");

        for src in [
            "2026-10-07T14:30:00Z",
            "2026-10-07T14:30:00",
            "2026-10-07T14:30",
            "2026-10-07 14:30:00",
            "2026-10-07t14:30:00z",
            "2026-10-07T14:30:00.123456Z",
        ] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    /// The reason this module exists. The library parser reads `14:30` and stops; a release recorded with
    /// a local offset would be filed three hours from when it happened and nothing would say so.
    #[test]
    fn a_zone_offset_is_applied_rather_than_ignored() {
        assert_eq!(
            read("2026-10-07T14:30:00+03:00"),
            utc("2026-10-07T11:30:00.000000Z")
        );
        assert_eq!(
            read("2026-10-07T01:00:00-02:30"),
            utc("2026-10-07T03:30:00.000000Z")
        );
        assert_eq!(
            read("2026-10-07T14:30:00+0300"),
            utc("2026-10-07T11:30:00.000000Z")
        );
        assert_eq!(
            read("2026-10-07T14:30+03"),
            utc("2026-10-07T11:30:00.000000Z")
        );
        // A zone can carry a moment over midnight, into the day before.
        assert_eq!(
            read("2026-10-07T01:00:00+03:00"),
            utc("2026-10-06T22:00:00.000000Z")
        );
    }

    #[test]
    fn what_is_not_a_date_is_refused_and_says_which_field() {
        for src in [
            "",
            "yesterday",
            "07.10.2026",
            "2026/10/07",
            "2026-13-01",          // no thirteenth month
            "2026-02-30",          // no such day
            "2026-10-07T25:00:00Z", // no such hour
            "2026-10-07X14:30:00",
            "2026-10-07T14:30:00+3",
            "2026-10-07T14:30:00+25:00",
            "2026-10-07T1430",
            "2026-10-07T14:30:00.Z",
            "1759836000", // a unix timestamp says nothing about its unit
            "2026-10-07T14:30:00Zjunk",
        ] {
            let refused = parse_caller_moment(src, "the release date");

            assert!(refused.is_err(), "{src:?} should be refused");
            assert!(refused.unwrap_err().starts_with("the release date"));
        }
    }
}
