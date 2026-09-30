from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/picture-link'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class PictureLinkTest(unittest.TestCase):
    def test_restored_picture_reopens_with_its_link(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        links = sorted(m for path in (FIXTURE / 'cold/read').glob('page-*.xml')
                       for m in re.findall(r'<one:Image [^>]*hyperlink="([^"]*)"',
                                           path.read_text(encoding='utf-8-sig')))
        self.assertIn('https://example.invalid/image/png', links)
        self.assertEqual(len(links), 5)


if __name__ == '__main__':
    unittest.main()
