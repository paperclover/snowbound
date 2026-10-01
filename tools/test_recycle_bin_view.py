"""VM-free gate for corpus/recycle-bin-view: OneNote 2010 reads a recycle bin Snowbound wrote, and
the notebook after pages and a section left it and the rest was emptied, without repairing
either."""

import pathlib
import unittest
import xml.etree.ElementTree as ElementTree

ROOT = pathlib.Path(__file__).resolve().parents[1] / "corpus" / "recycle-bin-view"
ONE = "{http://schemas.microsoft.com/office/onenote/2010/onenote}"


def tree(state):
    notebook = ElementTree.parse(ROOT / "cold" / state / "read" / "hierarchy.xml").getroot()

    def walk(node):
        return [
            (child.get("name"), [page.get("name") for page in child.findall(ONE + "Page")])
            if child.tag == ONE + "Section"
            else (child.get("name"), walk(child))
            for child in node
            if child.tag in (ONE + "Section", ONE + "SectionGroup")
        ]

    return walk(next(notebook.iter(ONE + "Notebook")) if notebook.tag != ONE + "Notebook" else notebook)


class RecycleBinView(unittest.TestCase):
    def test_onenote_reads_the_bin_snowbound_wrote(self):
        self.assertEqual(
            tree("before"),
            [
                ("New Section 1", ["Kept"]),
                (
                    "OneNote_RecycleBin",
                    [
                        ("Deleted Pages", ["Restored page", "Copied page", "Purged page"]),
                        ("Back", ["Restored page"]),
                        ("Copied", ["Copied page"]),
                        ("Purged", ["Purged page"]),
                        ("Restored", ["In a restored section"]),
                        ("Emptied", ["In an emptied section"]),
                    ],
                ),
            ],
        )

    def test_restored_copied_and_emptied_read_as_meant(self):
        self.assertEqual(
            tree("after"),
            [
                ("New Section 1", ["Kept", "Restored page", "Copied page"]),
                ("Restored", ["In a restored section"]),
                ("OneNote_RecycleBin", [("Deleted Pages", [])]),
            ],
        )

    def test_onenote_repaired_nothing(self):
        for state in ("before", "after"):
            candidate = ROOT / "candidate" / state
            for written in candidate.rglob("*.one*"):
                relative = written.relative_to(candidate)
                cold = ROOT / "cold" / state / "notebook" / relative
                self.assertEqual(written.read_bytes(), cold.read_bytes(), relative)


if __name__ == "__main__":
    unittest.main()
