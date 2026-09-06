import unittest
from types import SimpleNamespace

from compare import pdf_line_ends
from range_stress import check


def page(lines):
    return SimpleNamespace(chars=[
        {'text': character, 'x0': x, 'y0': baseline - 2, 'y1': baseline + 8, 'matrix': (1, 0, 0, 1, x, baseline)}
        for baseline, text in lines for x, character in enumerate(text)
    ])


class PdfRanges(unittest.TestCase):
    def test_preserves_source_spaces_at_wraps(self):
        native = page([(700, 'Title'), (600, 'one two'), (580, 'three'), (30, 'Footer')])
        self.assertEqual(pdf_line_ends(native, '  one  two   three  '), [13, 20])

    def test_style_baseline_offsets_stay_on_the_same_visual_line(self):
        native = page([(600, 'a'), (599.83, 'b'), (580, 'c')])
        native.chars[1]['x0'] = 1
        self.assertEqual(pdf_line_ends(native, 'ab c'), [3, 4])

    def test_long_word_has_no_invented_whitespace(self):
        self.assertEqual(pdf_line_ends(page([(600, 'extra'), (580, 'ordinary')]), 'extraordinary'), [5, 13])

    def test_missing_pdf_characters_are_not_accepted(self):
        self.assertIsNone(pdf_line_ends(page([(600, 'one tree')]), 'one three'))

    def test_ambiguous_match_is_not_selected_arbitrarily(self):
        self.assertIsNone(pdf_line_ends(page([(600, 'same'), (580, 'same')]), 'same'))

    def test_non_ascii_requires_other_evidence(self):
        self.assertIsNone(pdf_line_ends(page([(600, 'café')]), 'café'))


class SourceCoverage(unittest.TestCase):
    def test_utf16_offsets_preserve_supplementary_characters(self):
        source = [{'id': 'emoji', 'runs': [{'text': 'a🌳b'}]}]
        output = {'cases': [{'id': 'emoji', 'lines': [
            {'start_utf16': 0, 'end_utf16': 3, 'text': 'a🌳'},
            {'start_utf16': 3, 'end_utf16': 4, 'text': 'b'},
        ]}]}
        self.assertEqual(check(source, output), 2)
        output['cases'][0]['lines'][0]['end_utf16'] = 2
        with self.assertRaises(UnicodeDecodeError):
            check(source, output)

    def test_dropped_leading_space_is_detected(self):
        source = [{'id': 'space', 'runs': [{'text': ' a'}]}]
        output = {'cases': [{'id': 'space', 'lines': [
            {'start_utf16': 1, 'end_utf16': 2, 'text': 'a'},
        ]}]}
        with self.assertRaisesRegex(ValueError, 'noncontiguous'):
            check(source, output)

    def test_empty_paragraph_covers_an_empty_source(self):
        source = [{'id': 'empty', 'runs': [{'text': ''}]}]
        output = {'cases': [{'id': 'empty', 'lines': [
            {'start_utf16': 0, 'end_utf16': 0, 'text': ''},
        ]}]}
        self.assertEqual(check(source, output), 1)


if __name__ == '__main__':
    unittest.main()
