use task_manager_shared::priority::Priority;

/// Read a priority a caller wrote, or say what the five values are.
///
/// **Refused rather than defaulted**, which is the opposite of how a priority read back OUT of the database
/// is treated. A write is somebody deciding; quietly turning a typo into Normal is how a task nobody meant to
/// deprioritise ends up at the bottom of a column with no trace of why. A read has no decision in it — an
/// unrecognised stored value must not be able to fail a board — so the leniency lives on that side only.
///
/// Generous about spelling: `Super High`, `super_high` and `superhigh` all read, because the refusal exists
/// to catch a value that means nothing, not a hyphen.
///
/// Its own module rather than a helper on the task path, because a goal takes the same field and one function
/// is what keeps the two from growing two different sets of accepted spellings.
pub fn parse_priority(src: &str) -> Result<Priority, String> {
    src.parse::<Priority>().map_err(|_| {
        format!(
            "'{src}' is not a priority; it is one of: {}",
            Priority::ALL
                .iter()
                .map(|itm| rust_extensions::AsStr::as_str(itm))
                .collect::<Vec<&str>>()
                .join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_values_read() {
        for priority in Priority::ALL {
            assert_eq!(
                parse_priority(rust_extensions::AsStr::as_str(priority)),
                Ok(*priority)
            );
        }
    }

    /// The whole reason this is not `parse_or_default`: a value that means nothing has to come back as a
    /// message naming the five that do, not as a silent Normal.
    #[test]
    fn nonsense_is_refused_and_the_message_names_the_scale() {
        let err = parse_priority("p0").expect_err("should be refused");

        assert!(err.contains("super-high"), "{err}");
        assert!(err.contains("super-low"), "{err}");
    }

    /// Blank is refused too. A caller that wants to leave the priority alone omits the field; passing an
    /// empty string is the shape that clears a kind or an assignee, and there is nothing to clear here —
    /// every task has a priority, and "none" is spelled `normal`.
    #[test]
    fn blank_is_refused_because_there_is_nothing_to_clear() {
        assert!(parse_priority("").is_err());
        assert!(parse_priority("   ").is_err());
    }
}
