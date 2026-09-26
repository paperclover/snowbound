from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/extended-ascii'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class ExtendedAsciiTest(unittest.TestCase):
    def test_onenote_reads_extended_ascii_as_latin_1_whatever_the_language(self):
        expected = 'Extended bytes: ' + ' '.join(chr(byte) for byte in range(128, 256))
        for language in ('en', 'ru', 'ja'):
            with self.subTest(language=language):
                capture = FIXTURE / language / 'cold'
                with TemporaryDirectory() as temporary:
                    read = Path(temporary) / 'read'
                    shutil.copytree(capture / 'read', read)
                    compare(FIXTURE / language / 'candidate', read)
                page = ET.parse(capture / 'read/page-000.xml').getroot()
                texts = [t.text for t in page.iterfind('.//one:Outline//one:T', ns)]
                self.assertEqual(texts, [expected.replace('\xa0', '&nbsp;')])


if __name__ == '__main__':
    unittest.main()
