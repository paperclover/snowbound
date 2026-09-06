#!/usr/bin/env python3
"""Create a synthetic native baseline page with independent raster coordinate anchors."""
import argparse
import base64
import json
from pathlib import Path
import struct
import xml.etree.ElementTree as ET
import zlib

from native_fixture import NS


def build():
    def chunk(kind, data):
        return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data))

    page = ET.Element(f'{{{NS}}}Page', {'name': 'Baseline anchors'})
    ET.SubElement(page, f'{{{NS}}}QuickStyleDef', {
        'index': '0', 'name': 'p', 'font': 'Calibri', 'fontSize': '11',
        'fontColor': 'automatic', 'highlightColor': 'automatic', 'spaceBefore': '0', 'spaceAfter': '0',
    })
    ET.SubElement(page, f'{{{NS}}}QuickStyleDef', {
        'index': '1', 'name': 'h2', 'font': 'Arial', 'fontSize': '11', 'bold': 'false',
        'fontColor': 'automatic', 'highlightColor': 'automatic', 'spaceBefore': '3', 'spaceAfter': '4',
    })
    title = ET.SubElement(page, f'{{{NS}}}Title')
    oe = ET.SubElement(title, f'{{{NS}}}OE')
    ET.SubElement(oe, f'{{{NS}}}T').text = 'Baseline anchors'
    for index, (x, y, width, height) in enumerate([
        (20.125, 60.25, 11, 13), (440.5, 355.375, 17, 19), (25.75, 650.875, 23, 29),
    ]):
        color = bytes(255 if channel == index else 0 for channel in range(3))
        pixels = (b'\x00' + color * (width * 2)) * (height * 2)
        png = (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!2I5B', width * 2, height * 2, 8, 2, 0, 0, 0))
               + chunk(b'IDAT', zlib.compress(pixels)) + chunk(b'IEND', b''))
        image = ET.SubElement(page, f'{{{NS}}}Image', {'format': 'png'})
        ET.SubElement(image, f'{{{NS}}}Position', {'x': str(x), 'y': str(y), 'z': str(index)})
        ET.SubElement(image, f'{{{NS}}}Size', {'width': str(width), 'height': str(height), 'isSetByUser': 'true'})
        ET.SubElement(image, f'{{{NS}}}Data').text = base64.b64encode(png).decode()
    for index, (y, width, font, size, paragraphs) in enumerate([
        (90, 300, 'Arial', 11, ['Arial eleven HAMBURGEFONTS']),
        (150, 300, 'Arial', 16, ['Arial sixteen HAMBURGEFONTS']),
        (210, 300, 'Calibri', 11, ['Calibri eleven HAMBURGEFONTS']),
        (270, 300, 'Calibri', 17, ['Calibri seventeen HAMBURGEFONTS']),
        (330, 110, 'Arial', 11, ['Wrapped HAMBURGEFONTS abcdefghijklmnopqrstuvwxyz 0123456789']),
        (420, 300, 'Arial', 11, ['Mixed <span style="font-family:Arial;font-size:16pt">HAMBURGEFONTS</span> small']),
        (490, 300, 'Arial', 11, ['Spacing first', '', 'Spacing last']),
        (570, 300, 'Calibri', 11, ['Blank first', '', 'Blank last']),
    ]):
        outline = ET.SubElement(page, f'{{{NS}}}Outline')
        ET.SubElement(outline, f'{{{NS}}}Position', {'x': '120', 'y': str(y), 'z': str(index + 3)})
        ET.SubElement(outline, f'{{{NS}}}Size', {'width': str(width), 'height': '20', 'isSetByUser': 'true'})
        children = ET.SubElement(outline, f'{{{NS}}}OEChildren')
        for text in paragraphs:
            style = f'font-family:{font};font-size:{size}pt'
            oe = ET.SubElement(children, f'{{{NS}}}OE', {'style': style})
            if y == 490:
                oe.set('quickStyleIndex', '1')
            ET.SubElement(oe, f'{{{NS}}}T').text = text
    return {'sections': ['Baselines.one'], 'pages': [{'section': 'Baselines.one', 'xml': ET.tostring(page, encoding='unicode')}]}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    args.destination.mkdir(parents=True, exist_ok=False)
    (args.destination / 'fixture.json').write_text(json.dumps(build(), indent=2) + '\n')
