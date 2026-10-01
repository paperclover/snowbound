#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../crates/onestore/tests/support/ops.rs"]
mod ops;

#[path = "../../crates/notebook/tests/support/model_ops.rs"]
mod model_ops;
#[path = "../../crates/notebook/tests/support/server.rs"]
mod server;
#[path = "../../crates/notebook/tests/support/model_schedule.rs"]
mod model_schedule;

fuzz_target!(|input: &[u8]| model_schedule::run(input));
