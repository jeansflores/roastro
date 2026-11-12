use std::fmt;
use std::str::FromStr;

/// A named moment of a roast (see `GLOSSARY.md`). Each kind happens at most
/// once per roast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EventKind {
    /// Green coffee enters the hot drum; the zero of the roast clock.
    Charge,
    /// The lowest bean temperature after charge.
    TurningPoint,
    /// The beans change color; drying ends, Maillard begins.
    DryEnd,
    /// The first audible pops.
    FirstCrack,
    /// The start of the second cracking.
    SecondCrack,
    /// Roasted coffee leaves the drum.
    Drop,
}

impl EventKind {
    /// Every kind, in the order they happen in a roast.
    pub const ALL: [EventKind; 6] = [
        EventKind::Charge,
        EventKind::TurningPoint,
        EventKind::DryEnd,
        EventKind::FirstCrack,
        EventKind::SecondCrack,
        EventKind::Drop,
    ];

    /// The stable name used in storage and on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Charge => "charge",
            EventKind::TurningPoint => "turning_point",
            EventKind::DryEnd => "dry_end",
            EventKind::FirstCrack => "first_crack",
            EventKind::SecondCrack => "second_crack",
            EventKind::Drop => "drop",
        }
    }

    /// Whether only a person marks it (never detected or computed).
    #[must_use]
    pub fn is_manual_only(self) -> bool {
        matches!(
            self,
            EventKind::DryEnd | EventKind::FirstCrack | EventKind::SecondCrack
        )
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A name that is not an event kind.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {what} {name:?}")]
pub struct UnknownName {
    what: &'static str,
    name: String,
}

impl FromStr for EventKind {
    type Err = UnknownName;

    fn from_str(s: &str) -> Result<Self, UnknownName> {
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| UnknownName {
                what: "event kind",
                name: s.to_owned(),
            })
    }
}

/// Where an event's time came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// Detected by Roastro (a BT rule or the PLC timer).
    Auto,
    /// Marked or corrected by a person.
    Manual,
    /// Computed from the readings (the turning point).
    Computed,
}

impl Source {
    /// The stable name used in storage and on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Auto => "auto",
            Source::Manual => "manual",
            Source::Computed => "computed",
        }
    }
}

impl FromStr for Source {
    type Err = UnknownName;

    fn from_str(s: &str) -> Result<Self, UnknownName> {
        [Source::Auto, Source::Manual, Source::Computed]
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| UnknownName {
                what: "event source",
                name: s.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for k in EventKind::ALL {
            assert_eq!(k.as_str().parse::<EventKind>(), Ok(k));
        }
        for s in [Source::Auto, Source::Manual, Source::Computed] {
            assert_eq!(s.as_str().parse::<Source>(), Ok(s));
        }
        assert!("crack".parse::<EventKind>().is_err());
    }

    #[test]
    fn kinds_are_in_roast_order() {
        assert!(EventKind::ALL.windows(2).all(|w| w[0] < w[1]));
        assert!(EventKind::DryEnd.is_manual_only() && !EventKind::Drop.is_manual_only());
    }
}
