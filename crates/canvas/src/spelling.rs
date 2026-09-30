//! Spelling as OneNote 2010 checks it as you type. Its Proofing defaults decide what is a
//! word: words in UPPERCASE, words holding numbers, and Internet and file addresses are
//! left alone, and a word repeating the one before it is marked as repeated. A platform
//! dictionary checks each paragraph's words on a thread of its own, and results are kept
//! per paragraph text. Nothing about spelling is stored in the page.

use onestore::page::text::Paragraph;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex, mpsc};

/// Checked paragraphs kept before the cache starts over.
const KEPT: usize = 8192;
/// Paragraphs waiting to be checked that one call to the dictionary takes.
const BATCH: usize = 64;

/// A platform spell checker, called from the spelling thread and the host's. Languages are
/// Windows LCIDs, as runs store them.
pub trait Dictionary: Send + Sync {
    /// Whether each word is misspelled in its language; a word in a language no dictionary
    /// serves is not.
    fn misspelled(&self, words: &[(&str, u32)]) -> Vec<bool>;
    /// Corrections for `word`, best first.
    fn suggest(&self, word: &str, language: u32) -> Vec<String>;
    /// Add to Dictionary: accepts `word` from now on.
    fn learn(&self, word: &str);
}

/// Which of a platform's dictionaries, named `en_GB`, `en-GB` or `en`, serves `language`, an
/// LCID: its locale's, else its language's.
pub fn pick(language: u32, available: &[String]) -> Option<&str> {
    let tag = crate::language::tag(language)?.replace('-', "_");
    let (bare, region) = tag.split_once('_').unwrap_or((&tag, ""));
    // A bare tag is the locale Windows picks, most often the language's own country.
    let home = match bare {
        "en" => "US".to_owned(),
        _ => bare.to_uppercase(),
    };
    let wanted = if region.is_empty() {
        vec![format!("{bare}_{home}"), bare.to_owned()]
    } else {
        vec![tag.clone(), bare.to_owned()]
    };
    let name = |dictionary: &String| dictionary.replace('-', "_");
    wanted
        .iter()
        .find_map(|wanted| {
            available
                .iter()
                .find(|dictionary| name(dictionary) == *wanted)
        })
        .or_else(|| {
            available
                .iter()
                .find(|dictionary| name(dictionary).starts_with(&format!("{bare}_")))
        })
        .map(String::as_str)
}

/// A marked word: misspelled, or repeating the word before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Bytes of the paragraph's text.
    pub range: Range<usize>,
    pub repeated: bool,
}

/// A word of a paragraph: its bytes, its language, and whether its spelling is checked.
struct Word {
    range: Range<usize>,
    language: u32,
    checked: bool,
}

/// Letters, digits, underscores and combining marks, as against spaces, punctuation and
/// symbols.
fn word_character(character: char) -> bool {
    !character.is_whitespace()
        && !character.is_ascii_punctuation()
        && !matches!(u32::from(character),
            0x80..=0xbf | 0xd7 | 0xf7 | 0x2000..=0x2bff | 0x3000..=0x303f | 0xfe30..=0xfe4f
            | 0xff00..=0xff0f | 0x1f000..=0x1faff)
        || character == '_'
}

fn apostrophe(character: char) -> bool {
    matches!(character, '\'' | '\u{2019}' | '\u{02bc}')
}

/// An Internet or file address, which OneNote leaves unchecked whole.
fn address(chunk: &str) -> bool {
    let lower = chunk.to_lowercase();
    lower.contains("://")
        || lower.starts_with("www.")
        || chunk.contains('\\')
        || chunk
            .split_once('@')
            .is_some_and(|(user, host)| !user.is_empty() && host.contains('.'))
}

