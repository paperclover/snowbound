#!/usr/bin/env python3
"""OneNote 2010 page versions (Share, Page Versions): how they arise, what they store and
how OneNote shows, restores, copies and deletes them.

`observe OUTPUT` runs one disposable OneNote client on a new notebook: `virtual` writes a
page, `Other Person` edits it (OneNote keeps the page as it was as a version), then OneNote's
Copy Page To, Restore Version, Delete Version and Delete All Versions in Section run on it;
two more authors' edits, a Delete Version of the only version and the author views (Hide
Authors, Recent Edits, Find by Author) follow. The section after each step is
`step-NN/notebook`, the screens `shots/`.

`cold NOTEBOOK OUTPUT [--delete]` cold-opens a notebook whose page has versions in a fresh
clone, shows them and opens the newest, screenshots each view, and with `--delete` deletes
that version through its bar's menu and keeps the notebook OneNote leaves.
"""
import argparse
import base64
import json
from pathlib import Path
import sys
import time
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent / 'w7'))
import native_runner as runner
from native_runner import windows

ROOT = Path(r'C:\one-tests\versions')
SECTION = ROOT / 'Versions' / 'History.one'

# Coordinates on OneNote 2010 maximized at 800x600 with its ribbon collapsed, from the
# capture in corpus/page-versions/native: the Share tab, its Page Versions button and the
# arrow under it, the information bar, the first body line, the first version listed under
# the page and the page itself in the list.
SHARE = (202, 36)
PAGE_VERSIONS = (443, 80)
PAGE_VERSIONS_MENU = (462, 108)
DELETE_ALL_IN_SECTION = (525, 155)
BAR = (320, 93)
BODY = (160, 208)
FIRST_VERSION = (710, 143)
THE_PAGE = (700, 121)
HIDE_AUTHORS = (383, 88)
RECENT_EDITS = (296, 108)
FIND_BY_AUTHOR = (338, 95)
# Items of the version bar's menu.
RESTORE, DELETE, COPY = (396, 106), (393, 128), (395, 150)

# Reports any dialog OneNote raised, so a confirmation shows in the step's record.
DIALOG = ('if WinExist("ahk_class #32770 ahk_exe ONENOTE.EXE") {\n'
          '    FileAppend("dialog: " WinGetTitle() " | " WinGetText() "`n", "*", "UTF-8")\n'
          '}\n')
ACTIVATE = 'WinActivate("ahk_exe ONENOTE.EXE")\nCoordMode("Mouse", "Screen")\n'


def click(point, wait=1000):
    return f'Click({point[0]}, {point[1]})\nSleep({wait})\n'


class Clone:
    """A clone's desktop: `shot` runs an AutoHotkey step and keeps its screenshot."""

    def __init__(self, name, output):
        self.name, self.output = name, output
        (output / 'shots').mkdir(parents=True, exist_ok=True)

    def shot(self, label, script, delay=1500):
        (self.output / 'shots' / f'{label}.ahk').write_text(script)
        result = windows.do_exec(script, target=self.name, timeout_ms=180000, shot_delay_ms=delay)
        screenshot = result.pop('png_b64', None)
        if screenshot:
            (self.output / 'shots' / f'{label}.png').write_bytes(base64.b64decode(screenshot))
        (self.output / 'shots' / f'{label}.json').write_text(json.dumps(result, indent=2))
        if result.get('error') or result.get('exit') != 0:
            raise RuntimeError(f'{label}: {result}')
        return result

    def command(self, text, timeout_ms=120000):
        runner.command(self.name, text, self.output, timeout_ms)

    def keep(self, step, files=('History.one',)):
        """Keeps the section once OneNote has stopped writing it: its size and time unchanged
        for five seconds."""
        self.command('powershell -NoProfile -Command "$f = \'%s\'; $last = \'\'; $still = 0; '
                     'for ($i = 0; $i -lt 120 -and $still -lt 10; $i++) { Start-Sleep -Milliseconds 500; '
                     '$item = Get-Item $f; $now = \'\' + $item.Length + $item.LastWriteTimeUtc.Ticks; '
                     'if ($now -eq $last) { $still++ } else { $still = 0; $last = $now } }"' % SECTION)
        directory = self.output / f'step-{step:02}' / 'notebook'
        directory.mkdir(parents=True)
        for name in files:
            result = windows.do_get(str(SECTION.parent / name), directory / name, self.name)
            if result.get('error'):
                raise RuntimeError(result['error'])


