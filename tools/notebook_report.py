#!/usr/bin/env python3
"""Export a copied notebook to an offline, read-only document report."""
import argparse
from collections import Counter
from datetime import datetime, timedelta, timezone
import hashlib
from html import escape as esc
import json
import os
from io import BytesIO
from pathlib import Path, PureWindowsPath
from PIL import Image
import xml.etree.ElementTree as ET
from native_xml import ns
import subprocess
from urllib.parse import urlsplit
from zoneinfo import ZoneInfo

from document_model import BRIDGE, DEFAULT_CONTEXT, EXPORTER, ordered_pages, version_pages, view, walk

ROOT = Path(__file__).resolve().parent.parent


def hyperlink(fragment, target):
    if urlsplit(target).scheme.lower() in ('http', 'https', 'mailto', 'onenote'):
        return '<a href="' + esc(target) + '" rel="noreferrer">' + fragment + '</a>'
    return fragment + ' <span class="meta">(' + esc(target) + ')</span>'


def color(value):
    if value is None or value >> 24:
        return None
    return '#' + ''.join(f'{value >> shift & 255:02x}' for shift in (0, 8, 16))


def css(fmt):
    rules = []
    for field, prop, yes, no in [('bold', 'font-weight', 'bold', 'normal'),
                                ('italic', 'font-style', 'italic', 'normal')]:
        if fmt.get(field) is not None:
            rules.append(f'{prop}:{yes if fmt[field] else no}')
    decorations = [v for k, v in [('underline', 'underline'), ('strike', 'line-through')] if fmt.get(k)]
    if decorations:
        rules.append('text-decoration:' + ' '.join(decorations))
    for field, prop in [('font', 'font-family'), ('font_size', 'font-size'),
                        ('color', 'color'), ('highlight', 'background-color')]:
        value = fmt.get(field)
        if value is not None:
            if field == 'font':
                fallback = 'monospace' if value in ('Consolas', 'Courier New', 'Lucida Console') else 'serif' if value in ('Times New Roman', 'Cambria', 'Georgia') else 'sans-serif'
                value = json.dumps(value) + ',' + fallback
            else:
                value = f'{value:g}pt' if field == 'font_size' else color(value)
            if value:
                rules.append(f'{prop}:{value}')
    for field, prop in [('space_before', 'margin-top'), ('space_after', 'margin-bottom')]:
        if fmt.get(field) is not None:
            rules.append(f'{prop}:{fmt[field]:g}pt')
    if fmt.get('alignment') is not None:
        rules.append('text-align:' + {0: 'left', 1: 'center', 2: 'right'}.get(fmt['alignment'], 'start'))
    if fmt.get('line_spacing'):
        rules.append(f'line-height:max(1.5em,{fmt["line_spacing"]:g}pt)')
    if fmt.get('rtl'):
        rules.append('direction:rtl')
    return ';'.join(rules)


