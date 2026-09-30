from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/equation-select'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def mathml(path):
    """Each equation's MathML in a COM read, trailing spaces dropped."""
    xml = path.read_text(encoding='utf-8-sig')
    return [re.sub(r'(<mml:mo>&nbsp;</mml:mo>)+</mml:math>', '</mml:math>', m).replace(' display="block"', '')
            for m in re.findall(r'<mml:math[^!]*?</mml:math>', xml)]


class EquationSelectTest(unittest.TestCase):
    def test_edits_into_equation_objects_reopen_as_onenote_made_them(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        onenote = mathml(FIXTURE / 'lab/after.xml')
        cold = mathml(FIXTURE / 'cold/read/page-000.xml')
        # The lab page ends with "x" typed over a placeholder, which the candidate has no
        # placeholder left for.
        self.assertEqual(onenote[-1], '<mml:math xmlns:mml="http://www.w3.org/1998/Math/MathML"><mml:mi>x</mml:mi></mml:math>')
        self.assertEqual(cold, onenote[:-1])
        before = mathml(FIXTURE / 'lab/before.xml')
        self.assertIn('Type&nbsp;equation&nbsp;here.', ''.join(before))
        self.assertTrue(any('<mml:msqrt>' in m for m in before))
        self.assertFalse(any('<mml:msqrt>' in m or 'Type&nbsp;equation' in m for m in cold))
        self.assertTrue(any('<mml:mtable>' in m and 'lim' in m for m in cold))


if __name__ == '__main__':
    unittest.main()
