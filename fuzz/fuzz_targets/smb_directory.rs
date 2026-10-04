#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/notebook/src/smb/directory/records.rs"]
mod records;

fuzz_target!(|bytes: &[u8]| {
    let _ = records::decode(bytes);
});
