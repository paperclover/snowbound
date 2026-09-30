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
latest.json                    {"macos-aarch64": "2026-09-29-r10", "macos-10.6": ..., "linux-x86_64": ..., "linux-aarch64": ...}
2026-09-29.r10/
  build.json                   version, commit, and per platform: file, size, sha256, signature
  build.json.sig               ed25519 signature of build.json, hex
  Snowbound-2026-09-29-r10-macos-aarch64.zip
  Snowbound-2026-09-29-r10-macos-10.6.zip
  snowbound-2026-09-29-r10-linux-x86_64         the executable itself
  snowbound-2026-09-29-r10-linux-aarch64
```

A build folder is written once, under a hidden `.2026-09-29.r10.partial` name renamed into
place, and never changed or deleted. `latest.json` is replaced last, through a
rename, and only moves a platform forward. It carries no signature: the trust is
in the immutable `build.json`, whose version the app checks against the one it
was pointed to, and a forged pointer can only name another signed build, which
the app ignores unless it is newer than itself.

## Signing

Every `build.json` and archive is signed with the ed25519 release key in
`~/.config/snowbound/release-key` (PKCS#8, mode 600, never in the repository).
Its public half is `crates/snowbound/release-key.pub`, compiled into the app.
`cargo run -p snowbound --example release_sign -- KEY FILE...` prints
signatures and refuses a key that doesn't match `release-key.pub`;
`release_sign new KEY` makes a new key. Replacing the key means shipping a
build with the new public half, signed with the old key.

The macOS app is signed with Clover's Developer ID Application certificate
(team 9R7DPNW28H), named in `release.py` by its SHA-1 hash, since its name is
the account holder's legal name, which nothing here prints or stores.
`build_macos.py --sign developer-id` signs it, embedding the Developer ID
provisioning profile for `net.paperclover.snowbound` ("Snowbound Developer ID",
found where Xcode keeps profiles) as `Contents/embedded.provisionprofile`, with
hardened runtime, a secure timestamp, the production iCloud container
`iCloud.net.paperclover.snowbound` that Use iCloud Drive needs, and the
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
quarantine and open directly.

## Publishing

```sh
python3 tools/release.py              # all four platforms
python3 tools/release.py --ad-hoc     # the macOS app without Developer ID
python3 tools/release.py --dry-run    # the same into a new temporary folder
python3 tools/release.py --platforms macos-aarch64 linux-x86_64
```

The script refuses to run unless the working copy matches `main` (a dry run
only warns), runs Clippy (workspace, and `snowbound` without default features)
and the workspace tests, builds each platform with
`tools/canvas/build_macos.py` (`--snow-leopard` for 10.6) and
`crates/snowbound/linux/package.sh` (taking only its executables), then checks
the working copy didn't change meanwhile. It zips the apps with `ditto`, hashes and signs everything, and
publishes as above. Run again for the same commit, it only brings
`latest.json` up to date; a different commit that derives the same version is
refused. The 10.6 build needs the SDK and nightly toolchain
`platform/snow-leopard/cargo.sh` names; the Linux builds need `zig`, as the
cross linker against glibc 2.31. All four build from an Apple silicon Mac.

## In the app

`update.rs` runs one thread. With automatic checks on (Options, General) and a
published build, it checks shortly after launch and then daily, skipping while
Work Offline is on; Check for Updates… (the app menu on macOS, the command
palette elsewhere) checks at once and reports what it found. A check reads
`latest.json`, then the named build's `build.json` and signature, and
downloads the archive for this platform, verifying size, SHA-256 and
signature. It stages the update beside the install, so the swap is a rename:
the app unpacked into `.Snowbound.app.update` next to the bundle on macOS, the
executable into `.snowbound.update` next to it on Linux. The sync status icon gets a dot, and its popup offers
Build Folder and Restart to Update.

Restart to Update quits the app and starts the old executable with
`--finish-update`, which waits for the app to exit, renames the new bundle (or
executable) over the old one, and opens it. Where the install can't be
written, and on Mac OS X 10.6, the popup and the check name the build and
open its folder to download instead.
