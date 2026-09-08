import json
from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_format import native_characters
from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/formatted-insertion'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class FormattedInsertionTest(unittest.TestCase):
    def test_cold_native_content_and_formatting(self):
        manifest = json.loads((FIXTURE / 'manifest.json').read_text())
        for case in manifest['cases']:
            with self.subTest(case=case['name']), TemporaryDirectory() as temporary:
                fixture = FIXTURE / case['name']
                native = Path(temporary) / 'read'
                shutil.copytree(fixture / 'cold/read', native)
                compare(fixture / 'candidate', native)
        with TemporaryDirectory() as temporary:
            fixture = FIXTURE / 'empty/typed-after-cold'
            native = Path(temporary) / 'read'
            shutil.copytree(fixture / 'read', native)
            compare(fixture / 'candidate', native)

    def test_native_edits_preserve_the_observed_character_styles(self):
        manifest = json.loads((FIXTURE / 'manifest.json').read_text())
        for case in manifest['cases']:
            for variant, before_file, prefix in [('reverse', 'edit-before.xml', 'Native '),
                                                  ('ui', 'before-read/page-000.xml', 'UI ')]:
                if variant + '_sha256' not in case:
                    continue
                with self.subTest(case=case['name'], variant=variant), TemporaryDirectory() as temporary:
                    fixture = FIXTURE / case['name'] / variant
                    native = Path(temporary) / 'read'
                    shutil.copytree(fixture / 'read', native)
                    compare(fixture / 'notebook', native)
                    before = ET.parse(fixture / before_file).getroot()
                    after = ET.parse(native / 'page-000.xml').getroot()
                    expected = native_characters(before, before.findall('one:Outline', ns))
                    changed = 0
                    for index, paragraph in enumerate(expected):
                        if ''.join(char for char, _ in paragraph).startswith(('Bold ', 'Typed ')):
                            expected[index] = [(char, paragraph[0][1]) for char in prefix] + paragraph
                            changed += 1
                    self.assertEqual(changed, 1)
                    self.assertEqual(native_characters(after, after.findall('one:Outline', ns)), expected)


if __name__ == '__main__':
    unittest.main()
