//! Lethora spell check: the bundled Hunspell dictionaries (English and
//! German, see `dictionaries/README.md`) checked with `spellbook`, plus one
//! personal word list.
//!
//! UI-free. Tokenizing is the caller's job: it passes single words (letters
//! with inner apostrophes; hyphenated words whole, the dictionaries' `BREAK`
//! rules split them). Each dictionary is parsed on first use, which takes a
//! while, so callers run that off the UI thread.

use std::collections::{BTreeSet, HashMap};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use spellbook::Dictionary;

/// The most suggestions `suggest` returns.
pub const MAX_SUGGESTIONS: usize = 6;

/// A bundled dictionary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    EnUs,
    EnGb,
    DeDe,
    DeAt,
    DeCh,
}

impl Lang {
    /// The dictionary's folder name (`en-US`).
    pub fn code(self) -> &'static str {
        match self {
            Lang::EnUs => "en-US",
            Lang::EnGb => "en-GB",
            Lang::DeDe => "de-DE",
            Lang::DeAt => "de-AT",
            Lang::DeCh => "de-CH",
        }
    }

    /// The `.aff` and `.dic` text.
    fn files(self) -> (&'static str, &'static str) {
        macro_rules! dict {
            ($dir:literal) => {
                (
                    include_str!(concat!("../dictionaries/", $dir, "/index.aff")),
                    include_str!(concat!("../dictionaries/", $dir, "/index.dic")),
                )
            };
        }
        match self {
            Lang::EnUs => dict!("en-US"),
            Lang::EnGb => dict!("en-GB"),
            Lang::DeDe => dict!("de-DE"),
            Lang::DeAt => dict!("de-AT"),
            Lang::DeCh => dict!("de-CH"),
        }
    }

    fn is_german(self) -> bool {
        matches!(self, Lang::DeDe | Lang::DeAt | Lang::DeCh)
    }

    /// Parses the dictionary (the slow part).
    pub fn load(self) -> Dictionary {
        let (aff, dic) = self.files();
        // Bundled and tested, so a parse error is a build mistake.
        Dictionary::new(aff, dic).expect("bundled dictionary parses")
    }
}

/// The dictionary for a Typst `text.lang` (`en`, `de-AT`; any case, `-` or
/// `_`): the region's when bundled, else the language's default. `None` for
/// languages without one (no checking).
pub fn dictionary_for(lang: &str) -> Option<Lang> {
    let lang = lang.trim().to_ascii_lowercase().replace('_', "-");
    let (base, region) = lang.split_once('-').unwrap_or((&lang, ""));
    match (base, region) {
        ("en", "gb" | "uk") => Some(Lang::EnGb),
        ("en", _) => Some(Lang::EnUs),
        ("de", "at") => Some(Lang::DeAt),
        ("de", "ch" | "li") => Some(Lang::DeCh),
        ("de", _) => Some(Lang::DeDe),
        _ => None,
    }
}

/// The dictionaries loaded so far and the personal word list. Shared by
/// threads: checks and suggestions read, loading and adding write.
#[derive(Default)]
pub struct Speller {
    state: RwLock<State>,
}

#[derive(Default)]
struct State {
    dicts: HashMap<Lang, Dictionary>,
    personal: BTreeSet<String>,
}

impl Speller {
    /// A speller with `words` as the personal list.
    pub fn with_words(words: impl IntoIterator<Item = String>) -> Self {
        let speller = Self::default();
        for word in words {
            speller.add_word(&word);
        }
        speller
    }

    /// The personal list, sorted.
    pub fn words(&self) -> Vec<String> {
        self.read().personal.iter().cloned().collect()
    }

    /// Adds `word` to the personal list and every dictionary (loaded now or
    /// later). False when it was already there, is empty or has spaces.
    pub fn add_word(&self, word: &str) -> bool {
        let word = normalize(word.trim());
        if word.is_empty() || word.contains(char::is_whitespace) {
            return false;
        }
        let mut state = self.write();
        if !state.personal.insert(word.clone()) {
            return false;
        }
        for dict in state.dicts.values_mut() {
            add(dict, &word);
        }
        true
    }

    /// The words of `words` that `lang`'s dictionary doesn't know, in order.
    /// Words in the personal list are always right.
    pub fn check(&self, lang: Lang, words: &[String]) -> Vec<String> {
        self.with(lang, |dict, personal| {
            words
                .iter()
                .filter(|w| {
                    let w = normalize(w);
                    !personal.contains(&w) && !known(lang, dict, &w)
                })
                .cloned()
                .collect()
        })
    }

    /// Up to `MAX_SUGGESTIONS` corrections for `word`, best first.
    pub fn suggest(&self, lang: Lang, word: &str) -> Vec<String> {
        self.with(lang, |dict, _| {
            let mut out = Vec::new();
            dict.suggest(&normalize(word), &mut out);
            out.truncate(MAX_SUGGESTIONS);
            out
        })
    }

