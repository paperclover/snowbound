from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/object-outline'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def bodies(read):
    """Each page's body by title: each paragraph's content kind, with its text."""
    result = {}
    for path in sorted(read.glob('page-*.xml')):
        page = ET.parse(path).getroot()
        title = page.find('one:Title/one:OE/one:T', ns)
        body = page.find('one:Outline/one:OEChildren', ns)
        if title is None or body is None:
            continue
        result[title.text] = [
            next(child.tag.rsplit('}', 1)[1] for child in oe
                 if child.tag.rsplit('}', 1)[1] in ('T', 'Image', 'InsertedFile'))
            + (':' + re.sub(r'<[^>]*>', '', oe.find('one:T', ns).text or '') if oe.find('one:T', ns) is not None else '')
            for oe in body.findall('one:OE', ns)]
    return result


class ObjectOutlineTest(unittest.TestCase):
    def test_typing_beside_an_outlines_only_object_reopens_as_onenote_made_it(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        self.assertEqual(bodies(FIXTURE / 'native/read'),
                         {'File alone': ['InsertedFile'], 'Picture alone': ['Image']})
        onenote = bodies(FIXTURE / 'keys/read')
        self.assertEqual(onenote, {'File alone': ['InsertedFile', 'T:abc'], 'Picture alone': ['Image', 'T:abc']})
        self.assertEqual(bodies(FIXTURE / 'cold/read'), onenote)


if __name__ == '__main__':
    unittest.main()
