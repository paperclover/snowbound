#!/usr/bin/env python3
"""Compare PDF baselines using identical opaque image anchors, independently of text layout."""
import argparse
import base64
from collections import Counter, defaultdict
import hashlib
import io
import json
import math
from pathlib import Path
import statistics
import xml.etree.ElementTree as ET

import pdfplumber
from PIL import Image

from compare_page_pdf import compare
from native_fixture import NS


def pdf_rgb(image):
    stream = image['stream']
    if any(key in stream.attrs for key in ['Mask', 'SMask', 'Decode']) or image.get('imagemask'):
        return None
    space = image['colorspace']
    names = [getattr(value, 'name', None) for value in space]
    size = tuple(image['srcsize'])
    if names == ['DeviceRGB'] and image['bits'] == 8:
        return Image.frombytes('RGB', size, stream.get_data()).tobytes()
    if names[:2] == ['Indexed', 'DeviceRGB'] and len(space) == 4 and image['bits'] in (4, 8) and isinstance(space[3], bytes):
        pixels = Image.frombytes('P', size, stream.get_data(), 'raw', 'P;4' if image['bits'] == 4 else 'P')
        if len(space[3]) != (space[2] + 1) * 3 or pixels.getextrema()[1] > space[2]:
            return None
        pixels.putpalette(space[3])
        return pixels.convert('RGB').tobytes()
    return None


def register_images(xml, pages):
    sources = []
    for node in xml.findall(f'{{{NS}}}Image'):
        data = node.findtext(f'{{{NS}}}Data')
        if not data:
            continue
        with Image.open(io.BytesIO(base64.b64decode(''.join(data.split()), validate=True))) as image:
            if image.convert('RGBA').getchannel('A').getextrema() != (255, 255):
                continue
            key = (image.size, hashlib.sha256(image.convert('RGB').tobytes()).hexdigest())
        position = node.find(f'{{{NS}}}Position')
        size = node.find(f'{{{NS}}}Size')
        rect = [float(position.get('x')), float(position.get('y')), float(size.get('width')), float(size.get('height'))]
        if not all(map(math.isfinite, rect)) or min(rect[2:]) <= 0:
            raise ValueError('Invalid source image geometry')
        sources.append((key, node.get('objectID'), rect))
    counts = Counter(key for key, _, _ in sources)
    candidates = defaultdict(list)
    for index, page in enumerate(pages):
        for image in page.images:
            pixels = pdf_rgb(image)
            if pixels is not None:
                candidates[(tuple(image['srcsize']), hashlib.sha256(pixels).hexdigest())].append((index, image))
    registrations = defaultdict(list)
    for key, identity, rect in sources:
        matches = candidates[key]
        if counts[key] != 1 or len(matches) != 1:
            continue
        index, image = matches[0]
        pdf_rect = [image['x0'], image['top'], image['x1'], image['bottom']]
        if not all(map(math.isfinite, pdf_rect)) or pdf_rect[2] <= pdf_rect[0] or pdf_rect[3] <= pdf_rect[1]:
            raise ValueError('Invalid PDF image geometry')
        registrations[index].append({
            'source_id': identity, 'pixel_sha256': key[1], 'source_xywh': rect, 'pdf_rect': pdf_rect,
            'translation': [pdf_rect[0] - rect[0], pdf_rect[1] - rect[1]],
            'extent_scale': [(pdf_rect[2] - pdf_rect[0]) / rect[2], (pdf_rect[3] - pdf_rect[1]) / rect[3]],
        })
    return dict(registrations)


def baseline_report(probe, xml, pages):
    anchors = register_images(xml, pages)
    wraps = compare(probe, pages)
    outlines = {o['id']: o for o in probe['objects'] if o['kind'] == 'outline' and not o['is_title']}
    rows = []
    for paragraph in wraps['paragraphs']:
        if paragraph['status'] != 'matched':
            rows.append({**paragraph, 'lines': []})
            continue
        outline = outlines[paragraph['outline_id']]
        for instance in paragraph['native_instances']:
            page = instance['pdf_page'] - 1
            registration = anchors.get(page, [])
            row = {'outline_id': paragraph['outline_id'], 'paragraph_id': paragraph['paragraph_id'], 'pdf_page': page + 1, 'lines': []}
            rows.append(row)
            if len({a['source_xywh'][1] for a in registration}) < 2:
                row['status'] = 'insufficient_image_anchors'
                continue
            row['status'] = 'measured_translation_residuals'
            offsets = [a['translation'][1] for a in registration]
            for native, local in zip(instance['baseline_ranges_from_pdf_top'], paragraph['canvas_baselines_in_outline'], strict=True):
                document = outline['layout']['y'] + local
                row['lines'].append({
                    'canvas_document_baseline': document, 'native_pdf_baseline_range': native,
                    'residual_using_median_anchor_translation': [v - document - statistics.median(offsets) for v in native],
                    'residual_range_over_anchor_translations': [native[0] - document - max(offsets), native[1] - document - min(offsets)],
                })
    return {'anchors_by_pdf_page': {str(page + 1): rows for page, rows in anchors.items()},
            'wrap_counts': wraps['counts'], 'paragraphs': rows,
            'limits': 'Translation candidates come only from identical opaque images. Their spread and extent scales remain visible; no text-fitted registration or baseline acceptance tolerance is applied. PDF coordinates do not establish screen glyph baselines.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('probe', type=Path)
    parser.add_argument('native_xml', type=Path)
    parser.add_argument('native_pdf', type=Path)
    args = parser.parse_args()
    with pdfplumber.open(args.native_pdf) as pdf:
        print(json.dumps(baseline_report(json.loads(args.probe.read_text()), ET.parse(args.native_xml).getroot(), pdf.pages), indent=2))
