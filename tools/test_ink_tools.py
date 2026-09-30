from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/ink-tools'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def cold_pages(variant):
    """The cold read of a Rust candidate, after checking it holds what the candidate stored."""
    with TemporaryDirectory() as temporary:
        read = Path(temporary) / 'read'
        shutil.copytree(FIXTURE / variant / 'cold/read', read)
        compare(FIXTURE / variant / 'candidate', read)
    return pages(FIXTURE / variant / 'cold/read')


def pages(read):
    return {page.get('name'): page for page in (ET.parse(path).getroot() for path in sorted(read.glob('page-*.xml')))}


def drawings(page):
    return page.findall('one:InkDrawing', ns)


def place(drawing):
    position, size = drawing.find('one:Position', ns), drawing.find('one:Size', ns)
    return [float(position.get('x')), float(position.get('y')), float(size.get('width')), float(size.get('height'))]


class InkToolsTest(unittest.TestCase):
    def assertPlaced(self, drawing, expected, delta=0.06):
        for actual, wanted in zip(place(drawing), expected, strict=True):
            self.assertAlmostEqual(actual, wanted, delta=delta)

    def test_onenote_keeps_each_stroke_and_shape_a_drawing_of_its_own(self):
        read = pages(FIXTURE / 'native-ui/read')
        self.assertEqual(len(drawings(read['Gallery'])), 14)
        shapes = [d.find('one:ShapeInfo', ns) for d in drawings(read['Pens'])]
        self.assertEqual(sorted(shape.get('isLine') for shape in shapes if shape is not None), ['false', 'false', 'true', 'true'])
        self.assertTrue((FIXTURE / 'native-ui/read/page-001.png').exists())

    def test_snowbound_pens_highlighters_and_shapes_open_in_onenote(self):
        page, = cold_pages('drawn').values()
        found = drawings(page)
        self.assertEqual(len(found), 6)
        shapes = {d.get('objectID'): d.find('one:ShapeInfo', ns) for d in found}
        lines = [s for s in shapes.values() if s is not None and s.get('isLine') == 'true']
        closed = [s for s in shapes.values() if s is not None and s.get('isLine') == 'false']
        self.assertEqual((len(lines), len(closed)), (2, 2))
        rectangle = next(s for s in closed if len(s.findall('one:AnchorPoint', ns)) == 8)
        corners = [(float(a.get('x')), float(a.get('y'))) for a in rectangle.findall('one:AnchorPoint', ns)]
        self.assertAlmostEqual(corners[0][0], 252.0, delta=0.01)
        self.assertAlmostEqual(corners[4][1], 194.4, delta=0.01)
        # The highlighter's stroke spans its points, one HIMETRIC unit larger, as OneNote reports ink.
        highlighter = next(d for d in found if abs(place(d)[3]) < 0.1)
        self.assertPlaced(highlighter, [60.0, 190.0, 160.0, 0.03])
        self.assertTrue((FIXTURE / 'drawn/cold/read/page-000.png').exists())

    def test_onenote_drawings_keep_their_ink_after_snowbound_erases_moves_and_draws(self):
        # Object identities differ between clones' reads, so drawings match by where they lie.
        before = [place(d) for d in drawings(pages(FIXTURE / 'native-ui/read')['Pens'])]
        after = [place(d) for d in drawings(cold_pages('edited')['Pens'])]
        self.assertEqual(len(after), len(before) - 1 + 6)
        near = lambda a, b: all(abs(x - y) < 0.001 for x, y in zip(a, b))
        zigzag = next(p for p in before if abs(p[2] - 90.03) < 0.01 and abs(p[3] - 22.51) < 0.01)
        line = next(p for p in before if p[2] < 0.1 and abs(p[3] - 60.04) < 0.01)
        self.assertFalse(any(near(p, zigzag) for p in after))
        self.assertTrue(any(near(p, [line[0] + 36.0, line[1] + 18.0, line[2], line[3]]) for p in after))
        for kept in before:
            if kept not in (zigzag, line):
                self.assertTrue(any(near(p, kept) for p in after), kept)


if __name__ == '__main__':
    unittest.main()
