#!/usr/bin/env python3
"""Compare document order and text with independently captured OneNote XML."""
import argparse
import base64
import hashlib
from io import BytesIO
from tempfile import TemporaryDirectory
from PIL import Image
from datetime import datetime, timezone
import json
import math
from pathlib import Path, PureWindowsPath
from zoneinfo import ZoneInfo
import subprocess
import xml.etree.ElementTree as ET
import uuid
import unicodedata
from native_xml import ns, pages, texts, project_text
from document_model import EXPORTER, ordered_pages, version_pages, walk
from native_format import native_characters, compare_formats

ROOT = Path(__file__).resolve().parent.parent


def verify_pdf_black(path, paragraphs):
    import pdfplumber
    characters = []
    with pdfplumber.open(path) as pdf:
        for page in pdf.pages:
            black = [r for r in page.rects if r.get('fill') and r.get('non_stroking_color') in (0, (0, 0, 0))]
            for char in page.chars:
                if char['text'].isspace():
                    continue
                x, y = (char['x0'] + char['x1']) / 2, (char['top'] + char['bottom']) / 2
                covered = any(r['x0'] - .2 <= x <= r['x1'] + .2 and r['top'] - .2 <= y <= r['bottom'] + .2 for r in black)
                characters.extend((c, covered, (page.page_number, char, black)) for c in char['text'])
    text = ''.join(c for c, _, _ in characters)
    checked = 0
    for runs in paragraphs:
        source = [(c, run['format']['highlight'] == 0) for run in runs if not run['format']['hidden']
                  for c in run['text'] if not c.isspace()]
        if not any(black for _, black in source):
            continue
        value = ''.join(c for c, _ in source)
        omitted = []
        if value not in text:
            # OneNote can map rendered combining clusters to spaces in PDF text.
            projected = []
            index = 0
            while index < len(source):
                end = index + 1
                while end < len(source) and unicodedata.combining(source[end][0]):
                    end += 1
                if end > index + 1:
                    assert not any(black for _, black in source[index:end]), 'Black-highlighted unmapped text requires visual verification'
                    omitted.append(len(projected))
                else:
                    projected.append(source[index])
                index = end
            source = projected
            value = ''.join(c for c, _ in source)
        start = text.find(value)
        assert start >= 0 and text.find(value, start + 1) < 0, 'Native PDF paragraph text is missing or ambiguous'
        assert [(c, covered) for c, covered, _ in characters[start:start + len(source)]] == source, 'Native PDF black rectangles disagree with the stored character positions'
        for index in omitted:
            assert 0 < index < len(source), 'Unmapped text has no surrounding PDF characters'
            left_page, left, black = characters[start + index - 1][2]
            right_page, right, _ = characters[start + index][2]
            assert left_page == right_page and abs(left['top'] - right['top']) < .2 and left['x1'] < right['x0'], 'Unmapped text has no unambiguous inline PDF position'
            assert not any(r['x0'] < right['x0'] - .2 and r['x1'] > left['x1'] + .2
                           and r['top'] < min(left['bottom'], right['bottom']) - .2
                           and r['bottom'] > max(left['top'], right['top']) + .2 for r in black), 'Native PDF black rectangles cover unhighlighted unmapped text'
        checked += 1
    assert checked, 'No stored black-highlight paragraph was checked'
    return checked


def visible_text(node, space):
    kind = node['kind']
    data = kind['text'].encode('utf-16-le')
    text = ''.join(data[run['start'] * 2:run['end'] * 2].decode('utf-16-le') for run in kind['runs']
                   if not run['format'] or not space['nodes'][run['format']]['format']['hidden'])
    return "" if text == "\u00a0" else project_text(text.removesuffix('\r'))


