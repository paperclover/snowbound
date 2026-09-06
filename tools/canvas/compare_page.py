#!/usr/bin/env python3
"""Compare owned-page probe geometry with native XML after exact text matching."""
import argparse
from html.parser import HTMLParser
import json
from pathlib import Path
import xml.etree.ElementTree as ET

NS = {'one': 'http://schemas.microsoft.com/office/onenote/2010/onenote'}


class Text(HTMLParser):
    def __init__(self, markup):
        super().__init__(convert_charrefs=True)
        self.parts = []
        self.feed(markup)
        self.close()

    def handle_data(self, data):
        self.parts.append(data)

    def handle_starttag(self, tag, attrs):
        if tag == 'br':
            self.parts.append('\n')


def compare(probe, native, allow_native_empty_nbsp=False):
    if probe['title'] != native.get('name'):
        raise ValueError('Source and native page titles differ')
    outlines = native.findall('one:Outline', NS)
    available = []
    for outline in outlines:
        paragraphs = outline.findall('.//one:OE', NS)
        text = tuple(''.join(''.join(Text(t.text or '').parts)
                             for t in p.findall('one:T', NS)) for p in paragraphs)
        if any(p.find('one:Image', NS) is not None or p.find('one:Table', NS) is not None
               for p in paragraphs):
            raise ValueError('Native outline includes non-text paragraph content')
        available.append((text, outline))
    results = []
    for outline in probe['objects']:
        if outline['kind'] != 'outline' or outline['is_title']:
            continue
        paragraphs = outline['paragraphs']
        if outline['unsupported'] or any(p['unsupported'] for p in paragraphs):
            raise ValueError('Source outline includes unsupported content')
        text = tuple(p['visible_text'] for p in paragraphs)
        match_text = tuple('' if t == '\u00a0' else t for t in text) if allow_native_empty_nbsp else text
        matches = [(index, native) for index, (candidate, native) in enumerate(available)
                   if candidate == match_text]
        if len(matches) != 1:
            raise ValueError(f'{outline["id"]}: expected one native outline text match; got {len(matches)}')
        index, matched = matches[0]
        del available[index]
        position = matched.find('one:Position', NS).attrib
        size = matched.find('one:Size', NS).attrib
        layout = outline['layout']
        height = outline['size'][1] if outline.get('size') else sum(p['height'] for p in paragraphs)
        results.append({
            'source_id': outline['id'], 'native_id': matched.get('objectID'),
            'paragraphs': len(paragraphs),
            'text_matches_exactly': text == match_text,
            'native_empty_nbsp_paragraphs': [i for i, (a, b) in enumerate(zip(text, match_text)) if a != b],
            'source_width': layout['max_width'], 'native_width': float(size['width']),
            'layout_height': height, 'native_height': float(size['height']),
            'height_residual': height - float(size['height']),
            'inferred_native_margin_origin': [float(position[axis]) - layout[axis] + margin
                for axis, margin in zip(('x', 'y'), probe['margin_origin'])],
            'nested_paragraphs': sum(p['parent'] is not None for p in paragraphs),
            'list_paragraphs': sum(bool(p['lists']) for p in paragraphs),
        })
    if available:
        raise ValueError('Native capture contains unmatched outlines')
    return {'title': probe['title'], 'outlines': results,
            'limits': ['Text matching identifies outlines; this comparison does not verify run formatting.',
                       probe['measurement'],
                       'Total height does not establish wrap offsets or first-baseline placement.',
                       'Inferred margin origins are observations, not a general page-origin rule.']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('probe', type=Path)
    parser.add_argument('native_xml', type=Path)
    parser.add_argument('--allow-native-empty-nbsp', action='store_true',
                        help='Record native empty paragraphs replacing a sole source NBSP')
    args = parser.parse_args()
    print(json.dumps(compare(json.loads(args.probe.read_text()), ET.parse(args.native_xml).getroot(),
                             args.allow_native_empty_nbsp), indent=2))
