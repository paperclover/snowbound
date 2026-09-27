#!/bin/sh
# Soaks a dedicated machine until stopped: each round runs every seeded sweep in full with
# a fresh seed shift, then each fuzz target for a time limit. A failing round leaves its
# log in soak/ with the command that replays it; libFuzzer keeps crash inputs in
# fuzz/artifacts/<target>/.
#
#   tools/soak.sh [seconds per fuzz target, default 600]
set -u
cd "$(dirname "$0")/.."
seconds=${1:-600}
mkdir -p soak
# Release speed with the checks debug builds make.
sweeps="cargo test --release --no-fail-fast --config profile.release.debug-assertions=true --config profile.release.overflow-checks=true -p onestore -p notebook -p canvas"
$sweeps --no-run || exit 1
cargo +nightly fuzz build || exit 1
while :; do
    shift=$(od -An -N4 -tu4 /dev/urandom | tr -d ' ')
    log=soak/sweep-$shift.log
    if SNOWBOUND_SWEEP=$shift $sweeps >"$log" 2>&1; then
        rm "$log"
    else
        echo "replay: SNOWBOUND_SWEEP=$shift $sweeps" | tee -a "$log"
        sed -n 's/^---- \(.*\) stdout ----$/\1/p' "$log" | while read -r name; do
            echo "replay: SNOWBOUND_SWEEP=$shift $sweeps -- --exact $name" | tee -a "$log"
        done
    fi
    for target in $(cargo +nightly fuzz list); do
        log=soak/fuzz-$target-$(date +%Y%m%d-%H%M%S).log
        if cargo +nightly fuzz run "$target" -- -max_total_time="$seconds" >"$log" 2>&1; then
            rm "$log"
        else
            grep 'cargo fuzz run' "$log" | sed 's/^[[:space:]]*/replay: /' | tee -a "$log"
        fi
    done
done
