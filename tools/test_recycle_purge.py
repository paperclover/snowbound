"""VM-free gate for corpus/recycle-purge: OneNote 2010 reads a recycle bin Snowbound purged
as empty, without repairing it."""

import pathlib
import unittest
import xml.etree.ElementTree as ElementTree

ROOT = pathlib.Path(__file__).resolve().parents[1] / "corpus" / "recycle-purge"
ONE = "{http://schemas.microsoft.com/office/onenote/2010/onenote}"


class RecyclePurge(unittest.TestCase):
    def test_cold_read_finds_the_bin_empty(self):
        notebook = ElementTree.parse(ROOT / "cold" / "hierarchy.xml").getroot()
        bin_ = next(
            group
            for group in notebook.iter(ONE + "SectionGroup")
            if group.get("isRecycleBin") == "true"
        )
        sections = list(bin_.iter(ONE + "Section"))
        self.assertEqual([section.get("name") for section in sections], ["Deleted Pages"])
        self.assertEqual(list(sections[0].iter(ONE + "Page")), [])
        kept = [section.get("name") for section in notebook.findall(ONE + "Section")]
        self.assertEqual(kept, ["New Section 1"])
        self.assertFalse((ROOT / "candidate" / "OneNote_RecycleBin" / "Old.one").exists())


if __name__ == "__main__":
    unittest.main()
