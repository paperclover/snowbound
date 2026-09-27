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
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$here/../../target/snow-leopard} \
SNOW_LEOPARD_SDK=$sdk \
TARGET_CFLAGS="-isysroot $sdk -mmacosx-version-min=10.6" \
MACOSX_DEPLOYMENT_TARGET=10.6 \
exec cargo +nightly -Zbuild-std=std,panic_unwind -Zjson-target-spec \
    --config "target.$target.linker='$here/link.sh'" \
    --config "target.$target.rustflags=['-C','linker-flavor=darwin-cc','-C','metadata=rt-$runtime']" \
    "$command" --target "$here/$target.json" "$@"
