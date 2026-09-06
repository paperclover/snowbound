#!/usr/bin/env python3
"""Compare current Rust page content against independent native XML; requires Pillow."""
import base64
from collections import Counter
import io
from pathlib import Path
import subprocess
from PIL import Image
from native_xml import Text, ns, pages, texts

root = Path(__file__).resolve().parent.parent
private = root / 'corpus/private'
paths = sorted((private / 'original').rglob('*.one'))
output = subprocess.check_output(
    ['cargo', 'run', '-p', 'onestore', '--quiet', '--example', 'inventory', '--', *map(str, paths)],
    cwd=root, text=True,
)
records = [line.split('\t') for line in output.splitlines()]
regular = {row[1] for row in records if row[0] == 'page' and row[2] == '20030'}
assert len(regular) == 26
assert Counter(row[2] for row in records if row[0] == 'page') == {'20030': 26, '2003e': 1}
content = {osid: Counter() for osid in regular}
payloads = {osid: [] for osid in regular}
links = []
for kind, osid, value in records:
    if osid not in regular:
        continue
    if kind == 'file':
        payloads[osid].append(bytes.fromhex(value))
    elif kind in ('ascii', 'unicode'):
        data = bytes.fromhex(value)
        text = data.decode('utf-16-le').removesuffix('\0') if kind == 'unicode' else data.decode('latin1')
        if text.startswith('\ufddfHYPERLINK "'):
            target, text = text[len('\ufddfHYPERLINK "'):].rsplit('"', 1)
            links.append(target)
        if text.strip():
            content[osid][text] += 1

native = pages(private / 'exact-native/read')
native_content = [Counter(text for text in texts(page) if text.strip()) for page in native]
assert Counter(tuple(sorted(c.items())) for c in content.values()) == Counter(tuple(sorted(c.items())) for c in native_content)
native_links = [target for page in native for node in page.findall('.//one:T', ns) for target in Text(node.text or '').links]
assert len(links) == 3 and all(target in native_links for target in links)

exact = converted = 0
for page, expected in zip(native, native_content, strict=True):
    candidates = [data for osid, text in content.items() if text == expected for data in payloads[osid]]
    for node in page.findall('.//one:Image/one:Data', ns):
        data = base64.b64decode(node.text)
        if data in candidates:
            exact += 1
            continue
        image = Image.open(io.BytesIO(data)).convert('RGBA')
        matched = False
        for candidate in candidates:
            if not candidate.startswith((b'GIF87a', b'GIF89a')):
                continue
            gif = Image.open(io.BytesIO(candidate)).convert('RGBA')
            if gif.size == image.size and gif.tobytes() == image.tobytes():
                matched = True
                break
        assert matched, 'Native image differs from its stored payload'
        converted += 1

assert exact == 18 and converted == 3
assert sum(sum(c.values()) for c in content.values()) == 581
print('Passed: 26 pages, 581 nonempty text objects by page, 3 hyperlink targets, 18 exact images, 3 native GIF conversions')
