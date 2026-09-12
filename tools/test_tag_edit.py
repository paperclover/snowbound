from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/tag-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class TagEditTest(unittest.TestCase):
    def test_rust_tag_definitions_and_task_states_render_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        page, = (ET.parse(path).getroot() for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml')))
        definitions = [(d.get('index'), d.get('type'), d.get('symbol'), d.get('name')) for d in page.findall('one:TagDef', ns)]
        self.assertEqual(definitions, [('0', '0', '3', 'Rust task'), ('1', '1', '13', 'Important')])
        tagged = [([(t.get('index'), t.get('completed'), t.get('completionDate') is not None) for t in oe.findall('one:Tag', ns)],
                   re.sub(r'<[^>]*>', '', oe.find('one:T', ns).text or ''))
                  for oe in page.iter('{%s}OE' % ns['one']) if oe.find('one:T', ns) is not None]
        self.assertEqual(tagged, [
            ([('0', 'false', False)], 'Open task'),
            ([('0', 'true', True)], 'Completed task'),
            ([('0', 'false', False), ('1', 'false', False)], 'Important open task'),
            ([], 'Untagged'),
        ])


if __name__ == '__main__':
    unittest.main()
