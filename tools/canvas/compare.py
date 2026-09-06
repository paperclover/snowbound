#!/usr/bin/env python3
"""Compare text probe output with native XML geometry and recoverable PDF line ranges."""
import argparse
import json
from pathlib import Path
import xml.etree.ElementTree as ET

import pdfplumber
from native_fixture import build

NS = {'one': 'http://schemas.microsoft.com/office/onenote/2010/onenote'}


def pdf_lines(chars):
    lines = []
    for char in sorted(chars, key=lambda c: c['matrix'][5], reverse=True):
        overlaps = [line for line in lines if max(line[0], char['y0']) < min(line[1], char['y1'])]
        if len(overlaps) > 1:
            return None
        if overlaps:
            line = overlaps[0]
            line[0] = max(line[0], char['y0'])
            line[1] = min(line[1], char['y1'])
            line[2].append(char)
        else:
            lines.append([char['y0'], char['y1'], [char]])
    return [sorted(line[2], key=lambda c: c['x0']) for line in lines]


def pdf_line_ends(page, source):
    if not source or not source.isascii():
        return None
    groups = pdf_lines(page.chars)
    if groups is None:
        return None
    lines = [''.join(c['text'] for c in chars) for chars in groups]
    compact_source = ''.join(source.split())
    matches = []
    for start in range(len(lines)):
        for end in range(start + 1, len(lines) + 1):
            if ''.join(''.join(lines[start:end]).split()) == compact_source:
                matches.append(lines[start:end])
    if len(matches) != 1:
        return None
    offset = 0
    ends = []
    for line in matches[0]:
        for character in ''.join(line.split()):
            while offset < len(source) and source[offset].isspace():
                offset += 1
            if offset == len(source) or source[offset] != character:
                return None
            offset += 1
        while offset < len(source) and source[offset].isspace():
            offset += 1
        ends.append(offset)
    return ends if offset == len(source) else None


def compare(cases, parley, core_text, native):
    if json.loads((native.parent / 'notebook' / 'fixture.json').read_text(encoding='utf-8-sig')) != build(cases):
        raise ValueError('Native capture was generated from different input cases')
    parley = {case['id']: case for case in parley['cases']}
    core_text = {case['id']: case for case in core_text['cases']}
    pages = {}
    for path in native.glob('page-*.xml'):
        page = ET.parse(path).getroot()
        name = page.get('name')
        if name in pages:
            raise ValueError(f'Duplicate native page: {name}')
        pages[name] = (path, page)
    ids = {case['id'] for case in cases}
    if len(ids) != len(cases) or ids != parley.keys() or ids != core_text.keys() or ids != pages.keys():
        raise ValueError('Input and captured case identities must match exactly')
    results = []
    for case in cases:
        name = case['id']
        p, c = parley[name], core_text[name]
        for result in (p, c):
            if result['width'] != case['width']:
                raise ValueError(f'{name}: probe used a different width')
            actual = [{k: v for k, v in run.items() if k != 'font'} for run in result['requested_runs']]
            expected = [{k: v for k, v in run.items() if k != 'font'} for run in case['runs']]
            if actual != expected:
                raise ValueError(f'{name}: probe used different text or styles')
        path, page = pages[name]
        sizes = page.findall('one:Outline/one:Size', NS)
        source = ''.join(run['text'] for run in case['runs'])
        native_ends = None
        with pdfplumber.open(path.with_suffix('.pdf')) as pdf:
            if len(pdf.pages) == 1:
                native_ends = pdf_line_ends(pdf.pages[0], source)
        dimensions = [{k: float(size.attrib[k]) for k in ('width', 'height')} for size in sizes]
        width_changed = len(dimensions) != 1 or abs(dimensions[0]['width'] - case['width']) > 0.001
        windows_height = 0.0
        for line in p['lines']:
            runs = line['runs']
            if not runs or any(r[k] is None for r in runs for k in ('win_ascent', 'win_descent', 'units_per_em')):
                windows_height = None
                break
            windows_height += max(r['win_ascent'] * r['size'] / r['units_per_em'] for r in runs)
            windows_height += max(r['win_descent'] * r['size'] / r['units_per_em'] for r in runs)
        ends = {engine: [line['end_utf16'] for line in result['lines']]
                for engine, result in [('parley', p), ('core_text', c)]}
        results.append({
            'id': name,
            'requested_font_override': {engine: [r['font'] for r in result['requested_runs']]
                                        for engine, result in [('parley', p), ('core_text', c)]}
                if any(result['requested_runs'] != case['runs'] for result in (p, c)) else None,
            'requested_width': case['width'],
            'native_outlines': dimensions,
            'native_changed_width_or_removed_outline': width_changed,
            'native_pdf_end_utf16': native_ends,
            'end_utf16': ends,
            'parley_default_height': p['height'],
            'canvas_height': p.get('windows_height'),
            'canvas_height_residual': (p['windows_height'] - dimensions[0]['height'])
                if p.get('windows_height') is not None and not width_changed else None,
            'windows_metric_height_hypothesis': windows_height,
            'height_residual_hypothesis': (windows_height - dimensions[0]['height'])
                if windows_height is not None and not width_changed else None,
            'parley_resolved_faces': sorted({r['face'] for line in p['lines'] for r in line['runs']}),
            'core_text_requested_faces_resolved': c['requested_faces_resolved'],
            'same_width_pdf_break_agreement': {engine: values == native_ends for engine, values in ends.items()}
                if native_ends is not None and not width_changed else None,
        })
    return {'cases': results,
            'limits': ['PDF ranges are supplementary evidence, recovered only for unambiguous ASCII text.',
                       'Metric hypotheses use the Mac-resolved fonts and do not establish matching native font identity.',
                       'Width changes and removed empty outlines require separate native cases.',
                       'XML height alone does not establish first-baseline placement.']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('cases', type=Path)
    parser.add_argument('parley', type=Path)
    parser.add_argument('core_text', type=Path)
    parser.add_argument('native_read', type=Path)
    args = parser.parse_args()
    result = compare(json.loads(args.cases.read_text()), json.loads(args.parley.read_text()),
                     json.loads(args.core_text.read_text()), args.native_read)
    print(json.dumps(result, indent=2))
