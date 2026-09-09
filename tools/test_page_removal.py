import hashlib
import json
from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_page_removal import CASES
from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/page-lifecycle/removal'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class PageRemovalTest(unittest.TestCase):
    def test_captures_match_requests_and_native_navigation(self):
        provenance = json.loads((FIXTURE / 'provenance.json').read_text())
        self.assertEqual(provenance.keys(), CASES.keys())
        machines = set()
        for case, (_, count, selected, permanent) in CASES.items():
            with self.subTest(case=case):
                root = FIXTURE / case
                request = json.loads((root / 'removal.json').read_text())
                self.assertEqual(request['selected'], selected)
                self.assertEqual(request['permanent'], permanent)
                for relative, digest in provenance[case]['sha256'].items():
                    self.assertEqual(hashlib.sha256((root / relative).read_bytes()).hexdigest(), digest)
                for phase in ['native', 'cold']:
                    machine = json.loads((root / phase / 'machine.json').read_text())['name']
                    self.assertNotIn(machine, machines)
                    machines.add(machine)
                    self.assertTrue(json.loads((root / phase / 'teardown.json').read_text())['absent'])
                before = ET.parse(root / 'native/before/hierarchy.xml').findall('.//one:Page', ns)
                self.assertEqual(len(before), count)
                expected = [(page.get('name'), page.get('pageLevel', '1'))
                            for index, page in enumerate(before) if index not in selected]
                if expected:
                    expected[0] = (expected[0][0], '1')
                for hierarchy in ['hierarchy.xml', 'reopened-hierarchy.xml']:
                    xml = (root / 'native/after' / hierarchy).read_text(encoding='utf-8-sig').strip()
                    pages = ET.fromstring(xml).findall('.//one:Page', ns) if xml not in ('', '<?xml version="1.0"?>') else []
                    self.assertEqual([(page.get('name'), page.get('pageLevel', '1')) for page in pages], expected)
        self.assertEqual(len(machines), 22)

    def test_native_xml_and_payloads_match_source_and_cold_stores(self):
        for case in CASES:
            for notebook, read in [('native/before/notebook', 'native/before-read'),
                                   ('native/after/notebook', 'native/read'),
                                   ('native/after/notebook', 'cold/read'),
                                   ('cold/notebook', 'cold/read')]:
                with self.subTest(case=case, notebook=notebook, read=read), TemporaryDirectory() as temporary:
                    destination = Path(temporary) / 'read'
                    shutil.copytree(FIXTURE / case / read, destination)
                    compare(FIXTURE / case / notebook, destination)


if __name__ == '__main__':
    unittest.main()
