from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/to-do-list'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def items(read):
    """Each paragraph's text, list kind and tags' names, in page order."""
    page, = (ET.parse(path).getroot() for path in sorted(read.glob('page-*.xml')))
    names = {d.get('index'): d.get('name') for d in page.findall('one:TagDef', ns)}
    result = []
    for oe in page.iter('{%s}OE' % ns['one']):
        marker = oe.find('one:List', ns)
        kind = None if marker is None else list(marker)[0].tag.split('}')[1]
        tags = [names[tag.get('index')] for tag in oe.findall('one:Tag', ns)]
        result.append((oe.find('one:T', ns).text, kind, tags))
    return result


class ToDoListTest(unittest.TestCase):
    def test_onenote_tags_lists_without_removing_them(self):
        self.assertEqual(items(FIXTURE / 'native/read'), [
            ('Bullet item', 'Bullet', ['To Do']),
            ('First numbered', 'Number', ['To Do']),
            ('Second numbered', 'Number', ['To Do']),
            ('Nested numbered', 'Number', []),
            ('Restarted at three', 'Number', ['To Do']),
            ('Nested plain', None, []),
            ('Plain again', None, ['To Do']),
        ])

    def test_snowbound_to_do_and_bulleted_lists_reopen_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        self.assertEqual(items(FIXTURE / 'cold/read'), [
            ('Bullet item', None, ['To Do']),
            ('First numbered', None, ['To Do']),
            ('Second numbered', 'Bullet', []),
            ('Nested numbered', 'Bullet', []),
            ('Restarted at three', 'Number', []),
            ('Nested plain', None, []),
            ('Plain again', None, []),
        ])


if __name__ == '__main__':
    unittest.main()
