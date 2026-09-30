//! Spelling through the Spell Checking API that Windows 8 added, with the dictionaries of
//! the languages installed; Windows 7 has none, and words go unmarked there. The checkers
//! live on a thread of their own in COM's multithreaded apartment, which every call reaches
//! through a channel, whatever thread it comes from.

use canvas::spelling::{Dictionary, pick};
use std::{
    collections::HashMap,
    ffi::c_void,
    sync::mpsc::{Sender, channel},
};
use windows_sys::{
    Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    },
    core::GUID,
};

/// CLSID_SpellCheckerFactory and IID_ISpellCheckerFactory.
const FACTORY: GUID = GUID::from_u128(0x7ab36653_1796_484b_bdfa_e74f1db7c1dc);
const IID_FACTORY: GUID = GUID::from_u128(0x8e018a9d_2415_4677_bf08_794ea61f94bb);

/// A COM object: a pointer to its table of methods, which begins with IUnknown's.
struct Object(*mut *const usize);

impl Object {
    /// Method `index` of the object's table, of signature `F`.
    unsafe fn method<F: Copy>(&self, index: usize) -> F {
        unsafe { std::mem::transmute_copy(&*(*self.0).add(index)) }
    }

    fn this(&self) -> *mut c_void {
        self.0.cast()
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        type Release = unsafe extern "system" fn(*mut c_void) -> u32;
        unsafe { self.method::<Release>(2)(self.this()) };
    }
}

type Out = *mut *mut *const usize;

/// The object a method answered through `out`, where it answered one.
fn answered(result: i32, out: *mut *const usize) -> Option<Object> {
    (result >= 0 && !out.is_null()).then_some(Object(out))
}

