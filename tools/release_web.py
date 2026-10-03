#!/usr/bin/env python3
"""Builds Snowbound for the web (crates/snowbound for wasm32, with crates/snowbound/web) as a
static folder and deploys it to the VPS, which serves it at https://snowbound.paperclover.net.

    python3 tools/release_web.py --out /tmp/snowbound-web/dist   # build only
    python3 tools/release_web.py --deploy                        # build, then replace the site

The module and its JavaScript go in a folder named by their contents, b/HASH/, which
index.html names, so a page never pairs one build's module with another's JavaScript. The
server is snowbound-site (crates/relay), which pm2's `snowbound-site` runs from the VPS's
snowbound-web/ on 127.0.0.1:23593, serving site/ and the Live Share codes' pages: it has
index.html checked on every load and keeps a build's folder for good. Deploying builds it
for the VPS too, and restarts it only when its binary changed.
"""
import argparse
import gzip
import hashlib
import itertools
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
# The one target folder wasm builds share, apart from the native builds in target/.
TARGET = ROOT / 'target/wasm'
WEB = ROOT / 'crates/snowbound/web'
FONTS = ROOT / 'crates/canvas/assets/fonts'
# Where the loading shell's icons are looked up by name, in order.
ICONS = [ROOT / 'crates/snowbound/assets/icons', ROOT / 'crates/canvas/assets/tags', ROOT / 'crates/ui/assets']
# The VPS's folder: snowbound-site, and the site/ it serves on 127.0.0.1:23593 as pm2's
# `snowbound-site`, behind https://snowbound.paperclover.net.
HOST = 'clo@paperclover.net'
DEPLOYED = 'snowbound-web'
SERVER = 'snowbound-site'
URL = 'https://snowbound.paperclover.net'
# Builds the server keeps beside the newest, for pages still running one.
KEPT = 3
EMOJI_URL = 'https://github.com/googlefonts/noto-emoji/raw/v2.051/fonts/Noto-COLRv1.ttf'
EMOJI_SHA256 = '0ae57fe58645638523ba35f388d93739d292539a9acb84df5700c81b1e1a28d2'
# As `EMOJI` in src/web.rs names it.
EMOJI_FONT = 'Noto-COLRv1.ttf.gz'
# Spelling dictionaries: the name the page asks for, the nixpkgs hunspellDicts package, its
# files' stem, and the SPDX licences it comes under, each checked to allow redistribution
# (the LGPL's text with the GPL's, which it amends). Add a row to offer another language.
DICTIONARIES = [
    ('en_US', 'en_US', 'en_US', ['BSD-3-Clause']),
    ('en_GB', 'en_GB-ise', 'en_GB', ['BSD-3-Clause']),
    ('de_DE', 'de_DE', 'de_DE', ['GPL-2.0-only', 'GPL-3.0-only']),
    ('fr_FR', 'fr-moderne', 'fr-moderne', ['MPL-2.0']),
    ('es_ES', 'es_ES', 'es_ES', ['GPL-3.0-only', 'LGPL-3.0-only', 'MPL-1.1']),
    ('pt_BR', 'pt_BR', 'pt_BR', ['LGPL-3.0-only', 'GPL-3.0-only']),
    ('pt_PT', 'pt_PT', 'pt_PT', ['GPL-2.0-only', 'LGPL-2.1-only', 'MPL-1.1']),
    ('it_IT', 'it_IT', 'it_IT', ['GPL-3.0-only']),
    ('nl_NL', 'nl_NL', 'nl_NL', ['BSD-3-Clause', 'CC-BY-3.0']),
    ('sv_SE', 'sv_SE', 'sv_SE', ['LGPL-3.0-only', 'GPL-3.0-only']),
    ('ru_RU', 'ru_RU', 'ru_RU', ['MPL-2.0', 'LGPL-3.0-only', 'GPL-3.0-only']),
]
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
    """Writes index.html, the module, its JavaScript and the fonts to `out`, the module knowing
    when it was built."""
    built = str(int(time.time()))
    run(['cargo', 'build', '--locked', '-p', 'snowbound', '--release', '--target', 'wasm32-unknown-unknown', '--no-default-features', '--features', 'wgpu',
         *PROFILE], env={**environment(), 'SNOWBOUND_WEB_BUILD': built})
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    module = TARGET / 'wasm32-unknown-unknown/release/snowbound.wasm'
    code = out / 'b/next'
    run([tool('wasm-bindgen'), '--target', 'web', '--no-typescript', '--out-name', 'snowbound_web',
         '--out-dir', code, module])
    bound = code / 'snowbound_web_bg.wasm'
    run([tool('wasm-opt', 'binaryen'), '-Oz', '--strip-debug', '--strip-producers', bound, '-o', bound])
    digest = hashlib.sha256()
    for path in sorted(code.rglob('*')):
        if path.is_file():
            digest.update(str(path.relative_to(code)).encode() + b'\0' + path.read_bytes())
    code = code.rename(out / 'b' / digest.hexdigest()[:16])
    dictionaries(out / 'dictionaries')
    (out / 'fonts').mkdir()
    for font in sorted(FONTS.glob('*')):
        if font.suffix in ('.ttf', '.txt'):
            shutil.copy(font, out / 'fonts')
    fallbacks(out / 'fonts')
    page(out, code.relative_to(out).as_posix())
    for path in sorted(out.rglob('*')):
        if path.is_file():
            print(f'{path.stat().st_size:>12,}  {path.relative_to(out)}')


