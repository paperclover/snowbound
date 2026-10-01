#!/usr/bin/env python3
"""Builds, signs and publishes Snowbound for each desktop platform; see tools/RELEASE.md."""
import argparse
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import re
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
PLATFORMS = ['macos-aarch64', 'macos-x86_64', 'macos-10.6', 'linux-x86_64', 'linux-aarch64',
             'windows-x86_64', 'windows-aarch64']
# Windows 7 to 11 on x86_64 (nightly's tier-3 win7 target), and Windows 11 on Arm.
WINDOWS = {'x86_64': 'x86_64-win7-windows-gnu', 'aarch64': 'aarch64-pc-windows-gnullvm'}
# Clover's Developer ID Application certificate, by its SHA-1 hash: its name is the account
# holder's legal name, which nothing here prints or stores.
IDENTITY = 'BA308AA3591299E053E8824CEF1651F686F8908E'
# The App Store Connect API key that notarizes it: {"key": P8 PATH, "key_id": ID, "issuer": ID}.
NOTARY = Path('~/.config/snowbound/notary.json').expanduser()
# The first published build's commit: no client runs anything older, so changes start after it.
FIRST = '354f001dec3d731a4d1a6fac0a25d9e28550d781'
KINDS = {'feat': 'feature', 'fix': 'fix'}


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


def history():
    """Every commit on `main` by id: its parents' ids, when it was made, and its message."""
    template = ('commit_id ++ "\\x1f" ++ parents.map(|parent| parent.commit_id()).join(" ") ++ "\\x1f" ++ '
                'committer.timestamp().utc().format("%Y-%m-%dT%H:%M:%S+00:00") ++ "\\x1f" ++ description ++ "\\x1e"')
    commits = {}
    for record in jj('log', '--no-graph', '-r', '::main', '-T', template).split('\x1e')[:-1]:
        commit, parents, made, description = record.split('\x1f')
        commits[commit] = (parents.split(), datetime.fromisoformat(made), description)
    return commits


def ancestors(commits, commit):
    """`commit` and every commit before it."""
    found, stack = set(), [commit]
    while stack:
        commit = stack.pop()
        if commit not in found:
            found.add(commit)
            stack.extend(commits[commit][0])
    return found


def version_of(commits, commit):
    return derive(commits[commit][1], [commits[each][1] for each in ancestors(commits, commit)])


def entries(description):
    """What a commit brings, as (kind, title): one entry of its prefix's kind, or one per item where
    its body has a top-level bulleted list, each of the prefix's kind."""
    subject, _, body = description.strip().partition('\n')
    prefix = re.match(r'(\w+)(\([^)]*\))?!?:\s*', subject)
    kind = KINDS.get(prefix[1].lower(), 'other') if prefix else 'other'
    items, open_item = [], False
    for line in body.splitlines():
        if line.startswith(('- ', '* ')):
            items.append(line[2:].strip())
            open_item = True
        elif open_item and line[:1].isspace() and line.strip():
            items[-1] += ' ' + line.strip()
        else:
            open_item = False
    titles = [title.rstrip('.') for title in items or [subject[prefix.end():] if prefix else subject]]
    # Capitalized as a sentence, except a word like macOS or iCloud.
    return [(kind, title if re.match(r'\S+[A-Z]', title) else title[:1].upper() + title[1:])
            for title in titles if title]


def changes(commits, commit):
    """Every entry the commits after FIRST up to `commit` bring, oldest first, each with the version
    of the commit that brought it."""
    versions = {each: version_of(commits, each)
                for each in ancestors(commits, commit) - ancestors(commits, FIRST)}
    return [{'version': name(versions[each]), 'kind': kind, 'title': title}
            for each in sorted(versions, key=versions.get) for kind, title in entries(commits[each][2])]


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
    """The zipped app, which build_macos.py signs; 10.6's stays unsigned, as it predates Developer ID."""
    bundle = folder / 'Snowbound.app'
    if platform == 'macos-10.6':
        signing = ['--snow-leopard']
    elif developer_id:
        signing = ['--sign', 'developer-id', '--sign-identity', IDENTITY]
    else:
        signing = ['--sign', 'ad-hoc']
    if platform != 'macos-10.6':
        signing += ['--arch', platform.removeprefix('macos-')]
    run([sys.executable, ROOT / 'tools/canvas/build_macos.py', '--release', '--output', bundle, *signing])
    archive = folder / 'archive.zip'
    if notarize and platform != 'macos-10.6':
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


def build_windows(architectures):
    """The executables, each all of Snowbound for its architecture, one running on every
    Windows it supports."""
    built = {}
    for arch in architectures:
        run(['sh', ROOT / 'platform/windows/cargo.sh', arch, 'build', '--release', '-p', 'snowbound'],
            env={**os.environ, 'CARGO_PROFILE_RELEASE_STRIP': 'symbols'})
        built[f'windows-{arch}'] = ROOT / f'target/windows/{WINDOWS[arch]}/release/snowbound.exe'
    return built


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dry-run', action='store_true',
                        help='Publish into a new temporary folder instead, even from a changed working copy')
    parser.add_argument('--platforms', nargs='+', choices=PLATFORMS, default=PLATFORMS)
    parser.add_argument('--ad-hoc', action='store_true',
                        help='Sign the macOS app ad hoc instead of with Developer ID, unnotarized')
    args = parser.parse_args()
    developer_id = not args.ad_hoc and any(platform in ('macos-aarch64', 'macos-x86_64') for platform in args.platforms)
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
    commits = history()
    version = version_of(commits, commit)
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
        run([sys.executable, ROOT / 'tools/ci.py', '--rev', commit])
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
        windows = [platform.removeprefix('windows-') for platform in args.platforms if platform.startswith('windows')]
        if windows:
            built |= build_windows(windows)
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
            'changes': changes(commits, commit),
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
            # copy() keeps the Linux executables executable for anyone running them off the share.
            shutil.copy(file, partial / file.name)
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
    # Stable names for the readme's download links, always the newest build of each platform.
    downloads = published / 'latest'
    downloads.mkdir(exist_ok=True)
    for platform, newest_name in moved.items():
        if newest_name != name(version):
            continue
        file = build['archives'][platform]['file']
        stable = downloads / file.replace(f'-{name(version)}', '')
        partial = stable.with_name(f'.{stable.name}.partial')
        shutil.copy(target / file, partial)
        os.replace(partial, stable)
    if not args.dry_run:
        print(f'{URL}{folder(version)}/')


if __name__ == '__main__':
    main()
