#!/usr/bin/env python3
"""Builds snowbound-relay, the Live Share relay, and snowbound-site, which serves the web build and
the codes' pages (crates/relay), as static executables for each Linux architecture a server
runs, with the files that deploy them, into target/relay/ by default.
It ships on its own, outside the app's builds and latest.json; see crates/relay/README.md."""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
ARCHITECTURES = ['x86_64', 'aarch64']


def build(arch, output):
    """Linked against Rust's own musl by its own lld, so the file needs nothing of the Linux it
    runs on."""
    triple = f'{arch}-unknown-linux-musl'
    subprocess.run(['rustup', 'target', 'add', triple], check=True, capture_output=True)
    environment = {**os.environ, f'CARGO_TARGET_{triple.upper().replace("-", "_")}_LINKER': 'rust-lld',
                   'CARGO_PROFILE_RELEASE_STRIP': 'symbols'}
    subprocess.run(['cargo', 'build', '--locked', '--release', '-p', 'relay', '--target', triple],
                   cwd=ROOT, env=environment, check=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target')
    files = []
    for name in ['snowbound-relay', 'snowbound-site']:
        file = output / f'{name}-linux-{arch}'
        shutil.copy(target / triple / f'release/{name}', file)
        files.append(file)
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/relay')
    parser.add_argument('--architectures', nargs='+', choices=ARCHITECTURES, default=ARCHITECTURES)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    files = [file for arch in args.architectures for file in build(arch, args.output)]
    for deployed in (ROOT / 'crates/relay/deploy').iterdir():
        shutil.copy(deployed, args.output)
    shutil.copy(ROOT / 'crates/relay/README.md', args.output)
    sums = ''.join(f'{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.name}\n' for file in files)
    (args.output / 'SHA256SUMS').write_text(sums)
    print(f'{args.output}:\n{sums}', end='')


if __name__ == '__main__':
    main()