def compare_objects(space, roots, page, native_roots, assets, native_payloads, autofit):
    types = {'T': 'RichText', 'Image': 'Image', 'InsertedFile': 'Attachment', 'MediaFile': 'Attachment', 'Table': 'Table'}
    actual = [n for root in roots for _, n in walk(space, root)
              if n['kind']['type'] in types.values() and not n['kind'].get('boilerplate')]
    wanted = [n for root in native_roots for n in root.iter() if n.tag.rsplit('}', 1)[-1] in types]
    assert [n['kind']['type'] for n in actual] == [types[n.tag.rsplit('}', 1)[-1]] for n in wanted], 'Content object order differs'
    parents = {child: parent for parent in page.iter() for child in parent}
    definitions = {n.attrib['index']: n for n in page.findall('one:TagDef', ns)}
    count = 0
    for node, native in zip(actual, wanted, strict=True):
        kind = node['kind']
        if kind['type'] in ('Image', 'Attachment'):
            container = space['nodes'][kind['container']]['kind']
            data = assets[json.dumps(container['reference'], sort_keys=True)]
            if kind['type'] == 'Image':
                expected = base64.b64decode(native.find('one:Data', ns).text)
                if data != expected:
                    actual_image = Image.open(BytesIO(data)).convert('RGBA')
                    expected_image = Image.open(BytesIO(expected)).convert('RGBA')
                    assert actual_image.size == expected_image.size and actual_image.tobytes() == expected_image.tobytes(), 'Image pixels differ'
                assert bool(kind['background']) == (native.get('backgroundImage') == 'true'), 'Image background state differs'
                assert bool(kind['printout']) == (native.get('isPrintOut') == 'true'), 'Image printout state differs'
                assert kind['alt'] == native.get('alt'), 'Image alternative text differs'
                assert kind['link'] == native.get('hyperlink'), 'Image hyperlink differs'
                size = native.find('one:Size', ns)
                if size is None:
                    assert native.get('format') == 'png' and Image.open(BytesIO(expected)).size == (1, 1), 'Native image size is unavailable'
                    assert node['layout']['max_width'] is None and node['layout']['max_height'] is None, 'Native image omitted an explicit size'
                for axis in ('width', 'height'):
                    native_size = .75 if size is None else float(size.get(axis))
                    stored_size = node['layout']['max_' + axis]
                    if stored_size is None:
                        stored_size = kind['picture_' + axis]
                    if stored_size is not None:
                        # Native printout XML includes the 0.72-point border on each side.
                        frame_size = stored_size + (1.44 if kind['printout'] else 0)
                        assert abs(frame_size - native_size) < .002, ('Image display dimension differs', axis, frame_size, native_size)
            else:
                expected = [p for p in native_payloads if p['page'] == page.get('ID') and p['object'] == parents[native].get('objectID')]
                assert len(expected) == 1, 'Native attachment association is ambiguous'
                assert hashlib.sha256(data).hexdigest() == expected[0]['sha256'], 'Associated attachment bytes differ'
                assert kind['filename'] == expected[0]['name'], 'Attachment filename differs'
                media = native.find('one:MediaReference', ns)
                assert (kind['recording_id'] is None) == (media is None), 'Recording identity presence differs'
                if media is not None:
                    assert uuid.UUID(bytes_le=bytes(kind['recording_id'])) == uuid.UUID(media.get('mediaID')), 'Recording identity differs'
        elif kind['type'] == 'Table':
            rows = native.findall('one:Row', ns)
            columns = native.findall('one:Columns/one:Column', ns)
            assert len(rows) == kind['rows'] and len(columns) == kind['columns'], 'Table dimensions differ'
            assert (kind['locked'] or [False] * len(columns)) == [c.get('isLocked') == 'true' for c in columns], 'Table column lock state differs'
            for column, width in zip(columns, kind['widths'], strict=True):
                measured = float(column.get('width'))
                assert math.isfinite(measured) and measured >= 0, 'Invalid native table column width'
                if abs(measured - width) >= .002:
                    if column.get('isLocked') == 'true':
                        raise AssertionError('Locked table column width differs')
                    autofit.append({'page': page.get('ID'), 'table': parents[native].get('objectID'),
                                    'column': column.get('index'), 'stored': width, 'native': measured})
            assert len(node['children']) == len(rows), 'Table row count differs'
            for oid, row in zip(node['children'], rows, strict=True):
                assert len(space['nodes'][oid]['children']) == len(row.findall('one:Cell', ns)), 'Table cell count differs'
        tags = native.findall('one:Tag', ns) + parents[native].findall('one:Tag', ns)
        tasks = native.findall('one:OutlookTask', ns) + parents[native].findall('one:OutlookTask', ns)
        assert len(node['tags']) == len(tags) + len(tasks), 'Associated note tag count differs'
        by_type = {int(definitions[t.attrib['index']].attrib['type']): t for t in tags}
        assert len(by_type) == len(tags), 'Native note tag types are repeated'
        for tag in node['tags']:
            if tag['status'] & 4:
                assert len(tasks) == 1 and tag['definition'] is None, 'Task association differs'
                task = tasks[0]
                assert uuid.UUID(bytes_le=bytes(tag['task_id'])) == uuid.UUID(task.get('guidTask')), 'Task identity differs'
                for mask, name in [(1, 'completed'), (2, 'disabled')]:
                    assert bool(tag['status'] & mask) == (task.get(name) == 'true'), 'Task state differs'
                for field, name in [('created', 'creationDate'), ('completed', 'completionDate'), ('start', 'startDate'), ('due', 'dueDate')]:
                    if tag[field]:
                        actual = datetime.fromtimestamp(tag[field] + 315532800, timezone.utc)
                        assert actual == datetime.fromisoformat(task.get(name).replace('Z', '+00:00')), 'Task date differs'
                    else:
                        assert task.get(name) is None, 'Unset task date differs'
                count += 1
                continue
            definition = space['nodes'][tag['definition']]['kind']
            expected = by_type[definition['action_type']]
            native_definition = definitions[expected.attrib['index']]
            assert definition['label'] == native_definition.attrib['name'], 'Tag label differs'
            assert definition['shape'] == int(native_definition.attrib['symbol']), 'Tag shape differs'
            assert definition['action_type'] == int(native_definition.attrib['type']), 'Tag type differs'
            assert bool(tag['status'] & 1) == (expected.attrib['completed'] == 'true'), 'Tag completion differs'
            assert bool(tag['status'] & 2) == (expected.attrib['disabled'] == 'true'), 'Tag disabled state differs'
            for field, key in [('created', 'creationDate'), ('completed', 'completionDate')]:
                if field == 'completed' and tag[field] == 0:
                    assert key not in expected.attrib, 'Unset tag completion date was exported'
                    continue
                if tag[field] is not None:
                    timestamp = datetime.fromtimestamp(tag[field] + 315532800, timezone.utc)
                    assert timestamp == datetime.fromisoformat(expected.attrib[key].replace('Z', '+00:00')), 'Tag date differs'
            for field, key in [('color', 'fontColor'), ('highlight', 'highlightColor')]:
                value = definition[field]
                if value is not None:
                    color = native_definition.attrib[key].lower()
                    assert (value == 0xff000000 and color in ('automatic', 'none')) or color == '#' + ''.join(f'{(value >> (8 * i)) & 255:02x}' for i in range(3)), 'Tag color differs'
            count += 1
        indexes = native.findall('one:MediaIndex', ns) + parents[native].findall('one:MediaIndex', ns)
        assert [uuid.UUID(bytes_le=bytes(value)) for value in node['media_ids']] == [uuid.UUID(value.find('one:MediaReference', ns).get('mediaID')) for value in indexes], 'Recording annotation association differs'
        for index in indexes:
            assert node['media_time_ms'] == int(index.get('timeIndex')), 'Recording annotation time differs'
    return count