def html_page(title, nav, body, editable=False):
    policy = "default-src 'none'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; media-src 'self'; base-uri 'none'"
    if editable:
        policy += "; script-src 'self'; connect-src 'self'"
    return '''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="''' + policy + '''">
<title>''' + esc(title) + '''</title><style>
*{box-sizing:border-box}body{margin:0;color:#000;background:#fff;font:15px/1.5 system-ui,sans-serif}a{color:#175bb2}nav{position:fixed;inset:0 auto 0 0;width:240px;overflow:auto;background:#f5f6f7;padding:20px}nav a{display:block;padding:3px 0;overflow-wrap:anywhere}nav h2{font-size:14px;margin:20px 0 4px}main{margin-left:240px;padding:30px 40px;max-width:1250px}h1{font-size:28px;line-height:1.2}h2{font-size:18px}p{margin:6px 0}.outline{margin:24px 0;border-top:1px solid #d9dde2;padding-top:12px}.location,.meta{font:12px/1.5 system-ui;color:#666;margin:6px 0}.paragraph{min-height:1.3em;position:relative;overflow-wrap:anywhere}.nested{margin-left:24px}.text{white-space:pre-wrap}.tag{display:inline-block;font:12px system-ui;padding:2px 5px;border:1px solid #aaa;border-radius:3px;margin-right:5px}.list-marker{display:inline-block;min-width:22px;margin-left:-22px}.listed{margin-left:22px}img.content{max-width:100%;height:auto;vertical-align:top}figure{margin:12px 0}figcaption{font-size:12px;color:#666}table{border-collapse:collapse;margin:8px 0;max-width:100%}td{border:1px solid #bbb;padding:5px 8px;vertical-align:top;min-width:30px}table.no-borders td{border-color:transparent}.opaque{border:1px dashed #9aa2ad;padding:12px;margin:12px 0;background:#f7f8fa}details{margin:14px 0}summary{cursor:pointer;font:13px system-ui}pre{white-space:pre-wrap;overflow-wrap:anywhere;font:11px/1.4 ui-monospace,monospace;max-height:560px;overflow:auto}.page-link{padding-left:12px}code{font-size:12px}.references a{margin-right:14px}@media(max-width:750px){nav{position:static;width:auto;max-height:220px}main{margin:0;padding:20px}}@media print{nav{display:none}main{margin:0}details{display:none}}
</style>''' + ('<link rel="stylesheet" href="/editor.css"><script src="/editor.js" defer></script>' if editable else '') + '<nav>' + nav + '</nav><main>' + body + '</main></html>'