/// The words of `paragraph`'s shown text, in order. Hidden field codes and equations are
/// not text to check, and a word split by one is two.
fn words(paragraph: &Paragraph) -> Vec<Word> {
    let text = paragraph.text();
    let spans = paragraph.spans();
    let format = |byte: usize| &spans[spans.partition_point(|span| span.end <= byte)].format;
    // Runs of shown text between spaces, which formatting changes don't split.
    let mut chunks = Vec::new();
    let mut start = None;
    for (byte, character) in text.char_indices() {
        let format = format(byte);
        let apart =
            character.is_whitespace() || format.hidden == Some(true) || format.math == Some(true);
        match (apart, start) {
            (true, Some(from)) => {
                chunks.push(from..byte);
                start = None;
            }
            (false, None) => start = Some(byte),
            _ => {}
        }
    }
    chunks.extend(start.map(|from| from..text.len()));
    let mut words = Vec::new();
    for chunk in chunks {
        let at = chunk.start;
        let chunk = &text[chunk];
        if address(chunk) {
            continue;
        }
        let mut characters = chunk.char_indices().peekable();
        while let Some((offset, character)) = characters.next() {
            if !word_character(character) {
                continue;
            }
            let mut end = offset + character.len_utf8();
            while let Some(&(next, character)) = characters.peek() {
                let joined = apostrophe(character)
                    && chunk[next + character.len_utf8()..]
                        .chars()
                        .next()
                        .is_some_and(char::is_alphabetic);
                if !word_character(character) && !joined {
                    break;
                }
                end = next + character.len_utf8();
                characters.next();
            }
            let word = &chunk[offset..end];
            if word
                .chars()
                .any(|character| character.is_numeric() || crate::search::ideographic(character))
            {
                continue;
            }
            words.push(Word {
                range: at + offset..at + end,
                language: format(at + offset)
                    .language
                    .unwrap_or(crate::language::EN_US),
                checked: word.chars().any(char::is_lowercase),
            });
        }
    }
    words
}

/// Whether two words are the same word, as a repeated one is.
fn same(a: &str, b: &str) -> bool {
    let fold = |word: &str| {
        word.chars()
            .flat_map(char::to_lowercase)
            .map(|character| {
                if apostrophe(character) {
                    '\''
                } else {
                    character
                }
            })
            .collect::<String>()
    };
    fold(a) == fold(b)
}

/// Which of `words` repeat the word before them, with only spaces between.
fn repeated(text: &str, words: &[Word]) -> Vec<bool> {
    words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            index.checked_sub(1).is_some_and(|previous| {
                let previous = &words[previous].range;
                text[previous.end..word.range.start]
                    .chars()
                    .all(char::is_whitespace)
                    && same(&text[previous.clone()], &text[word.range.clone()])
            })
        })
        .collect()
}

/// The marks `dictionary` gives each of `paragraphs`, asking it once: each word repeating
/// the one before it, and each other checked word it finds misspelled.
fn check(paragraphs: &[&Paragraph], dictionary: &dyn Dictionary) -> Vec<Vec<Mark>> {
    let words: Vec<(Vec<Word>, Vec<bool>)> = paragraphs
        .iter()
        .map(|paragraph| {
            let words = words(paragraph);
            let repeated = repeated(paragraph.text(), &words);
            (words, repeated)
        })
        .collect();
    let asked: Vec<(&str, u32)> = paragraphs
        .iter()
        .zip(&words)
        .flat_map(|(paragraph, (words, repeated))| {
            words
                .iter()
                .zip(repeated)
                .filter(|(word, repeated)| word.checked && !**repeated)
                .map(|(word, _)| (&paragraph.text()[word.range.clone()], word.language))
        })
        .collect();
    let mut misspelled = dictionary.misspelled(&asked).into_iter();
    words
        .into_iter()
        .map(|(words, repeated)| {
            words
                .into_iter()
                .zip(repeated)
                .filter_map(|(word, repeated)| {
                    let marked = repeated || word.checked && misspelled.next().unwrap_or(false);
                    marked.then_some(Mark {
                        range: word.range,
                        repeated,
                    })
                })
                .collect()
        })
        .collect()
}

/// What a paragraph's marks are kept under: its text and the formatting that decides them.
fn key(paragraph: &Paragraph) -> u64 {
    let mut hasher = DefaultHasher::new();
    paragraph.text().hash(&mut hasher);
    for span in paragraph.spans() {
        (
            span.end,
            span.format.language,
            span.format.hidden,
            span.format.math,
        )
            .hash(&mut hasher);
    }
    hasher.finish()
}

struct Checked {
    text: String,
    marks: Vec<Mark>,
}

#[derive(Default)]
struct State {
    checked: HashMap<u64, Checked>,
    pending: HashSet<u64>,
    /// Words Ignore or Add to Dictionary accepted, misspelled or repeated, left unmarked
    /// everywhere.
    accepted: HashSet<String>,
}

impl State {
    /// The marks kept for `paragraph` under `key`, less accepted words.
    fn marks(&self, key: u64, paragraph: &Paragraph) -> Option<Vec<Mark>> {
        let checked = self
            .checked
            .get(&key)
            .filter(|checked| checked.text == paragraph.text())?;
        Some(
            checked
                .marks
                .iter()
                .filter(|mark| {
                    !self
                        .accepted
                        .contains(&paragraph.text()[mark.range.clone()])
                })
                .cloned()
                .collect(),
        )
    }

