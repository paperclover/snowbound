# Releases and updates

`tools/release.py` publishes the commit `main` points at, for every desktop
platform, into Clover's NAS folder `/Volumes/clover/Documents/Public/Snowbound/`,
which `https://file.paperclover.net/shr/snowbound/` serves read-only. The app
checks that URL and updates itself.

## Versions

A build is named by its commit on `main`: the day the commit was made, in
Los Angeles, and how many commits among it and its ancestors were made that
day, itself included. Commit times are jj's committer timestamps. The fourth
commit of 2026-09-29 is **Snowbound build 2026-09-29 revision 4**, written
`2026-09-29-r4` in manifests and published in the folder `2026-09-29.r4/`. A
build's version is compiled in from `SNOWBOUND_BUILD`; builds without it are
development builds, which never update themselves.

## The published folder

```text
latest.json                    {"macos-aarch64": "2026-09-29-r10", "macos-x86_64": ..., "macos-10.6": ..., "linux-x86_64": ..., ...}
history.json                   ["2026-09-28-r3", ..., "2026-09-29-r10"]: every build folder, oldest first
2026-09-29.r10/
  build.json                   version, commit, changes, and per platform: file, size, sha256, signature
  build.json.sig               ed25519 signature of build.json, hex
  build.json.minisig           and a minisign signature beside every file but build.json.sig
  Snowbound-2026-09-29-r10-macos-aarch64.zip
  Snowbound-2026-09-29-r10-macos-x86_64.zip
  Snowbound-2026-09-29-r10-macos-10.6.zip
  snowbound-2026-09-29-r10-linux-x86_64         the executable itself
  snowbound-2026-09-29-r10-linux-aarch64
  snowbound-2026-09-29-r10-windows-x86_64.exe
  snowbound-2026-09-29-r10-windows-aarch64.exe
  Snowbound-2026-09-29-r10-macos-aarch64.dSYM.zip    debug info, optional: one per archive
  snowbound-2026-09-29-r10-linux-x86_64.debug.zip
  snowbound-2026-09-29-r10-windows-x86_64.debug.zip
  ...
latest/                        each platform's newest archive, the version dropped from its name
  Snowbound-macos-aarch64.zip
  Snowbound-macos-aarch64.zip.minisig
  ...
```

A build folder is written once, under a hidden `.2026-09-29.r10.partial` name renamed into
place, and never changed or deleted. `history.json` and then `latest.json` are
replaced last, each through a rename, and `latest.json` only moves a platform
forward. Neither carries a signature: the trust is in the immutable
`build.json`, whose version the app checks against the one it was pointed to,
and a forged pointer can only name another signed build, which the app ignores
unless it is newer than itself. `latest.json` stays a map of platform to
version, which is all apps before `history.json` read.

## What a build changes

`build.json`'s `changes` lists, oldest first, every change the commits on
`main` after the build published before it, of any platform, up to this one
bring: `{"version": "2026-09-30-r12", "kind": "feature", "title": "Pinch zoom"}`,
where `version` is the version of the commit that made it. Builds published
before `history.json` list every change since the first published build
(`FIRST` in `release.py`, where a release still starts if the share holds no
build). The list is in `build.json` because that is signed.

An updating app reads the `build.json` of each build `history.json` names after
its own, up to the newest for its platform, in parallel, and sums their
changes, so releases it skipped count too, whichever platforms they built. A
commit's entries count once, from the newest build listing them, so builds
listing every change since the first don't count one twice. It says "3
features, 5 bug fixes, and 2 other changes" with the titles beneath. It reads
at most 20 builds, the newest included; past them, or where a build's
`build.json` is missing or its signature doesn't hold (it is skipped), or with
no `history.json`, it says "3 features, 5 bug fixes, and more" and ends the
list with "and more". `history.json` only says which folders to read: a build
it names counts only once its `build.json` verifies, and a forged one can only
hide changes from the list.

A commit counts by its conventional prefix: `feat` is a feature, `fix` a bug
fix, anything else (`docs`, `chore`, no prefix) another change. It counts once,
titled by its subject without the prefix, unless its body has a top-level
bulleted list (`- ` or `* ` at the start of a line, wrapped lines indented),
which counts each item instead, all of the prefix's kind. So a fix is best its
own small `fix:` commit, and a batch commit should bullet what it brings.

Apps ignore fields and files they don't know, and builds without `changes`
read as listing none. Apps before `history.json` sum the entries in the newest
build's `build.json` newer than themselves, so they list only that build's
changes.

## Signing

