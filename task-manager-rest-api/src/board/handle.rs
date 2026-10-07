/// The characters a prefix may be made of.
///
/// `-` is excluded on purpose: it is the separator in a handle, and allowing it would make
/// `AB-C-7` ambiguous between prefix `AB` / number `C-7` and prefix `AB-C` / number `7`. Refusing
/// it at the point a prefix is set costs one validation and buys an unambiguous parse forever.
pub fn is_valid_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix.len() <= 16
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Build the handle a person sees: `RMS` + 42 -> `RMS-42`.
///
/// The number as it reads, with nothing added to it. This used to be padded to six digits so a Done
/// list would line up, and the padding cost more than the column it bought: every id an agent handed
/// back had to be read past the zeros, and the UI carried a whole module whose only job was taking
/// them off again.
///
/// The padded spelling is still PARSED — see [`parse_task_handle`]. Nothing composes it any more.
pub fn compose_task_handle(prefix: &str, number: i64) -> String {
    format!("{prefix}-{number}")
}

/// The letter that marks a goal inside a handle: `RMS-G7`.
///
/// A goal takes its number from the same per-project counter a task does, so a number is unique across
/// both and the marker is NOT part of the identity — it says what kind of thing the number names. That is
/// what lets an agent read `RMS-G7` and know it has a goal in front of it without asking anybody.
const GOAL_MARKER: char = 'G';

/// One half of a parsed handle. Named rather than a tuple so a caller cannot swap the two.
///
/// Shared by tasks and goals: the two halves are the same question either way, and with one counter
/// behind both there is nothing to distinguish in the result.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedTaskHandle {
    pub prefix: String,
    pub number: i64,
}

/// Build the handle a person sees for a goal: `RMS` + 7 -> `RMS-G7`.
///
/// The marker and the number, and never anything else — a goal has never had a padded spelling, so
/// unlike a task there is no second form of this to accept back.
pub fn compose_goal_handle(prefix: &str, number: i64) -> String {
    format!("{prefix}-{GOAL_MARKER}{number}")
}

/// Split `RMS-G7` / `rms-g7` into its prefix and number.
///
/// `None` for anything that is not a goal handle — **including a task handle**. `RMS-7` names a number
/// without saying what kind of thing it is, and since one counter serves both, deciding what that number
/// means is a lookup, not a parse: see [`parse_task_handle`], then try the goal with the number it gives.
pub fn parse_goal_handle(src: &str) -> Option<ParsedTaskHandle> {
    parse_marked_handle(src, GOAL_MARKER)
}

/// The letter that marks a release inside a handle: `RMS-R7`.
///
/// The third kind of thing to draw from the project's one counter, and marked for the same reason a goal
/// is: the number is unique across all three, so the letter is not part of the identity — it says what the
/// number names.
const RELEASE_MARKER: char = 'R';

/// Build the handle a person sees for a release: `RMS` + 7 -> `RMS-R7`.
pub fn compose_release_handle(prefix: &str, number: i64) -> String {
    format!("{prefix}-{RELEASE_MARKER}{number}")
}

/// Split `RMS-R7` / `rms-r7` into its prefix and number.
///
/// `None` for anything that is not a release handle, a task's and a goal's included — the same rule
/// [`parse_goal_handle`] holds to, and for the same reason: a tool asked to change a release must not act
/// on something else because of a letter.
pub fn parse_release_handle(src: &str) -> Option<ParsedTaskHandle> {
    parse_marked_handle(src, RELEASE_MARKER)
}

/// The shared half of the two marked spellings: a valid prefix, the marker, then digits and nothing else.
fn parse_marked_handle(src: &str, marker: char) -> Option<ParsedTaskHandle> {
    let (prefix, marked_number) = src.trim().rsplit_once('-')?;

    let prefix = prefix.trim();

    // Held to the same rule as a prefix being assigned, for the same reason as in `parse_task_handle`.
    if !is_valid_prefix(prefix) {
        return None;
    }

    let mut chars = marked_number.chars();

    if !chars.next()?.eq_ignore_ascii_case(&marker) {
        return None;
    }

    let digits = chars.as_str();

    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let number: i64 = digits.parse().ok()?;

    if number <= 0 {
        return None;
    }

    Some(ParsedTaskHandle {
        prefix: prefix.to_uppercase(),
        number,
    })
}

