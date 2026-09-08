#!/usr/bin/env python3
"""Apply keyboard controls during a native paragraph-fixture inspection."""
import argparse
import base64
import json
from pathlib import Path
import xml.etree.ElementTree as ET

from native_runner import navigation_script, windows
from native_xml import ns


def apply(output):
    target = json.loads((output / 'machine.json').read_text())['name']
    pages = {page.get('name'): page for path in (output / 'read').glob('page-*.xml')
             for page in [ET.parse(path).getroot()]}
    cases = json.loads((Path(__file__).resolve().parent.parent /
                       'corpus/paragraph-edit/reconciliation/cases.json').read_text(encoding='utf-8-sig'))
    scripts = output / 'keyboard'
    scripts.mkdir()
    for case in cases:
        page = pages[case['name']]
        paragraphs = page.findall('one:Outline/one:OEChildren/one:OE', ns)
        join = case['operation'] == 'join'
        change = case['change']
        at = 1 if join and change in ('child', 'list', 'tag', 'format', 'right-boundary') else 0
        keys = {
            'prefix': '{Left}X',
            'boundary': '{Right}X' if join else '{Left}{Right 2}X',
            'right-boundary': '{Left}X',
            'child': '{Right}{Enter}{Tab}Native child',
            'list': '{Left}^.',
            'tag': '{Left}^1',
            'format': '^i',
            'sibling': 'Native sibling',
            'adoption': '{Left}X',
        }[change]
        if change == 'sibling':
            at = len(paragraphs) - 1
        steps = [(paragraphs[at].get('objectID'), keys)]
        if join and change == 'prefix':
            steps.append((paragraphs[1].get('objectID'), '{Right}Y'))
        script = ''.join(navigation_script(page.get('ID'), oid) + f'Send "{keys}"\nSleep 300\n'
                         for oid, keys in steps)
        stem = scripts / case['name'].lower().replace(' ', '-')
        stem.with_suffix('.ahk').write_text(script)
        result = windows.do_exec(script, target=target, shot_delay_ms=500)
        stem.with_suffix('.json').write_text(json.dumps({k: v for k, v in result.items() if k != 'png_b64'}, indent=2))
        if result.get('png_b64'):
            stem.with_suffix('.png').write_bytes(base64.b64decode(result['png_b64']))
        if result.get('error') or result.get('exit') != 0 or not result.get('png_b64'):
            raise RuntimeError(windows.text_result(result))
        print(case['name'], flush=True)
    (output / 'finish').touch()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    apply(parser.parse_args().output)
