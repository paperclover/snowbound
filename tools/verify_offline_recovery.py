#!/usr/bin/env python3
"""Compare every interrupted/recovered image with an independent cold native export."""
import argparse
import hashlib
import json
from pathlib import Path
import xml.etree.ElementTree as ET
from native_format import native_characters
from native_xml import ns


def paragraphs(path):
    root = ET.parse(path).getroot()
    return [''.join(char for char, _ in text) for text in native_characters(root, root.findall('one:Outline', ns))]


def verify(run, cold, source_capture):
    original = paragraphs(source_capture)
    target, = [text for text in original if text.startswith('Concurrent edits:')]
    other = [text for text in original if text != target]
    expected = {}
    for case in json.loads((run/'results.json').read_text()):
        for side in ('remote', 'local'):
            if side+'_image' not in case: continue
            digest, text = case[side+'_image'], case[side+'_text']
            assert text in (target, target+' [offline-recovery]'), 'Recovery oracle is unrelated to the native source'
            wanted = sorted([text, *other])
            if digest in expected: assert expected[digest] == wanted, 'One image has contradictory expected states'
            expected[digest] = wanted
    inputs = json.loads((cold/'run.json').read_text())['inputs']
    assert inputs == {digest+'.one': digest for digest in expected}, 'Cold input inventory differs from recovery images'
    records = json.loads((cold/'results.json').read_text(encoding='utf-8-sig'))
    if isinstance(records, dict): records = [records]
    assert len(records) == len(expected) and {record['name'] for record in records} == set(expected), 'Cold results omit or duplicate an image'
    for record in records:
        digest = record['name']
        assert record['source_sha256'] == digest, 'Native input hash differs'
        assert hashlib.sha256((run/'images'/f'{digest}.one').read_bytes()).hexdigest() == digest, 'Retained image changed'
        assert record['error'] is None and record['pages'] == 1, f'Native open failed: {record}'
        pages = list((cold/'results'/digest).glob('page-*.xml'))
        assert len(pages) == 1
        assert sorted(paragraphs(pages[0])) == expected[digest], f'Native content differs for {digest}'
    assert json.loads((cold/'teardown.json').read_text())['absent'], 'Cold VM remains'
    return {'exact_images':len(records), 'exact_paragraphs':sum(map(len, expected.values())), 'native_errors':0,
            'maximum_native_export_seconds':max(record['seconds'] for record in records)}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path)
    parser.add_argument('cold', type=Path)
    parser.add_argument('source_capture', type=Path)
    args = parser.parse_args()
    result = verify(args.run, args.cold, args.source_capture)
    (args.run/'cold-verification.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