    fn keep(&mut self, key: u64, paragraph: &Paragraph, marks: Vec<Mark>) {
        if self.checked.len() >= KEPT {
            self.checked.clear();
        }
        self.checked.insert(
            key,
            Checked {
                text: paragraph.text().to_owned(),
                marks,
            },
        );
    }
}

struct Shared {
    dictionary: Box<dyn Dictionary>,
    state: Mutex<State>,
}

/// Spell checking shared by the pages of one app. Paragraphs are checked when first asked
/// for, on a thread of its own, which wakes `redraw` when marks are ready.
#[derive(Clone)]
pub struct Spelling {
    shared: Arc<Shared>,
    jobs: mpsc::Sender<(u64, Paragraph)>,
}

impl Spelling {
    pub fn new(dictionary: Box<dyn Dictionary>, redraw: std::task::Waker) -> Self {
        let shared = Arc::new(Shared {
            dictionary,
            state: Mutex::default(),
        });
        let (jobs, receiver) = mpsc::channel::<(u64, Paragraph)>();
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("spelling".into())
            .spawn(move || {
                while let Ok(first) = receiver.recv() {
                    let jobs: Vec<(u64, Paragraph)> = std::iter::once(first)
                        .chain(receiver.try_iter().take(BATCH - 1))
                        .collect();
                    let paragraphs: Vec<&Paragraph> =
                        jobs.iter().map(|(_, paragraph)| paragraph).collect();
                    let marks = check(&paragraphs, &*worker.dictionary);
                    let mut state = worker.state.lock().unwrap();
                    for ((key, paragraph), marks) in jobs.iter().zip(marks) {
                        state.pending.remove(key);
                        state.keep(*key, paragraph, marks);
                    }
                    drop(state);
                    redraw.wake_by_ref();
                }
            })
            .expect("the spelling thread starts");
        Self { shared, jobs }
    }

    /// The marks on `paragraph` once it is checked, less accepted words; until then none,
    /// and it is checked.
    pub fn marks(&self, paragraph: &Paragraph) -> Vec<Mark> {
        let key = key(paragraph);
        let mut state = self.shared.state.lock().unwrap();
        if let Some(marks) = state.marks(key, paragraph) {
            return marks;
        }
        if state.pending.insert(key) {
            let _ = self.jobs.send((key, paragraph.clone()));
        }
        Vec::new()
    }

    /// The marks on `paragraph`, checked on this thread where not yet checked, as the
    /// Spelling pane walks the page.
    pub fn marks_now(&self, paragraph: &Paragraph) -> Vec<Mark> {
        let key = key(paragraph);
        if let Some(marks) = self.shared.state.lock().unwrap().marks(key, paragraph) {
            return marks;
        }
        let marks = check(&[paragraph], &*self.shared.dictionary).remove(0);
        let mut state = self.shared.state.lock().unwrap();
        state.keep(key, paragraph, marks);
        state.marks(key, paragraph).unwrap_or_default()
    }

    /// Corrections for `word` in `language`, best first.
    pub fn suggest(&self, word: &str, language: Option<u32>) -> Vec<String> {
        self.shared
            .dictionary
            .suggest(word, language.unwrap_or(crate::language::EN_US))
    }

    /// Ignore: leaves `word` unmarked everywhere until the app quits.
    pub fn ignore(&self, word: &str) {
        self.shared
            .state
            .lock()
            .unwrap()
            .accepted
            .insert(word.to_owned());
    }

