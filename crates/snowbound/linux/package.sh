#!/bin/sh
# Builds target/dist/snowbound-linux-ARCH.tar.gz for each ARCH given (x86_64, aarch64;
# both by default), cross-linking with zig (`platform/linux/cargo.sh`) so it runs from macOS or Linux.
set -eu
root=$(cd "$(dirname "$0")/../../.." && pwd)
here="$root/crates/snowbound/linux"
target="$root/target"
mkdir -p "$target/dist"

[ $# -gt 0 ] || set -- x86_64 aarch64
for arch do
    triple=$arch-unknown-linux-gnu
    CARGO_PROFILE_RELEASE_STRIP=symbols sh "$root/platform/linux/cargo.sh" "$arch" build \
        --manifest-path "$root/Cargo.toml" --release -p snowbound
    # PT_INTERP becomes PT_NULL, so the kernel starts the executable at `loader`'s entry, which
    # finds the system's dynamic loader, NixOS's too. zig rejects --no-dynamic-linker.
    python3 - "$target/$triple/release/snowbound" <<'PYTHON'
import struct, sys
with open(sys.argv[1], 'r+b') as elf:
    header = elf.read(64)
    table, = struct.unpack_from('<Q', header, 32)
    size, count = struct.unpack_from('<HH', header, 54)
    for index in range(count):
        elf.seek(table + index * size)
        if struct.unpack('<I', elf.read(4))[0] == 3:
            elf.seek(table + index * size)
            elf.write(struct.pack('<I', 0))
PYTHON

    name=snowbound-linux-$arch
    stage="$target/dist/$name"
    rm -rf "$stage"
    mkdir -p "$stage/bin"
    cp "$target/$triple/release/snowbound" "$stage/bin/"
    cp "$here/README.md" "$stage/"
    COPYFILE_DISABLE=1 tar --no-xattrs -C "$target/dist" -czf "$target/dist/$name.tar.gz" "$name"
    echo "$target/dist/$name.tar.gz"
done
