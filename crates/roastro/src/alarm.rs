//! The machine's alarms ("Machine alarms on screen", #55): points read like
//! any other, by name, 1 when active. Roastro only tells and records them:
//! the PLC and the flame controller shut the gas on their own.

/// An alarm the machine can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Alarm {
    /// The emergency stop button.
    Emergency,
    /// The flame went out; the gas valve closed.
    FlameFailure,
    /// A temperature passed the machine's safety limit; the burner is off.
    SafetyLimit,
}

impl Alarm {
    /// Every alarm.
    pub const ALL: [Self; 3] = [Self::Emergency, Self::FlameFailure, Self::SafetyLimit];

    /// The register-map point that raises it.
    #[must_use]
    pub fn point(self) -> &'static str {
        match self {
            Self::Emergency => "emergency",
            Self::FlameFailure => "flame_fault",
            Self::SafetyLimit => "safety_trip",
        }
    }

    /// Its name, as noted on the roast.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Emergency => "Emergency stop",
            Self::FlameFailure => "Flame failure",
            Self::SafetyLimit => "Safety limit",
        }
    }

    /// What happened and what to do, in plain words.
    #[must_use]
    pub fn advice(self) -> &'static str {
        match self {
            Self::Emergency => {
                "The machine is stopped. Release the emergency button on the machine when it is safe."
            }
            Self::FlameFailure => {
                "The burner is off. Check the flame before relighting on the machine's panel."
            }
            Self::SafetyLimit => {
                "A temperature passed the machine's limit and the burner is off. Let it cool before going on."
            }
        }
    }

    /// The alarms active in a reading's values.
    pub fn active<'a>(
        values: impl Fn(&str) -> Option<f64> + 'a,
    ) -> impl Iterator<Item = Self> + 'a {
        Self::ALL
            .into_iter()
            .filter(move |a| values(a.point()).is_some_and(|v| v >= 0.5))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn reads_alarms_by_point_name() {
        let values = HashMap::from([
            ("flame_fault".to_owned(), 1.0),
            ("emergency".to_owned(), 0.0),
        ]);
        let active: Vec<_> = Alarm::active(|p| values.get(p).copied()).collect();
        assert_eq!(active, [Alarm::FlameFailure]);
        let none = HashMap::<String, f64>::new();
        assert_eq!(
            Alarm::active(|p| none.get(p).copied()).count(),
            0,
            "a map without them"
        );
    }
}
