#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../crates/onestore/tests/support/current.rs"]
mod current;
#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;
#[path = "../../crates/onestore/tests/support/ops.rs"]
mod ops;
#[path = "../../crates/onestore/tests/support/tree_model.rs"]
mod tree_model;

fuzz_target!(|input: &[u8]| tree_model::run(input));
