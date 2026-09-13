import json
from pathlib import Path
import runpy
import subprocess
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from document_model import EXPORTER, ordered_pages
from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/native-ink/cold-ui-ink'
ink_extent = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['ink_extent']


class InkReadTest(unittest.TestCase):
    def test_decoded_ink_strokes_span_the_native_drawing_extents(self):
        """An independent decoding of the stored stroke packets reproduces the native positions and sizes."""
        with TemporaryDirectory() as temporary:
            exported = Path(temporary) / 'document'
            subprocess.run([EXPORTER, FIXTURE / 'notebook/synthetic.one', exported], check=True)
            document = json.loads((exported / 'document.json').read_text())
        (_, _, revision, page_id), = ordered_pages(document)
        source_page = revision['nodes'][page_id]
        native, = (ET.parse(path).getroot() for path in sorted((FIXTURE / 'read').glob('page-*.xml')))
        drawings = native.findall('one:InkDrawing', ns)
        self.assertEqual(len(drawings), 2)
        for drawing in drawings:
            z = int(drawing.find('one:Position', ns).get('z'))
            source = revision['nodes'][source_page['children'][z]]
            self.assertEqual(source['kind']['type'], 'Ink')
            position, size = drawing.find('one:Position', ns), drawing.find('one:Size', ns)
            expected = [float(position.get('x')), float(position.get('y')),
                        float(size.get('width')) - 72 / 2540, float(size.get('height')) - 72 / 2540]
            extent = ink_extent(revision, source)
            for actual, wanted in zip(extent, expected):
                self.assertAlmostEqual(actual, wanted, delta=0.002)


if __name__ == '__main__':
    unittest.main()
