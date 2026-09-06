#!/usr/bin/env python3
"""Measure vertical scrolling from overlapping native screenshots."""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path

import numpy as np
from PIL import Image


def compare(before, after, minimum_overlap=64):
    if (before.shape != after.shape or before.ndim != 3 or before.shape[2] != 3
            or before.dtype != np.uint8 or after.dtype != np.uint8):
        raise ValueError('Screenshots must have the same RGB dimensions')
    height = before.shape[0]
    if not 1 <= minimum_overlap <= height:
        raise ValueError('Minimum overlap must fit inside the screenshot')
    rows = defaultdict(list)
    for y, row in enumerate(before):
        if (row.min(axis=1) < 210).any():
            rows[row.tobytes()].append(y)
    votes = Counter()
    for y, row in enumerate(after):
        for old_y in rows.get(row.tobytes(), ()):
            delta = old_y - y
            if 0 <= delta <= height - minimum_overlap:
                votes[delta] += 1
    if not votes:
        raise ValueError('No exact foreground rows establish a vertical overlap')
    candidates = []
    for delta, count in votes.items():
        a, b = before[delta:], after[:height - delta]
        foreground = (a.min(axis=2) < 210) | (b.min(axis=2) < 210)
        equal = (a == b).all(axis=2)
        candidates.append({
            'scroll_y': delta, 'overlap_height': len(a), 'identical_rows': count,
            'foreground_pixels': int(foreground.sum()),
            'foreground_equal_fraction': float(equal[foreground].mean()),
            'foreground_mean_absolute_error': float(
                np.abs(a.astype(np.int16) - b.astype(np.int16))[foreground].mean()),
        })
    candidates.sort(key=lambda c: c['foreground_equal_fraction'], reverse=True)
    best = candidates[0]
    ambiguous = len(candidates) > 1 and (
        candidates[1]['foreground_equal_fraction'] == best['foreground_equal_fraction'])
    return {'scroll_y': None if ambiguous else best['scroll_y'],
            'ambiguous': ambiguous, 'candidates': candidates,
            'limits': ['Only downward integer scrolling with unchanged horizontal position is measured.',
                       'Candidates require identical foreground rows; a failure is not evidence of no scrolling.',
                       'Foreground excludes pixels whose three channels are all at least 210.']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('--crop', type=int, nargs=4, metavar=('LEFT', 'TOP', 'RIGHT', 'BOTTOM'))
    args = parser.parse_args()
    images = []
    for path in (args.before, args.after):
        with Image.open(path) as image:
            image = image.convert('RGB')
            if args.crop:
                left, top, right, bottom = args.crop
                if not (0 <= left < right <= image.width and 0 <= top < bottom <= image.height):
                    parser.error('The crop must fit inside each screenshot')
                image = image.crop(args.crop)
            images.append(np.asarray(image))
    print(json.dumps(compare(*images), indent=2))
