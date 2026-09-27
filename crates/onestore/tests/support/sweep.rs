//! How far seeded sweeps reach. By default each runs its smoke slice; `SNOWBOUND_SWEEP=n`
//! runs every sweep in full with its seeds shifted by `n` (`0` for the seeds as written), and
//! a failure replays under the same value.
#![allow(dead_code)]

use std::ops::Range;

/// The shift `SNOWBOUND_SWEEP` asks full sweeps to run with.
pub fn full() -> Option<u64> {
    let shift = std::env::var("SNOWBOUND_SWEEP").ok()?;
    Some(shift.parse().expect("SNOWBOUND_SWEEP is a seed shift"))
}

/// The seeds of `all` a sweep runs: its first `smoke`, or all of them shifted.
pub fn seeds(all: Range<u64>, smoke: u64) -> Range<u64> {
    match full() {
        None => all.start..all.end.min(all.start + smoke),
        Some(shift) => all.start + shift..all.end + shift,
    }
}
