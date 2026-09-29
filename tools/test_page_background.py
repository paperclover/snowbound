from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/page-background'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class PageBackgroundTest(unittest.TestCase):
    def test_backgrounds_read_back_as_the_menu_gave_them(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        pages = {}
        for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml')):
            page = ET.parse(path).getroot()
            pages[page.get('name')] = page

        def art(title):
            return [image.get('backgroundImage') for image in pages[title].findall('one:Image', ns)]

        def rules(title):
            return pages[title].find('one:PageSettings/one:RuleLines', ns).get('visible')

        # Art joins a ruled page, which keeps its lines; art taken away leaves none.
        self.assertEqual(art('Standard'), ['true'])
        self.assertEqual(rules('Standard'), 'true')
        self.assertEqual(art('Wide'), [])
        self.assertEqual(rules('Wide'), 'true')
        # A colour no menu offers reads back as written.
        self.assertEqual(pages['SmallGrid'].find('one:PageSettings', ns).get('color'), '#E8D8C8')


if __name__ == '__main__':
    unittest.main()
