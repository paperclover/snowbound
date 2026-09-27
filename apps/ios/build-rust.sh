#!/bin/sh
# Builds crates/mobile for the platform Xcode is building; Xcode links the static library.
set -eu
case "$PLATFORM_NAME" in
iphonesimulator) target=aarch64-apple-ios-sim ;;
*) target=aarch64-apple-ios ;;
esac
release=
[ "$CONFIGURATION" = Release ] && release=--release
root="$SRCROOT/../.."
# Xcode's SDK variables would point host build scripts at the iOS SDK.
exec env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/etc/profiles/per-user/$USER/bin:/run/current-system/sw/bin:/usr/bin:/bin:/usr/sbin:/sbin" USER="$USER" \
	CARGO_TARGET_DIR="$root/target/ios" IPHONEOS_DEPLOYMENT_TARGET="$IPHONEOS_DEPLOYMENT_TARGET" \
	cargo build --manifest-path "$root/Cargo.toml" -p mobile --target "$target" $release
