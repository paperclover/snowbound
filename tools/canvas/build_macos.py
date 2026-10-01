#!/usr/bin/env python3
"""Build the local macOS canvas bundle, or with --snow-leopard the Mac OS X 10.6 one."""
import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--release', action='store_true')
parser.add_argument('--output', type=Path, help='Create a separate bundle at a new .app path')
parser.add_argument('--bundle-id', help="Bundle identifier for the separate app; the app's own otherwise")
host = 'aarch64' if platform.machine() == 'arm64' else 'x86_64'
parser.add_argument('--arch', choices=['aarch64', 'x86_64'], default=host, help="The app's architecture; this Mac's by default")
parser.add_argument('--snow-leopard', action='store_true',
                    help='Build with platform/snow-leopard/cargo.sh into target/snow-leopard')
parser.add_argument('--sign-identity', metavar='SHA1',
                    help="A Developer ID Application certificate's SHA-1 hash; found in the keychain otherwise")
parser.add_argument('--profile', type=Path,
                    help='A Developer ID provisioning profile for the app; found where Xcode keeps them otherwise')
parser.add_argument('--sign', choices=['developer-id', 'ad-hoc'],
                    help='Require Developer ID with the iCloud container, or sign ad hoc; Developer ID where available otherwise')
args = parser.parse_args()
if args.bundle_id and not args.output:
    parser.error('Use --bundle-id with --output.')
if args.sign and args.snow_leopard:
    parser.error('The 10.6 bundle stays unsigned.')
# The oldest macOS each build runs on: 10.6's own build, wgpu's floor on Intel, and Apple
# silicon's first, which rustc also defaults to.
minimum = '10.6' if args.snow_leopard else {'x86_64': '10.13', 'aarch64': '11.0'}[args.arch]
if args.output and (args.output.suffix != '.app' or args.output.exists()):
    parser.error('Choose a new output path ending in .app.')
root = Path(__file__).resolve().parents[2]
# The Apple Developer team and what is registered under it. With the team's Developer ID
# Application identity and a Developer ID profile for BUNDLE_ID naming CONTAINER, the app is
# signed with hardened runtime and the iCloud container (iCloud notebooks, NSUbiquitousContainers);
# without them, ad hoc, and iCloud Drive works through folders the user picks.
TEAM = '9R7DPNW28H'
BUNDLE_ID = 'net.paperclover.snowbound'
CONTAINER = 'iCloud.net.paperclover.snowbound'
# OneNote's sections, tables of contents and packages, which no Mac app declares; OneNote may open them too.
ONENOTE_TYPES = [('com.microsoft.onenote.section', 'OneNote Section', 'one'),
                 ('com.microsoft.onenote.table-of-contents', 'OneNote Table of Contents', 'onetoc2'),
                 ('com.microsoft.onenote.package', 'OneNote Package', 'onepkg')]


def developer_id():
    """The team's Developer ID Application identity by SHA-1 hash. The listing names the
    certificate's holder, so none of it is printed."""
    listing = subprocess.run(['security', 'find-identity', '-v', '-p', 'codesigning'],
                             capture_output=True, text=True).stdout
    for line in listing.splitlines():
        if 'Developer ID Application:' in line and f'({TEAM})' in line:
            found = re.search(r'\b[0-9A-F]{40}\b', line)
            if found:
                return found.group(0)
    return None


def profile_entitlements(path):
    decoded = subprocess.run(['security', 'cms', '-D', '-i', str(path)], capture_output=True)
    if decoded.returncode != 0:
        return None
    profile = plistlib.loads(decoded.stdout)
    entitlements = profile.get('Entitlements', {})
    fits = (entitlements.get('com.apple.application-identifier') == f'{TEAM}.{BUNDLE_ID}'
            and CONTAINER in entitlements.get('com.apple.developer.icloud-container-identifiers', [])
            and 'ProvisionedDevices' not in profile
            and profile.get('ExpirationDate', datetime.max) > datetime.now())
    return entitlements if fits else None


def developer_id_profile():
    """A Developer ID provisioning profile for the app and its iCloud container."""
    folders = [Path.home() / 'Library/Developer/Xcode/UserData/Provisioning Profiles',
               Path.home() / 'Library/MobileDevice/Provisioning Profiles']
    for path in sorted(path for folder in folders if folder.is_dir()
                       for path in folder.glob('*.provisionprofile')):
        if profile_entitlements(path):
            return path
    return None
profile = ['--release'] if args.release else []
if args.snow_leopard:
    subprocess.run([root / 'platform/snow-leopard/cargo.sh', 'build', '-p', 'snowbound', '--no-default-features'] + profile,
                   cwd=root, check=True)
    target = root / 'target/snow-leopard'
    built = target / 'x86_64-apple-macosx10.6'
else:
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--format-version=1', '--no-deps'], cwd=root))
    target = built = Path(metadata['target_directory'])
    cross = [] if args.arch == host else ['--target', f'{args.arch}-apple-darwin']
    if cross:
        built = target / cross[1]
    subprocess.run(['cargo', 'build', '-p', 'snowbound'] + profile + cross, cwd=root, check=True,
                   env={**os.environ, 'MACOSX_DEPLOYMENT_TARGET': minimum} if args.arch == 'x86_64' else None)
bundle = args.output.resolve() if args.output else target / 'Snowbound.app'
if args.output:
    bundle.mkdir(parents=True, exist_ok=False)
