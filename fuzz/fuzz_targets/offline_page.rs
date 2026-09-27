#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;
#[path = "../../crates/notebook/tests/support/model_ops.rs"]
mod model_ops;
#[path = "../../crates/notebook/tests/support/page_schedule.rs"]
mod page_schedule;
#[path = "../../crates/notebook/tests/support/server.rs"]
mod server;

fuzz_target!(|input: &[u8]| page_schedule::run(input));
