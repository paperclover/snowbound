from pathlib import Path
import hashlib
import json
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/attachment-insert'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class AttachmentInsertTest(unittest.TestCase):
    def test_files_attached_through_the_editor_open_in_onenote_with_their_bytes(self):
        read = FIXTURE / 'cold/read'
        with TemporaryDirectory() as temporary:
            copy = Path(temporary) / 'read'
            shutil.copytree(read, copy)
            compare(FIXTURE / 'candidate', copy)
        page, = (ET.parse(path).getroot() for path in sorted(read.glob('page-*.xml')))
        paragraphs = []
        for element in page.findall('.//one:OE', ns):
            inserted = element.find('one:InsertedFile', ns)
            text = element.find('one:T', ns)
            paragraphs.append('[%s]' % inserted.get('preferredName') if inserted is not None
                              else re.sub(r'<[^>]*>', '', text.text or ''))
        self.assertEqual(paragraphs, ['Before the file', '[notes 🦀.txt]', 'After the file', '[large.bin]', ''])
        # The bytes OneNote hands an application when the file is opened.
        payloads = json.loads((read / 'payloads.json').read_text(encoding='utf-8-sig'))
        self.assertEqual(sorted(p['bytes'] for p in payloads), [30, 2 << 20])
        for payload in payloads:
            data = (read / (payload['sha256'] + '.attachment')).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), payload['sha256'])


if __name__ == '__main__':
    unittest.main()
