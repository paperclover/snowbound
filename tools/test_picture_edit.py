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


def native_page(notebook, cold):
    with TemporaryDirectory() as temporary:
        read = Path(temporary) / 'read'
        shutil.copytree(cold / 'read', read)
        compare(notebook, read)
    page, = (ET.parse(path).getroot() for path in sorted((cold / 'read').glob('page-*.xml')))
    return page


def texts(page):
    return [re.sub(r'<[^>]*>', '', t.text or '') for t in page.findall('.//one:OE/one:T', ns)]


class PictureEditTest(unittest.TestCase):
    def test_a_rust_inserted_picture_renders_natively_with_its_payload(self):
        page = native_page(FIXTURE / 'inserted/candidate', FIXTURE / 'inserted/cold')
        image, = page.iter('{%s}Image' % ns['one'])
        self.assertEqual(image.get('format'), 'png')
        payload = base64.b64decode(image.find('one:Data', ns).text)
        self.assertEqual(payload[:8], b'\x89PNG\r\n\x1a\n')
        self.assertEqual(len(payload), 69)
        self.assertIsNone(image.find('one:Size', ns))
        self.assertEqual(texts(page), ['Before the picture', 'After the picture'])

    def test_onenote_resizes_a_rust_inserted_picture_through_its_layout(self):
        page = native_page(FIXTURE / 'native-resize/notebook', FIXTURE / 'native-resize')
        image, = page.iter('{%s}Image' % ns['one'])
        size = image.find('one:Size', ns)
        self.assertEqual((size.get('width'), size.get('height'), size.get('isSetByUser')), ('144.0', '108.0', 'true'))
        self.assertEqual(image.get('alt'), 'Resized by OneNote')
        self.assertEqual(len(base64.b64decode(image.find('one:Data', ns).text)), 69)

    def test_a_rust_resized_native_picture_renders_with_its_size_and_description(self):
        page = native_page(FIXTURE / 'resized/candidate', FIXTURE / 'resized/cold')
        image, = page.iter('{%s}Image' % ns['one'])
        size = image.find('one:Size', ns)
        self.assertEqual((size.get('width'), size.get('height'), size.get('isSetByUser')), ('144.0', '108.0', 'true'))
        self.assertEqual(image.get('alt'), 'Resized in Rust')

    def test_onenote_places_a_picture_directly_on_a_page_with_a_rust_picture(self):
        page = native_page(FIXTURE / 'native-page-level/notebook', FIXTURE / 'native-page-level')
        images = list(page.iter('{%s}Image' % ns['one']))
        self.assertEqual(len(images), 2)
        placed = page.find('one:Image', ns)
        self.assertEqual(placed.get('alt'), 'Page-level picture')
        position = placed.find('one:Position', ns)
        self.assertEqual((position.get('x'), position.get('y')), ('360.0', '240.0'))

    def test_a_rust_page_level_picture_renders_with_its_position_size_and_description(self):
        page = native_page(FIXTURE / 'page-level/candidate', FIXTURE / 'page-level/cold')
        placed = page.find('one:Image', ns)
        self.assertIsNotNone(placed)
        position = placed.find('one:Position', ns)
        self.assertEqual((position.get('x'), position.get('y')), ('360.0', '240.0'))
        size = placed.find('one:Size', ns)
        self.assertEqual((size.get('width'), size.get('height'), size.get('isSetByUser')), ('96.00000762939453', '71.99998474121093', 'true'))
        self.assertEqual(placed.get('alt'), 'Placed in Rust')
        self.assertEqual(texts(page), ['Beside the picture'])


if __name__ == '__main__':
    unittest.main()
