//! Where each quantity lives in the PLC (data table, address and scale). It is
//! the only thing that changes once the real Carmomaq `.aset` register map
//! arrives. Code looks points up by name, never by address.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The Modbus data table a point lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Holding register: 16 bits, read/write.
    Holding,
    /// Input register: 16 bits, read only.
    Input,
    /// Coil: one bit, read/write.
    Coil,
    /// Discrete input: one bit, read only.
    Discrete,
}

impl Kind {
    /// Reports whether points of this kind are a single bit.
    #[must_use]
    pub fn is_bool(self) -> bool {
        matches!(self, Kind::Coil | Kind::Discrete)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Holding => "holding",
            Kind::Input => "input",
            Kind::Coil => "coil",
            Kind::Discrete => "discrete",
        })
    }
}

/// One named quantity in the PLC and how to encode it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Point {
    /// Fixed name the simulator and the app agree on (e.g. `bt`).
    pub name: String,
    /// Data table.
    pub kind: Kind,
    /// Address within the data table.
    pub addr: u16,
    /// Raw value = physical value × scale; 0 means 1.
    #[serde(default)]
    pub scale: f64,
    /// Whether the raw register is a two's complement `i16`.
    #[serde(default)]
    pub signed: bool,
    /// Whether the app may write it (still subject to the write guard).
    #[serde(default)]
    pub writable: bool,
    /// Physical unit, for display.
    #[serde(default)]
    pub unit: String,
    /// Human description.
    #[serde(default)]
    pub desc: String,
}

impl Point {
    fn scale(&self) -> f64 {
        if self.scale == 0.0 { 1.0 } else { self.scale }
    }

    /// Converts a physical value into a raw register value, clamping it to
    /// the register's range.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // float-to-int `as` saturates
    pub fn encode(&self, value: f64) -> u16 {
        let raw = (value * self.scale()).round();
        if self.signed {
            (raw as i16).cast_unsigned()
        } else {
            raw as u16
        }
    }

    /// Converts a raw register value into a physical value.
    #[must_use]
    pub fn decode(&self, raw: u16) -> f64 {
        let raw = if self.signed {
            f64::from(raw.cast_signed())
        } else {
            f64::from(raw)
        };
        raw / self.scale()
    }
}

/// Why a register map could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file could not be read.
    #[error("regmap {path}: {source}")]
    Read {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The JSON is malformed or does not match the schema.
    #[error("regmap: {0}")]
    Parse(#[from] serde_json::Error),
    /// Two points share a name.
    #[error("regmap: duplicate name {0:?}")]
    DuplicateName(String),
    /// Two points share a data table and address.
    #[error("regmap: {first} and {second} share address {kind}/{addr}")]
    DuplicateAddress {
        /// Point listed first.
        first: String,
        /// Point listed second.
        second: String,
        /// Shared data table.
        kind: Kind,
        /// Shared address.
        addr: u16,
    },
}

#[derive(Deserialize)]
struct RawMap {
    #[serde(default)]
    unit_id: u8,
    points: Vec<Point>,
}

/// A validated register map, indexed by name and by address.
#[derive(Debug, Clone)]
pub struct Map {
    unit_id: u8,
    points: Vec<Point>,
    by_name: HashMap<String, usize>,
    by_addr: HashMap<(Kind, u16), usize>,
}

impl Map {
    /// Reads and validates a register map from a JSON file.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read or [`Map::from_json`] rejects it.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let json = std::fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::from_json(&json)
    }

    /// Parses and validates a register map.
    ///
    /// # Errors
    ///
    /// Fails on malformed JSON, a duplicate name or a duplicate address.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        let raw: RawMap = serde_json::from_str(json)?;
        let mut by_name = HashMap::new();
        let mut by_addr = HashMap::new();
        for (i, p) in raw.points.iter().enumerate() {
            if by_name.insert(p.name.clone(), i).is_some() {
                return Err(Error::DuplicateName(p.name.clone()));
            }
            if let Some(other) = by_addr.insert((p.kind, p.addr), i) {
                return Err(Error::DuplicateAddress {
                    first: raw.points[other].name.clone(),
                    second: p.name.clone(),
                    kind: p.kind,
                    addr: p.addr,
                });
            }
        }
        Ok(Self {
            unit_id: raw.unit_id,
            points: raw.points,
            by_name,
            by_addr,
        })
    }

    /// Modbus unit (slave) id of the PLC.
    #[must_use]
    pub fn unit_id(&self) -> u8 {
        self.unit_id
    }

    /// All points, in file order.
    #[must_use]
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Looks a point up by its name.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&Point> {
        self.by_name.get(name).map(|&i| &self.points[i])
    }

    /// Looks a point up by its data table and address.
    #[must_use]
    pub fn at(&self, kind: Kind, addr: u16) -> Option<&Point> {
        self.by_addr.get(&(kind, addr)).map(|&i| &self.points[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(scale: f64, signed: bool) -> Point {
        Point {
            name: "p".into(),
            kind: Kind::Holding,
            addr: 0,
            scale,
            signed,
            writable: false,
            unit: String::new(),
            desc: String::new(),
        }
    }

    #[test]
    fn encodes_physical_values() {
        let cases = [
            ("unscaled", point(1.0, false), 60.0, 60),
            ("zero scale means 1", point(0.0, false), 42.0, 42),
            ("tenths", point(10.0, false), 198.7, 1987),
            ("signed negative", point(10.0, true), -12.5, 0xFF83), // -125
            ("unsigned clamps at zero", point(1.0, false), -5.0, 0),
            ("unsigned clamps at max", point(10.0, false), 7000.0, 65535),
            ("signed clamps at max", point(10.0, true), 4000.0, 32767),
        ];
        for (name, p, value, raw) in cases {
            assert_eq!(p.encode(value), raw, "{name}: encode({value})");
        }
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact round trip is the point of the test
    fn round_trips_signed_tenths() {
        let p = point(10.0, true);
        for v in [-40.0, 0.0, 25.3, 198.7, 280.0] {
            assert_eq!(p.decode(p.encode(v)), v);
        }
    }

    #[test]
    fn validates_on_load() {
        let valid = r#"{"unit_id":1,"points":[{"name":"bt","kind":"holding","addr":0},{"name":"burner","kind":"coil","addr":0}]}"#;
        assert!(Map::from_json(valid).is_ok());

        let dup_name = r#"{"points":[{"name":"bt","kind":"holding","addr":0},{"name":"bt","kind":"holding","addr":1}]}"#;
        assert!(matches!(Map::from_json(dup_name), Err(Error::DuplicateName(n)) if n == "bt"));

        let dup_addr = r#"{"points":[{"name":"bt","kind":"holding","addr":0},{"name":"et","kind":"holding","addr":0}]}"#;
        assert!(matches!(
            Map::from_json(dup_addr),
            Err(Error::DuplicateAddress { .. })
        ));

        assert!(matches!(Map::from_json("{"), Err(Error::Parse(_))));
        assert!(matches!(
            Map::load("/nonexistent/regmap.json"),
            Err(Error::Read { .. })
        ));
    }

    /// Keeps the committed register map loadable.
    #[test]
    fn loads_the_repository_map() {
        let m = Map::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../configs/regmap.json"
        ))
        .expect("load");
        assert!(m.by_name("bt").is_some(), r#"no "bt" point"#);
        assert_eq!(m.at(Kind::Coil, 0).map(|p| p.name.as_str()), Some("burner"));
    }
}
