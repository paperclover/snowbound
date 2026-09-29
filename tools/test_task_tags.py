from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/task-tags'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def tasks(read):
    """Each page's paragraphs by title: their text, task identities and normal tag count."""
    result = {}
    for path in sorted(read.glob('page-*.xml')):
        page = ET.parse(path).getroot()
        title = page.find('one:Title/one:OE/one:T', ns)
        if title is None:
            continue
        result[title.text] = [(re.sub(r'<[^>]*>', '', oe.find('one:T', ns).text or ''),
                          [(t.get('guidTask'), t.get('disabled'), t.get('startDate'), t.get('dueDate'))
                           for t in oe.findall('one:OutlookTask', ns)],
                          len(oe.findall('one:Tag', ns)))
                         for oe in page.find('one:Outline', ns).iter('{%s}OE' % ns['one'])]
    return result


class TaskTagsTest(unittest.TestCase):
    def test_removed_and_restored_task_tags_reopen_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        native = tasks(FIXTURE / 'native/read')
        cold = tasks(FIXTURE / 'cold/read')
        self.assertEqual(cold['Delete and undo'], native['Delete and undo'])
        removed = native['Remove a task']
        self.assertEqual([len(t) for _, t, _ in removed], [1, 1, 1, 0])
        self.assertEqual(cold['Remove a task'], [
            (removed[0][0], [], 0),
            removed[1],
            (removed[2][0], removed[2][1], 0),
            removed[3],
        ])


if __name__ == '__main__':
    unittest.main()
