#!/usr/bin/env python3
"""Verify captured native reads of Rust-written files."""
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET
from native_xml import ns, pages, texts

root = Path(__file__).resolve().parent.parent
corpus = root / 'corpus/writer'
manifest = json.loads((corpus / 'manifest.json').read_text())
for item in manifest['files']:
    data = (corpus / item['path']).read_bytes()
    assert len(data) == item['bytes']
    assert hashlib.sha256(data).hexdigest() == item['sha256']

for run in ['cold-edit-01', 'cold-edit-02']:
    captured = pages(corpus / run / 'read')
    assert len(captured) == 1 and texts(captured[0]) == ['Portable plain text...']

for before, after in [
    (root / 'corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one', corpus / 'edit-01/notebook/synthetic.one'),
    (corpus / 'native-edit-02/notebook/synthetic.one', corpus / 'edit-02/notebook/synthetic.one'),
    (root / 'corpus/native-ink/20260905-ui/notebook/synthetic.one', corpus / 'complex-01/notebook/synthetic.one'),
]:
    source = before.read_bytes()
    assert after.read_bytes()[1024:len(source)] == source[1024:]

native_saved = subprocess.check_output(['cargo', 'run', '-p', 'onestore', '--quiet', '--example', 'inventory', '--', str(corpus / 'native-edit-02/notebook/synthetic.one')], cwd=root, text=True)
assert [bytes.fromhex(row.split('\t')[2]).decode('ascii') for row in native_saved.splitlines() if row.startswith('ascii\t')] == ['Fictitious plain text.']

before = pages(root / 'corpus/native-ink/cold-ui-ink/read')[0]
after = pages(corpus / 'cold-complex-01/read')[0]
assert texts(after) == ['Compatible: café, 東京, مرحبا', 'Fictitious positioned outline.', 'Left cell', 'Right cell']


def normalized(page):
    page = copy.deepcopy(page)
    for node in page.iter():
        for name in ['ID', 'objectID', 'pathCache', 'selected']:
            node.attrib.pop(name, None)
        if node.text:
            node.text = node.text.replace('Fictitious: café', 'Compatible: café')
            if not node.text.strip():
                node.text = None
        node.tail = None
    for outline in page.findall('one:Outline', ns):
        if texts(outline) == ['Compatible: café, 東京, مرحبا']:
            # OneNote recalculates outline height when the replacement text wraps.
            outline.find('one:Size', ns).attrib.pop('height')
    return page


def fingerprint(node):
    return node.tag, sorted(node.attrib.items()), node.text, [fingerprint(child) for child in node]


assert fingerprint(normalized(before)) == fingerprint(normalized(after))
assert next((root / 'corpus/native-ink/cold-ui-ink/read').glob('*.attachment')).read_bytes() == next((corpus / 'cold-complex-01/read').glob('*.attachment')).read_bytes()
print('Passed: native edit/save/read cycle; Unicode edit preserves native formatting, positions, table, image, ink, and attachment bytes')

assert ET.parse(corpus / 'toc-01/hierarchy.xml').getroot().get('color') == '#336699'
created = ET.parse(corpus / 'create-05/page-000.xml').getroot()
assert texts(created) == ['Created in Rust: café 東京 🦀']
assert created.find('one:Outline', ns).get('author') == 'Fixture Author'
assert texts(ET.parse(corpus / 'create-05/returned-page.xml').getroot()) == ['Rust edited it again, with a longer paragraph.']
assert texts(ET.parse(corpus / 'create-notebook-01/page-000.xml').getroot()) == ['A notebook created in Rust.']
assert len(ET.parse(corpus / 'create-notebook-01/hierarchy.xml').getroot().findall('one:Section', ns)) == 1
print('Passed: native TOC color edit, template-free section and notebook creation, native save, and longer Rust replacement')

append = root / 'corpus/append/round-01'
for item in json.loads((append / 'manifest.json').read_text())['files']:
    data = (append / item['path']).read_bytes()
    assert len(data) == item['bytes'] and hashlib.sha256(data).hexdigest() == item['sha256']

expected = {
    'tx-17': 'Transaction 17', 'tx-255': 'Transaction 255',
    'tx-256': 'Transaction 256', 'tx-511': 'Transaction 256',
    'prepared-old': 'Transaction 255', 'tx-65535': 'Fictitious plain text.',
    'tx-65536': 'Crossed the second counter byte.',
    'tx-131071': 'Crossed the second counter byte.',
    'tx-130816': 'Crossed the second counter byte.',
    'shared-result': 'Rust committed through the SMB mount.',
    **{f'metadata-{i}': 'Transaction 255' for i in range(8)},
}
for name, text in expected.items():
    captured = pages(append / 'native' / f'append-01-{name}' / 'read')
    assert len(captured) == 1 and texts(captured[0]) == [text], name

after = pages(append / 'native/append-01-complex/read')[0]
assert fingerprint(normalized(before)) == fingerprint(normalized(after))
assert next((root / 'corpus/native-ink/cold-ui-ink/read').glob('*.attachment')).read_bytes() == next((append / 'native/append-01-complex/read').glob('*.attachment')).read_bytes()
assert ET.parse(append / 'native/append-01-toc/read/hierarchy.xml').getroot().get('color') == '#336699'
saved = subprocess.check_output(['cargo', 'run', '-p', 'onestore', '--quiet', '--example', 'inventory', '--', str(append / 'native-after-511.one')], cwd=root, text=True)
assert [bytes.fromhex(row.split('\t')[2]).decode('ascii') for row in saved.splitlines() if row.startswith('ascii\t')] == ['Native after interrupted cleanup.']
print('Passed: append encoding, native metadata and counter recovery, post-recovery native save, and filesystem-backed SMB commit')
