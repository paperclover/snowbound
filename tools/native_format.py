"""Independent formatting oracle from the application's XML and inline HTML."""
from html.parser import HTMLParser
from native_xml import ns, project_text
from PIL import ImageColor


def css(value):
    result = {}
    for item in value.split(';'):
        key, sep, text = item.partition(':')
        if not sep:
            continue
        key, text = key.strip().lower(), text.strip().strip('"\'')
        if key == 'font-family': result['font'] = text
        elif key == 'font-size': result['font_size'] = float(text.removesuffix('pt'))
        elif key == 'font-weight': result['bold'] = text in ('bold', '700')
        elif key == 'font-style': result['italic'] = text == 'italic'
        elif key == 'text-decoration':
            result['underline'] = 'underline' in text
            result['strike'] = 'line-through' in text
        elif key == 'vertical-align':
            result['superscript'] = text == 'super'
            result['subscript'] = text == 'sub'
        elif key == 'color': result['color'] = text.lower()
        elif key == 'background': result['highlight'] = text.lower()
    return result


class Runs(HTMLParser):
    def __init__(self, html, inherited):
        super().__init__()
        self.stack = [inherited]
        self.characters = []
        self.after_br = False
        self.feed(html)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        style = {**self.stack[-1], **css(attrs.get('style', ''))}
        if tag in ('b', 'strong'): style['bold'] = True
        if tag in ('i', 'em'): style['italic'] = True
        if tag == 'u': style['underline'] = True
        if tag in ('s', 'strike'): style['strike'] = True
        if tag == 'sup': style['superscript'] = True
        if tag == 'sub': style['subscript'] = True
        if tag == 'a':
            style['hyperlink'] = True
            style['link'] = attrs.get('href')
        if tag == 'br':
            self.characters.append(('\n', self.stack[-1]))
            self.after_br = True
        else:
            self.stack.append(style)

    def handle_endtag(self, tag):
        if len(self.stack) > 1:
            self.stack.pop()

    def handle_data(self, data):
        if self.after_br:
            data = data.removeprefix('\n')
        self.after_br = False
        self.characters.extend((char, self.stack[-1]) for char in data)


def native_characters(page, roots):
    definitions = {}
    for node in page.findall('one:QuickStyleDef', ns):
        definitions[node.attrib['index']] = {
            **{k: node.get(k, 'false') == 'true' for k in ('bold', 'italic', 'underline', 'superscript', 'subscript')},
            'strike': node.get('strikethrough', 'false') == 'true',
            **{k: node.attrib[v] for k, v in [('font', 'font'), ('color', 'fontColor'), ('highlight', 'highlightColor')] if v in node.attrib},
        }
        if 'fontSize' in node.attrib:
            definitions[node.attrib['index']]['font_size'] = float(node.attrib['fontSize'])
    result = []
    for root in roots:
        pending = [(root, {})]
        while pending:
            node, inherited = pending.pop()
            style = {**inherited, **definitions.get(node.get('quickStyleIndex'), {}), **css(node.get('style', ''))}
            if node.tag == '{' + ns['one'] + '}T':
                result.append(Runs(node.text or '', style).characters)
            pending.extend((child, style) for child in reversed(node))
    return result


def compare_formats(space, nodes, native):
    count = 0
    differences = {}
    for node_index, (node, expected) in enumerate(zip(nodes, native, strict=True)):
        kind = node['kind']
        base = space['nodes'][kind['paragraph_style']]['format'] if kind['paragraph_style'] else {}
        base = {**{k: v for k, v in base.items() if v is not None}, **{k: v for k, v in node['format'].items() if v is not None}}
        source = kind['text'].encode('utf-16-le')
        actual = []
        for run in kind['runs']:
            style = {**base, **({k: v for k, v in space['nodes'][run['format']]['format'].items() if v is not None} if run['format'] else {})}
            text = source[run['start'] * 2:run['end'] * 2].decode('utf-16-le')
            # Hidden runs and equation runs (exported as MathML) have no visible native text.
            if not style.get('hidden') and not style.get('math'):
                actual.extend((char, style) for char in text)
        if actual and actual[-1][0] == '\r':
            actual.pop()
        actual = [(projected, style) for char, style in actual for projected in project_text(char)]
        if kind['text'] == '\u00a0' and not expected:
            continue
        assert ''.join(c for c, _ in actual) == ''.join(c for c, _ in expected), 'Visible text runs differ'
        for position, ((_, observed), (_, wanted)) in enumerate(zip(actual, expected, strict=True)):
            for key in ('bold', 'italic', 'underline', 'strike', 'superscript', 'subscript', 'font', 'font_size', 'color', 'highlight'):
                if key not in observed:
                    continue
                value = observed[key]
                if key in ('color', 'highlight'):
                    if value == 0xff000000:
                        continue
                    value = '#' + ''.join(f'{(value >> (8 * i)) & 255:02x}' for i in range(3))
                    if observed.get('hyperlink') and key not in wanted:
                        continue
                expected_value = wanted.get(key, False if isinstance(value, bool) else None)
                if key in ("font", "font_size") and expected_value is None:
                    continue
                if key in ("color", "highlight") and expected_value not in (None, "automatic", "none"):
                    expected_value = "#" + "".join(f"{v:02x}" for v in ImageColor.getrgb(expected_value))
                if value != expected_value:
                    differences.setdefault((node_index, key, value, expected_value), []).append(position)
                count += 1
    return count, [{"paragraph": node, "field": key, "stored": actual, "native": expected,
                    "characters": len(positions), "positions": positions}
                   for (node, key, actual, expected), positions in differences.items()]
