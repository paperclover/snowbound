#!/bin/sh
# zig cc for $ZIG_TARGET, as `cargo.sh` sets it: the C compiler and linker. zig rejects
# rustc's --target, --no-undefined-version and -O1.
for arg do
    shift
    case $arg in --target=* | -Wl,--no-undefined-version | -Wl,-O1) ;; *) set -- "$@" "$arg" ;; esac
done
exec zig cc -target "$ZIG_TARGET" "$@"