    /// Add to Dictionary: the platform's dictionary accepts `word` from now on.
    pub fn learn(&self, word: &str) {
        self.shared.dictionary.learn(word);
        self.ignore(word);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use onestore::document::Format;

    /// Knows a few English and French words; English only by default.
    pub(crate) struct Fake;

    const ENGLISH: &[&str] = &[
        "this", "sentence", "has", "the", "typos", "word", "don't", "i", "a", "bar", "is", "here",
        "well", "known", "hello",
    ];
    const FRENCH: &[&str] = &["bonjour", "le", "monde", "une", "phrase"];

    impl Dictionary for Fake {
        fn misspelled(&self, words: &[(&str, u32)]) -> Vec<bool> {
            words
                .iter()
                .map(|(word, language)| {
                    let known = match language {
                        1033 => ENGLISH,
                        1036 => FRENCH,
                        _ => return false,
                    };
                    !known.contains(&word.to_lowercase().replace('\u{2019}', "'").as_str())
                })
                .collect()
        }

        fn suggest(&self, word: &str, _: u32) -> Vec<String> {
            match word {
                "Ths" => vec!["This".into(), "Thus".into()],
                "sentense" => vec!["sentence".into()],
                _ => Vec::new(),
            }
        }

        fn learn(&self, _: &str) {}
    }

    #[test]
    fn languages_pick_their_locale_then_their_language() {
        let available: Vec<String> = ["en", "en_GB", "fr", "de-AT", "de_DE", "pt_BR"]
            .map(String::from)
            .into();
        for (lcid, picked) in [
            (1033, Some("en")),
            (2057, Some("en_GB")),
            (3081, Some("en")),
            (3084, Some("fr")),
            (1031, Some("de_DE")),
            (3079, Some("de-AT")),
            (2070, Some("pt_BR")),
            (1049, None),
            (0x1007f, None),
        ] {
            assert_eq!(pick(lcid, &available), picked, "{lcid}");
        }
        let hunspell: Vec<String> = ["en_AU", "en_US", "fr_FR"].map(String::from).into();
        assert_eq!(pick(1033, &hunspell), Some("en_US"));
        assert_eq!(pick(1036, &hunspell), Some("fr_FR"));
    }

    fn marked(paragraph: &Paragraph) -> Vec<(&str, bool)> {
        check(&[paragraph], &Fake)
            .remove(0)
            .into_iter()
            .map(|mark| (&paragraph.text()[mark.range], mark.repeated))
            .collect()
    }

    /// OneNote 2010 under its default Proofing options (`/tmp/snowbound-spelling/lab`).
    #[test]
    fn words_are_marked_as_onenote_marks_them() {
        let paragraph = Paragraph::new(
            "Ths sentense has the typos. HELLO WRLD NASA abc123 x86 3rd foo_bar \
             www.exmple.com http://exmple.com/qwrt a@exmple.com C:\\Users\\qwrt \
             don't dont word word the The don't don\u{2019}t well-knwn (sentense) \
             \u{65e5}\u{672c}\u{8a9e}"
                .into(),
            Format::default(),
        );
        assert_eq!(
            marked(&paragraph),
            [
                ("Ths", false),
                ("sentense", false),
                ("foo_bar", false),
                ("dont", false),
                ("word", true),
                ("The", true),
                ("don\u{2019}t", true),
                ("knwn", false),
                ("sentense", false),
            ]
        );
    }

    #[test]
    fn each_run_is_checked_in_its_language_and_hidden_codes_are_not_text() {
        let french = Format {
            language: Some(1036),
            ..Format::default()
        };
        let paragraph = Paragraph::from_runs([
            ("bonjour le monde ".to_owned(), french.clone()),
            ("bonjjour ".to_owned(), french),
            (
                "hello ".to_owned(),
                Format {
                    language: Some(1061),
                    ..Format::default()
                },
            ),
            (
                "wo".to_owned(),
                Format {
                    bold: Some(true),
                    ..Format::default()
                },
            ),
            ("rd ".to_owned(), Format::default()),
            (
                "HYPERLINK \"qwrt\"".to_owned(),
                Format {
                    hidden: Some(true),
                    ..Format::default()
                },
            ),
            ("heere".to_owned(), Format::default()),
        ]);
        assert_eq!(marked(&paragraph), [("bonjjour", false), ("heere", false)]);
    }

    #[test]
    fn marks_arrive_from_the_thread_and_accepted_words_drop_out() {
        struct Wake(mpsc::Sender<()>);
        impl std::task::Wake for Wake {
            fn wake(self: Arc<Self>) {
                let _ = self.0.send(());
            }
        }
        let (woken, wakes) = mpsc::channel();
        let spelling = Spelling::new(Box::new(Fake), Arc::new(Wake(woken)).into());
        let paragraph = Paragraph::new("Ths sentense here".into(), Format::default());
        assert!(spelling.marks(&paragraph).is_empty());
        wakes
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the spelling thread wakes the host");
        let words = |spelling: &Spelling| {
            spelling
                .marks(&paragraph)
                .into_iter()
                .map(|mark| paragraph.text()[mark.range].to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(words(&spelling), ["Ths", "sentense"]);
        spelling.ignore("sentense");
        assert_eq!(words(&spelling), ["Ths"]);
        assert_eq!(spelling.suggest("Ths", None), ["This", "Thus"]);
        // The same text in another language is checked again.
        let french = Paragraph::new(
            "Ths sentense here".into(),
            Format {
                language: Some(1036),
                ..Format::default()
            },
        );
        assert!(spelling.marks(&french).is_empty());
        wakes
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the spelling thread wakes the host");
        assert_eq!(spelling.marks(&french).len(), 2);
    }
}
