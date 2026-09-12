from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/link-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class LinkEditTest(unittest.TestCase):
    def test_a_rust_hyperlink_renders_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        page, = (ET.parse(path).getroot() for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml')))
        texts = [oe.find('one:T', ns).text for oe in page.iter('{%s}OE' % ns['one']) if oe.find('one:T', ns) is not None]
        self.assertEqual([' '.join(text.split()) for text in texts],
                         ['Read about Rust <a href="https://example.invalid/rust">the Rust site</a>'])


if __name__ == '__main__':
    unittest.main()
