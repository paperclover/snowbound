//! Spelling through AppKit's shared NSSpellChecker, which the spelling thread reaches on
//! the main thread, as AppKit's objects expect.

use canvas::spelling::{Dictionary, pick};
use objc2::{class, msg_send, msg_send_id, rc::Retained, runtime::AnyObject};
use objc2_foundation::{MainThreadMarker, NSArray, NSRange, NSString};
use std::ffi::c_void;

unsafe extern "C" {
    /// The main queue; only its address matters.
    static _dispatch_main_q: u8;
    fn dispatch_sync_f(queue: *mut c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

/// Runs `work` on the main thread, waiting for it.
fn on_main<F: FnOnce() -> R + Send, R: Send>(work: F) -> R {
    if MainThreadMarker::new().is_some() {
        return work();
    }
    extern "C" fn run<F: FnOnce() -> R, R>(context: *mut c_void) {
        let (work, result) = unsafe { &mut *context.cast::<(Option<F>, Option<R>)>() };
        *result = work.take().map(|work| work());
    }
    let mut slot = (Some(work), None);
    unsafe {
        dispatch_sync_f(
            (&raw const _dispatch_main_q).cast_mut().cast(),
            (&raw mut slot).cast(),
            run::<F, R>,
        );
    }
    slot.1.expect("the main queue ran the work")
}

fn checker() -> Retained<AnyObject> {
    unsafe { msg_send_id![class!(NSSpellChecker), sharedSpellChecker] }
}

struct Checker {
    /// The document tag Snowbound's checks share.
    tag: isize,
    /// The spelling languages the system has, as `en` or `en_GB`.
    languages: Vec<String>,
}

impl Checker {
    fn language(&self, lcid: u32) -> Option<Retained<NSString>> {
        pick(lcid, &self.languages).map(NSString::from_str)
    }
}

impl Dictionary for Checker {
    fn misspelled(&self, words: &[(&str, u32)]) -> Vec<bool> {
        on_main(|| {
            let checker = checker();
            words
                .iter()
                .map(|(word, lcid)| {
                    let Some(language) = self.language(*lcid) else {
                        return false;
                    };
                    let found: NSRange = unsafe {
                        msg_send![&checker,
                            checkSpellingOfString: &*NSString::from_str(word),
                            startingAt: 0isize,
                            language: &*language,
                            wrap: false,
                            inSpellDocumentWithTag: self.tag,
                            wordCount: std::ptr::null_mut::<isize>()]
                    };
                    found.length > 0
                })
                .collect()
        })
    }

    fn suggest(&self, word: &str, lcid: u32) -> Vec<String> {
        on_main(|| {
            let Some(language) = self.language(lcid) else {
                return Vec::new();
            };
            let word = NSString::from_str(word);
            let guesses: Option<Retained<NSArray<NSString>>> = unsafe {
                msg_send_id![&checker(),
                    guessesForWordRange: NSRange::new(0, word.length()),
                    inString: &*word,
                    language: &*language,
                    inSpellDocumentWithTag: self.tag]
            };
            guesses.map_or_else(Vec::new, |guesses| {
                guesses.iter().map(|guess| guess.to_string()).collect()
            })
        })
    }

    fn learn(&self, word: &str) {
        on_main(|| unsafe {
            let _: () = msg_send![&checker(), learnWord: &*NSString::from_str(word)];
        });
    }
}

/// AppKit's spell checker with the languages the system has. Called on the main thread.
pub fn dictionary() -> Option<Box<dyn Dictionary>> {
    MainThreadMarker::new()?;
    let checker = checker();
    let languages: Retained<NSArray<NSString>> =
        unsafe { msg_send_id![&checker, availableLanguages] };
    let tag: isize = unsafe { msg_send![class!(NSSpellChecker), uniqueSpellDocumentTag] };
    Some(Box::new(Checker {
        tag,
        languages: languages
            .iter()
            .map(|language| language.to_string())
            .collect(),
    }))
}
