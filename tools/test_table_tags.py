from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/table-tags'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def tables(read):
    """Each page's tables by title: their first cell texts and tags as (name, completed)."""
    result = {}
    for path in sorted(read.glob('page-*.xml')):
        page = ET.parse(path).getroot()
        title = page.find('one:Title/one:OE/one:T', ns)
        if title is None:
            continue
        names = {d.get('index'): d.get('name') for d in page.findall('one:TagDef', ns)}
        result[title.text] = [
            ([cell.find('.//one:T', ns).text for cell in oe.find('one:Table', ns).iter('{%s}Cell' % ns['one'])],
             [(names[tag.get('index')], tag.get('completed')) for tag in oe.findall('one:Tag', ns)])
            for oe in page.iter('{%s}OE' % ns['one']) if oe.find('one:Table', ns) is not None]
    return result


class TableTagsTest(unittest.TestCase):
    def test_edited_tagged_tables_reopen_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        native = tables(FIXTURE / 'native/read')
        cold = tables(FIXTURE / 'cold/read')
        self.assertEqual(native['Check a table'], [(['Checked by a click', 'B'], [('To Do', 'false')])])
        self.assertEqual(cold['Check a table'], [(['Checked by a click', 'B'], [('To Do', 'true')])])
        edited = native['Edit a tagged table']
        self.assertEqual(cold['Edit a tagged table'], [(['Now '] + edited[0][0][1:], edited[0][1])])
        self.assertEqual(cold['Delete and undo'], native['Delete and undo'])


if __name__ == '__main__':
    unittest.main()
