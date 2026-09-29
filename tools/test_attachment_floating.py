from pathlib import Path
import hashlib
import json
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/attachment-floating'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class AttachmentFloatingTest(unittest.TestCase):
    def test_files_the_editor_put_on_the_page_open_in_onenote_where_they_were_placed(self):
        read = FIXTURE / 'cold/read'
        with TemporaryDirectory() as temporary:
            copy = Path(temporary) / 'read'
            shutil.copytree(read, copy)
            compare(FIXTURE / 'candidate', copy)
        page, = (ET.parse(path).getroot() for path in sorted(read.glob('page-*.xml')))
        self.assertEqual(page.findall('.//one:OE/one:InsertedFile', ns), [])
        placed = {}
        for inserted in page.findall('one:InsertedFile', ns):
            position = inserted.find('one:Position', ns)
            placed[inserted.get('preferredName')] = (
                round(float(position.get('x')), 2), round(float(position.get('y')), 2))
        # Attached at the caret a click on blank page left, the second then dragged.
        self.assertEqual(placed, {'float 🦀.txt': (342.0, 284.4), 'large.bin': (288.0, 140.4)})
        # The bytes OneNote hands an application when the file is opened.
        payloads = json.loads((read / 'payloads.json').read_text(encoding='utf-8-sig'))
        self.assertEqual(sorted((p['name'], p['bytes']) for p in payloads),
                         [('float 🦀.txt', 37), ('large.bin', 2 << 20)])
        for payload in payloads:
            data = (read / (payload['sha256'] + '.attachment')).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), payload['sha256'])


if __name__ == '__main__':
    unittest.main()
