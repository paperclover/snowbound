#!/bin/sh
# The linker for x86_64-win7-windows-gnu: llvm-mingw's clang, which links compiler-rt
# itself, with LLVM's libunwind (static) answering for libgcc's unwinder, and rt/shims.c
# standing in for the imports Windows 7 lacks.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
clang=$LLVM_MINGW/bin/x86_64-w64-mingw32-clang
for arg do
    shift
    case $arg in
        -lgcc_eh | -lgcc_s) set -- "$@" -l:libunwind.a ;;
        -lgcc) ;;
        *) set -- "$@" "$arg" ;;
    esac
done
work=$(mktemp -d "${TMPDIR:-/tmp}/windows-link.XXXXXX")
trap 'rm -rf "$work"' EXIT
"$clang" -O2 -Wall -c "$here/rt/shims.c" -o "$work/shims.o"
"$clang" "$work/shims.o" "$@"
