from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/rule-lines'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']

BLUE = '#CAEBFD'
MARGIN = ('Margin', '#FF5050', None)


def rule_lines(capture, notebook):
    """Each page's rule lines as OneNote's page XML reports them, by title, once the Rust
    reading of `notebook` matches the capture."""
    with TemporaryDirectory() as temporary:
        read = Path(temporary) / 'read'
        shutil.copytree(capture / 'read', read)
        compare(notebook, read)
    pages = {}
    for path in sorted((capture / 'read').glob('page-*.xml')):
        page = ET.parse(path).getroot()
        rules = page.find('one:PageSettings/one:RuleLines', ns)
        lines = [(line.tag.rsplit('}', 1)[-1], line.get('color'),
                  None if line.get('spacing') is None else round(float(line.get('spacing')), 3))
                 for line in rules]
        pages[page.get('name')] = (rules.get('visible'), lines)
    return pages


def ruled(spacing, color=BLUE):
    return ('true', [('Horizontal', color, spacing), MARGIN])


def grid(spacing, color=BLUE):
    return ('true', [('Horizontal', color, spacing), ('Vertical', color, spacing)])


NONE = ('false', [])


class RuleLinesTest(unittest.TestCase):
    def test_onenote_rules_pages_as_its_menu_names_them(self):
        pages = rule_lines(FIXTURE / 'native', FIXTURE / 'native/notebook')
        self.assertEqual(pages, {
            'None': NONE,
            'Narrow': NONE,
            'College': ruled(23.76),
            'Standard': ruled(33.12),
            'Wide': ruled(46.8),
            'SmallGrid': grid(12.0),
            'MediumGrid': grid(28.346, '#E0C9FF'),
            'LargeGrid': grid(42.52),
            'VeryLargeGrid': grid(56.693),
            'ColorRed': ruled(33.12, '#FFD4D6'),
            'Hidden': ruled(33.12, '#FFFFFF'),
        })

    def test_rust_rule_lines_read_back_as_written(self):
        pages = rule_lines(FIXTURE / 'cold', FIXTURE / 'candidate')
        self.assertEqual(pages, {
            'None': ruled(46.8),
            'Narrow': NONE,
            'College': NONE,
            'Standard': grid(12.0),
            'Wide': ruled(46.8),
            'SmallGrid': grid(12.0),
            'MediumGrid': ruled(13.428, '#FFD4D6'),
            'LargeGrid': grid(42.52),
            'VeryLargeGrid': grid(56.693),
            'ColorRed': ruled(33.12, '#FFD4D6'),
            'Hidden': ruled(33.12, '#FFFFFF'),
            'Created': grid(56.693),
        })


if __name__ == '__main__':
    unittest.main()
