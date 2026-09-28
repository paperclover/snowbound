from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/recycle-bin-repair'


def tree(node):
    out = []
    for child in node:
        tag = child.tag.rsplit('}', 1)[-1]
        if tag in ('Section', 'SectionGroup', 'Page'):
            flags = tuple(sorted(key for key in ('isRecycleBin', 'isDeletedPages', 'isInRecycleBin')
                                 if child.get(key) == 'true'))
            out.append((tag, child.get('name'), flags, tree(child)))
    return out


class RecycleBinRepairTest(unittest.TestCase):
    def test_onenote_repairs_nothing_in_the_bin_a_page_delete_repaired(self):
        candidates = sorted(path.relative_to(FIXTURE / 'candidate')
                            for path in (FIXTURE / 'candidate').rglob('*') if path.is_file())
        self.assertEqual(candidates, sorted(path.relative_to(FIXTURE / 'cold/notebook')
                                            for path in (FIXTURE / 'cold/notebook').rglob('*')
                                            if path.is_file()))
        for relative in candidates:
            self.assertEqual((FIXTURE / 'cold/notebook' / relative).read_bytes(),
                             (FIXTURE / 'candidate' / relative).read_bytes(), str(relative))

    def test_the_bin_holds_the_deleted_pages(self):
        hierarchy = ET.parse(FIXTURE / 'cold/read/hierarchy.xml').getroot()
        self.assertEqual(tree(hierarchy), [
            ('Section', 'New Section 1', (), [('Page', 'Kept', (), [])]),
            ('SectionGroup', 'OneNote_RecycleBin', ('isRecycleBin',), [
                ('Section', 'Deleted Pages', ('isDeletedPages', 'isInRecycleBin'), [
                    ('Page', 'Kept', ('isInRecycleBin',), []),
                    ('Page', 'Kept', ('isInRecycleBin',), []),
                ]),
            ]),
        ])


if __name__ == '__main__':
    unittest.main()
