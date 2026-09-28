"""Type hyperlinks into OneNote 2010 on a Rust-written page and capture what it stores: URLs
it links as they are typed, links made with the Link dialog (Ctrl+K), typing after a link and
Remove Link. The oracle for the canvas editor's link editing.

Usage: native_links.py NOTEBOOK_DIR OUTPUT_DIR
"""
import base64
import json
import sys
from pathlib import Path
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_runner as runner

windows = runner.windows

# Each typed URL sits between a word and a letter, one per paragraph; a space or Enter ends it.
TYPED = [
    'www.example.com',
    'http://example.org/a?b=1',
    'https://a.example/p.',
    '(http://b.example/q)',
    'ftp://c.example/f',
    'mailto:me@example.com',
    'me@example.com',
    'example.com',
    'onenote:#Page&section-id={x}',
    'file://c:/windows',
    '\\\\server\\share\\f',
    'news:comp.lang',
    'www.d.example,',
    'HTTP://E.EXAMPLE/UP',
    'https://f.example/a_b-c~d%20e#frag',
]


def text(value):
    return 'SendText("%s")' % value.replace('"', '""')


def interaction(name, output):
    lines = []
    for url in TYPED:
        lines += [text('t ' + url), 'Send(" ")', text('z'), 'Send("{Enter}")', 'Sleep(300)']
    # A URL ended by Enter, then a letter typed straight after the link it made.
    lines += [text('see http://g.example/end'), 'Send("{Enter}")', 'Send("{Up}{End}")', text('Z'),
              'Send("{Down}{End}")', 'Sleep(300)']
    # The Link dialog on the word at the caret: its text is the word, the address as typed.
    lines += [text('plain words here'), 'Send("^{Left}")', 'Send("^k")', 'Sleep(1500)',
              text('example.net/x'), 'Send("{Enter}")', 'Sleep(800)', 'Send("{End}")', text('Q'),
              'Send("{Enter}")', 'Sleep(300)']
    # The Link dialog on a selection.
    lines += [text('select me please'), 'Send("+^{Left}+^{Left}")', 'Send("^k")', 'Sleep(1500)',
              text('https://example.com/selected'), 'Send("{Enter}")', 'Sleep(800)',
              'Send("{End}{Enter}")', 'Sleep(300)']
    # The Link dialog with no text: the address becomes the text; typing after it is plain.
    lines += ['Send("^k")', 'Sleep(1500)', 'Send("!e")', 'Sleep(300)',
              text('https://example.com/only'), 'Send("{Enter}")', 'Sleep(800)', text(' tail'),
              'Send("{Enter}")', 'Sleep(300)']
    # Remove Link from the context menu (Shift+F10, R) with the caret inside a typed link.
    lines += [text('gone ftp://h.example/f'), 'Send(" ")', text('z'), 'Send("{Home}^{Right}^{Right}")',
              'Send("+{F10}")', 'Sleep(900)', 'Send("r")', 'Sleep(800)']
    page = ET.parse(sorted((output / 'read').glob('page-*.xml'))[0]).getroot().attrib['ID']
    script = '''ComObject("OneNote.Application").NavigateTo("PAGE_ID", "", false)
WinWait("ahk_exe ONENOTE.EXE", , 30)
WinActivate("ahk_exe ONENOTE.EXE")
WinWaitActive("ahk_exe ONENOTE.EXE", , 30)
WinMaximize("ahk_exe ONENOTE.EXE")
Sleep(3000)
CoordMode("Mouse", "Screen")
Click(200, 140)
Sleep(500)
Send("{End}")
Sleep(300)
Send("{Enter}")
Sleep(500)
%s
Sleep(2000)
''' % '\n'.join(lines)
    script = script.replace('PAGE_ID', page)
    (output / 'links.ahk').write_text(script)
    result = windows.do_exec(script, target=name, timeout_ms=240000, shot_delay_ms=1500)
    screenshot = result.pop('png_b64', None)
    if screenshot:
        (output / 'links.png').write_bytes(base64.b64decode(screenshot))
    (output / 'links.json').write_text(json.dumps(result, indent=2))
    if result.get('error') or result.get('exit') != 0:
        raise RuntimeError(str(result))


if __name__ == '__main__':
    notebook, output = Path(sys.argv[1]), Path(sys.argv[2])
    runner.capture(notebook, output, expected_pages=1, collect_notebook=True, interaction=interaction)
