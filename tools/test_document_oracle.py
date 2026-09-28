from importlib.util import find_spec
from pathlib import Path
import re
import runpy
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']
requires_pdfplumber = unittest.skipUnless(
    find_spec('pdfplumber'), 'pdfplumber is not installed; tools/TESTING.md sets up the Python lane')


class DocumentOracleTest(unittest.TestCase):
    @requires_pdfplumber
    def test_locked_table_width_requires_the_native_value(self):
        fixture = ROOT / 'corpus/m6/native-structure-01'
        with TemporaryDirectory() as temporary:
            native = Path(temporary) / 'read'
            shutil.copytree(fixture / 'read', native)
            compare(fixture / 'notebook', native)
            path = native / 'page-001.xml'
            xml = ET.parse(path)
            column = next(n for n in xml.getroot().iter()
                          if n.tag.endswith('}Column') and n.get('isLocked') == 'true')
            column.set('width', str(float(column.get('width')) + 1))
            xml.write(path, encoding='utf-8')
            with self.assertRaisesRegex(AssertionError, 'Locked table column width differs'):
                compare(fixture / 'notebook', native)

    @requires_pdfplumber
    def test_native_2010_does_not_render_the_documented_cell_shading_control(self):
        import pdfplumber
        fixture = ROOT / 'corpus/m6/cell-shading-control-01/read/page-001'
        page = ET.parse(fixture.with_suffix('.xml')).getroot()
        cells = [n for n in page.iter() if n.tag.endswith('}Cell')]
        self.assertEqual(len(cells), 6)
        self.assertTrue(all('shadingColor' not in n.attrib for n in cells))
        with pdfplumber.open(fixture.with_suffix('.pdf')) as pdf:
            table_page = pdf.pages[1]
            self.assertIn('Cell 0,0', table_page.extract_text())
            self.assertTrue(all(tuple(rect.get('non_stroking_color') or ()) != (1, 1, 0)
                                for rect in table_page.rects))

    @requires_pdfplumber
    def test_rust_saved_expansion_is_honored_by_a_fresh_native_cache(self):
        import pdfplumber
        fixture = ROOT / 'corpus/m6/rust-collapse-control-01'
        pages = [(p, ET.parse(p).getroot()) for p in (fixture / 'read').glob('page-*.xml')]
        path, page = next((p, r) for p, r in pages if r.get('name') == 'Paragraph controls')
        parent, = [n for n in page.iter() if n.tag.endswith('}OE')
                   and any(c.tag.endswith('}T') and c.text == 'Collapsed parent' for c in n)]
        self.assertIsNone(parent.get('collapsed'))
        with pdfplumber.open(path.with_suffix('.pdf')) as pdf:
            text = ''.join(p.extract_text() or '' for p in pdf.pages)
        self.assertIn('Hidden child remains stored', text)
        self.assertIn('Grandchild', text)

    @requires_pdfplumber
    def test_native_expansion_changes_the_view_without_changing_notebook_bytes(self):
        import pdfplumber
        original = ROOT / 'corpus/m6/native-features-01/notebook/Features.one'
        capture = ROOT / 'corpus/m6/native-expanded-control-ui-01'
        self.assertEqual(original.read_bytes(), (capture / 'notebook/Features.one').read_bytes())
        for directory, collapsed in [('before-read', '1'), ('read', None)]:
            candidates = [(p, ET.parse(p).getroot()) for p in (capture / directory).glob('page-*.xml')]
            path, page = next((p, r) for p, r in candidates if r.get('name') == 'Paragraph controls')
            parent, = [n for n in page.iter() if n.tag.endswith('}OE')
                       and any(c.tag.endswith('}T') and c.text == 'Collapsed parent' for c in n)]
            self.assertEqual(parent.get('collapsed'), collapsed)
            self.assertIn('Grandchild', ''.join(parent.itertext()))
            with pdfplumber.open(path.with_suffix('.pdf')) as pdf:
                text = ''.join(p.extract_text() or '' for p in pdf.pages)
            self.assertNotIn('Grandchild', text)

    def test_missing_or_empty_source_cannot_pass(self):
        fixture = ROOT / 'corpus/m6/native-math-01/read'
        with TemporaryDirectory() as temporary:
            source = Path(temporary)
            with self.assertRaises(FileNotFoundError):
                compare(source / 'missing', fixture)
            with self.assertRaisesRegex(AssertionError, 'No notebook sections'):
                compare(source, fixture)

    def test_equations_survive_a_fresh_native_cache(self):
        captures = []
        for fixture in ('native-math-01', 'native-math-cold-01'):
            pages = [ET.parse(p).getroot() for p in (ROOT / 'corpus/m6' / fixture / 'read').glob('page-*.xml')]
            page, = [p for p in pages if p.get('name') == 'Equation controls']
            equations = [ET.fromstring(expression) for node in page.iter() if node.tag.endswith('}T')
                         for expression in re.findall(r'<!--\[if mathML\]>(.*?)<!\[endif\]-->', node.text or '', re.S)]
            self.assertEqual(len(equations), 3)
            ns = '{http://www.w3.org/1998/Math/MathML}'
            self.assertEqual([n.tag for n in equations[0]], [ns + n for n in ('msup', 'mo', 'msup', 'mo', 'msup')])
            self.assertEqual(''.join(equations[0].itertext()), 'x2+y2=z2')
            self.assertEqual([n.tag for n in equations[1]], [ns + 'mfrac'])
            self.assertEqual([''.join(n.itertext()) for n in equations[1][0]], ['a+b', 'c+d'])
            self.assertEqual(''.join(equations[2].itertext()), '𝛼+𝛽')
            self.assertNotIn('display', equations[2].attrib)
            captures.append([ET.tostring(n) for n in equations])
        self.assertEqual(*captures)

    @requires_pdfplumber
    def test_rtl_xml_order_matches_native_visual_column_order(self):
        import pdfplumber
        fixture = ROOT / 'corpus/m6/native-page-direction-03'
        with pdfplumber.open(fixture / 'read/page-000.pdf') as pdf:
            words = pdf.pages[0].extract_words()
            right = [word for word in words if word['text'] == 'Right']
            left = [word for word in words if word['text'] == 'Left']
            self.assertEqual((len(right), len(left)), (1, 1))
            self.assertLess(right[0]['x0'], left[0]['x0'])
        with TemporaryDirectory() as temporary:
            native = Path(temporary) / 'read'
            shutil.copytree(fixture / 'read', native)
            compare(fixture / 'notebook', native)

    def test_changed_origin_requires_matching_native_coordinates(self):
        fixture = ROOT / 'corpus/m6/native-origin-controls-01'
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            notebook = output / 'notebook'
            shutil.copytree(fixture / 'input/notebook', notebook)
            shutil.copytree(fixture / 'read', output / 'read')
            compare(notebook, output / 'read')
            subprocess.run([ROOT / 'target/debug/examples/edit_property',
                            notebook / 'synthetic.one', output / 'changed.one',
                            '6000b', '14001d0f', '00004040', '00008040'], check=True)
            (output / 'changed.one').replace(notebook / 'synthetic.one')
            with self.assertRaisesRegex(AssertionError, 'coordinate differences'):
                compare(notebook, output / 'read')


if __name__ == '__main__':
    unittest.main()
