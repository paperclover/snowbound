#!/usr/bin/env python3
"""Capture explicit page removals and independently reopen the resulting notebook."""
import argparse
import hashlib
import json
from pathlib import Path
import runpy
import signal
from tempfile import TemporaryDirectory

import native_runner as runner

BASE = 'page-lifecycle/04-nested/notebook/Lifecycle.one'
LEADING = 'page-lifecycle/movement/08-collapsed-group-move/notebook/Lifecycle.one'
CASES = {
    'trailing': (BASE, 9, [8], False),
    'parent': (BASE, 9, [3], False),
    'child': (BASE, 9, [4], False),
    'grandchild': (BASE, 9, [5], False),
    'group': (BASE, 9, [3, 4, 5], False),
    'all': (BASE, 9, list(range(9)), False),
    'permanent-parent': (BASE, 9, [3], True),
    'leading-parent': (LEADING, 9, [0], False),
    'leading-group': (LEADING, 9, [0, 1], False),
    'features': ('m6/native-expanded-control-ui-01/notebook/Features.one', 13,
                 [0, 3, 8, 9, 10, 11, 12], False),
    'ink': ('native-ink/20260905-ui/notebook/synthetic.one', 1, [0], False),
}


def capture(case, output):
    source, count, selected, permanent = CASES[case]
    output.mkdir(parents=True, exist_ok=False)
    request = {'case': case, 'selected': selected, 'permanent': permanent,
               'expected_pages': count}
    (output / 'removal.json').write_text(json.dumps(request, indent=2) + '\n')
    scripts = {name: (runner.ROOT / 'tools/native' / name).read_bytes()
               for name in ['page-lifecycle.ps1', 'page-removal.ps1']}

    def interact(target, destination):
        for name, data in scripts.items():
            local = destination / 'scripts' / name
            local.write_bytes(data)
            result = runner.windows.do_put(local, 'C:\\one-tests\\' + name, target)
            if result.get('error'):
                raise RuntimeError(result['error'])
        result = runner.windows.do_put(output / 'removal.json', r'C:\one-tests\runs\capture\removal.json', target)
        if result.get('error'):
            raise RuntimeError(result['error'])
        (destination / 'interaction.json').write_text(json.dumps({
            name: hashlib.sha256(data).hexdigest()
            for name, data in scripts.items()}, indent=2) + '\n')
        runner.command(target, 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\one-tests\\page-removal.ps1 -Root C:\\one-tests\\runs\\capture -CloneHost ONE-' + target.upper(), destination, 180000)

    with TemporaryDirectory() as temporary:
        stage = Path(temporary)
        (stage / 'Lifecycle.one').symlink_to(runner.ROOT / 'corpus' / source)
        runner.capture(stage, output / 'native', expected_pages=count,
                       collect_notebook=True, interaction=interact)
    runner.capture(output / 'native/after/notebook', output / 'cold', collect_notebook=True)
    compare = runpy.run_path(str(runner.ROOT / 'tools/verify-document.py'))['compare']
    compare(output / 'native/after/notebook', output / 'cold/read')
    compare(output / 'cold/notebook', output / 'cold/read')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('case', choices=CASES)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()

    def interrupted(_signum, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    capture(args.case, args.output)
