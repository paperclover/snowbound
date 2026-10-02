#!/usr/bin/env python3
"""Builds Snowbound for the web (crates/snowbound for wasm32, with crates/snowbound/web) as a
static folder and publishes it to the share's web/ folder.

    python3 tools/release_web.py --out /tmp/snowbound-web/dist   # build only
    python3 tools/release_web.py --publish                       # build, then replace web/
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
# The one target folder wasm builds share, apart from the native builds in target/.
TARGET = ROOT / 'target/wasm'
WEB = ROOT / 'crates/snowbound/web'
FONTS = ROOT / 'crates/canvas/assets/fonts'
PUBLISHED = Path('/Volumes/clover/Documents/Public/Snowbound/web')
# copyparty lists the folder itself, so the app is at its index.html.
URL = 'https://file.paperclover.net/shr/snowbound/web/index.html'
# Size over speed where it costs little: the module is most of the first load.
PROFILE = ['--config', 'profile.release.opt-level="s"']


def run(command, **kwargs):
    print('+', ' '.join(map(str, command)), flush=True)
    subprocess.run(command, cwd=ROOT, check=True, **kwargs)


def tool(name, nix=None, wasm=False, store=False):
    """`name` from PATH or Cargo's bin folder, or else from nixpkgs package `nix`; with `wasm`,
    from nixpkgs only, as the system's clang may not build for wasm32; with `store`, the
    package's folder."""
    found = None if wasm or store else shutil.which(name) or shutil.which(name, path=str(Path.home() / '.cargo/bin'))
    if found:
        return found
    if nix and shutil.which('nix'):
        built = subprocess.check_output(['nix', 'build', f'nixpkgs#{nix}', '--no-link', '--print-out-paths'],
                                        text=True).split()[-1]
        return built if store else str(Path(built) / 'bin' / name)
    sys.exit(f'{name} is missing: cargo install wasm-bindgen-cli at the version Cargo.lock pins, '
             'and binaryen for wasm-opt')


def environment():
    """Cargo's environment for wasm32: the wasm target folder, and a clang and llvm-ar that
    build for wasm32, which SQLite (through sqlite-wasm-rs) needs and Apple's clang lacks."""
    tools = {}
    for variable, package, name in (('CC_wasm32_unknown_unknown', 'llvmPackages.clang-unwrapped', 'clang'),
                                     ('AR_wasm32_unknown_unknown', 'llvmPackages.llvm', 'llvm-ar')):
        tools[variable] = os.environ.get(variable) or tool(name, package, wasm=True)
    return {**os.environ, 'CARGO_TARGET_DIR': str(TARGET), **tools}


def build(out):
    """Writes index.html, the module, its JavaScript and the fonts to `out`."""
    run(['cargo', 'build', '--locked', '-p', 'snowbound', '--release', '--target', 'wasm32-unknown-unknown', '--no-default-features', '--features', 'wgpu',
         *PROFILE], env=environment())
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    module = TARGET / 'wasm32-unknown-unknown/release/snowbound.wasm'
    run([tool('wasm-bindgen'), '--target', 'web', '--no-typescript', '--out-name', 'snowbound_web',
         '--out-dir', out, module])
    bound = out / 'snowbound_web_bg.wasm'
    run([tool('wasm-opt', 'binaryen'), '-Oz', '--strip-debug', '--strip-producers', bound, '-o', bound])
    shutil.copy(WEB / 'index.html', out)
    # Hunspell's American English dictionary (SCOWL, BSD-3-Clause), for spelling.
    (out / 'dictionaries').mkdir()
    store = Path(tool('hunspell', 'hunspellDicts.en_US', store=True))
    for name in ('share/hunspell/en_US.aff', 'share/hunspell/en_US.dic', 'share/doc/hunspell-dict-en-us-wordlist.txt'):
        shutil.copy(store / name, out / 'dictionaries')
    (out / 'fonts').mkdir()
    for font in sorted(FONTS.glob('*')):
        if font.suffix in ('.ttf', '.txt'):
            shutil.copy(font, out / 'fonts')
    fallbacks(out / 'fonts')
    for path in sorted(out.rglob('*')):
        if path.is_file():
            print(f'{path.stat().st_size:>12,}  {path.relative_to(out)}')


def fallbacks(fonts):
    """Noto's faces for scripts the bundled ones lack, which the page fetches as it needs them
    (`FALLBACKS` in src/web.rs), under the SIL Open Font License."""
    noto = Path(tool('noto', 'noto-fonts', store=True)) / 'share/fonts/noto'
    for name in ('NotoSansArabic.ttf', 'NotoSansHebrew.ttf', 'NotoSansDevanagari.ttf', 'NotoSansThai.ttf',
                 'NotoSansSymbols2-Regular.otf'):
        shutil.copy(noto / name, fonts)
    # Cutting the CJK face takes a minute or two, so the cut is kept between builds.
    cjk = TARGET / 'NotoSansCJK.otf'
    if not cjk.exists():
        collection = Path(tool('noto', 'noto-fonts-cjk-sans', store=True)) / \
            'share/fonts/opentype/noto-cjk/NotoSansCJK-VF.otf.ttc'
        run(['uv', 'run', '--no-project', '--with', 'fonttools', 'python', ROOT / 'tools/web/subset_cjk.py',
             collection, cjk])
    shutil.copy(cjk, fonts)
    (fonts / 'Noto-OFL.txt').write_text('Noto fonts: SIL Open Font License 1.1, https://openfontlicense.org\n')


def publish(built):
    """Replaces the share's web/ folder with `built`, leaving every other folder alone."""
    if not PUBLISHED.parent.is_dir():
        sys.exit(f'{PUBLISHED.parent} is not mounted')
    staging = PUBLISHED.with_name('.web-staging')
    if staging.exists():
        shutil.rmtree(staging)
    shutil.copytree(built, staging)
    if PUBLISHED.exists():
        shutil.rmtree(PUBLISHED)
    staging.rename(PUBLISHED)
    print(f'Published {URL}')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--out', type=Path, help='Build into this folder (default: a temporary one)')
    parser.add_argument('--publish', action='store_true', help=f'Copy the build to {PUBLISHED}')
    args = parser.parse_args()
    if not args.out and not args.publish:
        parser.error('give --out, --publish or both')
    with tempfile.TemporaryDirectory() as scratch:
        out = args.out or Path(scratch) / 'web'
        build(out)
        if args.publish:
            publish(out)


if __name__ == '__main__':
    main()