class Page:
    def __init__(self, section, sid, rid):
        self.section = section
        self.space = section['document']['spaces'][sid]['revisions'][rid]
        self.text = section['text'][sid][rid]
        self.counts = Counter()
        self.numbering = {}
        self.has_paragraph = False
        self.active = set()

    def asset(self, container):
        if container is None:
            return None
        node = self.space['nodes'][container]
        if node['kind']['type'] != 'File':
            raise ValueError('The asset container is not a file object.')
        return self.section['assets'].get(json.dumps(node['kind']['reference'], sort_keys=True))

    def render(self, oid, depth=0, level=0):
        if oid in self.active or depth > 256:
            raise ValueError('The content graph is cyclic or exceeds the report depth limit.')
        self.active.add(oid)
        node = self.space['nodes'][oid]
        kind = node['kind']; typ = kind['type']
        self.counts[typ] += 1
        children = lambda refs: ''.join(self.render(x, depth + 1, level) for x in refs)
        tags = ''
        tag_format = {}
        for tag in node['tags']:
            definition = self.space['nodes'][tag['definition']]['kind'] if tag['definition'] else {}
            label = definition.get('label') or 'Task'
            if not tag['status'] & 2:
                for field in ('color', 'highlight'):
                    if color(definition.get(field)):
                        tag_format.setdefault(field, definition[field])
            if tag['due']:
                label += ' · Due ' + datetime.fromtimestamp(tag['due'] + 315532800, timezone.utc).strftime('%Y-%m-%d')
            if tag['status'] & 2:
                label += ' · Disabled'
            tags += '<span class="tag" title="' + esc(json.dumps(tag)) + '">' + (('☑ ' if tag['status'] & 1 else '☐ ') if definition.get('shape') == 3 or tag['status'] & 4 else '') + esc(label) + '</span>'
        if typ == 'RichText':
            body = ''
            structured_math = any(c in kind['text'] for c in '\ufdd0\ufdee\ufdef')
            equation = False
            for run_index, run in enumerate(self.text[oid]):
                if run['format']['hidden']:
                    continue
                if structured_math and run['format'].get('math'):
                    if not equation:
                        body += '<span class="meta">[Equation · see native reference]</span>'
                    equation = True
                    continue
                equation = False
                style = {**run['format'], **tag_format}
                if (style['superscript'] or style['subscript']) and style['font_size'] is not None:
                    style['font_size'] *= 2 / 3
                fragment = '<span data-text-object="' + esc(oid) + '" data-run="' + str(run_index) + '" style="' + esc(css(style)) + '">' + esc(run['text']) + '</span>'
                if run['format']['superscript']:
                    fragment = '<sup>' + fragment + '</sup>'
                if run['format']['subscript']:
                    fragment = '<sub>' + fragment + '</sub>'
                if run['link']:
                    fragment = hyperlink(fragment, run['link'])
                body += fragment
            body = tags + '<span class="text">' + body + '</span>'
        elif typ == 'Paragraph':
            body = children(node['content'])
            marker = ''
            for list_id in kind['lists']:
                item = self.space['nodes'][list_id]['kind']
                fmt = item['format'] or ''
                if '\ufffd' in fmt:
                    position = fmt.index('\ufffd'); style = ord(fmt[position + 1])
                    key = (fmt, level)
                    value = item['restart'] if item['restart'] is not None else self.numbering.get(key, 0) + 1
                    self.numbering[key] = value
                    number = str(value)
                    if style in (1, 2):
                        number = ''; rest = value
                        for quantity, letters in [(1000, 'M'), (900, 'CM'), (500, 'D'), (400, 'CD'), (100, 'C'), (90, 'XC'), (50, 'L'), (40, 'XL'), (10, 'X'), (9, 'IX'), (5, 'V'), (4, 'IV'), (1, 'I')]:
                            times, rest = divmod(rest, quantity)
                            number += letters * times
                        if style == 2: number = number.lower()
                    elif style in (3, 4):
                        number = ''; rest = value
                        while rest:
                            rest, digit = divmod(rest - 1, 26)
                            number = chr(65 + digit) + number
                        if style == 4: number = number.lower()
                    elif style != 0:
                        raise ValueError(f'Number format {style} requires interpretation before report generation.')
                    marker += fmt[:position] + number + fmt[position + 2:]
                else:
                    marker += {1: '•', 2: '◦', 3: '●', 4: '○', 5: '◉', 6: '◎', 7: '▪', 8: '▫', 9: '■', 10: '□', 11: '▸', 12: '▶', 13: '◇', 14: '♢', 15: '◆', 16: '❖', 17: '★', 18: '☆', 19: '☀', 20: '>', 21: '→', 22: '⇒', 23: '⇨', 24: '*', 25: '-', 26: '–', 27: '—'}.get(item['bullet'], fmt)
            if marker:
                body = '<span class="list-marker">' + esc(marker) + '</span>' + body
            fmt = node['format']
            if kind['paragraph_style']:
                parent = self.space['nodes'][kind['paragraph_style']]['format']
                fmt = {k: v if v is not None else parent[k] for k, v in fmt.items()}
            for child in node['content']:
                if child in self.text and self.text[child]:
                    resolved = self.text[child][0]['format']
                    fmt = {**fmt, **{k: resolved[k] for k in ('alignment', 'rtl', 'space_before', 'space_after', 'line_spacing') if resolved[k] is not None}}
                    break
            if not self.has_paragraph:
                fmt = {**fmt, 'space_before': None}
            self.has_paragraph = True
            body = '<div class="paragraph' + (' listed' if marker else '') + '" style="' + esc(css(fmt)) + '">' + tags + body + '</div>'
            if node['children']:
                child_level = node['child_level'] or 1
                descendants = '<div class="nested" style="margin-left:' + str(child_level * 24) + 'px">' + ''.join(self.render(x, depth + 1, level + child_level) for x in node['children']) + '</div>'
                state = kind.get('collapse_state')
                if state == 1:
                    descendants = '<details><summary>Collapsed paragraphs</summary>' + descendants + '</details>'
                elif state not in (None, 0):
                    descendants = '<div class="opaque">Uninterpreted collapse state: ' + str(state) + '</div>' + descendants
                body += descendants
        elif typ in ('Page', 'Title', 'Outline'):
            if typ == 'Outline':
                previous = self.numbering, self.has_paragraph
                self.numbering, self.has_paragraph = {}, False
            body = children(node['structure'] + node['content'] + node['children'])
            if typ == 'Outline':
                self.numbering, self.has_paragraph = previous
            if typ == 'Outline' and node['layout']['x'] is not None:
                values = ', '.join(f'{k} {v:g} pt' for k, v in node['layout'].items() if v is not None)
                body = '<section class="outline"><div class="location">' + esc(values) + '</div>' + body + '</section>'
            elif typ == 'Title':
                body = '<header>' + body + '</header>'
        elif typ == 'OutlineGroup':
            child_level = node['child_level'] or 1
            body = '<div class="nested" style="margin-left:' + str(child_level * 24) + 'px">' + ''.join(self.render(x, depth + 1, level + child_level) for x in node['children']) + '</div>'
        elif typ == 'Table':
            cols = ''.join(f'<col style="width:{width:g}pt">' if kind['locked'] and kind['locked'][i] else '<col>' for i, width in enumerate(kind['widths']))
            body = '<table class="' + ('no-borders' if kind['borders'] is False else '') + '"><colgroup>' + cols + '</colgroup>' + children(node['children']) + '</table>'
        elif typ == 'Row':
            body = '<tr>' + children(node['children']) + '</tr>'
        elif typ == 'Cell':
            shade = color(kind['shading'])
            body = '<td' + (' style="background:' + shade + '"' if shade else '') + '>' + children(node['children']) + '</td>'
        elif typ == 'Image':
            asset = self.asset(kind['container'])
            if asset:
                width = node['layout']['max_width'] if node['layout']['max_width'] is not None else kind['picture_width']
                height = node['layout']['max_height'] if node['layout']['max_height'] is not None else kind['picture_height']
                style = f'width:{width:g}pt' if width is not None else ''
                if width and height:
                    style += f';aspect-ratio:{width:g}/{height:g}'
                preview = self.section['previews'].get(asset, asset)
                if preview.endswith('.bin'):
                    content = '<span class="opaque">Image format is uninterpreted; the original payload is retained in the asset references.</span>'
                else:
                    content = '<img class="content" src="' + esc(preview) + '" style="' + style + '" alt="' + esc(kind['alt'] or '') + '">'
                body = '<figure>' + tags + (hyperlink(content, kind['link']) if kind['link'] else content)
                labels = [kind['filename']] if kind['filename'] else []
                if kind['background']: labels.append('Background image')
                if kind['printout']: labels.append('Printout image')
                if labels:
                    body += '<figcaption>' + esc(' · '.join(labels)) + '</figcaption>'
                body += '</figure>'
                if kind['background'] and not kind['printout']:
                    body = '<details><summary>Background image' + (' · ' + esc(kind['filename']) if kind['filename'] else '') + '</summary>' + body + '</details>'
            else:
                body = '<div class="opaque">Image payload is external; its source reference is retained.</div>'
        elif typ == 'Attachment':
            asset = self.asset(kind['container'])
            label = kind['filename'] or 'Attached file'
            body = tags + ('<a download="' + esc(label) + '" href="' + esc(asset) + '">' + esc(label) + '</a>' if asset else esc(label) + ' · External payload')
        elif typ == 'Ink':
            body = '<div class="opaque">Ink drawing · Stroke data is retained in the document structure.</div>'
        else:
            body = '<div class="opaque">' + ('Encrypted content' if typ == 'Encrypted' else f'Uninterpreted content · JCID {node["jcid"]:#x}') + '<br><code>' + esc(oid) + '</code>' + children(node['structure'] + node['content'] + node['children']) + '</div>'
        for recording in node['media_ids']:
            matches = [n['kind'] for n in self.space['nodes'].values()
                       if n['kind']['type'] == 'Attachment' and n['kind']['recording_id'] == recording]
            label = 'Recording reference'
            asset = None
            if len(matches) == 1:
                label = matches[0]['filename'] or label
                asset = self.asset(matches[0]['container'])
            if node['media_time_ms'] is not None:
                seconds = node['media_time_ms'] / 1000
                label += f' · {seconds:g} s'
            body += '<div class="meta">' + ('<a href="' + esc(asset) + '" download>' + esc(label) + '</a>' if asset else esc(label)) + '</div>'
        self.active.remove(oid)
        return '<div data-object="' + esc(oid) + '">' + body + '</div>' if typ not in ('RichText', 'Row', 'Cell') else body


