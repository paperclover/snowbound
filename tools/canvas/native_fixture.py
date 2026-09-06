#!/usr/bin/env python3
"""Build synthetic OneNote pages for tools/native_runner.py --author tools/native/pages.ps1."""
import argparse
import html
import json
import math
from pathlib import Path
import xml.etree.ElementTree as ET

NS = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
ET.register_namespace('one', NS)


def build(cases):
    pages = []
    seen = set()
    for case in cases:
        if case['id'] in seen:
            raise ValueError('Duplicate case identity')
        seen.add(case['id'])
        if not math.isfinite(case['width']) or case['width'] <= 0 or not case['runs']:
            raise ValueError('Expected positive width and at least one run')
        page = ET.Element(f'{{{NS}}}Page', {'name': case['id']})
        title = ET.SubElement(page, f'{{{NS}}}Title')
        title_oe = ET.SubElement(title, f'{{{NS}}}OE')
        ET.SubElement(title_oe, f'{{{NS}}}T').text = case['id']
        outline = ET.SubElement(page, f'{{{NS}}}Outline')
        ET.SubElement(outline, f'{{{NS}}}Position', {'x': '36', 'y': '90', 'z': '0'})
        ET.SubElement(outline, f'{{{NS}}}Size', {'width': str(case['width']), 'height': '20', 'isSetByUser': 'true'})
        children = ET.SubElement(outline, f'{{{NS}}}OEChildren')
        fragments = []
        styles = []
        for run in case['runs']:
            if not math.isfinite(run['size']) or run['size'] <= 0:
                raise ValueError('Expected positive font size')
            style = (f"font-family:{run['font']};font-size:{run['size']}pt;"
                     f"font-weight:{'bold' if run['bold'] else 'normal'};"
                     f"font-style:{'italic' if run['italic'] else 'normal'}")
            styles.append(style)
            fragments.append(f'<span style="{html.escape(style, quote=True)}">{html.escape(run["text"])}</span>')
        oe = ET.SubElement(children, f'{{{NS}}}OE', {'style': styles[0]})
        ET.SubElement(oe, f'{{{NS}}}T').text = ''.join(fragments)
        pages.append({'section': 'Layout.one', 'xml': ET.tostring(page, encoding='unicode')})
    return {'sections': ['Layout.one'], 'pages': pages}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('cases', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    fixture = build(json.loads(args.cases.read_text()))
    args.destination.mkdir(parents=True, exist_ok=False)
    (args.destination / 'fixture.json').write_text(json.dumps(fixture, ensure_ascii=False, indent=2) + '\n')
