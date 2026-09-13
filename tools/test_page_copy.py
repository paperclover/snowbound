from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/page-copy'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class PageCopyTest(unittest.TestCase):
    def test_pages_copied_between_sections_read_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        pages = [ET.parse(path).getroot() for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml'))]
        self.assertEqual(len(pages), int((FIXTURE / 'candidate/expected-count.txt').read_text()))
        kinds = {tag for page in pages for tag in {node.tag.rsplit('}', 1)[-1] for node in page.iter()}}
        self.assertTrue({'InsertedFile', 'Image', 'Table', 'InkDrawing', 'Tag', 'List', 'Bullet', 'Number'} <= kinds, kinds)
        hierarchy = ET.parse(FIXTURE / 'cold/read/hierarchy.xml').getroot()
        titles = [page.get('name') for page in hierarchy.iter('{%s}Page' % ns['one'])]
        self.assertEqual(titles[0], 'Destination')
        self.assertNotIn('Paragraph formatting', titles)
        self.assertEqual(len(set(titles)), len(titles))
        self.assertEqual(len(set(titles)), len(titles))


if __name__ == '__main__':
    unittest.main()
