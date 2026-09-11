#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;
#[path = "../../crates/notebook/tests/support/tree_schedule.rs"]
mod tree_schedule;

fuzz_target!(|input: &[u8]| tree_schedule::run(input));
