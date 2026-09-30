from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/table-widths'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def tables(read):
    """Each page's tables by title: their first cell text and (width, locked) columns."""
    result = {}
    for path in sorted(read.glob('page-*.xml')):
        page = ET.parse(path).getroot()
        title = page.find('one:Title/one:OE/one:T', ns).text
        result[title] = [
            (table.find('.//one:T', ns).text,
             [(round(float(column.get('width')), 2), column.get('isLocked') == 'true')
              for column in table.findall('one:Columns/one:Column', ns)])
            for table in page.iter('{%s}Table' % ns['one'])]
    return result


class TableWidthsTest(unittest.TestCase):
    def test_typed_and_dragged_widths_reopen_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        cold = tables(FIXTURE / 'cold/read')
        # OneNote 2010's own widths for the same table typed in it (lab, 2026-09-30).
        self.assertEqual(cold['Check a table'][1],
                         ('Fruit', [(60.76, False), (37.11, False), (139.09, False)]))
        self.assertEqual(cold['Edit a tagged table'][0][1], [(180.25, False), (37.11, False)])
        dragged = cold['Delete and undo'][-1]
        self.assertEqual(dragged[0], 'Dragged')
        self.assertEqual(dragged[1][0], (96.0, True))
        self.assertFalse(dragged[1][1][1])
        self.assertGreater(dragged[1][1][0], 300)


if __name__ == '__main__':
    unittest.main()