def page(out, code):
    """Writes index.html with its loading shell's icons inlined, the folder `code` it takes
    the module and its JavaScript from, and the sizes of the files it fetches before the app
    starts, for its progress bar."""
    count = itertools.count()

    def inline(match):
        name = match[2]
        svg = next(folder / f'{name}.svg' for folder in ICONS if (folder / f'{name}.svg').exists()).read_text()
        # Each copy's gradients keep ids of their own in the one document.
        prefix = f'i{next(count)}-'
        svg = re.sub(r'id="([^"]+)"', rf'id="{prefix}\1"', svg)
        svg = re.sub(r'url\(#([^)]+)\)', rf'url(#{prefix}\1)', svg)
        return f'<i{match[1]}>{svg.strip()}</i>'
    html = re.sub(r'<i([^>]*) icon="([^"]+)"></i>', inline, (WEB / 'index.html').read_text())
    sizes = {path.relative_to(out).as_posix(): path.stat().st_size
             for path in [out / code / 'snowbound_web_bg.wasm', *sorted((out / 'fonts').glob('*.ttf'))]}
    for marker, written in [('const SIZES = {};', f'const SIZES = {json.dumps(sizes)};'),
                            ('"./snowbound_web.js"', f'"./{code}/snowbound_web.js"'),
                            ('const MODULE = "snowbound_web_bg.wasm";', f'const MODULE = "{code}/snowbound_web_bg.wasm";')]:
        if marker not in html:
            sys.exit(f'index.html lacks `{marker}`')
        html = html.replace(marker, written)
    (out / 'index.html').write_text(html)


