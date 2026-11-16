//! Roastro in Portuguese (Brazil) and English ("Portuguese and English",
//! #71). The English text is the key: code reads `t("Roast time")`, and
//! `locales/pt-BR.toml` maps it to "Tempo de torra". A missing translation
//! shows the English, and a test keeps every key in the code translated.
//!
//! One language for the whole process (the UI and the engine run in the
//! same one), so what the engine writes (alarm notes) follows the screen.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// A language Roastro speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// English, the code's own.
    En,
    /// Portuguese (Brazil).
    PtBr,
}

impl Lang {
    /// Both, in the order the switch shows them.
    pub const ALL: [Self; 2] = [Self::PtBr, Self::En];

    /// The BCP 47 tag, as stored.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::PtBr => "pt-BR",
        }
    }

    /// The short label on the switch.
    #[must_use]
    pub fn short(self) -> &'static str {
        match self {
            Self::En => "EN",
            Self::PtBr => "PT",
        }
    }

    /// The language of a tag: any Portuguese is Portuguese, anything else
    /// English.
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        if tag.to_ascii_lowercase().starts_with("pt") {
            Self::PtBr
        } else {
            Self::En
        }
    }

    /// The operating system's language, as Roastro speaks it.
    #[must_use]
    pub fn system() -> Self {
        sys_locale::get_locale().map_or(Self::En, |tag| Self::from_tag(&tag))
    }
}

#[cfg(not(feature = "thread-local"))]
static CURRENT: AtomicU8 = AtomicU8::new(0);

#[cfg(feature = "thread-local")]
thread_local! {
    static CURRENT: AtomicU8 = const { AtomicU8::new(0) };
}

fn load() -> u8 {
    #[cfg(not(feature = "thread-local"))]
    return CURRENT.load(Ordering::Relaxed);
    #[cfg(feature = "thread-local")]
    return CURRENT.with(|c| c.load(Ordering::Relaxed));
}

fn store(value: u8) {
    #[cfg(not(feature = "thread-local"))]
    CURRENT.store(value, Ordering::Relaxed);
    #[cfg(feature = "thread-local")]
    CURRENT.with(|c| c.store(value, Ordering::Relaxed));
}

/// The language in use.
#[must_use]
pub fn current() -> Lang {
    match load() {
        1 => Lang::PtBr,
        _ => Lang::En,
    }
}

/// Switches the language for the whole process (for the calling thread
/// only with the `thread-local` feature, in tests).
pub fn set(lang: Lang) {
    store(match lang {
        Lang::En => 0,
        Lang::PtBr => 1,
    });
}

const PT_BR: &str = include_str!("../locales/pt-BR.toml");

fn portuguese() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| parse(PT_BR))
}

/// The translations in a locale file: `"English" = "Tradução"` pairs.
fn parse(text: &str) -> HashMap<String, String> {
    match toml::from_str::<toml::Table>(text) {
        Ok(table) => table
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_owned())))
            .collect(),
        Err(err) => {
            // A broken locale must not take the app down: English stays.
            eprintln!("roastro: pt-BR translations unreadable: {err}");
            HashMap::new()
        }
    }
}

/// The text in the current language.
#[must_use]
pub fn t(english: &'static str) -> &'static str {
    match current() {
        Lang::En => english,
        Lang::PtBr => portuguese().get(english).map_or(english, String::as_str),
    }
}

/// The text in the current language with its `{name}` holes filled.
#[must_use]
pub fn tf(english: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let mut text = t(english).to_owned();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Every `t("…")` and `tf("…"` key in the workspace's code.
    fn keys_in(dir: &Path, keys: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                keys_in(&path, keys);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for call in ["t(", "tf("] {
                    for (i, _) in text.match_indices(call) {
                        let before = text[..i].chars().last();
                        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            continue; // part of another name, e.g. `print(`
                        }
                        // rustfmt may put a long text on the next line.
                        let Some(rest) = text[i + call.len()..].trim_start().strip_prefix('"')
                        else {
                            continue;
                        };
                        let mut key = String::new();
                        let mut chars = rest.chars();
                        while let Some(c) = chars.next() {
                            match c {
                                '\\' => key.push(chars.next().unwrap_or('\\')),
                                '"' => break,
                                c => key.push(c),
                            }
                        }
                        keys.push(key);
                    }
                }
            }
        }
    }

    #[test]
    fn every_key_in_the_code_has_portuguese() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut keys = Vec::new();
        for name in ["desktop", "roastro"] {
            keys_in(&crates.join(name).join("src"), &mut keys);
        }
        keys.sort();
        keys.dedup();
        assert!(keys.len() > 50, "the scan found the keys: {}", keys.len());
        let missing: Vec<_> = keys
            .iter()
            .filter(|k| !portuguese().contains_key(*k))
            .collect();
        assert!(
            missing.is_empty(),
            "translate these in pt-BR.toml: {missing:#?}"
        );
        let unused: Vec<_> = portuguese().keys().filter(|k| !keys.contains(k)).collect();
        assert!(unused.is_empty(), "no longer in the code: {unused:#?}");
    }

    #[test]
    fn switches_and_fills_holes() {
        set(Lang::PtBr);
        assert_eq!(t("Roast time"), "Tempo de torra");
        assert_eq!(tf("Roast #{n}", &[("n", &41)]), "Torra #41");
        assert_eq!(
            t("Something nobody translated"),
            "Something nobody translated"
        );
        set(Lang::En);
        assert_eq!(tf("Roast #{n}", &[("n", &41)]), "Roast #41");
        assert_eq!(Lang::from_tag("pt_BR"), Lang::PtBr);
        assert_eq!(Lang::from_tag("en-US"), Lang::En);
        assert_eq!(Lang::from_tag("es-ES"), Lang::En);
    }
}
