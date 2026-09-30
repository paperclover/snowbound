from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/printout'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def pictures(read):
    """The printout page's pictures in order: printout state, printout page and position."""
    for path in sorted(read.glob('page-*.xml')):
        page = ET.parse(path).getroot()
        if page.get('name') == 'Printout':
            return [(image.get('isPrintOut'), image.get('originalPageNumber'),
                     image.find('one:Position', ns).get('y'))
                    for image in page.findall('one:Image', ns)]


class PrintoutTest(unittest.TestCase):
    def test_edited_printout_reopens_natively(self):
        for notebook, read in (('native/notebook', 'native/read'), ('candidate', 'cold/read')):
            with TemporaryDirectory() as temporary:
                copy = Path(temporary) / 'read'
                shutil.copytree(FIXTURE / read, copy)
                compare(FIXTURE / notebook, copy)
        native = pictures(FIXTURE / 'native/read')
        self.assertEqual([(p, n) for p, n, _ in native], [('true', '0'), ('true', '1')])
        # The first page moved half an inch down stays a printout; the second, deleted and
        # restored, is the picture OneNote showed of it.
        cold = pictures(FIXTURE / 'cold/read')
        self.assertEqual(cold, [('true', '0', str(float(native[0][2]) + 36)), (None, None, native[1][2])])


if __name__ == '__main__':
    unittest.main()
