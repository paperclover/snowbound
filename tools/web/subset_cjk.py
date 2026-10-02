#!/usr/bin/env python3
"""Writes Noto Sans CJK SC at its regular weight, cut to the characters GB 2312, JIS X 0208
and KS X 1001's Hangul hold, as the web build's fallback for Chinese, Japanese and Korean.

    uv run --with fonttools python tools/web/subset_cjk.py NotoSansCJK-VF.otf.ttc OUT.otf
"""
import sys

from fontTools import subset
from fontTools.ttLib import TTCollection
from fontTools.varLib import instancer


def main(collection, out):
    font = next(face for face in TTCollection(collection).fonts if face['name'].getDebugName(1) == 'Noto Sans CJK SC')
    font = instancer.instantiateVariableFont(font, {'wght': 400})
    # Punctuation, kana, Hangul compatibility jamo and full-width forms whole; ideographs and
    # syllables as the national standards list them.
    codes = set(range(0x3000, 0x3100)) | set(range(0x3130, 0x3190)) | set(range(0xff00, 0xfff0))
    for code, encodings in [*((code, ('gb2312', 'shift_jis')) for code in range(0x4e00, 0xa000)),
                            *((code, ('euc_kr',)) for code in range(0xac00, 0xd7a4))]:
        for encoding in encodings:
            try:
                chr(code).encode(encoding)
            except UnicodeEncodeError:
                continue
            codes.add(code)
            break
    options = subset.Options()
    options.layout_features = ['*']
    options.name_IDs = ['*']
    options.notdef_outline = True
    cut = subset.Subsetter(options)
    cut.populate(unicodes=codes)
    cut.subset(font)
    font.save(out)


if __name__ == '__main__':
    main(*sys.argv[1:])
