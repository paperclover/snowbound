from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/ink-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def native_page(variant):
    with TemporaryDirectory() as temporary:
        read = Path(temporary) / 'read'
        shutil.copytree(FIXTURE / variant / 'cold/read', read)
        compare(FIXTURE / variant / 'candidate', read)
    page, = (ET.parse(path).getroot() for path in sorted((FIXTURE / variant / 'cold/read').glob('page-*.xml')))
    return page


class InkEditTest(unittest.TestCase):
    def test_a_rust_drawing_renders_natively_at_its_stroke_extent(self):
        page = native_page('drawing')
        drawing, = page.findall('one:InkDrawing', ns)
        position, size = drawing.find('one:Position', ns), drawing.find('one:Size', ns)
        self.assertAlmostEqual(float(position.get('x')), 300.0, delta=0.03)
        self.assertAlmostEqual(float(position.get('y')), 120.0, delta=0.03)
        self.assertAlmostEqual(float(size.get('width')), 60.0, delta=0.06)
        self.assertAlmostEqual(float(size.get('height')), 60.0, delta=0.06)
        self.assertTrue((FIXTURE / 'drawing/cold/read/page-000.png').exists())

    def test_rust_handwriting_renders_natively_as_an_ink_paragraph(self):
        page = native_page('handwriting')
        self.assertEqual(len(list(page.iter('{%s}InkParagraph' % ns['one']))) + len(list(page.iter('{%s}InkDrawing' % ns['one']))), 1)


if __name__ == '__main__':
    unittest.main()
