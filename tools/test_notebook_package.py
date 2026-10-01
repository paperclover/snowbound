"""VM-free gate for corpus/notebook-package: a package Snowbound packs is a cabinet OneNote 2010
unpacks and opens, the notebook Snowbound unpacks cold-opens in OneNote as written, and
OneNote's own LZX package lists the files Snowbound unpacks."""

import pathlib
import struct
import unittest
import xml.etree.ElementTree as ElementTree
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[1] / "corpus" / "notebook-package"
ONE = "{http://schemas.microsoft.com/office/onenote/2010/onenote}"


def cabinet(data):
    """The compression of each folder and each file's name and bytes, read independently of
    Snowbound's reader: MS-CAB's header, folders, files and MSZIP data blocks."""
    signature, _, size, _, files_at, _, minor, major, folders, files, flags = struct.unpack_from(
        "<4sIIIIIBBHHH", data
    )
    assert signature == b"MSCF" and size == len(data) and (major, minor) == (1, 3) and flags == 0
    folder_list = []
    for index in range(folders):
        start, blocks, compression = struct.unpack_from("<IHH", data, 36 + 8 * index)
        folder_list.append((start, blocks, compression))
    entries, at = [], files_at
    for _ in range(files):
        length, offset, folder, _, _, _ = struct.unpack_from("<IIHHHH", data, at)
        end = data.index(b"\0", at + 16)
        entries.append((data[at + 16 : end].decode(), offset, length, folder))
        at = end + 1
    unpacked = []
    for start, blocks, compression in folder_list:
        stream, at, history = b"", start, b""
        for _ in range(blocks):
            _, compressed, _ = struct.unpack_from("<IHH", data, at)
            block = data[at + 8 : at + 8 + compressed]
            at += 8 + compressed
            if compression == 1:
                assert block[:2] == b"CK"
                inflate = zlib.decompressobj(-15, zdict=history) if history else zlib.decompressobj(-15)
                out = inflate.decompress(block[2:])
                history = (history + out)[-32768:]
            else:
                out = block
            stream += out
        unpacked.append((compression, stream))
    return [
        (name, unpacked[folder][0], unpacked[folder][1][offset : offset + length])
        for name, offset, length, folder in entries
    ]


def names(hierarchy):
    notebook = next(ElementTree.parse(hierarchy).getroot().iter(ONE + "Notebook"))

    def walk(node):
        for child in node:
            tag = child.tag.replace(ONE, "")
            if tag in ("Section", "SectionGroup", "Page"):
                yield (tag, child.get("name"), list(walk(child)))

    return list(walk(notebook))


EXPECTED = [
    ("Section", "New Section 1", [("Page", "First", [])]),
    ("SectionGroup", "Group", [("Section", "Inner", [("Page", "Inside", [])])]),
]


class NotebookPackage(unittest.TestCase):
    def test_snowbound_packs_mszip_files_outside_any_notebook(self):
        files = cabinet((ROOT / "candidate" / "Packed.onepkg").read_bytes())
        self.assertEqual(
            [name for name, *_ in files],
            ["Open Notebook.onetoc2", "New Section 1.one", "Group\\Open Notebook.onetoc2", "Group\\Inner.one"],
        )
        for name, compression, data in files:
            self.assertEqual(compression, 1, name)
            self.assertEqual(data[128:148], bytes(20), f"{name} names no ancestor")

    def test_onenote_unpacks_snowbounds_package(self):
        self.assertEqual(names(ROOT / "onenote-unpacked" / "hierarchy.xml"), EXPECTED)

    def test_snowbounds_unpacked_notebook_cold_opens_as_written(self):
        self.assertEqual(names(ROOT / "cold" / "read" / "hierarchy.xml"), EXPECTED)
        for written in (ROOT / "candidate" / "Unpacked").rglob("*.one*"):
            relative = written.relative_to(ROOT / "candidate" / "Unpacked")
            self.assertEqual(written.read_bytes(), (ROOT / "cold" / "notebook" / relative).read_bytes())

    def test_save_as_copies_open_in_onenote(self):
        root = ROOT / "save-as"
        notebook = ElementTree.parse(root / "cold" / "read" / "hierarchy.xml").getroot()
        sections = {
            section.get("name"): [page.get("name") for page in section.iter(ONE + "Page")]
            for section in notebook.iter(ONE + "Section")
        }
        self.assertEqual(sections, {"Inner copy": ["Inside"], "Inside": ["Inside"]})
        for written in (root / "candidate").glob("*.one"):
            data = written.read_bytes()
            self.assertEqual(data[128:148], bytes(20), written.name)
            # OneNote adopted them into the folder it opened, rewriting only their headers.
            self.assertEqual(data[1024:], (root / "cold" / "notebook" / written.name).read_bytes()[1024:])

    def test_onenotes_package_is_lzx(self):
        data = (ROOT / "native" / "Lab.onepkg").read_bytes()
        _, compression = struct.unpack_from("<HH", data, 36 + 4)
        self.assertEqual(compression & 0xF, 3, "LZX")


if __name__ == "__main__":
    unittest.main()
