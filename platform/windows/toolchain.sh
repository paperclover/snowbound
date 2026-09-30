#!/bin/sh
# Fetches llvm-mingw into target/windows/llvm-mingw: clang, lld and the MinGW-w64 sysroots
# for x86_64 and aarch64, built for msvcrt.dll, which Windows 7 has without updates, where
# the UCRT needs KB2999226. llvm-mingw publishes msvcrt builds only for Linux and Windows
# hosts, so on macOS its UCRT toolchain takes the msvcrt build's target files.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
release=20260922
out=$root/target/windows/llvm-mingw
stage=$root/target/windows/llvm-mingw.partial
base=https://github.com/mstorsjo/llvm-mingw/releases/download/$release
msvcrt=llvm-mingw-$release-msvcrt-ubuntu-22.04-x86_64
[ ! -e "$out" ] || { echo "$out exists" >&2; exit 1; }
rm -rf "${stage:?}"
mkdir -p "$stage"
curl -fsSL "$base/$msvcrt.tar.xz" | tar -xJ -C "$stage"
if [ "$(uname -s)" = Darwin ]; then
    host=llvm-mingw-$release-ucrt-macos-universal
    curl -fsSL "$base/$host.tar.xz" | tar -xJ -C "$stage"
    clang=$(ls "$stage/$host/lib/clang")
    for part in x86_64-w64-mingw32 aarch64-w64-mingw32 generic-w64-mingw32 \
        "lib/clang/$clang/lib/windows"; do
        rm -rf "${stage:?}/$host/$part"
        mv "$stage/$msvcrt/$part" "$stage/$host/$part"
    done
else
    host=$msvcrt
fi
# Only x86_64 and aarch64 are built.
rm -rf "${stage:?}/$host/i686-w64-mingw32" "${stage:?}/$host/armv7-w64-mingw32"
find "$stage/$host/bin" \( -name 'i686-*' -o -name 'armv7-*' -o -name '*uwp*' \) -delete
mv "$stage/$host" "$out"
rm -rf "${stage:?}"
echo "$out"
