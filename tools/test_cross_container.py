from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/cross-container'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def blocks(read):
    """Each page's body by title: paragraph texts, and tables as rows of cell paragraph texts."""
    text = lambda node: re.sub(r'<[^>]*>', '', node.text or '')
    result = {}
    for path in sorted(read.glob('page-*.xml')):
        page = ET.parse(path).getroot()
        title = page.find('one:Title/one:OE/one:T', ns)
        body = page.find('one:Outline/one:OEChildren', ns)
        if title is None or body is None:
            continue
        result[title.text] = [
            [[[text(t) for t in cell.findall('one:OEChildren/one:OE/one:T', ns)]
              for cell in row.findall('one:Cell', ns)]
             for row in oe.find('one:Table', ns).findall('one:Row', ns)]
            if oe.find('one:Table', ns) is not None else text(oe.find('one:T', ns))
            for oe in body.findall('one:OE', ns)]
    return result


class CrossContainerTest(unittest.TestCase):
    def test_deletions_across_a_tables_edge_reopen_as_onenote_made_them(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        onenote = blocks(FIXTURE / 'keys/read')
        cold = blocks(FIXTURE / 'cold/read')
        self.assertEqual(len(cold), 4)
        for title, body in cold.items():
            self.assertEqual(body, onenote[title], title)


if __name__ == '__main__':
    unittest.main()