/// The strings an IEnumString holds, freed as they are read.
fn strings(enumeration: Object) -> Vec<String> {
    type Next = unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16, *mut u32) -> i32;
    let mut all = Vec::new();
    loop {
        let mut text = std::ptr::null_mut();
        let mut fetched = 0;
        let result = unsafe {
            enumeration.method::<Next>(3)(enumeration.this(), 1, &mut text, &mut fetched)
        };
        if result != 0 || fetched == 0 || text.is_null() {
            return all;
        }
        let length = (0..)
            .take_while(|&at| unsafe { *text.add(at) } != 0)
            .count();
        all.push(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(text, length)
        }));
        unsafe { CoTaskMemFree(text.cast()) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// The factory and the checkers made so far, owned by the spelling thread.
struct Checkers {
    factory: Object,
    /// The languages the system spells, as `en-US`.
    languages: Vec<String>,
    by_language: HashMap<String, Object>,
}

impl Checkers {
    fn new() -> Option<Self> {
        let mut factory = std::ptr::null_mut();
        let created = unsafe {
            CoCreateInstance(
                &FACTORY,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_FACTORY,
                &mut factory,
            )
        };
        let factory = answered(created, factory.cast())?;
        type Languages = unsafe extern "system" fn(*mut c_void, Out) -> i32;
        let mut languages = std::ptr::null_mut();
        let listed = unsafe { factory.method::<Languages>(3)(factory.this(), &mut languages) };
        let languages = strings(answered(listed, languages)?);
        Some(Self {
            factory,
            languages,
            by_language: HashMap::new(),
        })
    }

    /// The checker for `lcid`'s language, where the system spells it.
    fn checker(&mut self, lcid: u32) -> Option<&Object> {
        let language = pick(lcid, &self.languages)?.to_owned();
        if !self.by_language.contains_key(&language) {
            type Create = unsafe extern "system" fn(*mut c_void, *const u16, Out) -> i32;
            let mut checker = std::ptr::null_mut();
            let tag = wide(&language);
            let factory = &self.factory;
            let result =
                unsafe { factory.method::<Create>(5)(factory.this(), tag.as_ptr(), &mut checker) };
            self.by_language
                .insert(language.clone(), answered(result, checker)?);
        }
        self.by_language.get(&language)
    }

    fn misspelled(&mut self, word: &str, lcid: u32) -> bool {
        type Check = unsafe extern "system" fn(*mut c_void, *const u16, Out) -> i32;
        type Next = unsafe extern "system" fn(*mut c_void, Out) -> i32;
        let Some(checker) = self.checker(lcid) else {
            return false;
        };
        let text = wide(word);
        let mut errors = std::ptr::null_mut();
        let checked =
            unsafe { checker.method::<Check>(4)(checker.this(), text.as_ptr(), &mut errors) };
        let Some(errors) = answered(checked, errors) else {
            return false;
        };
        let mut error = std::ptr::null_mut();
        // S_OK with an error; S_FALSE once there are none.
        let found = unsafe { errors.method::<Next>(3)(errors.this(), &mut error) };
        found == 0 && answered(found, error).is_some()
    }

    fn suggest(&mut self, word: &str, lcid: u32) -> Vec<String> {
        type Suggest = unsafe extern "system" fn(*mut c_void, *const u16, Out) -> i32;
        let Some(checker) = self.checker(lcid) else {
            return Vec::new();
        };
        let text = wide(word);
        let mut suggestions = std::ptr::null_mut();
        let result = unsafe {
            checker.method::<Suggest>(5)(checker.this(), text.as_ptr(), &mut suggestions)
        };
        answered(result, suggestions).map_or_else(Vec::new, strings)
    }

    /// Adds `word` to the user's dictionary, which every language's checker shares.
    fn learn(&mut self, word: &str) {
        type Add = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;
        let checker = self.by_language.values().next();
        if let Some(checker) = checker {
            let text = wide(word);
            unsafe { checker.method::<Add>(6)(checker.this(), text.as_ptr()) };
        }
    }
}

enum Request {
    Misspelled(Vec<(String, u32)>, Sender<Vec<bool>>),
    Suggest(String, u32, Sender<Vec<String>>),
    Learn(String),
}

struct Checker(Sender<Request>);

impl Checker {
    fn ask<R: Default>(&self, request: impl FnOnce(Sender<R>) -> Request) -> R {
        let (answer, answered) = channel();
        if self.0.send(request(answer)).is_err() {
            return R::default();
        }
        answered.recv().unwrap_or_default()
    }
}

impl Dictionary for Checker {
    fn misspelled(&self, words: &[(&str, u32)]) -> Vec<bool> {
        let words = words
            .iter()
            .map(|(word, lcid)| ((*word).to_owned(), *lcid))
            .collect();
        self.ask(|answer| Request::Misspelled(words, answer))
    }

    fn suggest(&self, word: &str, lcid: u32) -> Vec<String> {
        self.ask(|answer| Request::Suggest(word.to_owned(), lcid, answer))
    }

    fn learn(&self, word: &str) {
        let _ = self.0.send(Request::Learn(word.to_owned()));
    }
}

/// The system's spell checker with the languages installed, from Windows 8.
pub fn dictionary() -> Option<Box<dyn Dictionary>> {
    let (requests, received) = channel();
    let (ready, started) = channel();
    std::thread::Builder::new()
        .name("snowbound-spell".into())
        .spawn(move || {
            unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) };
            let Some(mut checkers) = Checkers::new() else {
                let _ = ready.send(false);
                return;
            };
            let _ = ready.send(true);
            for request in received {
                match request {
                    Request::Misspelled(words, answer) => {
                        let _ = answer.send(
                            words
                                .iter()
                                .map(|(word, lcid)| checkers.misspelled(word, *lcid))
                                .collect(),
                        );
                    }
                    Request::Suggest(word, lcid, answer) => {
                        let _ = answer.send(checkers.suggest(&word, lcid));
                    }
                    Request::Learn(word) => checkers.learn(&word),
                }
            }
        })
        .ok()?;
    started
        .recv()
        .ok()?
        .then(|| Box::new(Checker(requests)) as Box<dyn Dictionary>)
}
