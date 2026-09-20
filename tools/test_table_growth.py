from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/table-growth'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class TableGrowthTest(unittest.TestCase):
    def test_onenote_reads_object_groups_whose_id_tables_have_gaps(self):
        with TemporaryDirectory() as temporary:
            native = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', native)
            compare(FIXTURE / 'candidate/notebook', native)
            self.assertIn('and a few more words', (native / 'page-000.xml').read_text(encoding='utf-8-sig'))
        self.assertEqual((FIXTURE / 'candidate/notebook/synthetic.one').read_bytes(),
                         (FIXTURE / 'cold/notebook/synthetic.one').read_bytes())


if __name__ == '__main__':
    unittest.main()