binary = bundle / 'Contents/MacOS/Snowbound'
binary.parent.mkdir(parents=True, exist_ok=True)
pending = binary.with_suffix('.next')
shutil.copy2(built / ('release' if args.release else 'debug') / 'snowbound', pending)
pending.replace(binary)
icons = root / 'crates/snowbound/assets/icon'
resources = bundle / 'Contents/Resources'
resources.mkdir(exist_ok=True)
if args.snow_leopard:
    # 10.6 reads 256 and 512 pixel entries (ic08, ic09), which the checked-in icns lacks.
    with tempfile.TemporaryDirectory() as scratch:
        iconset = Path(scratch) / 'Snowbound.iconset'
        iconset.mkdir()
        for side in [16, 32, 128, 256, 512]:
            subprocess.run(['sips', '-z', str(side), str(side), icons / 'Snowbound-SnowLeopard.png',
                            '--out', iconset / f'icon_{side}x{side}.png'], check=True, capture_output=True)
        subprocess.run(['iconutil', '-c', 'icns', iconset, '-o', resources / 'Snowbound.icns'], check=True)
else:
    # macOS 26 draws the Liquid Glass icon from the catalog; it holds no flattened renditions
    # (see the icon folder's README), so earlier versions fall back to the icns.
    shutil.copy2(icons / 'Snowbound-Sequoia.icns', resources / 'Snowbound.icns')
    shutil.copy2(icons / 'Assets.car', resources)
# tools/release.py names the build, as 2026-09-29-r10, which About shows.
build = os.environ.get('SNOWBOUND_BUILD')
versions = {}
if build:
    date, revision = build.split('-r')
    versions = {'CFBundleShortVersionString': f'{date} revision {revision}',
                'CFBundleVersion': f'{date.replace("-", "")}.{revision}'}
(bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleExecutable': binary.name,
    'CFBundleIdentifier': args.bundle_id or BUNDLE_ID,
    'CFBundleName': bundle.stem,
    'CFBundleDisplayName': bundle.stem,
    'CFBundlePackageType': 'APPL',
    'CFBundleVersion': '1',
    'NSHighResolutionCapable': True,
    'NSPrincipalClass': 'NSApplication',
    'CFBundleIconFile': 'Snowbound',
    'CFBundleIconName': 'Snowbound-Tahoe',
    'NSLocalNetworkUsageDescription': 'Snowbound connects to file servers on your network to open and sync shared notebooks.',
    'NSMicrophoneUsageDescription':'Snowbound records audio into your notes when you choose Record Audio or Record Video.',
    'NSCameraUsageDescription': 'Snowbound records video into your notes when you choose Record Video.',
    'NSUbiquitousContainers': {CONTAINER: {
        'NSUbiquitousContainerIsDocumentScopePublic': True,
        'NSUbiquitousContainerName': 'Snowbound',
        'NSUbiquitousContainerSupportedFolderLevels': 'Any',
    }},
    'LSMinimumSystemVersion': minimum,
    'CFBundleDocumentTypes': [{
        'CFBundleTypeName': 'OneNote Notebook',
        'CFBundleTypeRole': 'Editor',
        'LSHandlerRank': 'Alternate',
        'LSItemContentTypes': [identifier for identifier, _, _ in ONENOTE_TYPES],
        'CFBundleTypeIconSystemGenerated': 1,
    }],
    'UTImportedTypeDeclarations': [{
        'UTTypeIdentifier': identifier,
        'UTTypeDescription': description,
        'UTTypeConformsTo': ['public.data'],
        'UTTypeTagSpecification': {'public.filename-extension': [extension],
                                   'public.mime-type': 'application/onenote'},
    } for identifier, description, extension in ONENOTE_TYPES],
} | versions))
# 10.6 runs the bundle unsigned.
if not args.snow_leopard:
    identity = args.sign != 'ad-hoc' and (args.sign_identity or developer_id())
    profile = args.sign != 'ad-hoc' and (args.profile or developer_id_profile())
    if identity and profile and (args.bundle_id or BUNDLE_ID) == BUNDLE_ID:
        shutil.copy2(profile, bundle / 'Contents/embedded.provisionprofile')
        with tempfile.TemporaryDirectory() as scratch:
            entitlements = Path(scratch) / 'entitlements.plist'
            entitlements.write_bytes(plistlib.dumps({
                'com.apple.application-identifier': f'{TEAM}.{BUNDLE_ID}',
                'com.apple.developer.team-identifier': TEAM,
                'com.apple.developer.icloud-services': ['CloudDocuments'],
                'com.apple.developer.icloud-container-identifiers': [CONTAINER],
                'com.apple.developer.ubiquity-container-identifiers': [CONTAINER],
                'com.apple.developer.icloud-container-environment': 'Production',
                # Hardened runtime's Record Audio and Record Video.
                'com.apple.security.device.audio-input': True,
                'com.apple.security.device.camera': True,
            }))
            # A release fails where the timestamp server can't be reached, as notarization needs it.
            timestamp = ['--timestamp'] if args.sign == 'developer-id' else []
            subprocess.run(['codesign', '--force', '--options', 'runtime', *timestamp, '--entitlements', entitlements,
                            '--sign', identity, str(bundle)], check=True)
        print(f'Signed with the Developer ID of team {TEAM}, with the iCloud container {CONTAINER}.')
    elif args.sign == 'developer-id':
        raise SystemExit(f'No Developer ID Application identity of team {TEAM} and profile for {BUNDLE_ID} with {CONTAINER}.')
    else:
        subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)], check=True)
    subprocess.run(['codesign', '--verify', '--strict', str(bundle)], check=True)
print(bundle)
