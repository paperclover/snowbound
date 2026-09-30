"""VM-free gate for corpus/section-color: OneNote 2010 reads each section the app coloured, None
included, and the notebook's colour as its own menus set them."""

import pathlib
import unittest
import xml.etree.ElementTree as ElementTree

ROOT = pathlib.Path(__file__).resolve().parents[1] / "corpus" / "section-color"
ONE = "{http://schemas.microsoft.com/office/onenote/2010/onenote}"


class SectionColor(unittest.TestCase):
    def test_cold_read_keeps_every_colour(self):
        menu = [
            line.rsplit(" ", 1)
            for line in (ROOT / "native" / "colors.txt").read_text().splitlines()
        ]
        hierarchy = ElementTree.parse(ROOT / "cold" / "read" / "hierarchy.xml").getroot()
        notebook = next(hierarchy.iter(ONE + "Notebook"))
        self.assertEqual(notebook.get("color"), "#4DBCCA")
        sections = {
            section.get("name"): section.get("color")
            for section in notebook.iter(ONE + "Section")
        }
        self.assertEqual(sections.pop("New Section 1"), "#8AA8E4")
        self.assertEqual(sections, dict(menu))


if __name__ == "__main__":
    unittest.main()
