from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import uuid
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/notebook-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def file_identity(path):
    return uuid.UUID(bytes_le=path.read_bytes()[16:32])


def entries(node):
    return [(child.tag.rsplit('}', 1)[-1], child.get('name'), child.get('color'), entries(child))
            for child in node if child.tag.rsplit('}', 1)[-1] in ('Section', 'SectionGroup')]


class NotebookEditTest(unittest.TestCase):
    def check(self, row, expected):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / row / 'cold/read', read)
            compare(FIXTURE / row / 'candidate', read)
        hierarchy = ET.parse(FIXTURE / row / 'cold/read/hierarchy.xml').getroot()
        self.assertEqual(entries(hierarchy), expected)
        for candidate in sorted((FIXTURE / row / 'candidate').rglob('*.one*')):
            relative = candidate.relative_to(FIXTURE / row / 'candidate')
            self.assertEqual(file_identity(FIXTURE / row / 'cold/notebook' / relative), file_identity(candidate), str(relative))

    def test_rust_sections_and_groups_open_natively_in_the_written_order(self):
        self.check('structured', [
            ('Section', 'Renamed', '#FFD75E', []),
            ('Section', 'First', '#8AA8E4', []),
            ('SectionGroup', 'Kept', None, [('Section', 'Inner', '#8AA8E4', [])]),
        ])

    def test_a_rust_deleted_section_sits_in_the_native_recycle_bin(self):
        self.check('deleted', [
            ('Section', 'Renamed', '#FFD75E', []),
            ('SectionGroup', 'Kept', None, [('Section', 'First', '#8AA8E4', [])]),
            ('SectionGroup', 'OneNote_RecycleBin', None, [('Section', 'First', '#8AA8E4', [])]),
        ])
        hierarchy = ET.parse(FIXTURE / 'deleted/cold/read/hierarchy.xml').getroot()
        bin = hierarchy.find("one:SectionGroup[@name='OneNote_RecycleBin']", ns)
        self.assertEqual(bin.get('isRecycleBin'), 'true')
        self.assertEqual(bin.find('one:Section', ns).get('isInRecycleBin'), 'true')

    def test_onenote_re_identifies_files_whose_header_disagrees_with_their_place(self):
        # The native session lists a Rust section created with a zero ancestor twice: once
        # under the identity Rust wrote and once under the identity OneNote assigned.
        toc = (FIXTURE / 'native-structure/notebook/Open Notebook.onetoc2').read_bytes()
        links = FIXTURE / 'native-structure/notebook/links.one'
        self.assertEqual(uuid.UUID(bytes_le=links.read_bytes()[128:144]), file_identity(FIXTURE / 'native-structure/notebook/Open Notebook.onetoc2'))
        self.assertIn(file_identity(links).bytes_le, toc)


if __name__ == '__main__':
    unittest.main()
