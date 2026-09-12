import base64
from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/picture-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class PictureEditTest(unittest.TestCase):
    def test_a_rust_inserted_picture_renders_natively_with_its_payload(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        page, = (ET.parse(path).getroot() for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml')))
        image, = page.iter('{%s}Image' % ns['one'])
        self.assertEqual(image.get('format'), 'png')
        payload = base64.b64decode(image.find('one:Data', ns).text)
        self.assertEqual(payload[:8], b'\x89PNG\r\n\x1a\n')
        self.assertEqual(len(payload), 69)
        texts = [re.sub(r'<[^>]*>', '', t.text or '') for t in page.findall('.//one:OE/one:T', ns)]
        self.assertEqual(texts, ['Before the picture', 'After the picture'])


if __name__ == '__main__':
    unittest.main()
