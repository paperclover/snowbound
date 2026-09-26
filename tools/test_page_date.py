from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/page-date'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def native_pages(notebook, capture):
    with TemporaryDirectory() as temporary:
        read = Path(temporary) / 'read'
        shutil.copytree(capture / 'read', read)
        compare(notebook, read)
    pages = (ET.parse(path).getroot() for path in sorted((capture / 'read').glob('page-*.xml')))
    return {page.get('name'): page for page in pages}


class PageDateTest(unittest.TestCase):
    def test_onenote_dates_a_page_through_its_creation_time(self):
        pages = native_pages(FIXTURE / 'native/notebook', FIXTURE / 'native')
        self.assertEqual(pages['Changed date'].get('dateTime'), '2024-03-05T14:30:00.000Z')
        self.assertNotEqual(pages['Kept date'].get('dateTime'), '2024-03-05T14:30:00.000Z')

    def test_a_rust_dated_page_reads_back_with_its_new_date(self):
        pages = native_pages(FIXTURE / 'candidate', FIXTURE / 'cold')
        self.assertEqual(pages['Kept date'].get('dateTime'), '2025-07-04T16:45:00.000Z')
        self.assertEqual(pages['Changed date'].get('dateTime'), '2024-03-05T14:30:00.000Z')
        title = pages['Kept date'].find('one:Title/one:OE/one:T', ns)
        self.assertEqual(title.text, 'Kept date')


if __name__ == '__main__':
    unittest.main()
