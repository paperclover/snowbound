#!/bin/sh
# The linker for x86_64-win7-windows-gnu: llvm-mingw's clang, which links compiler-rt
# itself, with LLVM's libunwind (static) answering for libgcc's unwinder.
set -eu
for arg do
    shift
    case $arg in
        -lgcc_eh | -lgcc_s) set -- "$@" -l:libunwind.a ;;
        -lgcc) ;;
        *) set -- "$@" "$arg" ;;
    esac
done
exec "$LLVM_MINGW/bin/x86_64-w64-mingw32-clang" "$@"
