#!/bin/sh
# cargo for Linux from macOS or Linux: `cargo.sh ARCH COMMAND ARGS...`, ARCH x86_64 or aarch64.
# zig compiles C and links against glibc 2.17, Rust's own floor, which reaches back to RHEL 7,
# Debian 8 and Ubuntu 14.04.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
case ${1:-} in
x86_64 | aarch64) ;;
*)
    echo "usage: $0 x86_64|aarch64 COMMAND ARGS..." >&2
    exit 2
    ;;
esac
arch=$1 command=$2
shift 2
command -v zig >/dev/null || { echo "zig is required as the cross linker" >&2; exit 1; }
triple=$arch-unknown-linux-gnu
rustup target add "$triple" >/dev/null
variable=$(echo "$triple" | tr - _)
# The build id ties an executable to its published .debug file.
env "CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_LINKER=$here/cc.sh" \
    "CC_$variable=$here/cc.sh" "AR_$variable=$here/ar.sh" \
    ZIG_TARGET="$arch-linux-gnu.2.17" \
    cargo "$command" --target "$triple" \
    --config "target.$triple.rustflags=['-C','link-arg=-Wl,--build-id']" "$@"
