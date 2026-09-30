//! Spelling through Enchant, loaded at run time with whichever dictionaries its providers
//! (Hunspell, Nuspell, Aspell) have. Without Enchant, words go unmarked.

use canvas::spelling::{Dictionary, pick};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::Mutex;

type Broker = *mut c_void;
type Dict = *mut c_void;
type Describe =
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char, *mut c_void);

struct Api {
    request: unsafe extern "C" fn(Broker, *const c_char) -> Dict,
    list: unsafe extern "C" fn(Broker, Describe, *mut c_void),
    check: unsafe extern "C" fn(Dict, *const c_char, isize) -> c_int,
    suggest: unsafe extern "C" fn(Dict, *const c_char, isize, *mut usize) -> *mut *mut c_char,
    free: unsafe extern "C" fn(Dict, *mut *mut c_char),
    add: unsafe extern "C" fn(Dict, *const c_char, isize),
}

struct Enchant {
    api: Api,
    broker: Broker,
    /// The dictionaries Enchant has, as `en_US`.
    languages: Vec<String>,
    /// Dictionaries opened so far, by LCID; none where no dictionary serves it.
    open: HashMap<u32, Option<Dict>>,
}

// SAFETY: Enchant's broker and dictionaries are used by one thread at a time, under the
// checker's lock.
unsafe impl Send for Enchant {}

impl Enchant {
    fn dict(&mut self, lcid: u32) -> Option<Dict> {
        if let Some(dict) = self.open.get(&lcid) {
            return *dict;
        }
        let dict = pick(lcid, &self.languages)
            .and_then(|name| CString::new(name).ok())
            .map(|name| unsafe { (self.api.request)(self.broker, name.as_ptr()) })
            .filter(|dict| !dict.is_null());
        self.open.insert(lcid, dict);
        dict
    }
}

struct Checker(Mutex<Enchant>);

impl Dictionary for Checker {
    fn misspelled(&self, words: &[(&str, u32)]) -> Vec<bool> {
        let mut enchant = self.0.lock().unwrap();
        words
            .iter()
            .map(|(word, lcid)| {
                enchant.dict(*lcid).is_some_and(|dict| {
                    let word = CString::new(*word).unwrap_or_default();
                    let length = word.as_bytes().len() as isize;
                    unsafe { (enchant.api.check)(dict, word.as_ptr(), length) > 0 }
                })
            })
            .collect()
    }

    fn suggest(&self, word: &str, lcid: u32) -> Vec<String> {
        let mut enchant = self.0.lock().unwrap();
        let (Some(dict), Ok(word)) = (enchant.dict(lcid), CString::new(word)) else {
            return Vec::new();
        };
        let mut count = 0;
        let length = word.as_bytes().len() as isize;
        unsafe {
            let list = (enchant.api.suggest)(dict, word.as_ptr(), length, &mut count);
            if list.is_null() {
                return Vec::new();
            }
            let suggestions = std::slice::from_raw_parts(list, count)
                .iter()
                .map(|suggestion| CStr::from_ptr(*suggestion).to_string_lossy().into_owned())
                .collect();
            (enchant.api.free)(dict, list);
            suggestions
        }
    }

    /// Adds `word` to the personal word list of every dictionary opened.
    fn learn(&self, word: &str) {
        let enchant = self.0.lock().unwrap();
        let Ok(word) = CString::new(word) else {
            return;
        };
        let length = word.as_bytes().len() as isize;
        for dict in enchant.open.values().flatten() {
            unsafe { (enchant.api.add)(*dict, word.as_ptr(), length) };
        }
    }
}

/// The function `name` in `library`, of C signature `F`.
unsafe fn symbol<F: Copy>(library: *mut c_void, name: &CStr) -> Option<F> {
    let symbol = unsafe { libc::dlsym(library, name.as_ptr()) };
    (!symbol.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&symbol) })
}

unsafe extern "C" fn describe(
    tag: *const c_char,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
    languages: *mut c_void,
) {
    let languages = unsafe { &mut *languages.cast::<Vec<String>>() };
    let tag = unsafe { CStr::from_ptr(tag) }
        .to_string_lossy()
        .into_owned();
    if !languages.contains(&tag) {
        languages.push(tag);
    }
}

/// Enchant 2 or 1 with its dictionaries; none where it is not installed.
pub fn dictionary() -> Option<Box<dyn Dictionary>> {
    unsafe {
        let library = [c"libenchant-2.so.2", c"libenchant.so.1"]
            .into_iter()
            .map(|name| libc::dlopen(name.as_ptr(), libc::RTLD_NOW))
            .find(|library| !library.is_null())?;
        let init: unsafe extern "C" fn() -> Broker = symbol(library, c"enchant_broker_init")?;
        let api = Api {
            request: symbol(library, c"enchant_broker_request_dict")?,
            list: symbol(library, c"enchant_broker_list_dicts")?,
            check: symbol(library, c"enchant_dict_check")?,
            suggest: symbol(library, c"enchant_dict_suggest")?,
            free: symbol(library, c"enchant_dict_free_string_list")?,
            add: symbol(library, c"enchant_dict_add")
                .or_else(|| symbol(library, c"enchant_dict_add_to_personal"))?,
        };
        let broker = init();
        if broker.is_null() {
            return None;
        }
        let mut languages: Vec<String> = Vec::new();
        (api.list)(broker, describe, (&raw mut languages).cast());
        Some(Box::new(Checker(Mutex::new(Enchant {
            api,
            broker,
            languages,
            open: HashMap::new(),
        }))))
    }
}

#[cfg(test)]
mod tests {
    /// Where Enchant has an English dictionary, it marks misspellings and corrects them; where
    /// it has none, words stay unmarked.
    #[test]
    fn enchant_checks_the_languages_it_has() {
        let Some(dictionary) = super::dictionary() else {
            return;
        };
        let marked = dictionary.misspelled(&[("sentence", 1033), ("sentense", 1033)]);
        if marked == [false, false] {
            return;
        }
        assert_eq!(marked, [false, true]);
        assert!(
            dictionary
                .suggest("sentense", 1033)
                .contains(&"sentence".to_owned())
        );
        assert_eq!(dictionary.misspelled(&[("sentense", 0x1007f)]), [false]);
    }
}
