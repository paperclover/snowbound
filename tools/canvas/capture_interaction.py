#!/usr/bin/env python3
"""Record one native input script, screenshot and page XML in an inspected clone."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'w7'))
import mcp_win7 as windows


def capture(target, page, script, output):
    machine = json.loads((page.parent.parent / 'machine.json').read_text())
    if machine['name'] != target:
        raise ValueError('The page belongs to a different native clone.')
    page_id = ET.parse(page).getroot().attrib['ID']
    if not re.fullmatch(r'\{[0-9A-Fa-f-]+\}\{[0-9]+\}\{[0-9A-Fa-f]+\}', page_id):
        raise ValueError('The page identity cannot be used for native export.')
    source = script.read_bytes()
    output.mkdir(parents=True, exist_ok=False)
    result = windows.do_exec(source.decode('utf-8'), target=target,
                             timeout_ms=60000, shot_delay_ms=600)
    record = {key: value for key, value in result.items() if key != 'png_b64'}
    record.update(target=target, page_id=page_id, script=str(script.resolve()),
                  script_sha256=hashlib.sha256(source).hexdigest(),
                  source_xml_sha256=hashlib.sha256(page.read_bytes()).hexdigest())
    (output / 'input.json').write_text(json.dumps(record, indent=2) + '\n')
    if result.get('error') or result.get('exit') != 0 or not result.get('png_b64'):
        raise RuntimeError('Native input failed; inspect input.json.')
    (output / 'screen.png').write_bytes(base64.b64decode(result['png_b64'], validate=True))
    remote = r'C:\one-tests\interaction.xml'
    command = ('powershell -NoProfile -Command "'
               "$app = New-Object -ComObject OneNote.Application; $xml = ''; "
               f"$app.GetPageContent('{page_id}', [ref]$xml, 1, 1); "
               f"[IO.File]::WriteAllText('{remote}', $xml, [Text.Encoding]::UTF8)\"")
    result = windows.do_cmd(command, target=target)
    (output / 'export.json').write_text(json.dumps(result, indent=2) + '\n')
    if result.get('error') or result.get('exit') != 0:
        raise RuntimeError('Native export failed; inspect export.json.')
    result = windows.do_get(remote, output / 'page.xml', target)
    if result.get('error'):
        raise RuntimeError(result['error'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('target')
    parser.add_argument('page_xml', type=Path)
    parser.add_argument('script', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    capture(args.target, args.page_xml, args.script, args.output)
