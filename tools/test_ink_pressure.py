"""Pressure ink as OneNote 2010 keeps and draws it (`corpus/ink-pressure`), read from its PDF
exports: every point of a stroke is a round stamp whose diameter is the pen's width times
0.25 + 1.5 * pressure."""
from collections import defaultdict
from pathlib import Path
import re
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET
import zlib

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/ink-pressure'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']
NUMBER = r'(-?[\d.]+)'
STAMP = re.compile(rf'{NUMBER} {NUMBER} m\s+{NUMBER} {NUMBER} {NUMBER} {NUMBER} {NUMBER} {NUMBER} c\s+'
                   rf'{NUMBER} {NUMBER} {NUMBER} {NUMBER} {NUMBER} {NUMBER} c')


def page(read, name):
    """A page's XML and its PDF export."""
    for path in sorted(read.glob('page-*.xml')):
        root = ET.parse(path).getroot()
        if root.get('name') == name:
            return root, path.with_suffix('.pdf')
    raise KeyError(name)


def stamps(pdf):
    """Each round stamp OneNote exports, as (bottom, diameter) in PDF points."""
    data = pdf.read_bytes()
    streams = []
    for match in re.finditer(rb'stream\r?\n(.*?)\r?\nendstream', data, re.S):
        try:
            streams.append(zlib.decompress(match.group(1)).decode('latin1'))
        except zlib.error:
            pass
    return [(float(m.group(8)), float(m.group(1)) - float(m.group(13)))
            for m in STAMP.finditer('\n'.join(streams))]


def rows(pdf):
    """Stamp diameters by the height of their bottom edge, rounded to a tenth of a point."""
    found = defaultdict(list)
    for bottom, diameter in stamps(pdf):
        found[round(bottom, 1)].append(diameter)
    return found


def thickness(pressure):
    return 0.25 + 1.5 * pressure


class InkPressureTest(unittest.TestCase):
    def test_onenote_draws_a_quarter_of_the_pen_at_no_pressure_and_1_75_times_at_full(self):
        _, pdf = page(FIXTURE / 'native/read', 'Levels')
        # Two drawings of nine 24-point strokes at pressures 0 to 1 in eighths: one pen reports
        # 1024 levels, the other 256, and both draw alike.
        found = sorted(rows(pdf).items(), reverse=True)
        diameters = [d for _, row in found for d in row]
        self.assertEqual(len(diameters), 2 * 9 * 5)
        eighths = [((d / 24 - 0.25) / 1.5) * 8 for d in diameters]
        for eighth in eighths:
            self.assertAlmostEqual(eighth, round(eighth), delta=0.03)
        self.assertEqual(sorted(round(e) for e in eighths), sorted(list(range(9)) * 10))

    def test_a_pen_ignoring_pressure_draws_one_width(self):
        root, _ = page(FIXTURE / 'native/read', 'Strokes')
        self.assertEqual(len(root.findall('one:InkDrawing', ns)), 5)
        self.assertTrue((FIXTURE / 'native/read/page-000.png').exists())

    def test_snowbound_pressure_strokes_open_in_onenote_at_their_widths(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        _, pdf = page(FIXTURE / 'cold/read', 'Levels')
        found = rows(pdf)
        # Snowbound's strokes of a 500 HIMETRIC pen at none, half and full pressure.
        pen = 500 * 72 / 2540
        snowbound = [row for bottom, row in found.items() if len(row) == 3]
        self.assertEqual(len(snowbound), 3)
        for pressure, row in zip([1.0, 0.5, 0.0], sorted(snowbound, key=lambda row: row[0], reverse=True)):
            for diameter in row:
                self.assertAlmostEqual(diameter, pen * thickness(pressure), delta=0.06)
        # Its ramp widens point by point from a quarter of the pen to 1.75 times it.
        ramp = sorted(d for _, row in found.items() if len(row) == 1 for d in row)
        self.assertEqual(len(ramp), 41)
        self.assertAlmostEqual(ramp[0], pen * 0.25, delta=0.06)
        self.assertAlmostEqual(ramp[-1], pen * 1.75, delta=0.06)

    def test_onenote_pressure_ink_keeps_its_widths_after_snowbound_edits(self):
        before = rows(page(FIXTURE / 'native/read', 'Levels')[1])
        after = rows(page(FIXTURE / 'cold/read', 'Levels')[1])
        # The erased stroke at no pressure leaves the 1024-level drawing; the rest draw as before.
        onenote = lambda found: sorted(round(d, 1) for row in found.values() if len(row) in (5, 10) for d in row)
        erased = onenote(before)
        for _ in range(5):
            erased.remove(6.0)
        self.assertEqual(onenote(after), erased)
        drawings = page(FIXTURE / 'cold/read', 'Strokes')[0].findall('one:InkDrawing', ns)
        self.assertEqual(len(drawings), 4)
        position = drawings[0].find('one:Position', ns)
        self.assertEqual((float(position.get('x')), float(position.get('y'))), (72.0, 108.0))


if __name__ == '__main__':
    unittest.main()
