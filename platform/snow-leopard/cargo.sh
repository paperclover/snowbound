#!/bin/sh
# cargo for Mac OS X 10.6, from any package: `cargo.sh build --release -p notebook ...`.
# Needs nightly with rust-src and the SDK from `remote.sh sdk`.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
sdk=${SNOW_LEOPARD_SDK:-$(cd "$here/../.." && pwd)/target/snow-leopard/MacOSX10.6.sdk}
target=x86_64-apple-macosx10.6
command=$1
shift
# Cargo can't see link.sh's inputs; folding their hash into the flags relinks on change.
runtime=$(cat "$here/link.sh" "$here"/rt/* | shasum | cut -c1-12)
# The deployment target is for cc-rs (bundled SQLite); rustc warns that it ignores it.
# rustc would infer ld64 from the name `link.sh`; it is a C compiler driver.
# 10.6's EVFILT_USER drops mio's wakes, so Tokio wakes its I/O thread through a pipe.
# objc2's debug builds check declared classes against today's SDKs, whose protocols have
# methods 10.6's lack (winit's window delegate declares 10.7's), so they stay off.
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$here/../../target/snow-leopard} \
SNOW_LEOPARD_SDK=$sdk \
TARGET_CFLAGS="-isysroot $sdk -mmacosx-version-min=10.6" \
MACOSX_DEPLOYMENT_TARGET=10.6 \
exec cargo +nightly -Zbuild-std=std,panic_unwind -Zjson-target-spec \
    --config "target.$target.linker='$here/link.sh'" \
    --config "target.$target.rustflags=['-C','linker-flavor=darwin-cc','-C','metadata=rt-$runtime','--cfg','mio_unsupported_force_waker_pipe']" \
    --config "profile.dev.package.objc2.debug-assertions=false" \
    "$command" --target "$here/$target.json" "$@"