def dictionaries(folder):
    """The Hunspell dictionaries in DICTIONARIES as UTF-8, gzipped for the glue to inflate,
    each beside its readme, the texts of the licences they come under in licenses/, and their
    names in index.json for the page to pick from (`pick` in canvas/src/spelling.rs)."""
    (folder / 'licenses').mkdir(parents=True)
    texts = Path(tool('spdx', 'spdx-license-list-data.text', store=True)) / 'text'
    for name, package, stem, licenses in DICTIONARIES:
        store = Path(tool('hunspell', f'hunspellDicts.{package}', store=True))
        affix = (store / f'share/hunspell/{stem}.aff').read_bytes()
        # Hunspell names the files' encoding in the affix file's SET line.
        encoding = next((line.split()[1] for line in affix.decode('latin-1').splitlines()
                         if line.startswith('SET ')), 'UTF-8')
        for kind in ('aff', 'dic'):
            text = (store / f'share/hunspell/{stem}.{kind}').read_bytes().decode(encoding)
            # One encoding and one line ending for spellbook, which reads only UTF-8. An
            # 8-bit file's flags are its characters by default, as FLAG UTF-8 keeps them.
            utf8 = 'SET UTF-8' if encoding == 'UTF-8' or 'FLAG ' in text else 'SET UTF-8\nFLAG UTF-8'
            text = ''.join((utf8 if line.startswith('SET ') else line) + '\n'
                           for line in text.splitlines())
            (folder / f'{name}.{kind}.gz').write_bytes(gzip.compress(text.encode(), 9, mtime=0))
        # Where a package has no readme, its affix file's header comment names the licences.
        readme = list((store / 'share/doc').glob('*.txt'))
        notice = readme[0].read_bytes() if readme else \
            ''.join(line + '\n' for line in affix.decode(encoding).splitlines() if line.startswith('#')).encode()
        (folder / f'{name}.txt').write_bytes(notice + f'\nLicences: {", ".join(licenses)} (licenses/)\n'.encode())
        for license in licenses:
            shutil.copyfile(texts / f'{license}.txt', folder / 'licenses' / f'{license}.txt')
    (folder / 'index.json').write_text(json.dumps([name for name, *_ in DICTIONARIES]))


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
    # Noto Color Emoji as COLRv1 outlines (4.8 MB), which src/render/colr.rs in draw paints,
    # gzipped to 2.7 MB for the glue to inflate; nixpkgs has only its 10 MB of bitmaps.
    emoji = TARGET / EMOJI_FONT
    if not emoji.exists():
        data = urllib.request.urlopen(EMOJI_URL).read()
        if hashlib.sha256(data).hexdigest() != EMOJI_SHA256:
            sys.exit(f'{EMOJI_URL} is not the font this build pins')
        emoji.write_bytes(gzip.compress(data, 9, mtime=0))
    shutil.copy(emoji, fonts)
    shutil.copy(ROOT / 'crates/draw/assets/NotoColorEmoji-OFL.txt', fonts)
    (fonts / 'Noto-OFL.txt').write_text('Noto fonts: SIL Open Font License 1.1, https://openfontlicense.org\n')


def deploy(built):
    """Makes the VPS's site `built`, each file taking its place once all have arrived, and
    leaves the last builds' folders for pages still running them; replaces its server, and
    restarts it, where the server built now differs."""
    sync = ['rsync', '--recursive', '--links', '--times', '--compress', '--itemize-changes']
    arch = subprocess.check_output(['ssh', HOST, 'uname -m'], text=True).strip()
    with tempfile.TemporaryDirectory() as scratch:
        run([sys.executable, ROOT / 'tools/release_relay.py', '--architectures', arch, '--output', scratch])
        server = subprocess.run([*sync, '--checksum', Path(scratch) / f'{SERVER}-linux-{arch}',
                                 f'{HOST}:{DEPLOYED}/{SERVER}'],
                                check=True, capture_output=True, text=True).stdout
    run([*sync, '--delete', '--delay-updates', '--filter=P b/*', f'{built}/', f'{HOST}:{DEPLOYED}/site/'])
    run(['ssh', HOST, f'cd {DEPLOYED}/site/b && ls -t | tail -n +{KEPT + 2} | xargs -r rm -rf --'])
    if server.strip():
        run(['ssh', HOST, f'pm2 restart {SERVER}'])
    print(f'Deployed {URL}')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--out', type=Path, help='Build into this folder (default: a temporary one)')
    parser.add_argument('--deploy', action='store_true', help=f'Copy the build to {DEPLOYED}')
    args = parser.parse_args()
    if not args.out and not args.deploy:
        parser.error('give --out, --deploy or both')
    with tempfile.TemporaryDirectory() as scratch:
        out = args.out or Path(scratch) / 'web'
        build(out)
        if args.deploy:
            deploy(out)


if __name__ == '__main__':
    main()
