import json
from pathlib import Path
import runpy
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest

from document_model import EXPORTER

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/protected-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class ProtectedEditTest(unittest.TestCase):
    def setUp(self):
        self.temporary = TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.password = self.root / 'password'
        manifest = json.loads((ROOT / 'corpus/native-encrypted/manifest.json').read_text())
        self.password.write_text(manifest['password'])

    def tearDown(self):
        self.temporary.cleanup()

    def test_onenote_unlocks_and_reads_the_page_saved_under_the_section_key(self):
        native = self.root / 'read'
        shutil.copytree(FIXTURE / 'candidate/read', native)
        compare(FIXTURE / 'candidate/notebook', native, password_file=self.password)
        page = (native / 'page-000.xml').read_text(encoding='utf-8-sig')
        self.assertIn('and edited under its key', page)
        self.assertIn('Written while protected', page)

    def test_the_native_edit_that_followed_unlocks_with_both_edits(self):
        output = self.root / 'export'
        subprocess.run([EXPORTER, FIXTURE / 'native-after/synthetic.one', output,
                        '--password-file', self.password], check=True, capture_output=True)
        text = (output / 'text.json').read_text()
        for expected in ['Native edit after Rust.', 'Written while protected', 'and edited under its key']:
            self.assertIn(expected, text)


if __name__ == '__main__':
    unittest.main()
