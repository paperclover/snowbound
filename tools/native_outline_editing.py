#!/usr/bin/env python3
"""Capture native outline controls during a disposable notebook inspection."""
import argparse
import base64
import json
from pathlib import Path
import xml.etree.ElementTree as ET

from native_runner import command, navigation_script, windows
from native_xml import ns, Text


def apply(output):
    target = json.loads((output / 'machine.json').read_text())['name']
    cases = json.loads((output / 'cases.json').read_text(encoding='utf-8-sig'))
    pages = {page.get('name'): page for path in (output / 'read').glob('page-*.xml')
             for page in [ET.parse(path).getroot()]}
    before = output / 'before'
    before.mkdir()
    (output / 'notebook').rename(before / 'notebook')
    actions = output / 'actions'
    actions.mkdir()
    for case in cases:
        page = pages[case['name']]
        outline = page.find('one:Outline', ns)
        paragraph, = [node for node in outline.findall('.//one:OE', ns)
                      if node.find('one:T', ns) is not None
                      and ''.join(Text(node.find('one:T', ns).text or '').parts).startswith('Target ')]
        page_id = page.get('ID')
        object_id = (outline if case.get('delete') == 'outline' else paragraph).get('objectID')
        stem = actions / case['name'].lower().replace(' ', '-')
        if 'keys' in case:
            script = navigation_script(page_id, object_id) + f'Send "{case["keys"]}"\nSleep 300\n'
            stem.with_suffix('.ahk').write_text(script)
            result = windows.do_exec(script, target=target, shot_delay_ms=500)
            stem.with_suffix('.json').write_text(json.dumps({k: v for k, v in result.items() if k != 'png_b64'}, indent=2))
            if result.get('png_b64'):
                stem.with_suffix('.png').write_bytes(base64.b64decode(result['png_b64']))
            if result.get('error') or result.get('exit') != 0 or not result.get('png_b64'):
                raise RuntimeError(windows.text_result(result))
        else:
            if 'delete' in case:
                page = ET.Element('delete', page=page_id, object=object_id)
                operation = "$app.DeletePageContent($xml.DocumentElement.GetAttribute('page'), $xml.DocumentElement.GetAttribute('object'), [DateTime]::MinValue, $false)"
            else:
                for child in list(page):
                    if child.tag.rsplit('}', 1)[-1] in ('Outline', 'Title') and child is not outline:
                        page.remove(child)
                if 'collapse' in case:
                    paragraph.set('collapsed', str(case['collapse']).lower())
                for name in ('position', 'size'):
                    for key, value in case.get(name, {}).items():
                        outline.find('one:' + name.title(), ns).set(key, str(value).lower())
                operation = "$app.UpdatePageContent($xml.OuterXml, [DateTime]::MinValue, 1, $false)"
            ET.register_namespace('one', ns['one'])
            payload = stem.with_suffix('.xml')
            payload.write_bytes(ET.tostring(page, encoding='utf-8'))
            result = windows.do_put(payload, r'C:\one-tests\outline-action.xml', target)
            if result.get('error'):
                raise RuntimeError(result['error'])
            script = ("$ErrorActionPreference='Stop'\n[xml]$xml=[IO.File]::ReadAllText('C:\\one-tests\\outline-action.xml', [Text.Encoding]::UTF8)\n"
                      "$app=New-Object -ComObject OneNote.Application\n" + operation +
                      "\n[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)\n")
            source = stem.with_suffix('.ps1')
            source.write_text(script, encoding='utf-8-sig')
            result = windows.do_put(source, r'C:\one-tests\outline-action.ps1', target)
            if result.get('error'):
                raise RuntimeError(result['error'])
            command(target, r'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\one-tests\outline-action.ps1', output)
        print(case['name'], flush=True)
    (output / 'finish').touch()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    apply(parser.parse_args().output)