def generate(source, destination, native=None, versions=(), zone=timezone.utc, editable=False, previous=None):
    source = source.resolve(strict=True)
    if destination.resolve().is_relative_to(source):
        raise ValueError('Choose an export directory outside the source notebook.')
    destination.mkdir(parents=True, exist_ok=False)
    (destination / 'model').mkdir(); (destination / 'assets').mkdir()
    cached = {row['path']: (row['sha256'], previous / 'model' / str(index))
              for index, row in enumerate(json.loads((previous / 'source.json').read_text()))} if previous else {}
    result = json.loads(subprocess.check_output([BRIDGE, 'catalog', source], timeout=120))
    if not result['ok']: raise ValueError(result['error'])
    catalog = result['catalog']
    (destination / 'catalog.json').write_text(json.dumps(catalog, indent=2, ensure_ascii=False))
    files = []; catalog_sections = {}
    def collect(folder):
        if folder['toc'] is not None: files.append(Path(folder['path']) / folder['toc']['filename'])
        for section in folder['sections']:
            relative = Path(section['path'])
            files.append(relative); catalog_sections[relative] = section
        for group in folder['groups']: collect(group)
    collect(catalog)
    sections = []; unavailable = []; manifest = []
    for index, relative in enumerate(sorted(files)):
        path = source / relative
        before = path.read_bytes()
        manifest.append({'path': relative.as_posix(), 'sha256': hashlib.sha256(before).hexdigest(), 'bytes': len(before)})
        state = catalog_sections.get(relative, {}).get('state', {})
        if isinstance(state, dict) and 'Unreadable' in state:
            unavailable.append((relative, state['Unreadable']))
            continue
        exported = destination / 'model' / str(index)
        reusable = cached.get(relative.as_posix())
        reused = reusable is not None and reusable[0] == manifest[-1]['sha256']
        if reused:
            exported.mkdir()
            for name in ('document.json', 'text.json', 'assets.json'):
                os.link(reusable[1] / name, exported / name)
        else:
            subprocess.run([EXPORTER, path, exported], check=True)
        document = json.loads((exported / 'document.json').read_text())
        if path.read_bytes() != before:
            raise ValueError('A source file changed during export.')
        assets = {}
        previews = {}
        image_references = {
            json.dumps(revision['nodes'][node['kind']['container']]['kind']['reference'], sort_keys=True)
            for space in document['spaces'].values() for revision in space['revisions'].values()
            for node in revision['nodes'].values()
            if node['kind']['type'] == 'Image' and node['kind']['container'] is not None
        }
        rows = json.loads((exported / 'assets.json').read_text())
        for asset in rows:
            reference = json.dumps(asset['reference'], sort_keys=True)
            if reused:
                name = Path(asset['path']).name
                assets[reference] = 'assets/' + name
                if not (destination / 'assets' / name).exists():
                    os.link(previous / 'assets' / name, destination / 'assets' / name)
                preview = name + '.png'
                if name.endswith('.tiff') and reference in image_references:
                    if not (destination / 'assets' / preview).exists():
                        os.link(previous / 'assets' / preview, destination / 'assets' / preview)
                    previews['assets/' + name] = 'assets/' + preview
                continue
            original = exported / asset['path']; data = original.read_bytes()
            extension = '.png' if data.startswith(b'\x89PNG') else '.jpg' if data.startswith(b'\xff\xd8') else '.gif' if data.startswith(b'GIF8') else '.bmp' if data.startswith(b'BM') else '.tiff' if data.startswith((b'II*\0', b'MM\0*')) else '.bin'
            name = hashlib.sha256(data).hexdigest() + extension
            target = destination / 'assets' / name
            if target.exists():
                original.unlink()
            else:
                original.rename(target)
            asset['path'] = '../../assets/' + name
            assets[reference] = 'assets/' + name
            if extension == '.tiff' and reference in image_references:
                preview = name + '.png'
                with Image.open(BytesIO(data)) as image:
                    if image.n_frames != 1:
                        raise ValueError('Multipage TIFF requires frame interpretation before report generation.')
                    image.convert('RGBA').save(destination / 'assets' / preview)
                previews['assets/' + name] = 'assets/' + preview
        if not reused:
            (exported / 'assets').rmdir()
            (exported / 'assets.json').write_text(json.dumps(rows, indent=2))
        if path.suffix.lower() == '.one':
            sections.append({'path': relative, 'export': exported.relative_to(destination), 'document': document,
                             'text': json.loads((exported / 'text.json').read_text()), 'assets': assets, 'previews': previews})
    order = {path: index for index, path in enumerate(catalog_sections)}
    sections.sort(key=lambda section: order[section['path']])
    pages = []; locked_sections = []; histories = {}; nav = '<a href="index.html">Notebook review</a>'
    for section in sections:
        state = catalog_sections[section['path']]['state']
        name = state.get('Readable', {}).get('name') if isinstance(state, dict) else None
        if name is None: name = section['path'].stem
        nav += '<h2>' + esc(str(section['path'].parent) + ' / ' + name) + '</h2>'
        _, root_space = view(section['document'], section['document']['root'])
        if root_space['nodes'][root_space['roots']['1']]['kind']['type'] == 'Encrypted':
            locked_sections.append(str(section['path']))
            nav += '<p>Locked section · Page count unavailable</p><a href="' + section['export'].as_posix() + '/document.json">Encrypted structure</a>'
            continue
        ordinary = list(ordered_pages(section['document']))
        known = {(sid, rid, oid) for sid, rid, _, oid in ordinary}
        additional = [(sid, rid, revision, oid) for sid in section['document']['spaces']
                      for rid, revision in [view(section['document'], sid)]
                      for oid, node in revision['nodes'].items() if node['kind']['type'] == 'Page' and (sid, rid, oid) not in known]
        historical = []
        for sid, _, current, parent_oid in ordinary + additional:
            for context, rid, revision, oid, proxy in version_pages(section['document'], sid, current):
                historical.append((sid, context, rid, revision, oid))
                modified = proxy['kind']['modified_filetime']
                histories[(section['path'], sid, context, oid)] = {
                    'modified': (datetime(1601, 1, 1, tzinfo=timezone.utc) + timedelta(microseconds=modified // 10)).astimezone(zone).isoformat() if modified is not None else None,
                    'source': (section['path'], sid, DEFAULT_CONTEXT, parent_oid),
                }
        if not ordinary and not additional:
            nav += '<p>Empty section</p>'
        current_pages = [(sid, DEFAULT_CONTEXT, rid, revision, oid) for sid, rid, revision, oid in ordinary + additional]
        for ordinal, (sid, context, rid, space, oid) in enumerate(current_pages + historical):
            category = 'Historical version' if ordinal >= len(ordinary) + len(additional) else 'Additional stored page' if ordinal >= len(ordinary) else 'Recycle bin' if 'OneNote_RecycleBin' in section['path'].parts else 'Page'
            metadata = space['nodes'].get(space['roots'].get('2'), {}).get('kind', {})
            if sid == root_space['nodes'][root_space['roots']['1']]['kind'].get('default_template'):
                if metadata.get('type') != 'TemplateMetadata':
                    raise ValueError('The default page template has unrecognized metadata.')
                category = 'Default page template'
            titles = [n for root in space['nodes'][oid]['structure'] for _, n in walk(space, root) if n['kind']['type'] == 'RichText' and not n['kind']['boilerplate']]
            title = ' '.join(n['kind']['text'] for n in titles) or space['nodes'][oid]['kind']['alternate_title'] or metadata.get('title') or metadata.get('name') or 'Untitled page'
            filename = f'page-{len(pages):03}.html'
            if metadata.get('type') == 'ConflictMetadata':
                category = 'Conflict page'
            level = metadata.get('level') or 1
            label = category + ' · ' + title if category in ('Additional stored page', 'Default page template', 'Conflict page', 'Historical version') else title
            version = histories.get((section['path'], sid, context, oid))
            if version and version['modified']:
                label += ' · ' + version['modified'][:10]
            nav += '<a class="page-link" style="margin-left:' + str((level - 1) * 12) + 'px" href="' + filename + '">' + esc(label) + '</a>'
            pages.append((section, ordinal, sid, context, rid, oid, title, filename, category))
    references = {}
    if native is not None:
        native = native.resolve(strict=True)
        captured = native.parent / 'notebook'
        if captured.exists():
            hashes = {p.relative_to(captured).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in captured.rglob('*') if p.is_file()}
        else:
            hashes = {item['path']: item['sha256'] for item in json.loads((native.parent / 'source.json').read_text())}
        if any(hashes.get(item['path']) != item['sha256'] for item in manifest):
            raise ValueError('The native capture does not describe these source files.')
        captures = {ET.parse(p).getroot().get('ID'): p.stem for p in native.glob('page-*.xml')}
        hierarchy = ET.parse(native / 'hierarchy.xml').getroot()
        for section in sections:
            relative = section['path']
            matches = [node for node in hierarchy.findall('.//one:Section', ns)
                       if PureWindowsPath(node.attrib['path']).parts[-len(relative.parts):] == relative.parts]
            if len(matches) != 1:
                raise ValueError('The native section association is ambiguous.')
            native_pages = matches[0].findall('one:Page', ns)
            for (sid, _, _, oid), page in zip(ordered_pages(section['document']), native_pages, strict=True):
                references[(relative, sid, DEFAULT_CONTEXT, oid)] = Path('reference') / captures[page.get('ID')]
        (destination / 'reference').symlink_to(native, target_is_directory=True)
    for index, capture in enumerate(versions):
        capture = capture.resolve(strict=True)
        config = json.loads((capture / 'version-ui.json').read_text())
        matching = [section for section in sections if section['path'].name == Path(config['section']).name
                    and hashlib.sha256((source / section['path']).read_bytes()).hexdigest() == config['source_sha256']]
        if len(matching) != 1:
            raise ValueError('The historical capture source association is ambiguous.')
        section, = matching
        captures = {ET.parse(p).getroot().get('ID'): p.stem for p in (capture / 'read').glob('page-*.xml')}
        directory = Path(f'reference-history-{index}')
        (destination / directory).symlink_to(capture / 'read', target_is_directory=True)
        for row in json.loads((capture / 'version-associations.json').read_text()):
            key = (section['path'], row['space'], row['context'], row['object'])
            rid, revision = view(section['document'], row['space'], row['context'])
            if key not in histories or rid != row['revision'] or revision['nodes'][row['object']]['kind']['type'] != 'Page':
                raise ValueError('The historical capture does not describe this page revision.')
            if key in references:
                raise ValueError('A page has multiple native reference captures.')
            references[key] = directory / captures[row['native_id']]
    accounting = []
    source_pages = {(section['path'], conflict): filename
                    for section, _, sid, _, rid, _, _, filename, category in pages if category != 'Historical version'
                    for revision in [section['document']['spaces'][sid]['revisions'][rid]]
                    for conflict in revision['nodes'][revision['roots']['1']]['spaces']}
    page_files = {(section['path'], sid, context, oid): filename for section, _, sid, context, _, oid, _, filename, _ in pages}
    for section, ordinal, sid, context, rid, oid, title, filename, category in pages:
        page = Page(section, sid, rid)
        body = '<div class="meta">' + esc(str(section['path'])) + ' · ' + category + ' ' + str(ordinal + 1) + '</div>'
        if not page.space['nodes'][oid]['structure']:
            body += '<h1>' + esc(title) + '</h1>'
        metadata = page.space['nodes'].get(page.space['roots'].get('2'), {}).get('kind', {})
        if metadata.get('type') == 'ConflictMetadata' and metadata['author']:
            body += '<p class="meta">Conflicting author: ' + esc(metadata['author']) + '</p>'
        source_page = source_pages.get((section['path'], sid))
        version = histories.get((section['path'], sid, context, oid))
        if version:
            source_page = page_files[version['source']]
            if version['modified']:
                body += '<p class="meta">Version modified: ' + esc(version['modified']) + '</p>'
        if source_page:
            body += '<p><a href="' + source_page + '">Source page</a></p>'
        reference = references.get((section['path'], sid, context, oid))
        if reference:
            body += '<p class="references">'
            for suffix, label in [('.pdf', 'Native PDF'), ('.png', 'Native screenshot'), ('.xml', 'Native XML')]:
                if (destination / reference.with_suffix(suffix)).exists():
                    body += '<a href="' + reference.with_suffix(suffix).as_posix() + '">' + label + '</a>'
            body += '</p>'
        body += page.render(oid)
        body += '<details><summary>Document structure and source identities</summary><pre>' + esc(json.dumps(page.space, indent=2, ensure_ascii=False)) + '</pre></details>'
        body += '<p class="references"><a href="' + section['export'].as_posix() + '/document.json">Document JSON</a><a href="' + section['export'].as_posix() + '/assets.json">Asset references</a></p>'
        (destination / filename).write_text(html_page(title, nav, body, editable and category == 'Page'))
        accounting.append({'section': str(section['path']), 'ordinal': ordinal, 'space': sid, 'revision': rid, 'object': oid, 'title': title, 'report': filename, 'category': category, 'context': context, 'version_modified': version['modified'] if version else None, 'source_report': source_page, 'native_reference': reference.as_posix() if reference else None, 'rendered': dict(page.counts)})
    (destination / 'source.json').write_text(json.dumps(manifest, indent=2))
    (destination / 'pages.json').write_text(json.dumps(accounting, indent=2, ensure_ascii=False))
    intro = '<h1>Notebook review</h1><p>' + str(len(sections) + len(unavailable)) + ' sections · ' + str(len(pages)) + ' stored pages</p><p>Readable content follows the stored object order. Outline positions are shown in points. The document structure retains properties and identities that the readable view does not interpret.</p><p><a href="source.json">Source hashes</a> · <a href="pages.json">Page inventory</a> · <a href="catalog.json">Notebook catalog</a></p>'
    if locked_sections:
        intro += '<p>Page counts are unavailable for locked sections: ' + esc(', '.join(locked_sections)) + '.</p>'
    for path, error in unavailable:
        intro += '<p>Cannot read ' + esc(str(path)) + ': ' + esc(error['message']) + ' (offset ' + str(error['offset']) + ').</p>'
    (destination / 'index.html').write_text(html_page('Notebook review', nav, intro, editable))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('destination', type=Path)
    parser.add_argument('--native', type=Path, help='Associate a matching native capture directory.')
    parser.add_argument('--versions', type=Path, action='append', default=[], help='Associate a verified historical capture; repeat for each section.')
    parser.add_argument('--timezone', type=ZoneInfo, default=timezone.utc, help='Display historical timestamps in this time zone; default UTC.')
    args = parser.parse_args()
    generate(args.source, args.destination, args.native, args.versions, args.timezone)
