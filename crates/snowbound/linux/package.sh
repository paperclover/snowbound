#!/bin/sh
# Builds target/dist/snowbound-linux-ARCH.tar.gz for each ARCH given (x86_64, aarch64;
# both by default), cross-linking with zig so it runs from macOS or Linux.
set -eu
root=$(cd "$(dirname "$0")/../../.." && pwd)
here="$root/crates/snowbound/linux"
icon="$root/crates/snowbound/assets/icon/Snowbound-SnowLeopard.png"
target="$root/target"
tools="$target/zig"
mkdir -p "$tools" "$target/dist"
command -v zig >/dev/null || { echo "zig is required as the cross linker" >&2; exit 1; }

printf '#!/bin/sh\nexec zig ar "$@"\n' > "$tools/ar"
chmod +x "$tools/ar"

[ $# -gt 0 ] || set -- x86_64 aarch64
for arch do
    triple=$arch-unknown-linux-gnu
    variable=$(echo "$triple" | tr 'a-z-' 'A-Z_')
    linker="$tools/cc-$arch"
    # glibc 2.31 reaches back to Debian 11 and Ubuntu 20.04; zig rejects rustc's
    # --target, --no-undefined-version and -O1.
    cat > "$linker" <<EOF
#!/bin/sh
for arg do
    shift
    case "\$arg" in --target=*|-Wl,--no-undefined-version|-Wl,-O1) ;; *) set -- "\$@" "\$arg" ;; esac
done
exec zig cc -target $arch-linux-gnu.2.31 "\$@"
EOF
    chmod +x "$linker"
    rustup target add "$triple" >/dev/null
    env "CARGO_TARGET_${variable}_LINKER=$linker" \
        "CC_$(echo "$triple" | tr - _)=$linker" \
        "AR_$(echo "$triple" | tr - _)=$tools/ar" \
        CARGO_PROFILE_RELEASE_STRIP=symbols \
        cargo build --manifest-path "$root/Cargo.toml" --release --target "$triple" -p snowbound

    name=snowbound-linux-$arch
    stage="$target/dist/$name"
    rm -rf "$stage"
    mkdir -p "$stage/bin" "$stage/share/applications"
    cp "$target/$triple/release/snowbound" "$stage/bin/"
    cp "$here/snowbound.desktop" "$stage/share/applications/"
    cp "$here/README.md" "$here/install.sh" "$stage/"
    for side in 16 24 32 48 64 128 256 512; do
        folder="$stage/share/icons/hicolor/${side}x$side/apps"
        mkdir -p "$folder"
        if command -v sips >/dev/null; then
            sips -s format png -z "$side" "$side" "$icon" --out "$folder/snowbound.png" >/dev/null
        else
            magick "$icon" -resize "${side}x$side" -depth 8 "$folder/snowbound.png"
        fi
    done
    COPYFILE_DISABLE=1 tar --no-xattrs -C "$target/dist" -czf "$target/dist/$name.tar.gz" "$name"
    echo "$target/dist/$name.tar.gz"
done
