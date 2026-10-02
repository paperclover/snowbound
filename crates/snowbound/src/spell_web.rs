//! Spelling in the browser, which gives pages no checker to ask: Hunspell dictionaries through
//! `spellbook`, each fetched beside the module the first time a word in its language is
//! checked, with the words Add to Dictionary learns kept in the browser's files.

use canvas::spelling::{Dictionary, pick};
use std::{collections::BTreeMap, io::Write, sync::Mutex};

const LEARNED: &str = "/Settings/dictionary.txt";

/// The dictionaries the site has, as `en_US`, which `platform::start` lists.
static AVAILABLE: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The dictionaries asked for, by name: `None` until one arrives, or where it failed to.
static LOADED: Mutex<BTreeMap<String, Option<spellbook::Dictionary>>> = Mutex::new(BTreeMap::new());

struct Checker;

/// The checker, where the site has dictionaries.
pub fn dictionary() -> Option<Box<dyn Dictionary>> {
    let available = AVAILABLE.lock().ok()?;
    (!available.is_empty()).then(|| Box::new(Checker) as Box<dyn Dictionary>)
}

/// Lists the dictionaries the site has.
pub fn offer(names: Vec<String>) {
    if let Ok(mut available) = AVAILABLE.lock() {
        *available = names;
    }
}

/// Dictionary `name` arrived as its affix and word files.
pub fn arrived(name: &str, affix: &str, words: &str) {
    let dictionary = spellbook::Dictionary::new(affix, words)
        .ok()
        .map(|mut dictionary| {
            for word in notebook::fs::read_to_string(LEARNED)
                .unwrap_or_default()
                .lines()
            {
                let _ = dictionary.add(word);
            }
            dictionary
        });
    if let Ok(mut loaded) = LOADED.lock() {
        loaded.insert(name.to_owned(), dictionary);
    }
}

/// Runs `check` with the dictionary serving `language`, an LCID, asking for it the first
/// time; `None` until it arrives, or where none serves the language.
fn with<T>(language: u32, check: impl FnOnce(&mut spellbook::Dictionary) -> T) -> Option<T> {
    let name = pick(language, &AVAILABLE.lock().ok()?)?.to_owned();
    let mut loaded = LOADED.lock().ok()?;
    match loaded.get_mut(&name) {
        Some(dictionary) => dictionary.as_mut().map(check),
        None => {
            loaded.insert(name.clone(), None);
            crate::platform::fetch_dictionary(&name);
            None
        }
    }
}

impl Dictionary for Checker {
    fn misspelled(&self, words: &[(&str, u32)]) -> Vec<bool> {
        words
            .iter()
            .map(|(word, language)| {
                with(*language, |dictionary| !dictionary.check(word)).unwrap_or(false)
            })
            .collect()
    }

    fn suggest(&self, word: &str, language: u32) -> Vec<String> {
        with(language, |dictionary| {
            let mut suggestions = Vec::new();
            dictionary.suggest(word, &mut suggestions);
            suggestions
        })
        .unwrap_or_default()
    }

    fn learn(&self, word: &str) {
        if let Ok(mut loaded) = LOADED.lock() {
            for dictionary in loaded.values_mut().flatten() {
                let _ = dictionary.add(word);
            }
        }
        let _ = notebook::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(LEARNED)
            .and_then(|mut file| writeln!(file, "{word}"));
    }
}