/// Split `RMS-42` / `RMS-000042` / `rms-42` into its prefix and number.
///
/// The padded spelling is accepted although nothing composes it any more: it was what this board
/// handed out for months, so it is sitting in chat logs, in commit messages and in `?search=` links
/// that were shared before the change. Leading zeros carry no meaning here — a number is a number —
/// so refusing them would break those references to prove a point nobody benefits from.
///
/// Returns `None` for anything that is not a handle at all — no separator, an empty half, a
/// non-numeric or non-positive number, or a prefix that no project could legally hold. The prefix
/// comes back upper-cased, which is the spelling prefixes are stored in, so a caller never has to
/// normalise before looking one up.
pub fn parse_task_handle(src: &str) -> Option<ParsedTaskHandle> {
    // Split on the LAST separator: the number is always the final segment.
    let (prefix, number) = src.trim().rsplit_once('-')?;

    let prefix = prefix.trim();

    if number.is_empty() {
        return None;
    }

    // The prefix is held to the same rule as one being assigned to a project. Without this,
    // `RMS--5` would split into prefix `RMS-` and number 5 and "parse" into a handle whose prefix
    // can never exist — a lookup would then fail for the wrong reason, several layers away.
    if !is_valid_prefix(prefix) {
        return None;
    }

    // Digits only, checked before parsing: `"-5".parse::<i64>()` succeeds, and a negative number is
    // not a task.
    if !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let number: i64 = number.parse().ok()?;

    if number <= 0 {
        return None;
    }

    Some(ParsedTaskHandle {
        prefix: prefix.to_uppercase(),
        number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number as it reads, at every width. A composed handle is now exactly what a person would
    /// type, which is the whole point of the change.
    #[test]
    fn a_composed_handle_carries_the_bare_number() {
        assert_eq!(compose_task_handle("RMS", 42), "RMS-42");
        assert_eq!(compose_task_handle("RMS", 1), "RMS-1");
        assert_eq!(compose_task_handle("RMS", 12_345_678), "RMS-12345678");
    }

    /// The padded form is history, not output — anything still quoting it has to keep resolving.
    #[test]
    fn padded_and_unpadded_spellings_parse_to_the_same_task() {
        let expected = ParsedTaskHandle {
            prefix: "RMS".to_string(),
            number: 42,
        };

        assert_eq!(parse_task_handle("RMS-000042"), Some(expected));
        assert_eq!(
            parse_task_handle("RMS-42"),
            Some(ParsedTaskHandle {
                prefix: "RMS".to_string(),
                number: 42,
            })
        );
    }

    #[test]
    fn parsing_upper_cases_the_prefix_and_ignores_surrounding_space() {
        assert_eq!(
            parse_task_handle("  rms-7 "),
            Some(ParsedTaskHandle {
                prefix: "RMS".to_string(),
                number: 7,
            })
        );
    }

    /// Everything that is not a handle has to come back as "not a handle", so a caller can say so
    /// rather than looking up something it invented.
    #[test]
    fn non_handles_are_refused() {
        for src in [
            "", "RMS",     // no separator
            "RMS-",    // no number
            "-42",     // no prefix
            "RMS-abc", // not a number
            "RMS--5",  // splits into prefix `RMS-`, which no project may hold
            "RMS-0",   // numbers start at 1
            "RMS-4 2", // space inside the number
            "RMS-4.2", // not an integer
            "АБВ-1",   // non-ascii prefix
        ] {
            assert_eq!(parse_task_handle(src), None, "{src:?} should not parse");
        }
    }

    #[test]
    fn a_goal_handle_carries_its_marker_and_nothing_else() {
        assert_eq!(compose_goal_handle("RMS", 7), "RMS-G7");
        assert_eq!(compose_goal_handle("RMS", 1_234), "RMS-G1234");
    }

    #[test]
    fn a_goal_handle_round_trips_in_either_case() {
        let expected = ParsedTaskHandle {
            prefix: "RMS".to_string(),
            number: 7,
        };

        assert_eq!(parse_goal_handle("RMS-G7"), Some(expected));
        assert_eq!(
            parse_goal_handle("  rms-g7 "),
            Some(ParsedTaskHandle {
                prefix: "RMS".to_string(),
                number: 7,
            })
        );
    }

    /// The two spellings must not bleed into each other. A task parser that accepted `RMS-G7` would hand
    /// back number 7 and lose the one bit that said which of the two things it is — and with one counter
    /// behind both, that number names something real either way, so nothing downstream would notice.
    #[test]
    fn the_two_handle_kinds_do_not_parse_as_each_other() {
        assert_eq!(parse_task_handle("RMS-G7"), None);
        assert_eq!(parse_goal_handle("RMS-7"), None);
    }

    #[test]
    fn a_release_handle_round_trips_in_either_case() {
        assert_eq!(compose_release_handle("RMS", 12), "RMS-R12");

        for src in ["RMS-R12", "  rms-r12 "] {
            assert_eq!(
                parse_release_handle(src),
                Some(ParsedTaskHandle {
                    prefix: "RMS".to_string(),
                    number: 12,
                }),
                "{src:?} should parse"
            );
        }
    }

    /// Three kinds of thing share one counter, so the marker is the ONLY thing that says which one a handle
    /// names. A parser that accepted another kind's spelling would hand back a number that is real — it
    /// names something — and nothing downstream could tell it had the wrong thing.
    #[test]
    fn a_release_handle_is_neither_a_task_nor_a_goal() {
        assert_eq!(parse_task_handle("RMS-R7"), None);
        assert_eq!(parse_goal_handle("RMS-R7"), None);
        assert_eq!(parse_release_handle("RMS-7"), None);
        assert_eq!(parse_release_handle("RMS-G7"), None);

        for src in ["RMS-R", "RMS-R0", "RMS-RR7", "RMS-R7x", "RMS-R-7"] {
            assert_eq!(parse_release_handle(src), None, "{src:?} should not parse");
        }
    }

    #[test]
    fn non_goal_handles_are_refused() {
        for src in [
            "", "RMS", "RMS-G",     // marker with no number
            "RMS-G0",    // numbers start at 1
            "RMS-GG7",   // marker twice
            "RMS-G7x",   // trailing rubbish
            "RMS-G 7",   // space inside the number
            "RMS-G-7",   // splits into prefix `RMS-G`, which no project may hold
            "RMS-G4.2",  // not an integer
            "АБВ-G1",    // non-ascii prefix
        ] {
            assert_eq!(parse_goal_handle(src), None, "{src:?} should not parse");
        }
    }

    /// `-` in a prefix is what would make a handle ambiguous, so it is refused where the prefix is
    /// set rather than guessed at where it is read.
    #[test]
    fn prefix_validation_refuses_what_would_make_a_handle_ambiguous() {
        assert!(is_valid_prefix("RMS"));
        assert!(is_valid_prefix("TASK_MANAGER"));
        assert!(is_valid_prefix("A1"));

        assert!(!is_valid_prefix(""));
        assert!(!is_valid_prefix("AB-C"));
        assert!(!is_valid_prefix("AB C"));
        assert!(!is_valid_prefix("АБВ")); // non-ascii
        assert!(!is_valid_prefix("THIS_PREFIX_IS_FAR_TOO_LONG"));
    }

    /// Compose and parse are each other's inverse for every valid prefix — otherwise an id shown on
    /// a sticker would not be an id anyone could type back.
    #[test]
    fn compose_and_parse_round_trip() {
        for (prefix, number) in [("RMS", 1_i64), ("TM", 999_999), ("A1", 42), ("X_Y", 7)] {
            let handle = compose_task_handle(prefix, number);
            assert_eq!(
                parse_task_handle(&handle),
                Some(ParsedTaskHandle {
                    prefix: prefix.to_string(),
                    number,
                }),
                "{handle} does not round-trip"
            );
        }
    }
}
