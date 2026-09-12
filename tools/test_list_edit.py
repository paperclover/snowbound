from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/list-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def items(read):
    page, = (ET.parse(path).getroot() for path in sorted(read.glob('page-*.xml')))
    outline, = page.findall('one:Outline', ns)

    def walk(element, depth):
        marker = element.find('one:List', ns)
        kind = None
        if marker is not None:
            child, = list(marker)
            kind = (child.tag.split('}')[1], child.get('text'))
        text = re.sub(r'<[^>]*>', '', element.find('one:T', ns).text or '')
        yield depth, kind, text
        for nested in element.findall('one:OEChildren/one:OE', ns):
            yield from walk(nested, depth + 1)

    return [item for element in outline.findall('one:OEChildren/one:OE', ns) for item in walk(element, 0)]


class ListEditTest(unittest.TestCase):
    def test_rust_bullets_numbering_and_restarts_render_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        self.assertEqual(items(FIXTURE / 'cold/read'), [
            (0, ('Bullet', None), 'Bullet item'),
            (0, ('Number', 'I.'), 'First numbered'),
            (0, ('Number', 'II.'), 'Second numbered'),
            (1, ('Number', 'I.'), 'Nested numbered'),
            (0, ('Number', 'III.'), 'Restarted at three'),
            (1, None, 'Nested plain'),
            (0, None, 'Plain again'),
        ])


if __name__ == '__main__':
    unittest.main()
