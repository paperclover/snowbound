#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../crates/onestore/tests/support/page_schedule.rs"]
mod page_schedule;

fuzz_target!(|input: &[u8]| page_schedule::run(input));
