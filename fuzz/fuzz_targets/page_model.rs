#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../crates/onestore/tests/support/page_edits.rs"]
mod page_edits;

fuzz_target!(|input: &[u8]| page_edits::run(input));
