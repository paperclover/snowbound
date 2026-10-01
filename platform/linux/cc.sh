#!/bin/sh
# zig cc for $ZIG_TARGET, as `cargo.sh` sets it: the C compiler and linker. zig rejects
# rustc's --target, --no-undefined-version and -O1, and strips the symbol table with the debug
# info, so the debug info goes after linking.
strip= output= previous=
for arg do
    shift
    [ "$previous" = -o ] && output=$arg
    previous=$arg
    case $arg in
    --target=* | -Wl,--no-undefined-version | -Wl,-O1) ;;
    -Wl,--strip-debug) strip=1 ;;
    *) set -- "$@" "$arg" ;;
    esac
done
zig cc -target "$ZIG_TARGET" "$@" || exit
[ -z "$strip" ] ||
    "$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/rust-objcopy" \
        --strip-debug "$output"
