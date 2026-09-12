import html
from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/table-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def tables(read, title):
    for path in sorted(read.glob('page-*.xml')):
        page = ET.parse(path).getroot()
        if page.get('name') != title:
            continue
        out = []
        for table in page.iter('{%s}Table' % ns['one']):
            columns = [(column.get('width'), column.get('isLocked')) for column in table.find('one:Columns', ns)]
            rows = [[[html.unescape(re.sub(r'<[^>]*>', '', text.text or '')) for text in cell.findall('.//one:T', ns)]
                     for cell in row.findall('one:Cell', ns)] for row in table.findall('one:Row', ns)]
            out.append((table.get('bordersVisible'), columns, rows))
        return out
    raise AssertionError(title)


class TableEditTest(unittest.TestCase):
    def check(self, name):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / name / 'cold/read', read)
            compare(FIXTURE / name / 'candidate', read)

    def test_a_rust_table_renders_natively(self):
        self.check('created')
        self.assertEqual(tables(FIXTURE / 'created/cold/read', 'Before the table'), [
            ('true', [('144.0', 'true'), ('216.0', 'true')],
             [[['Name'], ['Value']], [['Rust 🦀'], ['Written natively']]]),
        ])

    def test_rows_and_columns_added_to_a_native_table_render_natively(self):
        self.check('edited')
        (borders, columns, rows), = tables(FIXTURE / 'edited/cold/read', 'Delete cell subtree')
        self.assertEqual(borders, 'true')
        self.assertEqual(columns[:2], [('240.0', 'true'), ('180.0', 'true')])
        self.assertEqual(len(columns), 3)
        self.assertIsNone(columns[2][1])
        self.assertEqual(rows[0][2], ['Third column'])
        self.assertEqual(rows[1], [['Second row'], ['Second row, second column'], ['Second row, third column']])


if __name__ == '__main__':
    unittest.main()