def compare(notebook, native, versions=None, password_file=None):
    notebook = notebook.resolve(strict=True)
    native = native.resolve(strict=True)
    sections = sorted(notebook.rglob('*.one'))
    assert sections, 'No notebook sections were supplied for comparison'
    captures = {page.attrib['ID']: (page, path.with_suffix('.pdf'))
                for path, page in zip(sorted(native.glob('page-*.xml')), pages(native), strict=True)}
    native_payloads = json.loads((native / 'payloads.json').read_text(encoding='utf-8-sig')) if (native / 'payloads.json').exists() else []
    hierarchy = ET.parse(native / 'hierarchy.xml').getroot()
    compared = formatting = tags = 0
    discrepancies = []
    pdf_checks = []
    geometry = []
    autofit = []
    for path in sections:
        relative = path.relative_to(notebook)
        if versions is not None and relative.as_posix() != versions['section']:
            continue
        with TemporaryDirectory() as temporary:
            exported = Path(temporary) / 'document'
            command = [EXPORTER, path, exported]
            if password_file is not None:
                command.extend(['--password-file', password_file])
            subprocess.run(command, check=True)
            document = json.loads((exported / 'document.json').read_text())
            resolved_text = json.loads((exported / 'text.json').read_text())
            assets = {json.dumps(a['reference'], sort_keys=True): (exported / a['path']).read_bytes()
                      for a in json.loads((exported / 'assets.json').read_text())}
        candidates = [n for n in hierarchy.findall('.//one:Section', ns)
                      if PureWindowsPath(n.attrib['path']).parts[-len(relative.parts):] == relative.parts]
        assert len(candidates) == 1, (relative, 'native section identity')
        expected = [captures[n.attrib['ID']][0] for n in candidates[0].findall('one:Page', ns)]
        actual = list(ordered_pages(document))
        if versions is not None:
            assert hashlib.sha256(path.read_bytes()).hexdigest() == versions['source_sha256'], 'Historical source changed'
            section_pages = {n.get('ID') for n in candidates[0].findall('one:Page', ns)}
            historical = []
            expected = []
            associations = []
            for row in versions['pages']:
                assert row['native_id'] in section_pages, 'Copied version belongs to a different section'
                sid, _, revision, _ = actual[row['source_ordinal']]
                choices = list(version_pages(document, sid, revision))
                if row.get('selection') == 'sole-version':
                    assert len(choices) == 1, 'The native sole-version selection has multiple stored candidates'
                    matches = [(context, rid, page, oid) for context, rid, page, oid, _ in choices]
                else:
                    matches = [(context, rid, page, oid) for context, rid, page, oid, proxy in choices
                               if proxy['kind']['modified_filetime'] is not None
                               and datetime.fromtimestamp(proxy['kind']['modified_filetime'] / 10_000_000 - 11644473600, ZoneInfo(versions['timezone'])).date().isoformat() == row['displayed_date']]
                assert len(matches) == 1, 'Native version association is ambiguous'
                context, rid, page, oid = matches[0]
                historical.append((sid, rid, page, oid))
                expected.append(captures[row['native_id']][0])
                associations.append({**row, 'space': sid, 'context': context, 'revision': rid, 'object': oid})
            available = {(sid, context, oid) for sid, _, revision, _ in actual
                         for context, _, _, oid, _ in version_pages(document, sid, revision)}
            assert {(row['space'], row['context'], row['object']) for row in associations} == available, 'Native captures do not cover every historical page'
            assert len(historical) == len(available), 'Historical page is captured more than once'
            actual = historical
        assert len(actual) == len(expected), (relative, 'page count', len(actual), len(expected))
        for ordinal, ((sid, rid, space, oid), page) in enumerate(zip(actual, expected, strict=True)):
            title_nodes = [n for _, n in walk(space, oid) if n['kind']['type'] == 'RichText'
                           and any(field['id'] == 0x88001cb4 for field in n['extra'][0])]
            title = title_nodes[0]['kind']['text'].lstrip().split('\r')[0] if len(title_nodes) == 1 else ''
            if title:
                assert space['nodes'][space['roots']['2']]['kind']['title'] == title, 'Cached page title differs from visible title text'
            cached = space['nodes'][space['roots']['2']]['kind']['title'] or ''
            assert page.get('name') == (cached.replace('\t', ' ') or 'Untitled page'), 'Cached page title differs from native navigation'
            native_z = {int(n.find('one:Position', ns).attrib['z']) for n in page if n.find('one:Position', ns) is not None}
            source_page = space['nodes'][oid]
            assert bool(source_page['kind']['rtl']) == (page.find('one:PageSettings', ns).get('RTL') == 'true'), 'Page direction differs'
            if source_page['kind']['rtl']:
                # Storage orders columns visually left to right; native XML follows page direction.
                for table in page.findall('.//one:Table', ns):
                    columns = table.find('one:Columns', ns)
                    columns[:] = reversed(columns[:])
                    for row in table.findall('one:Row', ns):
                        row[:] = reversed(row[:])
            assert sum(space['nodes'][child]['kind']['type'] == 'Ink' for child in source_page['children']) == len(page.findall('one:InkDrawing', ns)), 'Opaque ink object count differs'
            for child in page:
                position = child.find('one:Position', ns)
                if position is None:
                    continue
                z = int(position.attrib['z'])
                source = space['nodes'][source_page['children'][z]]
                if source['kind']['type'] == 'Ink':
                    continue
                for axis in ('x', 'y'):
                    if source['layout'][axis] is not None:
                        stored_position, native_position = source['layout'][axis], float(position.attrib[axis])
                        origin = source_page['kind']['margin_origin_' + axis]
                        canonical_origin = (-36.0 if source_page['kind']['rtl'] else 36.0) if axis == 'x' else 14.4
                        projected = stored_position + (canonical_origin - origin if origin is not None else 0)
                        if axis == 'x' and source_page['kind']['rtl']:
                            assert source['kind']['type'] == 'Outline' and source['layout']['max_width'] is not None, 'RTL positioning requires an outline width'
                            assert not any(p['id'] == 0x14001c84 for group in source['extra'] for p in group), 'Explicit RTL outline alignment requires a native control'
                            # Default RTL outlines keep their right edge when native layout changes width.
                            projected += source['layout']['max_width'] - float(child.find('one:Size', ns).get('width'))
                        if abs(projected - native_position) > 0.002:
                            geometry.append({'file': str(relative), 'page': ordinal, 'z': z, 'axis': axis,
                                             'stored': stored_position, 'origin': origin, 'projected': projected, 'native': native_position})
            roots = list(source_page['structure'])
            for z, child in enumerate(source_page['children']):
                subtree = list(walk(space, child))
                if z not in native_z:
                    assert all(n['kind']['type'] in ('Outline', 'Paragraph', 'RichText') for _, n in subtree)
                    # Native XML omits otherwise empty outlines containing only ASCII spaces.
                    assert not any(n['kind'].get('text', '').strip(' ') or n['kind'].get('lists') or n['tags'] for _, n in subtree)
                    continue
                roots.append(child)
            text_nodes = [n for root in roots for _, n in walk(space, root)
                          if n['kind']['type'] == 'RichText' and not n['kind']['boilerplate']]
            observed = [visible_text(n, space) for n in text_nodes]
            native_children = list(page)
            content = [n for n in native_children if n.find('one:Position', ns) is not None]
            content.sort(key=lambda n: int(n.find('one:Position', ns).attrib['z']))
            wanted = [text for n in native_children if n.tag == '{' + ns['one'] + '}Title' for text in texts(n)]
            wanted.extend(text for n in content for text in texts(n))
            if observed != wanted:
                destination = native.parent / 'text-order-diff.json'
                destination.write_text(json.dumps({'file': str(relative), 'page': ordinal, 'space': sid,
                                                  'actual': observed, 'native': wanted}, indent=2))
                raise AssertionError(f'{relative}: page {ordinal}: ordered text differs; inspect {destination}')
            native_roots = [n for n in native_children if n.tag == '{' + ns['one'] + '}Title'] + content
            try:
                tags += compare_objects(space, roots, page, native_roots, assets, native_payloads, autofit)
                native_runs = native_characters(page, native_roots)
                text_ids = [oid for root in roots for oid, n in walk(space, root)
                            if n['kind']['type'] == 'RichText' and not n['kind']['boilerplate']]
                for text_id, expected_runs in zip(text_ids, native_runs, strict=True):
                    observed_runs = [(char, run['link']) for run in resolved_text[sid][rid][text_id]
                                     if not run['format']['hidden'] for char in run['text']]
                    if observed_runs and observed_runs[-1][0] == '\r':
                        observed_runs.pop()
                    observed_runs = [(projected, link) for char, link in observed_runs for projected in project_text(char)]
                    expected_links = [(char, style.get('link')) for char, style in expected_runs]
                    if observed_runs == [('\u00a0', None)] and not expected_links:
                        observed_runs = []
                    assert observed_runs == expected_links, 'Resolved text or associated hyperlink differs'
                count, differences = compare_formats(space, text_nodes, native_runs)
                formatting += count
                pdf = captures[page.attrib['ID']][1]
                if differences and pdf.exists() and all(d['field'] == 'highlight' and d['stored'] == '#000000' and d['native'] == 'automatic' for d in differences):
                    paragraphs = verify_pdf_black(pdf, [resolved_text[sid][rid][i] for i in text_ids])
                    pdf_checks.append({'file': str(relative), 'page': ordinal, 'paragraphs': paragraphs,
                                       'pdf': pdf.name, 'pdf_sha256': hashlib.sha256(pdf.read_bytes()).hexdigest(),
                                       'source_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                                       'xml_omissions': differences})
                    differences = []
                discrepancies.extend({"file": str(relative), "page": ordinal, **d} for d in differences)
            except AssertionError as error:
                raise AssertionError((str(relative), ordinal, str(error))) from error
            compared += 1
    if versions is not None:
        assert compared == len(versions['pages']) > 0, 'No historical source section was compared'
    (native.parent / 'geometry-differences.json').write_text(json.dumps(geometry, indent=2))
    (native.parent / 'table-autofit.json').write_text(json.dumps(autofit, indent=2))
    (native.parent / 'pdf-format-checks.json').write_text(json.dumps(pdf_checks, indent=2))
    destination = native.parent / 'format-differences.json'
    destination.write_text(json.dumps(discrepancies, indent=2))
    if geometry:
        raise AssertionError(f'{len(geometry)} coordinate differences; inspect {native.parent / "geometry-differences.json"}')
    if discrepancies:
        raise AssertionError(f'{len(discrepancies)} formatting differences across {compared} pages; inspect {destination}')
    if versions is not None:
        (native.parent / 'version-associations.json').write_text(json.dumps(associations, indent=2))
    print(f'Passed: {compared} pages with native-normalized paragraph text/order; {tags} associated tags; {formatting} explicit character-format comparisons')
    if pdf_checks:
        print(f'Native PDF rectangles independently verified black-highlight character positions in {sum(p["paragraphs"] for p in pdf_checks)} paragraphs where XML omitted formatting.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('notebook', type=Path)
    parser.add_argument('native', type=Path)
    parser.add_argument('--versions', type=Path, help='Native history UI date and copied-page associations.')
    parser.add_argument('--password-file', type=Path, help='Exact UTF-8 password bytes; requires the protected exporter feature.')
    args = parser.parse_args()
    compare(args.notebook.resolve(), args.native.resolve(), json.loads(args.versions.read_text()) if args.versions else None, args.password_file)
