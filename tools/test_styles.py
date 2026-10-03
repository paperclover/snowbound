from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/styles'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']
GALLERY = {'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'PageTitle', 'cite', 'blockquote', 'code', 'p'}


def styles(path):
    """The page's name and its quick styles by name: font, size, bold, colour."""
    page = ET.parse(path).getroot()
    defined = {}
    for style in page.findall('one:QuickStyleDef', ns):
        defined.setdefault(style.get('name'), []).append(
            (style.get('font'), float(style.get('fontSize')), style.get('bold') == 'true', style.get('fontColor')))
    return page.get('name'), defined


class StylesTest(unittest.TestCase):
    def test_themed_pages_cold_open_as_written(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)

    def test_onenote_lists_each_theme_under_its_names(self):
        heading = {
            'OneNote': ('Calibri', 16.0, True, '#17365D'),
            'Manuscript': ('Georgia', 20.0, False, '#3B2F2A'),
            'Editorial': ('Georgia', 18.0, True, '#9A3B1F'),
            'Modern': ('Arial', 16.0, True, '#2E5CB8'),
        }
        seen = set()
        for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml')):
            name, defined = styles(path)
            theme = name.removeprefix('Heading 1 in ')
            seen.add(theme)
            self.assertEqual(set(defined), GALLERY, theme)
            self.assertEqual(defined['h1'], [heading[theme]], theme)
            if theme == 'Modern':
                # OneNote's own page title, in Arial.
                self.assertEqual(defined['PageTitle'], [('Arial', 17.0, False, 'automatic')])
        self.assertEqual(seen, set(heading))

    def test_onenote_s_own_styles_read_under_the_same_names(self):
        _, applied = styles(FIXTURE / 'onenote/applied.xml')
        self.assertEqual(set(applied), GALLERY)
        # OneNote's gallery on a themed page adds its own h2, and Enter its own p.
        _, edited = styles(FIXTURE / 'onenote-edit/Manuscript.xml')
        self.assertEqual(sorted(edited['h2']), [('Calibri', 13.0, True, '#366092'), ('Georgia', 16.0, True, '#3B2F2A')])
        self.assertEqual(edited['p'], [('Calibri', 11.0, False, 'automatic')])


if __name__ == '__main__':
    unittest.main()
