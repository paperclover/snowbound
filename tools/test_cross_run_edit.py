import json
from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/cross-run-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class CrossRunEditTest(unittest.TestCase):
    def test_cold_native_text_styles_and_structure(self):
        for case in json.loads((FIXTURE / 'manifest.json').read_text())['cases']:
            with self.subTest(case=case['name']), TemporaryDirectory() as temporary:
                fixture = FIXTURE / case['name']
                native = Path(temporary) / 'read'
                shutil.copytree(fixture / 'cold/read', native)
                compare(fixture / 'candidate', native)
                compare(fixture / 'cold/notebook', native)


if __name__ == '__main__':
    unittest.main()
