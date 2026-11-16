//! The machines Roastro knows how to read ("Machine setup and connection
//! lifecycle", #34): a model is a register map built into the app, and an
//! address is typed by a person, never discovered on the network.

use regmap::Map;

/// The real machine's model, from Artisan's Caloratto 2.0 preset. Read only:
/// it has no writable points (see `CLAUDE.md`).
pub const CALORATTO: &str = "caloratto-2.0";
/// The built-in simulator's model, also the Demo machine's.
pub const SIMULATOR: &str = "simulator";

/// Every model, with the name a person picks it by.
pub const MODELS: [(&str, &str); 2] = [
    (CALORATTO, "Carmomaq Caloratto 2.0"),
    (SIMULATOR, "Roastro simulator"),
];

/// The register map of a model.
///
/// # Errors
///
/// Fails on an unknown model, or if a built-in map does not load.
pub fn model_map(model: &str) -> anyhow::Result<Map> {
    let json = match model {
        CALORATTO => include_str!("../../../configs/regmap.caloratto-2.0.json"),
        SIMULATOR => include_str!("../../../configs/regmap.json"),
        other => anyhow::bail!("unknown machine model {other:?}"),
    };
    Ok(Map::from_json(json)?)
}

/// A model's name, for people.
#[must_use]
pub fn model_title(model: &str) -> &str {
    MODELS
        .iter()
        .find(|(id, _)| *id == model)
        .map_or(model, |(_, title)| title)
}

/// The address as the PLC client wants it, from what a person typed:
/// `192.168.5.11`, `192.168.5.11:502` or `tcp://192.168.5.11:502`. Port 502
/// (Modbus TCP) when none is given. `None` when it cannot be an address.
#[must_use]
pub fn normalize_address(typed: &str) -> Option<String> {
    let rest = typed.trim();
    let rest = rest.strip_prefix("tcp://").unwrap_or(rest);
    if rest.is_empty() || rest.contains(char::is_whitespace) || rest.contains('/') {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        // An IPv6 address without a port has colons too.
        Some((host, port)) if !host.contains(':') || host.ends_with(']') => {
            (host.to_owned(), port.parse::<u16>().ok()?)
        }
        _ => (rest.to_owned(), 502),
    };
    if host.is_empty() || port == 0 {
        return None;
    }
    Some(format!("tcp://{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_addresses_as_people_type_them() {
        for (typed, want) in [
            ("192.168.5.11", Some("tcp://192.168.5.11:502")),
            (" 192.168.5.11:5502 ", Some("tcp://192.168.5.11:5502")),
            ("tcp://roaster.local:502", Some("tcp://roaster.local:502")),
            ("[::1]:5502", Some("tcp://[::1]:5502")),
            ("", None),
            ("192.168.5.11:port", None),
            ("192.168.5.11:0", None),
            ("http://192.168.5.11/", None),
            ("192.168 5.11", None),
        ] {
            assert_eq!(normalize_address(typed).as_deref(), want, "{typed:?}");
        }
    }

    #[test]
    fn every_model_has_a_map_and_the_real_one_writes_nothing() {
        for (id, title) in MODELS {
            let map = model_map(id).unwrap();
            assert_eq!(model_title(id), title);
            if id == CALORATTO {
                assert!(
                    map.points().iter().all(|p| !p.writable),
                    "the real machine's map stays read only"
                );
            }
        }
        assert!(model_map("other").is_err());
    }
}
