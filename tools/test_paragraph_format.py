from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_format import native_characters
from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/paragraph-format'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']
IGNORED = {'objectID', 'lastModifiedTime', 'creationTime'}


def paragraphs(read):
    for path in sorted(read.glob('page-*.xml')):
        root = ET.parse(path).getroot()
        if root.get('name') == 'Paragraph formatting':
            outline, = root.findall('one:Outline', ns)
            return [({k: v for k, v in oe.attrib.items() if k not in IGNORED}, oe)
                    for oe in outline.findall('one:OEChildren/one:OE', ns)], root
    raise AssertionError('The paragraph formatting page is missing')


class ParagraphFormatTest(unittest.TestCase):
    def test_native_controls_and_rust_paragraph_properties_reopen_natively(self):
        with TemporaryDirectory() as temporary:
            for notebook, read in [(FIXTURE / 'before', FIXTURE / 'before/read'),
                                   (FIXTURE / 'candidate', FIXTURE / 'cold/read')]:
                folder = Path(temporary) / notebook.name
                shutil.copytree(read, folder / 'read')
                compare(notebook, folder / 'read')
        before, before_page = paragraphs(FIXTURE / 'before/read')
        after, after_page = paragraphs(FIXTURE / 'cold/read')
        self.assertEqual(len(before), 6)
        self.assertEqual([attributes for attributes, _ in before], [
            {'alignment': 'left', 'quickStyleIndex': '0'},
            {'alignment': 'left', 'quickStyleIndex': '0'},
            {'alignment': 'center', 'spaceBefore': '12.0', 'spaceAfter': '6.0', 'spaceBetween': '20.0', 'quickStyleIndex': '0'},
            {'RTL': 'true', 'alignment': 'right', 'spaceBefore': '3.0', 'spaceAfter': '9.0', 'spaceBetween': '18.0', 'quickStyleIndex': '0'},
            {'alignment': 'left', 'spaceBefore': '288.0', 'spaceAfter': '144.0', 'quickStyleIndex': '1'},
            {'alignment': 'center', 'spaceBefore': '288.0', 'spaceAfter': '2.0', 'spaceBetween': '16.0', 'quickStyleIndex': '1'},
        ])
        for index, ((original, _), (attributes, _)) in enumerate(zip(before, after, strict=True)):
            expected = {'alignment': 'center', 'spaceAfter': '2.0', 'spaceBetween': '16.0',
                        'quickStyleIndex': original['quickStyleIndex']}
            if original.get('RTL') == 'true':
                expected['RTL'] = 'true'
            self.assertEqual(attributes, expected, index)
        self.assertEqual(native_characters(after_page, after_page.findall('one:Outline', ns)),
                         native_characters(before_page, before_page.findall('one:Outline', ns)))


if __name__ == '__main__':
    unittest.main()
