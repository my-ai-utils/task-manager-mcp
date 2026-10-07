use rust_extensions::AsStr;
use std::str::FromStr;

/// The fixed palette a kind's colour is picked from.
///
/// Kinds themselves are configured per project, but their colours are not: a free-form colour
/// field would let one project's board look like nothing else in the product, and a hex input is a
/// worse control than eight swatches. The server validates against this list; the UI renders it as
/// swatches rather than a dropdown, because a colour picker that shows the colours beats one that
/// shows their names.
///
/// On the wire this is a `String` (see the crate docs) — [`KindColor::parse_or_default`] is how a
/// reader turns it back, and an unknown value falls back to [`KindColor::Gray`] rather than
/// failing a whole board read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindColor {
    #[default]
    Gray,
    Red,
    Orange,
    Amber,
    Green,
    Teal,
    Blue,
    Purple,
}

impl KindColor {
    pub const ALL: &'static [Self] = &[
        Self::Gray,
        Self::Red,
        Self::Orange,
        Self::Amber,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
    ];

    /// The colour as CSS, so a sticker is styled identically wherever it is drawn and does not
    /// depend on which Bootstrap version happens to be loaded.
    pub fn hex(&self) -> &'static str {
        match self {
            Self::Gray => "#6c757d",
            Self::Red => "#dc3545",
            Self::Orange => "#fd7e14",
            Self::Amber => "#ffc107",
            Self::Green => "#198754",
            Self::Teal => "#20c997",
            Self::Blue => "#0d6efd",
            Self::Purple => "#6f42c1",
        }
    }

    /// How the colour is named to a human, in Projects setup.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Gray => "Gray",
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Amber => "Amber",
            Self::Green => "Green",
            Self::Teal => "Teal",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
        }
    }

    /// Read a wire value, falling back to the default swatch.
    ///
    /// Deliberately infallible: the colour is decoration, and a value this build does not
    /// recognise must not be able to fail the read that carried it.
    pub fn parse_or_default(src: &str) -> Self {
        Self::from_str(src).unwrap_or_default()
    }
}

impl AsStr for KindColor {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Gray => "gray",
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Amber => "amber",
            Self::Green => "green",
            Self::Teal => "teal",
            Self::Blue => "blue",
            Self::Purple => "purple",
        }
    }
}

impl FromStr for KindColor {
    type Err = ();

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        let src = src.trim().to_lowercase();
        Self::ALL
            .iter()
            .find(|itm| itm.as_str() == src)
            .copied()
            .ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `as_str` is the wire value and `from_str` reads it back: the two must round-trip for every
    /// variant, or a colour saved by one side is silently reset to Gray by the other.
    #[test]
    fn every_colour_round_trips_through_its_wire_value() {
        for colour in KindColor::ALL {
            assert_eq!(
                KindColor::from_str(colour.as_str()),
                Ok(*colour),
                "{} does not round-trip",
                colour.title()
            );
        }
    }

    #[test]
    fn an_unknown_wire_value_reads_as_the_default_swatch() {
        assert_eq!(KindColor::parse_or_default("chartreuse"), KindColor::Gray);
        assert_eq!(KindColor::parse_or_default(""), KindColor::Gray);
    }

    /// Case and stray whitespace come from hand-written settings and hand-typed tool calls, not
    /// from a bug — they should read, not fall back.
    #[test]
    fn parsing_is_case_and_whitespace_insensitive() {
        assert_eq!(KindColor::parse_or_default("  RED "), KindColor::Red);
    }
}
