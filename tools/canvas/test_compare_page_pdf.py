import unittest
from types import SimpleNamespace

from compare import pdf_lines
from compare_page_pdf import compare


def probe(texts, ends=None):
    return {'title': 'Test', 'objects': [{'kind': 'outline', 'is_title': False, 'id': 'outline',
            'paragraphs': [{'id': str(i), 'visible_text': text, 'origin': [0, i * 12],
                            'spans': [{'end_utf8': len(text.encode()), 'format': {'font_size': 10}}],
                            'lines': [{'text': text, 'end_utf16': len(text) if ends is None else ends[i],
                                       'baseline': 8}]}
                           for i, text in enumerate(texts)]}]}


def page(lines):
    return SimpleNamespace(height=800, chars=[
        {'text': ch, 'x0': x * 5, 'y0': y - 2, 'y1': y + 8, 'size': 10,
         'matrix': (1, 0, 0, 1, x * 5, y)}
        for y, text in lines for x, ch in enumerate(text)])


class PagePdf(unittest.TestCase):
    def test_title_opt_in_requires_its_own_native_glyphs(self):
        source = probe(['body'])
        title = probe(['title'])['objects'][0]
        title.update(id='title', is_title=True)
        source['objects'].append(title)
        native = page([(700, 'title'), (680, 'body')])
        self.assertEqual(compare(source, [native])['counts'], {'matched': 1})
        result = compare(source, [native], include_titles=True)
        self.assertEqual(result['counts'], {'matched': 2})
        self.assertTrue(result['paragraphs'][1]['is_title'])
        missing = compare(source, [page([(680, 'body')])], include_titles=True)
        self.assertEqual(missing['paragraphs'][1]['status'], 'unresolved_match')

    def test_outline_context_resolves_repeated_paragraphs(self):
        result = compare(probe(['same', 'same']), [page([(700, 'same'), (680, 'same')])])
        self.assertEqual(result['counts'], {'matched': 2})
        self.assertEqual([p['native_instances'][0]['baseline_ranges_from_pdf_top']
                          for p in result['paragraphs']], [[[100, 100]], [[120, 120]]])

    def test_different_wraps_are_reported(self):
        result = compare(probe(['one two']), [page([(700, 'one'), (680, 'two')])])
        row = result['paragraphs'][0]
        self.assertEqual(row['status'], 'different_wraps')
        self.assertEqual(row['native_end_utf16'], [4, 7])
        self.assertFalse(row['breaks_match'])

    def test_repeated_exports_must_agree(self):
        source = probe(['one two'])
        one_line = page([(700, 'one two')])
        matching = compare(source, [one_line, one_line])['paragraphs'][0]
        self.assertTrue(matching['breaks_match'])
        self.assertEqual(len(matching['native_instances']), 2)
        different = compare(source, [one_line, page([(700, 'one'), (680, 'two')])])['paragraphs'][0]
        self.assertEqual(different['status'], 'unrecoverable_line_ranges')
        self.assertIsNone(different['breaks_match'])

    def test_incomplete_outline_does_not_use_ambiguous_source_substrings(self):
        result = compare(probe(['one', 'one other']), [page([(700, 'one extra one other')])])
        self.assertEqual(result['paragraphs'][0]['status'], 'unresolved_match')
        self.assertEqual(result['paragraphs'][0]['match_context'], 'ambiguous_source_context')

    def test_empty_text_is_not_a_geometry_pass(self):
        result = compare(probe(['', ' ', '\u00a0']), [page([])])
        self.assertEqual(result['counts'], {'no_visible_glyphs': 3})
        self.assertTrue(all(p['breaks_match'] is None for p in result['paragraphs']))

    def test_overlapping_line_candidates_are_not_chosen_arbitrarily(self):
        native = page([(700, 'a'), (680, 'b'), (660, 'c')])
        native.chars[-1]['y1'] = 720
        self.assertIsNone(pdf_lines(native.chars))


if __name__ == '__main__':
    unittest.main()
