from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/equation-join'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def mathml(xml):
    """Each equation's MathML in a COM read, trailing spaces dropped."""
    return [re.sub(r'(<mml:mo>&nbsp;</mml:mo>)+</mml:math>', '</mml:math>', m)
            for m in re.findall(r'<mml:math[^!]*?</mml:math>', xml)]


class EquationJoinTest(unittest.TestCase):
    def test_joined_equations_reopen_as_onenote_joined_them(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        onenote = mathml((FIXTURE / 'lab/equation-seam.xml').read_text())
        cold = mathml(''.join(path.read_text(encoding='utf-8-sig') for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml'))))
        self.assertEqual(len(onenote), 1)
        self.assertEqual([m.replace(' display="block"', '') for m in cold],
                         [m.replace(' display="block"', '') for m in onenote])


if __name__ == '__main__':
    unittest.main()
