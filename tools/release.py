#!/usr/bin/env python3
"""Builds, signs and publishes Snowbound for each desktop platform; see tools/RELEASE.md."""
import argparse
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parents[1]
PUBLISHED = Path('/Volumes/clover/Documents/Public/Snowbound')
URL = 'https://file.paperclover.net/shr/snowbound/'
KEY = Path.home() / '.config/snowbound/release-key'
ZONE = ZoneInfo('America/Los_Angeles')
PLATFORMS = ['macos-aarch64', 'macos-10.6', 'linux-x86_64', 'linux-aarch64']
CHECKS = [
    ['cargo', 'clippy', '--workspace', '--all-targets', '--all-features', '--', '-D', 'warnings'],
    ['cargo', 'clippy', '-p', 'snowbound', '--no-default-features', '--', '-D', 'warnings'],
    ['cargo', 'test', '--workspace', '--all-features'],
]
# Clover's Developer ID Application certificate, by its SHA-1 hash: its name is the account
# holder's legal name, which nothing here prints or stores.
IDENTITY = 'BA308AA3591299E053E8824CEF1651F686F8908E'
# The App Store Connect API key that notarizes it: {"key": P8 PATH, "key_id": ID, "issuer": ID}.
NOTARY = Path('~/.config/snowbound/notary.json').expanduser()
# What hardened runtime needs for Record Audio and Record Video.
ENTITLEMENTS = {
    'com.apple.security.device.audio-input': True,
    'com.apple.security.device.camera': True,
}


def derive(release, commits):
    """The version of the commit made at `release`, given when each commit leading to it was
    made, itself included: its day in Los Angeles, and how many of those were made that day."""
    day = release.astimezone(ZONE).date()
    return day.isoformat(), sum(1 for made in commits if made.astimezone(ZONE).date() == day)


def name(version):
    return f'{version[0]}-r{version[1]}'


def folder(version):
    """The build's top-level folder."""
    return f'{version[0]}.r{version[1]}'


def parse(text):
    date, revision = text.split('-r')
    return date, int(revision)


def newest(latest, archives, version):
    """`latest.json`'s map with each of `archives`' platforms moved to `version` where that is
    newer than what it names."""
    return {**latest, **{platform: name(version) for platform in archives
                         if platform not in latest or parse(latest[platform]) < version}}


def jj(*args):
    return subprocess.check_output(['jj', *args], cwd=ROOT, text=True)


def clean(dry_run, moment):
    """Refuses to go on, or in a dry run warns, where the working copy isn't `main`."""
    changed = jj('diff', '--from', 'main', '--to', '@', '--summary').strip()
    if changed and not dry_run:
        sys.exit(f'The working copy differs from main {moment}:\n{changed}')
    if changed:
        print(f'Dry run: the working copy differs from main {moment}.', file=sys.stderr)


def run(command, **kwargs):
    print('+', ' '.join(map(str, command)), flush=True)
    subprocess.run(command, cwd=ROOT, check=True, **kwargs)


def commit_times(revset):
    template = 'committer.timestamp().utc().format("%Y-%m-%dT%H:%M:%S+00:00") ++ "\\n"'
    return [datetime.fromisoformat(line) for line in jj('log', '--no-graph', '-r', revset, '-T', template).split()]


def sign(files):
    output = subprocess.check_output(
        ['cargo', 'run', '--quiet', '--release', '-p', 'snowbound', '--example', 'release_sign', '--', KEY, *files],
        cwd=ROOT, text=True)
    return output.split()


def zip_bundle(bundle, archive):
    run(['ditto', '-c', '-k', '--norsrc', '--noextattr', '--noqtn', '--noacl', '--keepParent', bundle, archive])


def notary():
    """notarytool's credential arguments from NOTARY, if they sign in."""
    if not NOTARY.exists():
        return None
    key = json.loads(NOTARY.read_text())
    arguments = ['--key', str(Path(key['key']).expanduser()), '--key-id', key['key_id'], '--issuer', key['issuer']]
    signs_in = subprocess.run(['xcrun', 'notarytool', 'history', *arguments], capture_output=True).returncode == 0
    return arguments if signs_in else None


def build_mac(platform, folder, developer_id, notarize):
    """The zipped app; 10.6's stays unsigned, as it predates Developer ID."""
    bundle = folder / 'Snowbound.app'
    run([sys.executable, ROOT / 'tools/canvas/build_macos.py', '--release', '--output', bundle]
        + (['--snow-leopard'] if platform == 'macos-10.6' else []))
    archive = folder / 'archive.zip'
    if developer_id and platform != 'macos-10.6':
        entitlements = folder / 'entitlements.plist'
        entitlements.write_bytes(plistlib.dumps(ENTITLEMENTS))
        run(['codesign', '--force', '--options', 'runtime', '--timestamp', '--entitlements', entitlements,
             '--sign', IDENTITY, bundle])
        if notarize:
            zip_bundle(bundle, archive)
            run(['xcrun', 'notarytool', 'submit', archive, *notarize, '--wait'])
            run(['xcrun', 'stapler', 'staple', bundle])
            archive.unlink()
    zip_bundle(bundle, archive)
    return archive


