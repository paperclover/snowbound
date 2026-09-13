from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/media-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class MediaEditTest(unittest.TestCase):
    def test_an_edit_next_to_a_recording_keeps_the_recording_and_its_annotation(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        pages = [ET.parse(path).getroot() for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml'))]
        page = next(page for page in pages if page.get('name') == 'Files and recording')
        media = page.find('.//one:MediaFile', ns)
        self.assertEqual(media.get('preferredName'), 'silence.wav')
        recording = media.find('one:MediaReference', ns).get('mediaID')
        index = page.find('.//one:MediaIndex', ns)
        self.assertEqual(index.get('timeIndex'), '500')
        self.assertEqual(index.find('one:MediaReference', ns).get('mediaID'), recording)
        texts = [t.text for t in page.findall('.//one:T', ns) if t.text]
        self.assertIn('Added after recording', texts)


if __name__ == '__main__':
    unittest.main()
