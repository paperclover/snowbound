from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/rtl-page'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class RtlPageTest(unittest.TestCase):
    def test_a_right_to_left_page_edited_in_snowbound_reopens_where_it_was_put(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        page = ET.parse(FIXTURE / 'cold/read/page-000.xml').getroot()
        self.assertEqual(page.find('one:PageSettings', ns).get('RTL'), 'true')
        positions = {''.join(outline.find('.//one:T', ns).itertext()): outline.find('one:Position', ns).attrib
                     for outline in page.findall('one:Outline', ns) if outline.find('.//one:T', ns) is not None}
        self.assertEqual((positions['Typed on a right-to-left page']['x'], positions['Fictitious positioned outline.']['x']),
                         ('126.0', '180.0'))
        self.assertEqual(len(page.findall('one:InkDrawing', ns)), 3)


if __name__ == '__main__':
    unittest.main()
