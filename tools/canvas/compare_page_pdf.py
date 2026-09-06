#!/usr/bin/env python3
"""Recover page-paragraph wrap evidence from the matching native PDF export."""
import argparse
from collections import Counter
import json
from pathlib import Path
from types import SimpleNamespace
import xml.etree.ElementTree as ET

import pdfplumber
from compare import pdf_line_ends, pdf_lines
from compare_page import compare as compare_xml


def occurrences(text, needle):
    start = 0
    while needle and (start := text.find(needle, start)) != -1:
        yield start
        start += 1


def compare(probe, pages, include_titles=False):
    outlines = [o for o in probe['objects'] if o['kind'] == 'outline' and (include_titles or not o['is_title'])]
    source_paragraphs = [''.join(p['visible_text'].split()) for o in outlines for p in o['paragraphs']]
    streams = []
    for page in pages:
        chars = [dict(char, text=scalar) for char in page.chars
                 for scalar in char['text'] if not scalar.isspace()]
        streams.append((''.join(c['text'] for c in chars), chars, page.height))
    results = []
    for outline in outlines:
        compact = [''.join(p['visible_text'].split()) for p in outline['paragraphs']]
        whole = ''.join(compact)
        outline_matches = [(page, start) for page, (text, _, _) in enumerate(streams)
                           for start in occurrences(text, whole)]
        offset = 0
        for index, (paragraph, text) in enumerate(zip(outline['paragraphs'], compact)):
            row = {'outline_id': outline['id'], 'paragraph_id': paragraph['id'], 'paragraph_index': index,
                   'is_title': outline['is_title'],
                   'canvas_end_utf16': [line['end_utf16'] for line in paragraph['lines']],
                   'native_end_utf16': None, 'breaks_match': None}
            results.append(row)
            if not text:
                row['status'] = 'no_visible_glyphs'
                continue
            if outline_matches:
                matches = [(page, start + offset) for page, start in outline_matches]
                row['match_context'] = 'complete_outline'
            else:
                matches = [(page, start) for page, (stream, _, _) in enumerate(streams)
                           for start in occurrences(stream, text)]
                row['match_context'] = 'unique_paragraph'
                if sum(len(list(occurrences(source, text))) for source in source_paragraphs) != 1:
                    matches = []
                    row['match_context'] = 'ambiguous_source_context'
            offset += len(text)
            if not matches:
                row['status'] = 'unresolved_match'
                continue
            first_byte = len(paragraph['visible_text'][:next(i for i, c in enumerate(paragraph['visible_text'])
                                                            if not c.isspace())].encode('utf-8'))
            span = next(s for s in paragraph['spans'] if first_byte < s['end_utf8'])
            size = span['format'].get('font_size')
            if size is None:
                size = 11.0
            instances = []
            row['native_instances'] = instances
            for page, start in matches:
                _, chars, height = streams[page]
                matched = chars[start:start + len(text)]
                ends = pdf_line_ends(SimpleNamespace(chars=matched), paragraph['visible_text'])
                instance = {'pdf_page': page + 1, 'end_utf16': ends}
                instances.append(instance)
                groups = pdf_lines(matched)
                if groups is None:
                    continue
                baselines = [(min(c['matrix'][5] for c in group), max(c['matrix'][5] for c in group))
                             for group in groups]
                instance['baseline_ranges_from_pdf_top'] = [[height - high, height - low] for low, high in baselines]
                instance['line_advance_ranges_pdf'] = [[a[0] - b[1], a[1] - b[0]]
                                                      for a, b in zip(baselines, baselines[1:])]
                # Exported font sizes expose print scaling without fitting against canvas geometry.
                instance['first_glyph_nominal_font_scale'] = matched[0]['size'] / size
            ends = instances[0]['end_utf16']
            if ends is None or any(instance['end_utf16'] != ends for instance in instances) or any(
                    not line['text'].strip() for line in paragraph['lines']):
                row['status'] = 'unrecoverable_line_ranges'
                continue
            row['native_end_utf16'] = ends
            row['breaks_match'] = ends == row['canvas_end_utf16']
            row['status'] = 'matched' if row['breaks_match'] else 'different_wraps'
            row['canvas_baselines_in_outline'] = [paragraph['origin'][1] + line['baseline']
                                                  for line in paragraph['lines']]
            row['canvas_line_advances'] = [b['baseline'] - a['baseline']
                                           for a, b in zip(paragraph['lines'], paragraph['lines'][1:])]
    return {'title': probe['title'], 'counts': dict(Counter(row['status'] for row in results)),
            'paragraphs': results,
            'limits': ['Recovered wrap offsets require unambiguous ASCII paragraph text.',
                       'Complete-outline context disambiguates repeated paragraph text when available.',
                       'Repeated PDF occurrences must agree on every recovered break; all instances remain in the report.',
                       'PDF baseline coordinates include print scaling and pagination; they are not screen-baseline acceptance.',
                       'Empty paragraphs have no PDF glyph position; their geometry requires other evidence.']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('probe', type=Path)
    parser.add_argument('native_xml', type=Path)
    parser.add_argument('--allow-native-empty-nbsp', action='store_true')
    parser.add_argument('--include-titles', action='store_true')
    args = parser.parse_args()
    probe = json.loads(args.probe.read_text())
    compare_xml(probe, ET.parse(args.native_xml).getroot(), args.allow_native_empty_nbsp)
    with pdfplumber.open(args.native_xml.with_suffix('.pdf')) as pdf:
        result = compare(probe, pdf.pages, args.include_titles)
    print(json.dumps(result, indent=2))
