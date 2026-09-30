"""VM-free gate for corpus/default-font: OneNote 2010 reads the editor's new text and title
in the Default font, colour included, as the quick styles it writes itself."""

import pathlib
import unittest
import xml.etree.ElementTree as ElementTree

ROOT = pathlib.Path(__file__).resolve().parents[1] / "corpus" / "default-font"
ONE = "{http://schemas.microsoft.com/office/onenote/2010/onenote}"


class DefaultFont(unittest.TestCase):
    def test_cold_read_keeps_the_default_font_styles(self):
        page = ElementTree.parse(ROOT / "cold" / "page.xml").getroot()
        styles = {
            style.get("index"): (
                style.get("name"),
                style.get("font"),
                style.get("fontSize"),
                style.get("fontColor"),
            )
            for style in page.iter(ONE + "QuickStyleDef")
        }
        self.assertIn(("PageTitle", "Georgia", "17.0", "#0070C0"), styles.values())
        bodies = [
            element
            for outline in page.iter(ONE + "Outline")
            for element in outline.iter(ONE + "OE")
        ]
        self.assertEqual(len(bodies), 2)
        for body in bodies:
            self.assertEqual(styles[body.get("quickStyleIndex")], ("p", "Georgia", "14.0", "#0070C0"))
            self.assertIsNone(body.find(ONE + "T").get("style"))


if __name__ == "__main__":
    unittest.main()
