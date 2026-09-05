#!/usr/bin/env python3
"""Compare a Rust scale fixture against its generator and fresh native capture."""
import argparse
import hashlib
import json
from pathlib import Path, PureWindowsPath
import subprocess
import xml.etree.ElementTree as ET

from document_model import EXPORTER, ordered_pages, walk
from native_xml import ns, pages, texts, project_text

ROOT = Path(__file__).resolve().parent.parent


def compare(source, capture):
    expected = json.loads((source / 'expected.json').read_text())
    manifest = {row['path']: row for row in json.loads((capture / 'source.json').read_text())}
    for name, row in manifest.items():
        assert hashlib.sha256((source / 'notebook' / name).read_bytes()).hexdigest() == row['sha256'], 'Source changed since native capture'
    hierarchy = ET.parse(capture / 'read/hierarchy.xml').getroot()
    sections = hierarchy.findall('.//one:Section', ns)
    order = [PureWindowsPath(section.get('path')).name for section in sections]
    assert order == [row['section'] for row in reversed(expected)], 'Native section order differs from the generated reverse order'
    native = {page.get('ID'): page for page in pages(capture / 'read')}
    section_pages = {PureWindowsPath(section.get('path')).name: section.findall('one:Page', ns) for section in sections}
    for row in expected:
        output = subprocess.check_output([EXPORTER, source / 'notebook' / row['section']])
        document = json.loads(output)
        (_, _, space, oid), = ordered_pages(document)
        actual = [node['kind']['text'] for _, node in walk(space, oid) if node['kind']['type'] == 'RichText']
        assert actual == [row['text']], (row['section'], 'Rust text differs from generator input')
        page, = section_pages[row['section']]
        assert texts(native[page.get('ID')]) == [project_text(row['text'])], (row['section'], 'Native text differs from generator input')
    print(f'Passed {len(expected)} sections and {sum(len(row["text"]) for row in expected):,} Unicode scalars against the independent generator input; reverse section order survived native import.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('capture', type=Path)
    args = parser.parse_args()
    compare(args.source, args.capture)