Every `build.json` and archive is signed with the ed25519 release key in
`~/.config/snowbound/release-key` (PKCS#8, mode 600, never in the repository).
Its public half is `minisign.pub` at the repository's root, in minisign's
format, which the app compiles in.
`cargo run -p snowbound --example release_sign -- KEY FILE...` prints
signatures and refuses a key that doesn't match `minisign.pub`;
`release_sign new KEY` makes a new key and prints its `minisign.pub`. Replacing
the key means shipping a build with the new public half, signed with the old
key.

Every published file but `build.json.sig` also gets `FILE.minisig`, which
[minisign](https://jedisct1.github.io/minisign/) (or `rsign verify`) checks:

```sh
minisign -Vm Snowbound-macos-aarch64.zip -P RWRa9nZujiIE7lr2dm6OIgTuUvMpr09SM747BkGcHfD4x9ghErMdNGsJ
```

minisign is Ed25519, so the release key makes these too, with no second key to
guard: `release.py` has `release_sign` sign each file's BLAKE2b-512, then that
signature followed by the trusted comment, as `minisign -S` does, and the key id
is the public key's first 8 bytes. Neither scheme's signature passes for the
other's: what minisign signs is a 64-byte hash, or a signature and a comment,
never a `build.json` or an archive with the hash `build.json` lists.

The app reads the archive's size and SHA-256 from the signed `build.json`.
Each archive's `signature` there, the release key's of its raw bytes, is for
apps that predate that, which check it as well.

The macOS app is signed with Clover's Developer ID Application certificate
(team 9R7DPNW28H), named in `release.py` by its SHA-1 hash, since its name is
the account holder's legal name, which nothing here prints or stores.
`build_macos.py --sign developer-id` signs it, embedding the Developer ID
provisioning profile for `net.paperclover.snowbound` ("Snowbound Developer ID",
found where Xcode keeps profiles) as `Contents/embedded.provisionprofile`, with
hardened runtime, a secure timestamp, the production iCloud container
`iCloud.net.paperclover.snowbound` that iCloud notebooks need, and the
microphone and camera entitlements recording needs. It fails rather than fall
back to ad hoc. `--ad-hoc` signs ad hoc instead, without iCloud; the 10.6 bundle
stays unsigned, as it predates Developer ID. codesign fails with
`errSecInternalComponent` where it can't ask to use the private key, as from an
agent's shell; `security set-key-partition-list -S apple-tool:,apple:,codesign:
-s -k PASSWORD ~/Library/Keychains/login.keychain-db` lets it sign without
asking.

Notarization runs when `~/.config/snowbound/notary.json` names an App Store
Connect API key that signs in (`{"key": "~/.config/snowbound/AuthKey_ID.p8",
"key_id": "ID", "issuer": "ISSUER"}`, mode 600), and is skipped with a note
otherwise. A notarytool keychain profile would do, but storing one fails from an
agent's shell, where the keychain refuses new items.

```sh
xcrun notarytool store-credentials snowbound --key AuthKey_KEYID.p8 --key-id KEYID --issuer ISSUER-UUID
```

or with an app-specific password from account.apple.com:
`xcrun notarytool store-credentials snowbound --apple-id EMAIL --team-id 9R7DPNW28H --password APP-SPECIFIC-PASSWORD`.
Until the app is notarized, a download opened in Finder needs Open from its
context menu the first time; updates the app installs itself carry no
quarantine and open directly. The zips carry their `.minisig` like every
download, which for the unsigned 10.6 app is the only signature.

Windows executables are unsigned, so SmartScreen warns on a download.
Authenticode would take a code signing certificate: Azure Trusted Signing at
about $10 a month, where it accepts an individual developer, or an OV
certificate at a few hundred dollars a year, now kept on a hardware token or
cloud HSM. `osslsigncode` or `jsign` would sign from the Mac.

## Publishing

```sh
python3 tools/release.py              # all seven platforms
python3 tools/release.py --ad-hoc     # the macOS app without Developer ID
python3 tools/release.py --dry-run    # the same into a new temporary folder
python3 tools/release.py --platforms macos-aarch64 linux-x86_64
```

The script refuses to run unless the working copy matches `main` (a dry run
only warns), gates the commit with `tools/ci.py` (see `tools/TESTING.md`),
builds each platform with
`tools/canvas/build_macos.py` (`--snow-leopard` for 10.6),
`crates/snowbound/linux/package.sh` (taking only its executables) and
`platform/windows/cargo.sh`, then checks
the working copy didn't change meanwhile. It zips the apps with `ditto`, hashes and signs everything, and
publishes as above. Run again for the same commit, it only brings
`history.json` and `latest.json` up to date; a different commit that derives
the same version is refused. The 10.6 build needs the SDK and nightly toolchain
`platform/snow-leopard/cargo.sh` names; the Linux builds need `zig`, as the
cross linker against glibc 2.17; the Windows builds need llvm-mingw, which
`platform/windows/toolchain.sh` fetches, and nightly with `rust-src` for
x86_64. All seven build from an Apple silicon Mac.

## Minimum systems

Each build runs as far back as its dependencies allow, and newer calls are
checked at run time (`respondsToSelector:`, `available!`, `dlsym`).

| Platform | Oldest system | Set by |
| --- | --- | --- |
| `macos-aarch64` | macOS 11 | Apple silicon's first; rustc's default |
| `macos-x86_64` | macOS 10.13 | wgpu's Metal floor; `MACOSX_DEPLOYMENT_TARGET` |
| `macos-10.6` | Mac OS X 10.6 | its own SDK, runtime shims and OpenGL renderer |
| `linux-*` | glibc 2.17 (RHEL 7, Debian 8, Ubuntu 14.04) | Rust's floor; zig's glibc target |
| `windows-x86_64` | Windows 7 SP1 | nightly's `x86_64-win7-windows-gnu` |
| `windows-aarch64` | Windows 11 on Arm | `aarch64-pc-windows-gnullvm` |

`build_macos.py` writes the minimum as `LSMinimumSystemVersion`; the binary
carries it as `LC_BUILD_VERSION` `minos` (`LC_VERSION_MIN_MACOSX` below 10.14),
which `otool -l` shows. `objdump -T` of a Linux executable names no `GLIBC_`
version above 2.17. The Linux executable links only libc, libm, libpthread and
libdl; Wayland, X11, xkbcommon, EGL, Vulkan, fontconfig, GStreamer and Enchant
are loaded at run time.

## Symbols and frame pointers

Every executable keeps its symbol table, so backtraces, crash logs, `perf`, gdb
and Instruments name functions; Cargo's release profile strips only debug info
(`platform/linux/cc.sh` does that itself, as zig's linker would strip both).
`.cargo/config.toml` builds every target with frame pointers, and Rust's
standard library ships with them. The symbols cost about a fifth: Linux x86_64
grows from 53 to 62 MB, Windows x86_64 from 55 to 72 MB, and the macOS app
already carried them.

The release builds with line tables, then moves them out of each executable
into the build folder's zipped symbol files, which neither `latest.json` nor
`build.json` names, so the app never downloads them: a dSYM for each Mac
(matched by its UUID; `build_macos.py --dsym`), and for Linux and Windows a
DWARF `.debug` file, which the executable names in its `.gnu_debuglink` (and on
Linux by its build id). Unzipped beside the executable, or the dSYM beside the
app, they give lldb, gdb, `perf`, Instruments and `llvm-symbolizer` files,
lines and inlined calls. They run about 35 MB zipped per Mac, 55 MB per Linux
and 40 MB per Windows architecture. Windows gets DWARF rather than a PDB: rustc
emits CodeView only for MSVC targets, and lld builds a PDB's functions and lines
from CodeView alone, so from these DWARF objects its PDB holds only the global
symbols, which few Rust functions are; WPA and Visual Studio see no names.

glibc's `backtrace_symbols_fd` reads only dynamic symbols, so for a signal
Linux's crash log starts the executable again with `--symbolize` to name its
frames from the symbol table.

## In the app

`update.rs` runs one thread. With automatic checks on (Options, General) and a
published build, it checks shortly after launch and then daily, skipping while
Work Offline is on; Check for Updates… (the app menu on macOS, the command
palette elsewhere) checks at once and reports what it found. A check reads
`latest.json`, then the named build's `build.json` and signature, then
`history.json` and the `build.json` of each build since its own, and
downloads the archive for this platform, verifying its size and SHA-256
against the signed `build.json` before unpacking it. An Intel build that Rosetta runs takes `macos-aarch64`'s, even at
its own version. It stages the update beside the install, so the swap is a rename:
the app unpacked into `.Snowbound.app.update` next to the bundle on macOS, the
executable into `.snowbound.update` next to it on Linux (`.snowbound.exe.update` on
Windows). The sync status icon gets a dot, and its popup says what the update
changes, unfolding to the titles, and offers Build Folder and Restart to
Update; Check for Updates… lists them in its dialog.

Restart to Update quits the app and starts the old executable with
`--finish-update`, which waits for the app to exit, renames the old bundle (or
executable) aside and the new one into its place, and opens it. Windows renames an
executable while it runs, as this one does, but won't delete it: the old one waits beside
the new as `.snowbound.exe.old` until the next update removes it. Where the install can't be
written, and on Mac OS X 10.6, the popup and the check name the build and
open its folder to download instead.
