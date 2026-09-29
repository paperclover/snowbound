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
            return [(image.get('backgroundImage'), image.find('one:Size', ns).get('width'))
                    for image in pages[title].findall('one:Image', ns)]

        def rules(title):
            return pages[title].find('one:PageSettings/one:RuleLines', ns).get('visible')

        def color(title):
            return pages[title].find('one:PageSettings', ns).get('color')

        ivy, sparks = [('true', '174.5')], [('true', '611.4000244140625')]
        # Art joins a ruled page, which keeps its lines; art taken away or undone leaves none;
        # redone, or undone after other art replaced it, the art is back.
        self.assertEqual(art('Standard'), ivy)
        self.assertEqual(art('Wide'), [])
        self.assertEqual(art('College'), [])
        self.assertEqual(art('MediumGrid'), ivy)
        self.assertEqual(art('LargeGrid'), ivy)
        for title in ('Standard', 'Wide', 'College', 'MediumGrid', 'LargeGrid', 'VeryLargeGrid'):
            self.assertEqual(rules(title), 'true', title)
        # A colour no menu offers reads back as written; art over a colour keeps it.
        self.assertEqual(color('SmallGrid'), '#E8D8C8')
        self.assertEqual(art('VeryLargeGrid'), sparks)
        self.assertEqual(color('VeryLargeGrid'), '#FDFDDD')

if __name__ == '__main__':
    unittest.main()
