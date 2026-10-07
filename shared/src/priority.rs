use rust_extensions::AsStr;
use std::str::FromStr;

/// How urgent a task or a goal is — the one field that decides where it sits in its column.
///
/// A fixed five-value scale rather than a number: a number invites 7-vs-8 arguments and drifts upward until
/// everything is a 9, whereas five named steps make a person choose between *this* and *that*. It is also
/// per-product rather than per-project, unlike columns and kinds — urgency does not mean something different
/// on another board, and one scale is what lets two boards be read side by side.
///
/// On the wire it is a `String`, like a kind's colour, and [`Priority::parse_or_default`] reads it back: an
/// unknown value falls back to [`Priority::Normal`] rather than failing the read that carried it. Which is
/// also what every row written before this field existed reads as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    SuperHigh,
    High,
    #[default]
    Normal,
    Low,
    SuperLow,
}

impl Priority {
    /// Every value, **top of the list first** — which is the order a picker offers them in and the order
    /// they sit in on a board.
    pub const ALL: &'static [Self] = &[
        Self::SuperHigh,
        Self::High,
        Self::Normal,
        Self::Low,
        Self::SuperLow,
    ];

    /// How far down the column this priority goes: `0` is the top.
    ///
    /// Deliberately a number that ASCENDS as urgency falls, so every sort site is a plain
    /// `sort_by_key(|itm| itm.priority.order())` with no `Reverse` to forget. `Ord` is not derived on the
    /// enum on purpose: `SuperHigh < Low` would have to be true for the derive to sort correctly, and code
    /// reading `a < b` as "a is less urgent" would then be quietly backwards.
    pub fn order(&self) -> u8 {
        match self {
            Self::SuperHigh => 0,
            Self::High => 1,
            Self::Normal => 2,
            Self::Low => 3,
            Self::SuperLow => 4,
        }
    }

    /// How it is named to a person.
    pub fn title(&self) -> &'static str {
        match self {
            Self::SuperHigh => "Super High",
            Self::High => "High",
            Self::Normal => "Normal",
            Self::Low => "Low",
            Self::SuperLow => "Super Low",
        }
    }

    /// Whether a card should say anything about its priority at all.
    ///
    /// Normal is what almost everything is, and a badge repeated on every card is a badge nobody sees. Only
    /// the four values that are a deliberate choice are drawn — which is what makes them legible.
    pub fn is_worth_showing(&self) -> bool {
        !matches!(self, Self::Normal)
    }

    /// The colour of the badge, from the same palette task types and goals are coloured from: the product
    /// has one set of colours, and a sixth red would make the other five mean less.
    ///
    /// Urgent runs warm and quiet runs cool, so the scale reads without the words being read.
    pub fn hex(&self) -> &'static str {
        match self {
            Self::SuperHigh => "#dc3545",
            Self::High => "#fd7e14",
            Self::Normal => "#6c757d",
            Self::Low => "#0d6efd",
            Self::SuperLow => "#97a0af",
        }
    }

    /// Read a wire value, falling back to Normal.
    ///
    /// Infallible on purpose, exactly as a colour is: an unrecognised priority — or the empty string a row
    /// written before this field carries — must not be able to fail a board read.
    pub fn parse_or_default(src: &str) -> Self {
        Self::from_str(src).unwrap_or_default()
    }

    /// Where a wire value sorts, for a reader holding the string rather than the enum.
    pub fn order_of(src: &str) -> u8 {
        Self::parse_or_default(src).order()
    }
}

impl AsStr for Priority {
    fn as_str(&self) -> &'static str {
        match self {
            Self::SuperHigh => "super-high",
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
            Self::SuperLow => "super-low",
        }
    }
}

impl FromStr for Priority {
    type Err = ();

    /// Reads the wire value, and also what a person or an agent is likely to type instead: `Super High`,
    /// `super_high`, `superhigh`. A write refuses what it cannot read, so being generous here is the
    /// difference between a tool call that works and one that comes back with a list of five spellings.
    fn from_str(src: &str) -> Result<Self, Self::Err> {
        let src: String = src
            .trim()
            .to_lowercase()
            .chars()
            .filter(|itm| itm.is_ascii_alphanumeric())
            .collect();

        Self::ALL
            .iter()
            .find(|itm| {
                itm.as_str()
                    .chars()
                    .filter(|itm| itm.is_ascii_alphanumeric())
                    .eq(src.chars())
            })
            .copied()
            .ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `as_str` is what gets stored and `from_str` reads it back: a value saved by one side and silently
    /// reset to Normal by the other is the whole failure this test exists to prevent.
    #[test]
    fn every_priority_round_trips_through_its_wire_value() {
        for priority in Priority::ALL {
            assert_eq!(
                Priority::from_str(priority.as_str()),
                Ok(*priority),
                "{} does not round-trip",
                priority.title()
            );
        }
    }

    /// The order is the feature: this is the sequence a column is drawn in, and it must not depend on the
    /// order the variants happen to be declared in tomorrow.
    #[test]
    fn the_order_runs_from_urgent_to_quiet() {
        let mut sorted = Priority::ALL.to_vec();
        sorted.sort_by_key(|itm| itm.order());

        assert_eq!(
            sorted,
            vec![
                Priority::SuperHigh,
                Priority::High,
                Priority::Normal,
                Priority::Low,
                Priority::SuperLow,
            ]
        );
    }

    /// A row written before this field existed carries nothing, and everything a person did not think about
    /// is Normal — the two have to read the same or half a board would sort by an accident.
    #[test]
    fn an_unknown_or_missing_value_reads_as_normal() {
        assert_eq!(Priority::parse_or_default(""), Priority::Normal);
        assert_eq!(Priority::parse_or_default("urgent!!!"), Priority::Normal);
        assert_eq!(Priority::parse_or_default("p1"), Priority::Normal);
    }

    /// What an agent or a person actually types. Refusing `Super High` because the wire says `super-high`
    /// would make the field annoying enough to leave alone.
    #[test]
    fn the_spellings_a_human_writes_are_accepted() {
        for written in ["Super High", "super_high", "SUPERHIGH", "  super-high  "] {
            assert_eq!(
                Priority::parse_or_default(written),
                Priority::SuperHigh,
                "{written} should read as Super High"
            );
        }

        assert_eq!(Priority::parse_or_default("Low"), Priority::Low);
        assert_eq!(Priority::parse_or_default("super low"), Priority::SuperLow);
    }

    /// Only a deliberate choice is drawn. Normal on every card is noise that hides the four that matter.
    #[test]
    fn only_a_deliberate_priority_is_drawn() {
        assert!(!Priority::Normal.is_worth_showing());

        for priority in Priority::ALL.iter().filter(|itm| **itm != Priority::Normal) {
            assert!(priority.is_worth_showing(), "{} should show", priority.title());
        }
    }
}
