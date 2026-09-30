from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/object-tags'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def page(read, title):
    for path in sorted(read.glob('page-*.xml')):
        xml = path.read_text(encoding='utf-8-sig')
        if re.search(r'<one:Page [^>]*name="%s"' % re.escape(title), xml):
            return xml
    raise AssertionError(title)


def tags(xml):
    """Each object's check box state in page order: the outline's picture and file, then
    the picture and file on the page."""
    return [m == 'true' for m in re.findall(r'<one:Tag index="0" completed="(\w+)"', xml)]


class ObjectTagsTest(unittest.TestCase):
    def test_tags_on_pictures_and_files_reopen_as_checked(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        native = page(FIXTURE / 'native/read', 'Pictures and files')
        cold = page(FIXTURE / 'cold/read', 'Pictures and files')
        self.assertEqual(tags(native), [False] * 4)
        self.assertEqual(tags(cold), [True, False, True, True])
        # OneNote keeps its tags on the objects themselves, and the picture's recognised text.
        for xml in (native, cold):
            self.assertRegex(xml, r'<one:Image [^>]*>(<one:Position[^>]*/>)?<one:Size[^>]*/><one:Tag ')
            self.assertRegex(xml, r'<one:InsertedFile [^>]*>(<one:Position[^>]*/>)?<one:Size[^>]*/><one:Tag ')
            self.assertIn('<one:OCRText><![CDATA[GLACIER HARBOR\nInvoice 4471]]></one:OCRText>', xml)

    def test_restored_picture_reopens_with_its_recognised_text(self):
        restored = FIXTURE / 'restored'
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(restored / 'cold/read', read)
            compare(restored / 'candidate', read)
        ocr = lambda xml: re.findall(r'<one:OCRData.*?</one:OCRData>', xml, re.S)
        native = ocr(page(FIXTURE / 'native/read', 'Pictures and files'))
        self.assertEqual(len(native), 1)
        self.assertEqual(ocr(page(restored / 'cold/read', 'Pictures and files')), native)


if __name__ == '__main__':
    unittest.main()
