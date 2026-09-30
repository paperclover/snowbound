#!/usr/bin/env python3
"""Build the local macOS canvas bundle, or with --snow-leopard the Mac OS X 10.6 one."""
import argparse
import json
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--release', action='store_true')
parser.add_argument('--output', type=Path, help='Create a separate bundle at a new .app path')
parser.add_argument('--bundle-id', help='Bundle identifier for the separate app')
parser.add_argument('--snow-leopard', action='store_true',
                    help='Build with platform/snow-leopard/cargo.sh into target/snow-leopard')
args = parser.parse_args()
if bool(args.output) != bool(args.bundle_id):
    parser.error('Use --output and --bundle-id together.')
if args.output and (args.output.suffix != '.app' or args.output.exists()):
    parser.error('Choose a new output path ending in .app.')
root = Path(__file__).resolve().parents[2]
profile = ['--release'] if args.release else []
if args.snow_leopard:
    subprocess.run([root / 'platform/snow-leopard/cargo.sh', 'build', '-p', 'snowbound', '--no-default-features'] + profile,
                   cwd=root, check=True)
    target = root / 'target/snow-leopard'
    built = target / 'x86_64-apple-macosx10.6'
else:
    subprocess.run(['cargo', 'build', '-p', 'snowbound'] + profile, cwd=root, check=True)
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--format-version=1', '--no-deps'], cwd=root))
    target = built = Path(metadata['target_directory'])
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
    icon = 'Snowbound-Tahoe' if int(platform.mac_ver()[0].split('.')[0] or 0) >= 26 else 'Snowbound-Sequoia'
    shutil.copy2(icons / f'{icon}.icns', resources / 'Snowbound.icns')
(bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleExecutable': binary.name,
    'CFBundleIdentifier': args.bundle_id or 'dev.snowbound.app',
    'CFBundleName': bundle.stem,
    'CFBundleDisplayName': bundle.stem,
    'CFBundlePackageType': 'APPL',
    'CFBundleVersion': '1',
    'NSHighResolutionCapable': True,
    'NSPrincipalClass': 'NSApplication',
    'CFBundleIconFile': 'Snowbound',
    'NSMicrophoneUsageDescription': 'Snowbound records audio into your notes when you choose Record Audio.',
} | ({'LSMinimumSystemVersion': '10.6'} if args.snow_leopard else {})))
# 10.6 runs the bundle unsigned.
if not args.snow_leopard:
    subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)], check=True)
    subprocess.run(['codesign', '--verify', '--strict', str(bundle)], check=True)
print(bundle)
