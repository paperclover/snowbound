//! Spelling in the browser, which gives pages no checker to ask: Hunspell's American English
//! dictionary through `spellbook`, which `index.html` fetches beside the module, with the
//! words Add to Dictionary learns kept in the browser's files.

use canvas::spelling::{Dictionary, pick};
use std::{io::Write, sync::Mutex};

/// The dictionary's files, where `web::start` puts them, and the words learned.
pub const AFFIX: &str = "/Dictionaries/en_US.aff";
pub const WORDS: &str = "/Dictionaries/en_US.dic";
const LEARNED: &str = "/Settings/dictionary.txt";

struct Checker(Mutex<spellbook::Dictionary>);

pub fn dictionary() -> Option<Box<dyn Dictionary>> {
    let affix = notebook::fs::read_to_string(AFFIX).ok()?;
    let words = notebook::fs::read_to_string(WORDS).ok()?;
    let mut dictionary = spellbook::Dictionary::new(&affix, &words).ok()?;
    for word in notebook::fs::read_to_string(LEARNED)
        .unwrap_or_default()
        .lines()
    {
        let _ = dictionary.add(word);
    }
    Some(Box::new(Checker(Mutex::new(dictionary))))
}

/// Whether the dictionary serves `language`, an LCID.
fn serves(language: u32) -> bool {
    pick(language, &["en_US".to_owned()]).is_some()
}

impl Dictionary for Checker {
    fn misspelled(&self, words: &[(&str, u32)]) -> Vec<bool> {
        let Ok(dictionary) = self.0.lock() else {
            return vec![false; words.len()];
        };
        words
            .iter()
            .map(|(word, language)| serves(*language) && !dictionary.check(word))
            .collect()
    }

    fn suggest(&self, word: &str, language: u32) -> Vec<String> {
        let mut suggestions = Vec::new();
        if serves(language)
            && let Ok(dictionary) = self.0.lock()
        {
            dictionary.suggest(word, &mut suggestions);
        }
        suggestions
    }

    fn learn(&self, word: &str) {
        if let Ok(mut dictionary) = self.0.lock() {
            let _ = dictionary.add(word);
        }
        let _ = notebook::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(LEARNED)
            .and_then(|mut file| writeln!(file, "{word}"));
    }
}
