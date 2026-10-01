//! Spelling in the browser, which offers pages no checker to ask: words stay unmarked until
//! Hunspell dictionaries load in wasm.

use canvas::spelling::Dictionary;

pub fn dictionary() -> Option<Box<dyn Dictionary>> {
    None
}
