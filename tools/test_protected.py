import json
import os
from pathlib import Path
import runpy
import shutil
import stat
import subprocess
from tempfile import TemporaryDirectory
import unittest

from document_model import EXPORTER

ROOT = Path(__file__).resolve().parent.parent
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class ProtectedDocumentTest(unittest.TestCase):
    def test_native_autofit_is_distinct_from_a_fixed_column_width(self):
        import xml.etree.ElementTree as ET
        fixture = ROOT / 'corpus/native-encrypted'
        manifest = json.loads((fixture / 'manifest.json').read_text())
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            password = root / 'password'
            password.write_text(manifest['password'])
            native = root / 'read'
            shutil.copytree(fixture / 'cold-2010-4763/read', native)
            compare(fixture / 'encrypted-01/notebook', native, password_file=password)
            differences = json.loads((root / 'table-autofit.json').read_text())
            self.assertEqual(len(differences), 1)
            self.assertAlmostEqual(differences[0]['stored'], 38.61, places=4)
            self.assertAlmostEqual(differences[0]['native'], 39.146141, places=4)
            xml_path = native / 'page-000.xml'
            xml = ET.parse(xml_path)
            column = next(n for n in xml.getroot().iter() if n.tag.endswith('}Column'))
            column.set('isLocked', 'true')
            xml.write(xml_path, encoding='utf-8')
            with self.assertRaisesRegex(AssertionError, 'Table column lock state differs'):
                compare(fixture / 'encrypted-01/notebook', native, password_file=password)
            column.attrib.pop('isLocked')
            column.set('width', 'NaN')
            xml.write(xml_path, encoding='utf-8')
            with self.assertRaisesRegex(AssertionError, 'Invalid native table column width'):
                compare(fixture / 'encrypted-01/notebook', native, password_file=password)

    def test_native_page_formatting_and_all_attachment_boundaries(self):
        fixture = ROOT / 'corpus/native-protected-boundaries'
        manifest = json.loads((fixture / 'manifest.json').read_text())
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            password = root / 'password'
            password.write_text(manifest['password'])
            native = root / 'read'
            shutil.copytree(fixture / 'read', native)
            compare(fixture / 'notebook', native, password_file=password)
            output = root / 'export'
            subprocess.run([EXPORTER, fixture / 'notebook/synthetic.one', output,
                            '--password-file', password], check=True, capture_output=True)
            if os.name == 'posix':
                self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o700)
            password.write_text(manifest['password'] + '\n')
            failed = root / 'failed'
            result = subprocess.run([EXPORTER, fixture / 'notebook/synthetic.one', failed,
                                     '--password-file', password], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b'')
            self.assertFalse(failed.exists())
            self.assertNotIn(manifest['password'].encode(), result.stderr)
            password.write_text('x' * 65537)
            result = subprocess.run([EXPORTER, fixture / 'notebook/synthetic.one',
                                     '--password-file', password], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b'')


if __name__ == '__main__':
    unittest.main()