def open_page(author):
    """Closes OneNote, names the Office user `author` and opens the page."""
    name, initials = author
    return ('if WinExist("ahk_exe ONENOTE.EXE") {\n    WinClose("ahk_exe ONENOTE.EXE")\n    Sleep(4000)\n}\n'
            'RunWait("cmd /c taskkill /IM ONENOTE.EXE /F", , "Hide")\nSleep(1000)\n'
            f'RegWrite("{name}", "REG_SZ", "HKCU\\Software\\Microsoft\\Office\\Common\\UserInfo", "UserName")\n'
            f'RegWrite("{initials}", "REG_SZ", "HKCU\\Software\\Microsoft\\Office\\Common\\UserInfo", "UserInitials")\n'
            f'page := Trim(FileRead("{ROOT}\\page.txt", "UTF-8"), "`r`n ")\n'
            'app := ComObject("OneNote.Application")\napp.NavigateTo(page)\n'
            'WinWait("ahk_exe ONENOTE.EXE", , 20)\nWinMaximize("ahk_exe ONENOTE.EXE")\n'
            + ACTIVATE + 'Sleep(3000)\n')


def type_line(text, new_line=True):
    return (ACTIVATE + click(BODY, 300) + ('Send("^{End}{Enter}")\n' if new_line else 'Send("{End}")\n')
            + f'SendText("{text}")\nSleep(3000)\n')


SETUP = r'''$ErrorActionPreference = 'Stop'
$app = New-Object -ComObject OneNote.Application
$notebook = ''
$app.OpenHierarchy('%(root)s\Versions', '', [ref]$notebook, 1)
$section = ''
$app.OpenHierarchy('History.one', $notebook, [ref]$section, 3)
$page = ''
$app.CreateNewPage($section, [ref]$page, 0)
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app.UpdatePageContent('<one:Page xmlns:one="' + $namespace + '" ID="' + $page + '"><one:Title><one:OE><one:T>Versioned</one:T></one:OE></one:Title><one:Outline><one:Position x="36" y="86"/><one:OEChildren><one:OE><one:T>First state.</one:T></one:OE></one:OEChildren></one:Outline></one:Page>')
$page | Out-File '%(root)s\page.txt' -Encoding UTF8
'''

VIRTUAL, OTHER = ('virtual', 'v'), ('Other Person', 'OP')


