from html.parser import HTMLParser
import xml.etree.ElementTree as ET

ns = {'one': 'http://schemas.microsoft.com/office/onenote/2010/onenote'}


class Text(HTMLParser):
    def __init__(self, html):
        super().__init__()
        self.parts = []
        self.links = []
        self.after_br = False
        self.feed(html)

    def handle_data(self, data):
        if self.after_br:
            data = data.removeprefix('\n')
        self.after_br = False
        self.parts.append(data)

    def handle_starttag(self, tag, attrs):
        if tag == 'br':
            self.parts.append('\n')
            self.after_br = True
        if tag == 'a':
            self.links.extend(value for key, value in attrs if key == 'href')


def texts(page):
    return [''.join(Text(node.text or '').parts) for node in page.findall('.//one:T', ns)]


def pages(directory):
    return [ET.parse(p).getroot() for p in sorted(directory.glob('page-*.xml'))]


def project_text(text):
    """Native HTML expands tabs to eight NBSPs and emits stored CR as a line break."""
    return text.replace('\t', '\u00a0' * 8).replace('\r', '\n')
