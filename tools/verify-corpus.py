#!/usr/bin/env python3
"""Check native fixture content independently of a revision-store parser."""
import base64
from collections import Counter
import hashlib
import json
from pathlib import Path
import xml.etree.ElementTree as ET
from native_xml import ns, texts, pages

root = Path(__file__).resolve().parent.parent
native = root / 'corpus/native/20260905-05'


manifest = json.loads((native / 'manifest.json').read_text(encoding='utf-8-sig'))
assert manifest['status'] == 'completed'
assert manifest['office']['file_version'] == '14.0.7015.1000'
assert len(manifest['fixtures']) == 9
for fixture in manifest['fixtures']:
    name = fixture['name']
    assert fixture['status'] == 'captured', name
    snapshot = native / 'snapshots' / name
    recorded = json.loads((snapshot / 'snapshot.json').read_text(encoding='utf-8-sig'))
    for item in recorded['files']:
        data = (snapshot / 'notebook' / item['path'].replace('\\', '/')).read_bytes()
        assert len(data) == item['bytes'], name
        assert hashlib.sha256(data).hexdigest() == item['sha256'], name
    warm = pages(native / 'evidence' / name / 'pages')
    cold = pages(root / 'corpus/native' / ('cold-05-' + name) / 'read')
    number = int(name[:2])
    assert len(warm) == len(cold) == (0 if number == 0 else 1), name
    if not cold:
        continue
    page = cold[0]
    assert texts(page) == texts(warm[0]), name
    expected = []
    if number == 2:
        expected = ['Fictitious plain text.']
    elif number >= 3:
        expected = ['Fictitious: café, 東京, مرحبا']
    if number >= 4:
        expected += ['Fictitious positioned outline.']
    if number >= 7:
        expected += ['Left cell', 'Right cell']
    assert texts(page) == expected, name
    if number >= 3:
        assert 'font-weight:bold' in ''.join(n.text or '' for n in page.findall('.//one:T', ns)), name
        assert any('color:#1F4E79' in n.get('style', '') for n in page.findall('.//one:OE', ns)), name
    if number >= 4:
        positioned = [o for o in page.findall('one:Outline', ns) if texts(o) == ['Fictitious positioned outline.']]
        assert len(positioned) == 1, name
        position = positioned[0].find('one:Position', ns)
        assert float(position.get('x')) == 144 and float(position.get('y')) == 96, name
    images = page.findall('.//one:Image/one:Data', ns)
    assert len(images) == int(number >= 5), name
    for data in images:
        assert base64.b64decode(data.text) == (native / 'assets/fictitious-image.png').read_bytes(), name
    attachments = page.findall('.//one:InsertedFile', ns)
    assert len(attachments) == int(number >= 6), name
    if attachments:
        assert attachments[0].get('preferredName') == 'fictitious-attachment.txt', name
    tables = page.findall('.//one:Table', ns)
    assert len(tables) == int(number >= 7), name
    if tables:
        assert [texts(cell) for cell in tables[0].findall('one:Row/one:Cell', ns)] == [['Left cell'], ['Right cell']], name

ink = root / 'corpus/native-ink/cold-ui-ink/read'
ink_pages = pages(ink)
assert len(ink_pages) == 1
drawings = ink_pages[0].findall('one:InkDrawing', ns)
assert len(drawings) == 2
for drawing in drawings:
    assert len(base64.b64decode(drawing.find('one:Data', ns).text)) > 20
attachment = list(ink.glob('*.attachment'))
assert len(attachment) == 1
assert attachment[0].read_bytes() == (native / 'assets/fictitious-attachment.txt').read_bytes()

encrypted = root / 'corpus/native-encrypted'
provenance = json.loads((encrypted / 'manifest.json').read_text())
for item in provenance['files']:
    data = (encrypted / item['path']).read_bytes()
    assert len(data) == item['bytes']
    assert hashlib.sha256(data).hexdigest() == item['sha256']
unlocked = pages(encrypted / 'cold-encrypted-02/read')
assert len(unlocked) == 1
assert texts(unlocked[0]) == [''] + texts(pages(root / 'corpus/native/cold-05-07-table/read')[0])
assert next((encrypted / 'cold-encrypted-02/read').glob('*.attachment')).read_bytes() == attachment[0].read_bytes()

deleted = root / 'corpus/native-delete/cold-deletion-05/read'
deleted_pages = pages(deleted)
assert len(deleted_pages) == 2
marker = [page for page in deleted_pages if texts(page) == ['Recoverable deletion marker.']]
assert len(marker) == 1
hierarchy = ET.parse(deleted / 'hierarchy.xml')
trash = [p.get('ID') for section in hierarchy.findall('.//one:Section', ns)
         if section.get('isInRecycleBin') == 'true' for p in section.findall('one:Page', ns)]
assert marker[0].get('ID') in trash

private = root / 'corpus/private'
source = json.loads((private / 'source-manifest.json').read_text())
for item in source['files']:
    assert hashlib.sha256((private / 'original' / item['path']).read_bytes()).hexdigest() == item['sha256']
private_pages = pages(private / 'exact-native/read')
private_ids = {p.get('ID') for p in private_pages}
tree = ET.parse(private / 'exact-native/read/hierarchy.xml')
assert private_ids == {p.get('ID') for p in tree.findall('.//one:Page', ns)}
assert len(private_ids) == 26
features = Counter(e.tag.rsplit('}', 1)[-1] for page in private_pages for e in page.iter())
assert features['InkDrawing'] and features['MediaFile'] and features['Image'] and features['Tag']
print('Passed: 9 generated snapshots, cold ink, deletion and encryption fixtures, attachment bytes, 26 private pages')
