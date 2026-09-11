#!/usr/bin/env python3
"""Generate schema-validated inputs for native document feature controls."""
import argparse
import json
import os
from pathlib import Path
import shutil
import wave

from lxml import etree as ET
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
NS = 'http://schemas.microsoft.com/office/onenote/2010/onenote'


def generate(destination):
    destination.mkdir(parents=True, exist_ok=False)
    notebook = destination / 'notebook'
    shutil.copytree(ROOT / 'corpus/writer/create-notebook-01/notebook', notebook)
    assets = notebook / 'fixture-data'; assets.mkdir()
    image = Image.new('RGB', (96, 64))
    image.putdata([(x * 2, y * 3, (x + y) % 256) for y in range(64) for x in range(96)])
    for extension in ('png', 'jpg', 'bmp', 'tiff', 'gif'):
        image.save(assets / ('pattern.' + extension))
    (assets / 'attachment.txt').write_bytes(b'Fictitious attachment\r\n\x00\xff\r\n')
    with wave.open(str(assets / 'silence.wav'), 'wb') as sound:
        sound.setparams((1, 2, 8000, 8000, 'NONE', 'not compressed'))
        sound.writeframes(b'\0' * 16000)
    fixture = {'sections': ['Features.one', 'Group A/Duplicate.one', 'Group B/Nested/Duplicate.one', 'Empty.one'], 'pages': []}
    xsd = os.environ.get('ONENOTE_2010_XSD')
    if not xsd:
        raise SystemExit('Set ONENOTE_2010_XSD to the OneNote 2010 COM schema (OneNote2010.xsd); it is not distributed with this repository.')
    schema = ET.XMLSchema(ET.parse(xsd))

    def page(title, content, section='Features.one', attributes='', definitions=''):
        xml = f'<one:Page xmlns:one="{NS}" {attributes}>{definitions}<one:Title><one:OE><one:T>{title}</one:T></one:OE></one:Title>{content}</one:Page>'
        schema.assertValid(ET.fromstring(xml.encode()))
        fixture['pages'].append({'section': section, 'xml': xml})

    def outline(body, x=72, y=108):
        return f'<one:Outline><one:Position x="{x}" y="{y}"/><one:Size width="480" height="400"/><one:OEChildren>{body}</one:OEChildren></one:Outline>'

    for extension in ('png', 'jpg', 'bmp', 'tiff', 'gif'):
        body = f'<one:Image format="auto" alt="Pattern {extension}" hyperlink="https://example.invalid/image/{extension}"><one:Position x="72" y="108"/><one:Size width="144" height="48"/><one:File path="__ASSETS__\\pattern.{extension}"/></one:Image>'
        page('Image ' + extension, body)
    for background, printout in [('false', 'false'), ('true', 'false'), ('false', 'true'), ('true', 'true')]:
        page(f'Image role background={background}, printout={printout}', f'<one:Image format="auto" backgroundImage="{background}" isPrintOut="{printout}" originalPageNumber="2" alt="Role pattern"><one:Position x="36" y="36"/><one:Size width="288" height="192"/><one:File path="__ASSETS__\\pattern.png"/></one:Image>' + outline('<one:OE><one:T>Foreground role control</one:T></one:OE>'))
    page('Files and recording', outline('<one:OE><one:InsertedFile pathSource="__ASSETS__\\attachment.txt" preferredName="Fictitious attachment.txt"/></one:OE><one:OE><one:MediaFile pathSource="__ASSETS__\\silence.wav" preferredName="silence.wav"><one:MediaReference mediaID="{9A418E57-7F64-40C5-87D0-8A8F7C1E5422}"/></one:MediaFile></one:OE><one:OE><one:MediaIndex timeIndex="500"><one:MediaReference mediaID="{9A418E57-7F64-40C5-87D0-8A8F7C1E5422}"/></one:MediaIndex><one:T>Recording annotation</one:T></one:OE>'))
    page('Paragraph controls', outline('<one:OE quickStyleIndex="0"><one:T>Inherited strike and spacing</one:T></one:OE><one:OE alignment="center" spaceBefore="12" spaceAfter="6"><one:T>Centered paragraph</one:T></one:OE><one:OE RTL="true" alignment="right"><one:T>مرحبا بالعالم</one:T></one:OE><one:OE collapsed="true"><one:T>Collapsed parent</one:T><one:OEChildren indent="2"><one:OE><one:T>Hidden child remains stored</one:T><one:OEChildren><one:OE><one:T>Grandchild</one:T></one:OE></one:OEChildren></one:OE></one:OEChildren></one:OE><one:OE><one:T><![CDATA[A<br><br>B&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;C]]></one:T></one:OE>'), definitions='<one:QuickStyleDef index="0" name="Fixture style" font="Calibri" fontSize="14" strikethrough="true" spaceBefore="0.25" spaceAfter="0.125"/>')
    body = ''
    for sequence in range(5):
        for index in range(2):
            restart = ' restartNumberingAt="3"' if index == 0 else ''
            body += f'<one:OE><one:List><one:Number numberSequence="{sequence}" numberFormat="##"{restart} font="Calibri"/></one:List><one:T>Sequence {sequence}, item {index}</one:T></one:OE>'
    page('Numbering controls', outline(body))
    page('Task controls', outline('<one:OE><one:OutlookTask guidTask="{45AF40E3-1D54-463C-90D3-C454F2F71A2E}" completed="false" disabled="true" creationDate="2020-01-02T03:04:05Z" startDate="2020-01-02T00:00:00Z" dueDate="2020-01-03T00:00:00Z"/><one:T>Disabled standalone task</one:T></one:OE>'))
    for section in ('Group A/Duplicate.one', 'Group B/Nested/Duplicate.one'):
        for level in (1, 2, 3):
            page('Repeated title', outline(f'<one:OE><one:T>Group path {section}, level {level}</one:T></one:OE>'), section, f'pageLevel="{level}"')
    (notebook / 'fixture.json').write_text(json.dumps(fixture, ensure_ascii=False, indent=2))
    print(f'Generated {len(fixture["pages"])} schema-validated native pages; expected total {len(fixture["pages"]) + 1}.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    generate(parser.parse_args().destination)
