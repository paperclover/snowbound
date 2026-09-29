#!/bin/sh
# rustc's linker for the Mac OS X 10.6 targets: links against the 10.6 SDK (see
# `remote.sh sdk`), so an import 10.6 lacks fails here, plus the shims in rt/.
# rustc clamps the deployment target to 10.12; this pins it back to 10.6.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
sdk=${SNOW_LEOPARD_SDK:-$here/../../target/snow-leopard/MacOSX10.6.sdk}
[ -d "$sdk" ] || { echo "link.sh: no 10.6 SDK at $sdk; run remote.sh sdk" >&2; exit 1; }
arch=x86_64
prev=
for arg; do
    shift
    case "$arg" in
        -mmacosx-version-min=*) arg=-mmacosx-version-min=10.6 ;;
    esac
    # 10.6 keeps these inside ApplicationServices, which re-exports them to its clients.
    if [ "$prev" = -framework ]; then
        case "$arg" in
            CoreGraphics | CoreText | ImageIO | ColorSync) arg=ApplicationServices ;;
            # Test binaries whose dev-dependencies take draw's wgpu backend (ui, canvas)
            # name Metal, which 10.6 lacks; nothing they run on 10.6 calls into it.
            Metal) arg=Foundation ;;
        esac
    fi
    [ "$prev" = -arch ] && arch=$arg
    prev=$arg
    set -- "$@" "$arg"
done
work=$(mktemp -d "${TMPDIR:-/tmp}/snow-leopard-link.XXXXXX")
trap 'rm -rf "$work"' EXIT
for source in "$here"/rt/*.c; do
    clang -arch "$arch" -isysroot "$sdk" -mmacosx-version-min=10.6 -O2 -Wall \
        -c "$source" -o "$work/$(basename "$source" .c).o"
done
# shims.o overrides libSystem, so it links as an object; the rest fill in functions later
# systems added, from an archive, so only binaries calling them pull in their frameworks.
ar rcs "$work/libsnow_leopard.a" $(ls "$work"/*.o | grep -v /shims.o)
status=0
clang -isysroot "$sdk" "$@" "$work/shims.o" "$work/libsnow_leopard.a" 2>"$work/log" || status=$?
# rustc's objects claim 10.12; ld-prime's stub deprecation is noise for a frozen SDK.
grep -v -e "was built for newer 'macOS' version (10.12)" -e "dylib stub (MH_DYLIB_STUB)" \
    -e "no platform load command found in .*crt1" "$work/log" >&2 || true
exit $status
