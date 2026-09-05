#!/usr/bin/env python3
"""Compare native captures while excluding cache identities and selection state."""
import argparse
import base64
import difflib
import hashlib
import json
from pathlib import Path
import xml.etree.ElementTree as ET
from native_xml import ns, pages


def content(node):
    attrs = {key: value for key, value in node.attrib.items()
             if key not in ('ID', 'objectID', 'pathCache', 'selected', 'isCurrentlyViewed')}
    text = node.text
    if node.tag == '{%s}Data' % ns['one'] and text:
        text = hashlib.sha256(base64.b64decode(''.join(text.split()), validate=True)).hexdigest()
    elif text is not None and not text.strip() and node.tag != '{%s}T' % ns['one']:
        text = None
    return node.tag, sorted(attrs.items()), text, [content(child) for child in node]


def compare(actual, expected):
    captured = []
    for directory in (actual, expected):
        by_id = {page.get('ID'): page for page in pages(directory)}
        hierarchy = ET.parse(directory / 'hierarchy.xml')
        ordered = [page.get('ID') for page in hierarchy.findall('.//one:Page', ns)]
        if len(ordered) != len(by_id) or set(ordered) != set(by_id):
            raise ValueError('Native page capture does not match its hierarchy: ' + str(directory))
        captured.append({
            'pages': [content(by_id[key]) for key in ordered],
            'attachments': sorted(hashlib.sha256(path.read_bytes()).hexdigest()
                                  for path in directory.glob('*.attachment')),
        })
    if captured[0] != captured[1]:
        difference = '\n'.join(difflib.unified_diff(
            json.dumps(captured[1], indent=2, ensure_ascii=False).splitlines(),
            json.dumps(captured[0], indent=2, ensure_ascii=False).splitlines(),
            fromfile=str(expected), tofile=str(actual),
        ))
        (actual.parent / 'native.diff').write_text(difference + '\n')
        raise AssertionError('Native content differs; inspect ' + str(actual.parent / 'native.diff'))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('actual', type=Path)
    parser.add_argument('expected', type=Path)
    args = parser.parse_args()
    compare(args.actual, args.expected)
    print('Native document content and attachment bytes match')
