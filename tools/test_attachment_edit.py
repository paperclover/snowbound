from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/attachment-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class AttachmentEditTest(unittest.TestCase):
    def check(self, variant):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / variant / 'cold/read', read)
            compare(FIXTURE / variant / 'candidate', read)
        page, = (ET.parse(path).getroot() for path in sorted((FIXTURE / variant / 'cold/read').glob('page-*.xml')))
        inserted, = page.iter('{%s}InsertedFile' % ns['one'])
        self.assertEqual(inserted.get('preferredName'), 'notes 🦀.txt')
        texts = [re.sub(r'<[^>]*>', '', t.text or '') for t in page.findall('.//one:OE/one:T', ns)]
        self.assertEqual(texts, ['Before the file', 'After the file'])

    def test_a_rust_inserted_attachment_renders_natively_with_its_payload(self):
        self.check('plain')

    def test_a_rust_inserted_attachment_with_an_icon_preview_renders_natively(self):
        self.check('icon')


if __name__ == '__main__':
    unittest.main()