def build_linux(architectures):
    """The executables, each all of Snowbound for its architecture."""
    run(['sh', ROOT / 'crates/snowbound/linux/package.sh', *architectures])
    return {f'linux-{arch}': ROOT / f'target/{arch}-unknown-linux-gnu/release/snowbound' for arch in architectures}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dry-run', action='store_true',
                        help='Publish into a new temporary folder instead, even from a changed working copy')
    parser.add_argument('--platforms', nargs='+', choices=PLATFORMS, default=PLATFORMS)
    parser.add_argument('--ad-hoc', action='store_true',
                        help='Sign the macOS app ad hoc instead of with Developer ID, unnotarized')
    args = parser.parse_args()
    developer_id = not args.ad_hoc and any(platform == 'macos-aarch64' for platform in args.platforms)
    identities = subprocess.run(['security', 'find-identity', '-v', '-p', 'codesigning'],
                                capture_output=True, text=True).stdout
    if developer_id and IDENTITY not in identities:
        sys.exit(f'The keychain has no signing identity {IDENTITY}; release with --ad-hoc, or add it.')
    notarize = notary() if developer_id else None
    if developer_id and not notarize:
        print(f'Not notarizing: no API key in {NOTARY} that signs in (see tools/RELEASE.md).',
              file=sys.stderr)

    commit = jj('log', '--no-graph', '-r', 'main', '-T', 'commit_id').strip()
    clean(args.dry_run, 'to release it')
    version = derive(commit_times('main')[0], commit_times('::main'))
    print(f'Snowbound build {version[0]} revision {version[1]}, commit {commit}', flush=True)

    published = Path(tempfile.mkdtemp(prefix='snowbound-release-')) if args.dry_run else PUBLISHED
    if not published.is_dir():
        sys.exit(f'{published} is not mounted.')
    target = published / folder(version)
    if target.exists():
        build = json.loads((target / 'build.json').read_text())
        if build['commit'] != commit:
            sys.exit(f'{target} holds commit {build["commit"]}, not {commit}.')
        print(f'{target} is already published.')
    else:
        for check in CHECKS:
            run(check)
        clean(args.dry_run, 'after the checks')
        stage = ROOT / 'target/release-stage' / name(version)
        shutil.rmtree(stage, ignore_errors=True)
        stage.mkdir(parents=True)
        # The app reads its version from this as it compiles.
        os.environ['SNOWBOUND_BUILD'] = name(version)
        built = {}
        for platform in args.platforms:
            if platform.startswith('macos'):
                work = stage / platform
                work.mkdir()
                built[platform] = build_mac(platform, work, developer_id, notarize)
        linux = [platform.removeprefix('linux-') for platform in args.platforms if platform.startswith('linux')]
        if linux:
            built |= build_linux(linux)
        clean(args.dry_run, 'after the build')
        files = {}
        for platform, source in built.items():
            prefix = 'Snowbound' if platform.startswith('macos') else 'snowbound'
            files[platform] = stage / f'{prefix}-{name(version)}-{platform}{source.suffix}'
            shutil.copy2(source, files[platform])
        signatures = sign(files.values())
        build = {
            'version': name(version),
            'commit': commit,
            'published': datetime.now(ZONE).isoformat(timespec='seconds'),
            'archives': {platform: {
                'file': file.name,
                'size': file.stat().st_size,
                'sha256': hashlib.sha256(file.read_bytes()).hexdigest(),
                'signature': signature,
            } for (platform, file), signature in zip(files.items(), signatures)},
        }
        (stage / 'build.json').write_text(json.dumps(build, indent=2) + '\n')
        (stage / 'build.json.sig').write_text(sign([stage / 'build.json'])[0] + '\n')
        partial = target.with_name(f'.{target.name}.partial')
        shutil.rmtree(partial, ignore_errors=True)
        partial.mkdir()
        for file in [*files.values(), stage / 'build.json', stage / 'build.json.sig']:
            shutil.copyfile(file, partial / file.name)
        partial.rename(target)
        shutil.rmtree(stage)
        print(f'Published {target}')

    latest_file = published / 'latest.json'
    latest = json.loads(latest_file.read_text()) if latest_file.exists() else {}
    build = json.loads((target / 'build.json').read_text())
    moved = newest(latest, build['archives'], version)
    if moved != latest:
        partial = latest_file.with_name('.latest.json.partial')
        partial.write_text(json.dumps(moved, indent=2, sort_keys=True) + '\n')
        os.replace(partial, latest_file)
    print(f'{latest_file}: {json.dumps(moved, sort_keys=True)}')
    if not args.dry_run:
        print(f'{URL}{folder(version)}/')


if __name__ == '__main__':
    main()
