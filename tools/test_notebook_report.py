import base64
from io import BytesIO
import json
from pathlib import Path
import re
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from PIL import Image
from notebook_report import generate


class NotebookReportTest(unittest.TestCase):
    def test_tiff_preview_matches_native_pixels_and_retains_original_bytes(self):
        fixture = Path(__file__).resolve().parent.parent / 'corpus/m6/native-features-01'
        with TemporaryDirectory() as temporary:
            output = Path(temporary) / 'report'
            generate(fixture / 'notebook', output)
            page, = [p for p in json.loads((output / 'pages.json').read_text()) if p['title'] == 'Image tiff']
            source, = re.findall(r'<img[^>]*src="([^"]+)"', (output / page['report']).read_text())
            preview = output / source
            self.assertEqual(preview.suffix, '.png')
            original, = (output / 'assets').glob('*.tiff')
            self.assertEqual(original.read_bytes(), (fixture / 'notebook/fixture-data/pattern.tiff').read_bytes())
            native, = [root for p in (fixture / 'read').glob('page-*.xml')
                       if (root := ET.parse(p).getroot()).get('name') == 'Image tiff']
            data, = [n.text for n in native.iter() if n.tag.endswith('}Data')]
            with Image.open(BytesIO(base64.b64decode(data))) as expected, Image.open(preview) as actual:
                self.assertEqual(actual.size, expected.size)
                self.assertEqual(actual.convert('RGBA').tobytes(), expected.convert('RGBA').tobytes())
            references = [a['path'] for p in (output / 'model').glob('*/assets.json') for a in json.loads(p.read_text())]
            self.assertIn('../../assets/' + original.name, references)


if __name__ == '__main__':
    unittest.main()