def observe(output):
    output.mkdir(parents=True, exist_ok=False)
    with runner.clone(output) as name:
        clone = Clone(name, output)
        setup = output / 'setup.ps1'
        setup.write_text(SETUP % {'root': ROOT})
        clone.command(f'mkdir {ROOT}')
        result = windows.do_put(setup, str(ROOT / 'setup.ps1'), name)
        if result.get('error'):
            raise RuntimeError(result['error'])
        clone.command(f'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File {ROOT}\\setup.ps1')
        # One author: no version.
        clone.shot('01-typed', open_page(VIRTUAL) + type_line(' Typed by virtual.', new_line=False))
        clone.keep(1, ('History.one', 'Open Notebook.onetoc2'))
        # A second author's edit keeps the page as it was as a version.
        clone.shot('02-second-author', open_page(OTHER) + type_line('Second author line.'))
        clone.keep(2)
        clone.shot('02-menu', ACTIVATE + click(SHARE, 600) + click(PAGE_VERSIONS_MENU, 800))
        clone.shot('02-shown', ACTIVATE + click((486, 129), 1500))
        clone.shot('02-version', ACTIVATE + click(FIRST_VERSION, 1500))
        clone.shot('02-version-menu', ACTIVATE + click(BAR, 1000))
        # Copy Page To copies the version as a page at the section's end.
        clone.shot('03-copy-dialog', ACTIVATE + click(COPY, 1500))
        clone.shot('03-copied', 'WinActivate("Move or Copy Pages")\n' + click((289, 200), 500)
                   + click((474, 494), 3000), 2000)
        clone.keep(3)
        # Restore Version: the page as it stood becomes the newest version.
        clone.shot('04-shown', ACTIVATE + click(THE_PAGE, 1000) + click(SHARE, 600) + click(PAGE_VERSIONS, 1200))
        clone.shot('04-restored', ACTIVATE + click(FIRST_VERSION, 1200) + click(BAR, 800)
                   + click(RESTORE, 2500) + DIALOG, 2000)
        clone.keep(4)
        # Delete Version, without a confirmation.
        clone.shot('05-newest', ACTIVATE + click(FIRST_VERSION, 1500))
        clone.shot('05-deleted', ACTIVATE + click(BAR, 800) + click(DELETE, 2500) + DIALOG, 2000)
        clone.keep(5)
        # Delete All Versions in Section asks first.
        clone.shot('06-confirm', ACTIVATE + click(SHARE, 600) + click(PAGE_VERSIONS_MENU, 800)
                   + click(DELETE_ALL_IN_SECTION, 1500) + DIALOG)
        clone.shot('06-deleted-all', 'WinActivate("Microsoft OneNote ahk_class #32770")\nSend("!y")\nSleep(2500)\n', 2500)
        clone.keep(6)
        # Deleting the only version clears the page's HasVersionPages.
        clone.shot('07-third-author-edit', open_page(VIRTUAL) + type_line('Third line by virtual.')
                   + click(SHARE, 600) + click(PAGE_VERSIONS, 1200))
        clone.keep(7)
        clone.shot('08-deleted-only', ACTIVATE + click(FIRST_VERSION, 1500) + click(BAR, 800)
                   + click(DELETE, 2500) + DIALOG, 2000)
        clone.keep(8)
        # Authors: Hide Authors off, Recent Edits, Find by Author.
        clone.shot('09-authors', open_page(OTHER) + type_line('Fourth line by Other Person.')
                   + click(SHARE, 600) + click(HIDE_AUTHORS, 1500))
        clone.shot('09-recent-edits', ACTIVATE + click(SHARE, 600) + click(RECENT_EDITS, 1200), 1000)
        clone.shot('09-find-by-author', ACTIVATE + 'Send("{Esc}")\nSleep(400)\n' + click(SHARE, 900)
                   + click(FIND_BY_AUTHOR, 3000), 2500)
        clone.keep(9)


def cold(notebook, output, delete):
    """Opens the page with versions, shows them, opens the newest and its menu; `delete`
    then deletes it."""
    def interaction(name, output):
        clone = Clone(name, output)
        page = ET.parse(sorted((output / 'read').glob('page-*.xml'))[0]).getroot().attrib['ID']
        clone.shot('cold-main', runner.navigation_script(page) + 'Sleep(2000)\n')
        clone.shot('cold-shown', ACTIVATE + click(SHARE, 600) + click(PAGE_VERSIONS, 1500))
        clone.shot('cold-version', ACTIVATE + click(FIRST_VERSION, 1500))
        clone.shot('cold-menu', ACTIVATE + click(BAR, 1000))
        if delete:
            clone.shot('cold-delete', ACTIVATE + click(DELETE, 2500) + DIALOG, 2000)
            # OneNote writes the deletion in the background; give it time before the re-read.
            time.sleep(20)
        else:
            clone.shot('cold-closed', ACTIVATE + 'Send("{Esc}")\nSleep(800)\n')
    runner.capture(notebook, output, collect_notebook=True, interaction=interaction)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    observed = commands.add_parser('observe')
    observed.add_argument('output', type=Path)
    opened = commands.add_parser('cold')
    opened.add_argument('notebook', type=Path)
    opened.add_argument('output', type=Path)
    opened.add_argument('--delete', action='store_true')
    args = parser.parse_args()
    if args.command == 'observe':
        observe(args.output)
    else:
        cold(args.notebook, args.output, args.delete)
