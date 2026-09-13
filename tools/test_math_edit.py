import json
from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/math-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def exported_mathml(path):
    text = path.read_text(encoding='utf-8-sig')
    out = []
    for body in re.findall(r'<mml:math[^>]*>(.*?)</mml:math>', text, re.S):
        out.append(re.sub(r'&#(\d+);', lambda m: chr(int(m.group(1))), body))
    return out


class MathEditTest(unittest.TestCase):
    def test_equation_editor_expressions_export_as_mathml(self):
        self.assertEqual(len(exported_mathml(FIXTURE / 'native-editor/read/page-000.xml')), 5)

    def test_rust_written_equations_export_the_expected_mathml(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'written/cold/read', read)
            compare(FIXTURE / 'written/candidate', read)
        expected = json.loads((FIXTURE / 'written/candidate/expected-mathml.json').read_text())
        self.assertEqual(exported_mathml(FIXTURE / 'written/cold/read/page-000.xml'), expected)
        self.assertTrue((FIXTURE / 'written/cold/read/page-000.png').exists())


if __name__ == '__main__':
    unittest.main()
