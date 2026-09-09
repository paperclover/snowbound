#!/usr/bin/env python3
"""Capture native page indentation and tab movement in an owned clone."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import shutil
import signal
from tempfile import TemporaryDirectory
import xml.etree.ElementTree as ET

import native_runner as runner

NAMES = ['Untitled page'] * 3 + [
    'Same title', 'child', 'grandchild', 'Renamed 🦋 é',
    'Body trailing 🦀 é preserved', 'Body automatic 🦀 é preserved']
PHASES = [
    ('01-demoted-parent', 3, [('menu', 179)], list(range(9)), [1, 1, 1, 2, 2, 3, 1, 1, 1]),
    ('02-promoted-parent', 3, [('menu', 202)], list(range(9)), [1, 1, 1, 1, 2, 3, 1, 1, 1]),
    ('03-selected-group-move', 3, [('select', 2), ('drag', 308)], [0, 1, 2, 6, 7, 8, 3, 4, 5], [1, 1, 1, 1, 1, 1, 1, 2, 3]),
    ('04-subpage-move', 4, [('drag', 182)], [0, 1, 2, 4, 6, 7, 8, 3, 5], [1, 1, 1, 1, 1, 1, 1, 1, 3]),
    ('05-insert-before-subpage', 4, [('drag', 286)], [0, 1, 2, 6, 7, 8, 3, 4, 5], [1, 1, 1, 1, 1, 1, 1, 3, 3]),
    ('06-promoted-level-two', 4, [('menu', 202)], [0, 1, 2, 6, 7, 8, 3, 4, 5], [1, 1, 1, 1, 1, 1, 1, 2, 3]),
    ('07-promoted-root', 4, [('menu', 202)], [0, 1, 2, 6, 7, 8, 3, 4, 5], [1, 1, 1, 1, 1, 1, 1, 1, 3]),
    ('08-collapsed-group-move', 4, [('menu', 224), ('drag', 117)], [4, 5, 0, 1, 2, 6, 7, 8, 3], [1, 3, 1, 1, 1, 1, 1, 1, 1]),
]


def ui(target, source, destination):
    destination.with_suffix('.ahk').write_text(source)
    result = runner.windows.do_exec(source, target=target, timeout_ms=15000, shot_delay_ms=300)
    destination.with_suffix('.json').write_text(json.dumps({k: v for k, v in result.items() if k != 'png_b64'}, indent=2))
    if result.get('png_b64'):
        destination.with_suffix('.png').write_bytes(base64.b64decode(result['png_b64']))
    if result.get('exit') != 0 or result.get('error') or not result.get('png_b64'):
        raise RuntimeError('Native desktop action failed; inspect its saved capture.')


def interact(target, output):
    helper = runner.ROOT / 'tools/native/page-lifecycle.ps1'
    shutil.copy2(helper, output / 'scripts/page-lifecycle.ps1')
    shutil.copy2(__file__, output / 'controller.py')
    (output / 'interaction.json').write_text(json.dumps({
        'controller_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'helper_sha256': hashlib.sha256(helper.read_bytes()).hexdigest(),
    }, indent=2))
    result = runner.windows.do_put(helper, r'C:\one-tests\page-lifecycle.ps1', target)
    if result.get('error'):
        raise RuntimeError(result['error'])
    hierarchy = output / 'read/hierarchy.xml'
    for phase, selected, actions, order, levels in PHASES:
        pages = list(ET.parse(hierarchy).getroot().iter('{http://schemas.microsoft.com/office/onenote/2010/onenote}Page'))
        row, page = next((i, page) for i, page in enumerate(pages) if page.get('name') == NAMES[selected])
        destination = output / 'desktop' / phase
        destination.mkdir(parents=True)
        ui(target, 'if A_ScreenWidth != 800 || A_ScreenHeight != 600\n    throw Error("This fixture requires an 800 by 600 desktop")\n'
           + runner.navigation_script(page.get('ID')), destination / 'navigate')
        y = 120 + row * 22
        for ordinal, (action, value) in enumerate(actions):
            if action == 'menu':
                ui(target, f'Click(710,{y},"Right")\nSleep(500)', destination / f'{ordinal}-menu')
                script = f'Click(578,{y + value})\nSleep(700)'
            elif action == 'select':
                script = f'Click(710,{y})\nSend("{{Shift down}}")\nClick(710,{y + value * 22})\nSend("{{Shift up}}")\nSleep(700)'
            else:
                script = f'MouseMove(710,{y})\nSend("{{LButton down}}")\nSleep(250)\nMouseMove(710,{value})\nSleep(500)\nSend("{{LButton up}}")\nSleep(700)'
            ui(target, script, destination / f'{ordinal}-action')
        runner.command(target, 'powershell -NoProfile -ExecutionPolicy Bypass -File C:\\one-tests\\page-lifecycle.ps1 -Root C:\\one-tests\\runs\\capture -CloneHost ONE-' + target.upper() + ' -CaptureOnly ' + phase, output, 60000)
        hierarchy = destination / 'reopened-hierarchy.xml'
        result = runner.windows.do_get('C:\\one-tests\\runs\\capture\\' + phase + '\\reopened-hierarchy.xml', hierarchy, target)
        if result.get('error'):
            raise RuntimeError(result['error'])
        pages = list(ET.parse(hierarchy).getroot())
        actual = [(page.get('name'), int(page.get('pageLevel'))) for page in pages]
        expected = [(NAMES[i], level) for i, level in zip(order, levels, strict=True)]
        if actual != expected:
            raise AssertionError((phase, actual, expected))
        print('Verified native desktop phase: ' + phase, flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    with TemporaryDirectory() as temporary:
        source = Path(temporary)
        shutil.copy2(runner.ROOT / 'corpus/page-lifecycle/04-nested/notebook/Lifecycle.one', source / 'Lifecycle.one')
        runner.capture(source, args.output, expected_pages=9, collect_notebook=True, interaction=interact)
