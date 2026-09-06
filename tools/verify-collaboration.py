#!/usr/bin/env python3
"""Verify captured SMB collaboration and recovery against native cold reads."""
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET
from native_xml import ns, pages, texts

root = Path(__file__).resolve().parent.parent
corpus = root / 'corpus/collaboration/round-01'
for item in json.loads((corpus / 'manifest.json').read_text())['files']:
    data = (corpus / item['path']).read_bytes()
    assert len(data) == item['bytes']
    assert hashlib.sha256(data).hexdigest() == item['sha256'], item['path']

expected = {
    'fault-01': ['A notebook created in Rust.'],
    'fault-02': ['Publication survived a lost reply.'],
    'fault-03': ['Cleanup survived a lost reply.'],
    'live-01': ['Live Rust: café, 東京, مرحبا', 'Rust same paragraph.', 'Left cell', 'Right cell'],
    'offline-02': ['Rust edited during the native outage.'],
    'contention-01': ['Native edit waited for the lock.'],
}
for name, text in expected.items():
    captured = pages(corpus / 'native' / f'stage5-cold-{name}' / 'read')
    assert len(captured) == 1 and texts(captured[0]) == text, name

subprocess.run(['cargo', 'build', '-p', 'onestore', '--quiet', '--example', 'inventory'], cwd=root, check=True)
for case, main, conflict in [
    ('live', 'Rust same paragraph.', 'Native same paragraph.'),
    ('offline', 'Rust edited during the native outage.', 'Native edited while disconnected.'),
]:
    output = subprocess.check_output([root / 'target/debug/examples/inventory', corpus / case / 'notebook/synthetic.one'], text=True)
    by_space = defaultdict(list)
    metadata = {}
    for row in output.splitlines():
        kind, space, value = row.split('\t')
        if kind == 'page':
            metadata[space] = int(value, 16)
        elif kind in ('ascii', 'unicode'):
            by_space[space].append(bytes.fromhex(value).decode('ascii' if kind == 'ascii' else 'utf-16-le').rstrip('\0'))
    assert sum(metadata.get(space) == 0x20030 and main in text for space, text in by_space.items()) == 1
    assert any(metadata.get(space) == 0x20038 and conflict in text for space, text in by_space.items())

before = ET.parse(corpus / 'live/before-native-edit.xml').getroot()
merged = ET.parse(corpus / 'live/merged.xml').getroot()
assert 'Fictitious: café, 東京, مرحبا' in texts(before)
assert texts(merged) == ['Live Rust: café, 東京, مرحبا', 'Native edited the other outline.', 'Left cell', 'Right cell']

# Cache identities and edit timestamps change across native COM saves.
def content(node):
    attrs = {key: value for key, value in node.attrib.items()
             if key not in ('objectID', 'pathCache', 'creationTime', 'lastModifiedTime')}
    return node.tag, sorted(attrs.items()), (node.text or '').strip(), [content(child) for child in node]

original = pages(root / 'corpus/native-ink/cold-ui-ink/read')[0]
final = pages(corpus / 'native/stage5-cold-live-01/read')[0]
# These exact geometry changes appeared in the first native COM merge capture.
ink_position = original.findall('one:InkDrawing/one:Position', ns)[1]
assert ink_position.get('z') == '7'
assert merged.findall('one:InkDrawing/one:Position', ns)[1].get('z') == '6'
ink_position.set('z', '6')
column = original.find('.//one:Table/one:Columns/one:Column', ns)
assert column.get('width') == '39.14614105224609'
assert merged.find('.//one:Table/one:Columns/one:Column', ns).get('width') == '38.61000061035156'
column.set('width', '38.61000061035156')
for tag in ('Image', 'InkDrawing', 'Table', 'InsertedFile'):
    old = [content(node) for node in original.findall(f'.//one:{tag}', ns)]
    new = [content(node) for node in final.findall(f'.//one:{tag}', ns)]
    assert old and old == new, tag
assert [content(node) for node in original.findall('one:QuickStyleDef', ns)] == [content(node) for node in final.findall('one:QuickStyleDef', ns)]
assert [node.attrib for node in original.findall('one:Outline/one:Position', ns)] == [node.attrib for node in final.findall('one:Outline/one:Position', ns)]
assert next((root / 'corpus/native-ink/cold-ui-ink/read').glob('*.attachment')).read_bytes() == next((corpus / 'native/stage5-cold-live-01/read').glob('*.attachment')).read_bytes()

lock = json.loads((corpus / 'contention/lock-result.json').read_text())
assert lock['changed_while_locked'] is False
assert lock['before_sha256'] == lock['after_sha256']
assert hashlib.sha256((corpus / 'contention/notebook/synthetic.one').read_bytes()).hexdigest() != lock['after_sha256']
assert 'write' in (corpus / 'contention/locks.txt').read_text()
assert json.loads((corpus / 'contention/controller/inbox/0002.json').read_text())['text'] == expected['contention-01'][0]
for action in (2, 3):
    assert (corpus / f'contention/controller/outbox/{action}/done').read_text() == 'ok'

trace = [json.loads(line) for line in (corpus / 'smb-faults.jsonl').read_text().splitlines()]
cuts = [i for i, row in enumerate(trace) if 'cut' in row]
assert len(cuts) == 3
start = 0
for end, counter, state, case in zip(cuts, (1, 2, 256), ('NotCommitted', 'Unknown', 'Committed'), ('fault-01', 'fault-02', 'fault-03')):
    cut = trace[end]['cut']
    assert cut['command'] == 7 and cut['direction'] == 'response' and cut['status'] == '0x0'
    segment = trace[start:end]
    assert [row['transactions'] for row in segment if 'transactions' in row][-1] == counter
    assert any([0, 2**64 - 1, 18] in row.get('locks', []) for row in segment)
    result = json.loads((corpus / case / 'result.json').read_text())
    assert result['exit'] != 0
    assert result.get('state') == state or f'state: {state},' in result.get('stderr', '')
    if counter == 256:
        publication = next(i for i, row in enumerate(segment) if row.get('transactions') == 511)
        cleanup = next(i for i, row in enumerate(segment) if row.get('transactions') == 256)
        assert any(row.get('direction') == 'response' and row.get('command') == 7 and row.get('status') == '0x0' for row in segment[publication + 1:cleanup])
    start = end + 1
print('Passed: native lock contention, disjoint merge, retained conflict edits, offline convergence, three lost-FLUSH outcomes, and cold native content preservation')
