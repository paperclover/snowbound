#!/bin/sh
# cargo for Windows from macOS or Linux: `cargo.sh ARCH COMMAND ARGS...`.
#   x86_64   Windows 7 SP1 to 11: nightly's tier-3 x86_64-win7-windows-gnu, std built here
#   aarch64  Windows 11 on Arm: aarch64-pc-windows-gnullvm
# Both link with llvm-mingw (`toolchain.sh`) against msvcrt.dll, which every Windows has.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
LLVM_MINGW=${LLVM_MINGW:-$root/target/windows/llvm-mingw}
export LLVM_MINGW
[ -x "$LLVM_MINGW/bin/clang" ] || {
    echo "No llvm-mingw at $LLVM_MINGW; run $here/toolchain.sh" >&2
    exit 1
}
arch=$1 command=$2
shift 2
case $arch in
x86_64)
    target=x86_64-win7-windows-gnu
    toolchain=+nightly
    # rustc would infer a bare ld from the name `link.sh`; it is a C compiler driver.
    # raw-dylib imports (the windows crates) are made by llvm-mingw's dlltool. Cargo
    # can't see link.sh's inputs; folding their hash into the flags relinks on change.
    runtime=$(cat "$here/link.sh" "$here"/rt/* | shasum | cut -c1-12)
    set -- -Zbuild-std=std,panic_abort,panic_unwind \
        --config "target.$target.linker='$here/link.sh'" \
        --config "target.$target.rustflags=['-C','linker-flavor=gcc','-C','metadata=rt-$runtime','-C','dlltool=$LLVM_MINGW/bin/x86_64-w64-mingw32-dlltool']" "$@"
    ;;
aarch64)
    target=aarch64-pc-windows-gnullvm
    toolchain=+stable
    rustup target add --toolchain stable "$target" >/dev/null
    # crt-static links libunwind in rather than beside the executable. clang ignores
    # rustc's -no-pie.
    set -- --config "target.$target.linker='$LLVM_MINGW/bin/aarch64-w64-mingw32-clang'" \
        --config "target.$target.rustflags=['-C','target-feature=+crt-static','-C','link-arg=-Wno-unused-command-line-argument']" "$@"
    ;;
*)
    echo "usage: $0 x86_64|aarch64 COMMAND ARGS..." >&2
    exit 2
    ;;
esac
variable=$(echo "$target" | tr - _)
# cc-rs (bundled SQLite, ring) compiles with the same clang and archives with its llvm-ar.
env "CC_$variable=$LLVM_MINGW/bin/$arch-w64-mingw32-clang" \
    "AR_$variable=$LLVM_MINGW/bin/llvm-ar" \
    CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target/windows}" \
    cargo "$toolchain" "$command" --target "$target" "$@"
