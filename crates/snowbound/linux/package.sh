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

    name=snowbound-linux-$arch
    stage="$target/dist/$name"
    rm -rf "$stage"
    mkdir -p "$stage/bin"
    cp "$target/$triple/release/snowbound" "$stage/bin/"
    cp "$here/README.md" "$stage/"
    COPYFILE_DISABLE=1 tar --no-xattrs -C "$target/dist" -czf "$target/dist/$name.tar.gz" "$name"
    echo "$target/dist/$name.tar.gz"
done
