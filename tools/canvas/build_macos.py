#!/usr/bin/env python3
"""Build the local macOS canvas bundle."""
import argparse
import json
from pathlib import Path
import plistlib
import shutil
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--release', action='store_true')
parser.add_argument('--output', type=Path, help='Create a separate bundle at a new .app path')
parser.add_argument('--bundle-id', help='Bundle identifier for the separate app')
args = parser.parse_args()
if bool(args.output) != bool(args.bundle_id):
    parser.error('Use --output and --bundle-id together.')
if args.output and (args.output.suffix != '.app' or args.output.exists()):
    parser.error('Choose a new output path ending in .app.')
root = Path(__file__).resolve().parents[2]
subprocess.run(['cargo', 'build', '-p', 'one-canvas-app'] + (['--release'] if args.release else []), cwd=root, check=True)
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--format-version=1', '--no-deps'], cwd=root))
target = Path(metadata['target_directory'])
bundle = args.output.resolve() if args.output else target / 'One Canvas.app'
if args.output:
    bundle.mkdir(parents=True, exist_ok=False)
binary = bundle / 'Contents/MacOS/OneCanvas'
binary.parent.mkdir(parents=True, exist_ok=True)
pending = binary.with_suffix('.next')
shutil.copy2(target / ('release' if args.release else 'debug') / 'one-canvas-app', pending)
pending.replace(binary)
(bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleExecutable': binary.name,
    'CFBundleIdentifier': args.bundle_id or 'dev.onecanvas.app',
    'CFBundleName': bundle.stem,
    'CFBundleDisplayName': bundle.stem,
    'CFBundlePackageType': 'APPL',
    'CFBundleVersion': '1',
    'NSHighResolutionCapable': True,
    'NSPrincipalClass': 'NSApplication',
}))
subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)], check=True)
subprocess.run(['codesign', '--verify', '--strict', str(bundle)], check=True)
print(bundle)
