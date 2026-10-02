from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/word-lists'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class WordListsTest(unittest.TestCase):
    def test_lists_pasted_from_word_open_in_onenote_as_its_own_paste_gave_them(self):
        read = FIXTURE / 'cold/read'
        with TemporaryDirectory() as temporary:
            copy = Path(temporary) / 'read'
            shutil.copytree(read, copy)
            compare(FIXTURE / 'candidate', copy)
        page = ET.parse(read / 'page-000.xml').getroot()
        lines = []

        def walk(children, depth):
            for element in children.findall('one:OE', ns):
                bullet = element.find('one:List/one:Bullet', ns)
                number = element.find('one:List/one:Number', ns)
                marker = ('bullet %s ' % bullet.get('bullet') if bullet is not None
                          else number.get('text') + ' ' if number is not None else '')
                text = re.sub(r'<[^>]*>', '', element.find('one:T', ns).text or '')
                lines.append('  ' * depth + marker + text)
                for nested in element.findall('one:OEChildren', ns):
                    walk(nested, depth + 1)

        outline = page.findall('one:Outline', ns)[-1]
        for children in outline.findall('one:OEChildren', ns):
            walk(children, 0)
        self.assertEqual(lines, [
            'bullet 1 Bullet one',
            '  bullet 0 Bullet nested',
            '    bullet 13 Bullet deeper',
            'bullet 1 Bullet two',
            'Plain paragraph',
            '1. Number one',
            '  a. Number nested',
            '    i. Number deeper',
            '2. Number two',
            'Last plain',
        ])


if __name__ == '__main__':
    unittest.main()