    /// Runs `f` on `lang`'s dictionary, loading it first if needed (outside
    /// the lock, so other languages keep working meanwhile).
    fn with<R>(&self, lang: Lang, f: impl FnOnce(&Dictionary, &BTreeSet<String>) -> R) -> R {
        {
            let state = self.read();
            if let Some(dict) = state.dicts.get(&lang) {
                return f(dict, &state.personal);
            }
        }
        let mut dict = lang.load();
        let mut state = self.write();
        let State { dicts, personal } = &mut *state;
        let dict = dicts.entry(lang).or_insert_with(|| {
            for word in personal.iter() {
                add(&mut dict, word);
            }
            dict
        });
        f(dict, personal)
    }

    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// Typographic apostrophes as ASCII ones, which the dictionaries use.
fn normalize(word: &str) -> String {
    word.replace('’', "'")
}

/// Whether `dict` knows `word`. The German dictionaries have no words with
/// apostrophes, so there an elision ("geht's") counts when its parts do,
/// single letters aside.
fn known(lang: Lang, dict: &Dictionary, word: &str) -> bool {
    dict.check(word)
        || lang.is_german()
            && word.contains('\'')
            && word
                .split('\'')
                .all(|part| part.chars().count() < 2 || dict.check(part))
}

/// Adds a personal word to `dict`. `add` reads a `.dic` line, where `/`
/// starts the flags, so such words stay only in the personal list.
fn add(dict: &mut Dictionary, word: &str) {
    if !word.contains('/') {
        let _ = dict.add(word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn misspelled(speller: &Speller, lang: Lang, words: &[&str]) -> Vec<String> {
        let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        speller.check(lang, &words)
    }

    #[test]
    fn languages_map_to_bundled_dictionaries() {
        assert_eq!(dictionary_for("en"), Some(Lang::EnUs));
        assert_eq!(dictionary_for("en-US"), Some(Lang::EnUs));
        assert_eq!(dictionary_for("en-AU"), Some(Lang::EnUs));
        assert_eq!(dictionary_for("en-GB"), Some(Lang::EnGb));
        assert_eq!(dictionary_for("en_uk"), Some(Lang::EnGb));
        assert_eq!(dictionary_for("de"), Some(Lang::DeDe));
        assert_eq!(dictionary_for("DE-at"), Some(Lang::DeAt));
        assert_eq!(dictionary_for("de-CH"), Some(Lang::DeCh));
        assert_eq!(dictionary_for("de-LI"), Some(Lang::DeCh));
        assert_eq!(dictionary_for("de-BE"), Some(Lang::DeDe));
        assert_eq!(dictionary_for("fr"), None);
        assert_eq!(dictionary_for("eng"), None);
        assert_eq!(dictionary_for(""), None);
    }

    #[test]
    fn english_words_and_apostrophes() {
        let speller = Speller::default();
        let words = [
            "The",
            "the",
            "don't",
            "don’t",
            "children's",
            "well-known",
            "colour",
            "teh",
            "recieve",
        ];
        assert_eq!(
            misspelled(&speller, Lang::EnUs, &words),
            ["colour", "teh", "recieve"]
        );
        assert_eq!(misspelled(&speller, Lang::EnGb, &words), ["teh", "recieve"]);
    }

    #[test]
    fn german_compounds() {
        let speller = Speller::default();
        let words = [
            "Donaudampfschiff",
            "Haustür",
            "Fahrradschloss",
            "Straße",
            "geht's",
            "gibt’s",
            "E-Mail-Adresse",
            "Fahrrad-Schloss",
            "Hausstür",
            "Fahradschloss",
            "Hauss's",
        ];
        assert_eq!(
            misspelled(&speller, Lang::DeDe, &words),
            ["Hausstür", "Fahradschloss", "Hauss's"]
        );
        // Swiss German writes ss for ß (the de-CH list is LibreOffice's).
        assert_eq!(
            misspelled(
                &speller,
                Lang::DeCh,
                &["Strasse", "gross", "Grüsse", "Straße"]
            ),
            ["Straße"]
        );
        assert_eq!(misspelled(&speller, Lang::DeDe, &["Strasse"]), ["Strasse"]);
        let suggestions = speller.suggest(Lang::DeCh, "Strase");
        assert!(
            suggestions.iter().any(|s| s == "Strasse"),
            "{suggestions:?}"
        );
    }

    #[test]
    fn suggestions() {
        let speller = Speller::default();
        let suggestions = speller.suggest(Lang::EnUs, "recieve");
        assert!(
            suggestions.iter().any(|s| s == "receive"),
            "{suggestions:?}"
        );
        assert!(suggestions.len() <= MAX_SUGGESTIONS);
    }

    #[test]
    fn personal_words_count_in_loaded_and_later_dictionaries() {
        let speller = Speller::with_words(["Lethora".to_owned()]);
        assert!(misspelled(&speller, Lang::EnUs, &["Lethora", "Quorbel"]) == ["Quorbel"]);
        assert!(speller.add_word("Quorbel"));
        assert!(!speller.add_word("Quorbel"));
        assert!(!speller.add_word("  "));
        assert!(!speller.add_word("two words"));
        assert!(misspelled(&speller, Lang::EnUs, &["Quorbel"]).is_empty());
        assert!(misspelled(&speller, Lang::DeDe, &["Lethora", "Quorbel"]).is_empty());
        assert_eq!(speller.words(), ["Lethora", "Quorbel"]);
    }

    /// Load times; run with
    /// `cargo test --release -p lethora-spell -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn load_times() {
        for lang in [Lang::EnUs, Lang::EnGb, Lang::DeDe, Lang::DeAt, Lang::DeCh] {
            let start = std::time::Instant::now();
            let dict = lang.load();
            let loaded = start.elapsed();
            let start = std::time::Instant::now();
            let mut out = Vec::new();
            dict.suggest("Fahradschlos", &mut out);
            println!(
                "{}: load {loaded:?}, one suggestion {:?}",
                lang.code(),
                start.elapsed()
            );
        }
    }
}
