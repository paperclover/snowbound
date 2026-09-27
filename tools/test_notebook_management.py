from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import uuid
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/notebook-management'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def file_identity(path):
    return uuid.UUID(bytes_le=path.read_bytes()[16:32])


def tree(node):
    """Sections, groups and pages as OneNote lists them, with what the app set on each."""
    out = []
    for child in node:
        tag = child.tag.rsplit('}', 1)[-1]
        if tag in ('Section', 'SectionGroup'):
            out.append((tag, child.get('name'), child.get('color'), tree(child)))
        elif tag == 'Page':
            out.append((tag, child.get('name'), child.get('pageLevel'), child.get('isInRecycleBin')))
    return out


def page(title):
    for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml')):
        root = ET.parse(path).getroot()
        if root.get('name') == title:
            return root
    raise AssertionError(f'No page titled {title!r}')


class NotebookManagementTest(unittest.TestCase):
    def test_the_managed_notebook_reopens_natively_as_written(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        for candidate in sorted((FIXTURE / 'candidate').rglob('*.one*')):
            relative = candidate.relative_to(FIXTURE / 'candidate')
            self.assertEqual(file_identity(FIXTURE / 'cold/notebook' / relative),
                             file_identity(candidate), str(relative))

    def test_sections_groups_and_pages_sit_where_the_app_put_them(self):
        hierarchy = ET.parse(FIXTURE / 'cold/read/hierarchy.xml').getroot()
        self.assertEqual(tree(hierarchy), [
            ('Section', 'New Section 1', '#8AA8E4', [
                ('Page', 'Garden plan', '1', None),
                ('Page', 'Ivy page', '1', None),
                ('Page', 'Teal page', '1', None),
                ('Page', 'Subpage', '2', None),
                ('Page', 'Meeting page', '1', None),
            ]),
            ('SectionGroup', 'Archive', None, [
                ('Section', 'Inner', '#8AA8E4', [('Page', 'Untitled page', '1', None)]),
                ('Section', 'Kitchen', '#F6B078', [
                    ('Page', 'Untitled page', '1', None),
                    ('Page', 'Moved page', '1', None),
                ]),
            ]),
            ('SectionGroup', 'OneNote_RecycleBin', None, [
                ('Section', 'Deleted Pages', '#E1E1E1', [('Page', 'Deleted page', '1', 'true')]),
                ('Section', 'Gone', '#8AA8E4', [('Page', 'Untitled page', '1', 'true')]),
                ('Section', 'Binned', '#D5A4BB', [('Page', 'Untitled page', '1', 'true')]),
            ]),
        ])
        bin = hierarchy.find("one:SectionGroup[@name='OneNote_RecycleBin']", ns)
        self.assertEqual(bin.get('isRecycleBin'), 'true')
        deleted = bin.find("one:Section[@name='Deleted Pages']", ns)
        self.assertEqual(deleted.get('isDeletedPages'), 'true')

    def test_pages_keep_their_date_colour_and_template_art(self):
        teal = page('Teal page')
        self.assertEqual(teal.find('one:PageSettings', ns).get('color'), '#D4F9F2')
        # The title's second outline holds the date and the time.
        title = teal.find('one:Title', ns)
        texts = [t.text for t in title.iter('{%s}T' % ns['one'])]
        self.assertEqual(len(texts), 1)
        ivy = page('Ivy page')
        images = ivy.findall('one:Image', ns)
        self.assertEqual([image.get('backgroundImage') for image in images], ['true'])
        self.assertEqual(page('Garden plan').find('one:PageSettings', ns).get('color'), 'automatic')

    def test_meeting_notes_content_reads_back_as_onenote_writes_it(self):
        meeting = page('Meeting page')
        self.assertEqual(len(meeting.findall('one:Image', ns)), 1)
        outlines = meeting.findall('one:Outline', ns)
        self.assertEqual(len(outlines), 4)
        texts = [t.text or '' for t in meeting.iter('{%s}T' % ns['one'])]
        for text in ('Agenda', 'Action Items', 'Important Dates', 'Meeting Details', 'Attendees:', 'Next Meeting'):
            self.assertTrue(any(text in line for line in texts), text)
        self.assertEqual(len(meeting.findall('.//one:Tag', ns)), 1)
        self.assertEqual(len(meeting.findall('.//one:Number', ns)), 1)
        self.assertEqual(len(meeting.findall('.//one:Bullet', ns)), 11)


if __name__ == '__main__':
    unittest.main()
