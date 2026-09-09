#!/usr/bin/env python3
"""Cold-validate exported offline page batches in disposable OneNote clones."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parent.parent


def capture(case, output):
    destination = output / case.name
    with TemporaryDirectory() as temporary:
        source = Path(temporary)
        (source / 'Lifecycle.one').symlink_to((case / 'Lifecycle.one').resolve(strict=True))
        with (output / (case.name + '.log')).open('w') as log:
            subprocess.run([sys.executable, ROOT / 'tools/native_runner.py', source,
                            destination, '--expected-pages', '9', '--collect-notebook'],
                           stdout=log, stderr=subprocess.STDOUT, check=True)
            subprocess.run([sys.executable, ROOT / 'tools/verify_page_creation.py', case,
                            destination], stdout=log, stderr=subprocess.STDOUT, check=True)
    assert json.loads((destination / 'teardown.json').read_text())['absent']
    assert (case / 'Lifecycle.one').read_bytes()[1024:] == (
        destination / 'notebook/Lifecycle.one').read_bytes()[1024:]
    print(f'Verified and removed: {case.name}', flush=True)
    return case.name


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('candidates', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--workers', type=int, default=4, choices=range(1, 9))
    args = parser.parse_args()
    cases = sorted(p for p in args.candidates.iterdir() if (p / 'Lifecycle.one').is_file())
    if len(cases) != 18:
        parser.error('Export all 18 native/offline page cases before starting the captures.')
    args.output.mkdir(parents=True, exist_ok=False)
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        completed = list(pool.map(lambda case: capture(case, args.output), cases))
    (args.output / 'verified.json').write_text(json.dumps(completed, indent=2) + '\n')
