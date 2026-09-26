#!/usr/bin/env python3
"""OneNote 2010 conflict pages: how they arise, what they store and how OneNote shows them.

`sync LINUX OUTPUT` runs two disposable OneNote clients on one Samba notebook: client A edits
two paragraphs offline while client B edits one of them and a table cell online; A then
reconnects, which makes OneNote write a conflict page. The server file is kept before and
after, A and B are screenshotted with the information bar, the page list shown, the conflict
page open and its menu open, and A then deletes the conflict page through that menu.

`pages LINUX OUTPUT` runs the same two clients on page-list changes: client A, offline, moves
a page that client B also moves, moves another page, and edits a page B deletes; A then
reconnects. The server's page order and pages, with the recycle bin, are kept at each step.

`restore LINUX OUTPUT` runs them on a page in the middle of the list: client A, offline, edits
it and moves a page B leaves alone while B deletes it; A then reconnects.

`cold NOTEBOOK OUTPUT [--delete]` cold-opens a notebook holding a conflict page in a fresh
clone and screenshots the same views; `--delete` deletes the conflict page through the menu
and captures the notebook OneNote leaves.
"""
import argparse
from contextlib import ExitStack
from concurrent.futures import ThreadPoolExecutor
import base64
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent / 'w7'))
import native_runner as runner
from native_runner import ROOT, clone, command, windows, vm
import linux_vm

FIXTURE = ROOT / 'corpus/native-ink/cold-ui-ink/notebook'
SHARE = 'm6-conflict'
ORIGINAL = 'Fictitious: café, 東京, مرحبا'
OTHER_OUTLINE = 'Fictitious positioned outline.'
CELL = 'Left cell'

# Coordinates on OneNote 2010 maximized at 800x600, from the stage-5, m8 and sync-01 captures:
# the information bar's text, and the first conflict page listed under its page.
BAR = (332, 100)
FIRST_CONFLICT = (725, 142)
# Delete Conflict Page, first in the menu the bar opens on a conflict page.
DELETE = (420, 113)


def shot(name, output, label, script):
    """Runs an AutoHotkey step on the clone, keeping its screenshot as `label.png`."""
    (output / f'{label}.ahk').write_text(script)
    result = windows.do_exec(script, target=name, timeout_ms=120000, shot_delay_ms=1500)
    screenshot = result.pop('png_b64', None)
    if screenshot:
        (output / f'{label}.png').write_bytes(base64.b64decode(screenshot))
    (output / f'{label}.json').write_text(json.dumps(result, indent=2))
    if result.get('error') or result.get('exit') != 0:
        raise RuntimeError(f'{label}: {result}')
    return result


def click(x, y):
    return ('WinActivate("ahk_exe ONENOTE.EXE")\nCoordMode("Mouse", "Screen")\n'
            f'Click({x}, {y})\nSleep(1200)\n')


# Reports any dialog OneNote raised, so a confirmation shows in the step's record.
DIALOG = ('if WinExist("ahk_class #32770 ahk_exe ONENOTE.EXE") {\n'
          '    FileAppend("dialog: " WinGetTitle() " | " WinGetText() "`n", "*", "UTF-8")\n'
          '}\n')


def conflict_views(name, output, page, prefix, delete=False):
    """Screenshots a page with a conflict page: the bar, the list shown, the conflict page and
    its menu; `delete` then chooses Delete Conflict Page."""
    shot(name, output, f'{prefix}-main', runner.navigation_script(page) + 'Sleep(2000)\n')
    shot(name, output, f'{prefix}-shown', click(*BAR))
    shot(name, output, f'{prefix}-conflict', click(*FIRST_CONFLICT))
    shot(name, output, f'{prefix}-menu', click(*BAR))
    if delete:
        shot(name, output, f'{prefix}-delete', click(*DELETE) + DIALOG)
    else:
        shot(name, output, f'{prefix}-closed', 'WinActivate("ahk_exe ONENOTE.EXE")\nSend("{Esc}")\nSleep(800)\n')


def cold(notebook, output, delete):
    def interaction(name, output):
        page = ET.parse(sorted((output / 'read').glob('page-*.xml'))[0]).getroot().attrib['ID']
        conflict_views(name, output, page, 'cold', delete)
        if delete:
            # OneNote writes the deletion in the background; give it time before the re-read.
            time.sleep(20)
    runner.capture(notebook, output, collect_notebook=True, interaction=interaction)


class Lab:
    """Two OneNote clients on one Samba notebook: `action` runs a controller command,
    `state` copies the share and exports its section, `settle` syncs until a check holds."""


def page_list(model):
    """The section's pages in order, each as its sorted texts."""
    from document_model import ordered_pages, walk
    return [sorted(node['kind']['text'] for _, node in walk(page, oid) if node['kind']['type'] == 'RichText')
            for _, _, page, oid in ordered_pages(model)]


def edits(lab):
    from native_collaboration import reachable_page_text
    a, b, output, action, settle = lab.a, lab.b, lab.output, lab.action, lab.settle
    state = lambda label: reachable_page_text(lab.state(label))
    texts = lambda state: {text for values in state.values() for text in values}
    state('initial')
    vm.qmp(a['name'], 'set_link', {'name': 'lab', 'up': False})
    try:
        action(a, 'edit', expected=ORIGINAL, text='Client A conflicting edit.')
        action(a, 'edit', expected=OTHER_OUTLINE, text='Client A disjoint edit.')
        action(a, 'snapshot')
        action(b, 'edit', expected=ORIGINAL, text='Client B conflicting edit.')
        action(b, 'edit', expected=CELL, text='Client B disjoint cell.')
        settle(b, lambda label: {'Client B conflicting edit.', 'Client B disjoint cell.'} <= texts(state(label)), 'b-published')
    finally:
        vm.qmp(a['name'], 'set_link', {'name': 'lab', 'up': True})
    settle(a, lambda label: len(found := state(label)) > 1 and 'Client A disjoint edit.' in texts(found), 'conflict')
    (output / 'conflict.json').write_text(json.dumps(state('conflict'), indent=2, ensure_ascii=False))
    page = action(a, 'snapshot')[0].attrib['ID']
    conflict_views(a['name'], output / 'a', page, 'a')
    action(b, 'sync')
    time.sleep(5)
    page_b = action(b, 'snapshot')[0].attrib['ID']
    conflict_views(b['name'], output / 'b', page_b, 'b')
    conflict_views(a['name'], output / 'a', page, 'a-again', delete=True)
    settle(a, lambda label: len(state(label)) == 1, 'deleted')
    action(b, 'sync')
    time.sleep(5)
    shot(b['name'], output / 'b', 'b-after-delete', runner.navigation_script(page_b) + 'Sleep(2000)\n')


PAGES = ['One', 'Two', 'Three', 'Four', 'Target']


def titled(model, titles):
    """The section's pages in order, each named by the title among `titles` it holds."""
    return [next((text for text in texts if text in titles), '') for texts in page_list(model)]


def offline(lab, titles, a_actions, b_actions):
    """Creates the titled pages; A runs its actions offline while B runs and publishes its
    own; A then reconnects. Keeps the merged order and pages, and what each client lists."""
    a, b, output, action = lab.a, lab.b, lab.output, lab.action
    action(a, 'pages', titles=titles)
    lab.settle(a, lambda label: 'Body Target.' in str(page_list(lab.state(label))), 'initial')
    deadline = time.monotonic() + 300
    while not any(node.get('name') == 'Target' for node in action(b, 'snapshot')):
        action(b, 'sync')
        if time.monotonic() > deadline: raise TimeoutError('B never listed the new pages')
    vm.qmp(a['name'], 'set_link', {'name': 'lab', 'up': False})
    try:
        for kind, parameters in a_actions:
            action(a, kind, **parameters)
        action(a, 'snapshot')
        for kind, parameters in b_actions:
            action(b, kind, **parameters)
        action(b, 'snapshot')
        lab.settle(b, lambda label: 'Body Target.' not in str(page_list(lab.state(label))), 'b-published')
    finally:
        vm.qmp(a['name'], 'set_link', {'name': 'lab', 'up': True})
    # A's changes reach the server when its sync writes; wait until the file stops changing.
    previous, steady = None, 0
    deadline = time.monotonic() + 300
    while steady < 3 and time.monotonic() < deadline:
        action(a, 'sync')
        time.sleep(10)
        current = page_list(lab.state('merged'))
        steady = steady + 1 if current == previous else 0
        previous = current
    (output / 'merged.json').write_text(json.dumps({'order': titled(lab.state('merged'), titles), 'pages': previous}, indent=2, ensure_ascii=False))
    for client, label in ((a, 'a'), (b, 'b')):
        action(client, 'sync')
        time.sleep(5)
        listed = action(client, 'snapshot')
        (output / label / 'listed.json').write_text(json.dumps([node.get('name') for node in listed], ensure_ascii=False))
        if listed:
            shot(client['name'], output / label, f'{label}-merged', runner.navigation_script(listed[0].get('ID')) + 'Sleep(2000)\n')


def pages(lab):
    """Competing page moves, and an offline edit of a page the other client deletes."""
    offline(lab, PAGES, [
        ('move', {'page': 'Four', 'before': 'One'}),
        ('move', {'page': 'Two', 'before': None}),
        ('edit', {'expected': 'Body Target.', 'text': 'Body Target edited offline.'}),
    ], [
        ('move', {'page': 'Four', 'before': None}),
        ('move', {'page': 'Three', 'before': 'One'}),
        ('delete-page', {'page': 'Target'}),
    ])


def restore(lab):
    """An offline edit of a page in the middle of the list the other client deletes, with an
    offline move of a page the other client leaves alone."""
    offline(lab, ['One', 'Two', 'Target', 'Three', 'Four'], [
        ('edit', {'expected': 'Body Target.', 'text': 'Body Target edited offline.'}),
        ('move', {'page': 'Four', 'before': 'One'}),
    ], [
        ('delete-page', {'page': 'Target'}),
    ])


def sync(server, output, scenario):
    from document_model import EXPORTER

    if linux_vm.instance_path(server).exists():
        raise ValueError('Choose a new Linux VM name; existing machines are not owned by this run.')
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    scripts = output / 'scripts'
    scripts.mkdir()
    for name in ('cold.ps1', 'collaborate.ps1', 'network.ps1', 'text.ps1', 'stress.ps1'):
        shutil.copyfile(ROOT / 'tools/native' / name, scripts / name)
    shutil.copyfile(Path(__file__), output / 'native_conflict.py')

    def ssh(text, timeout=90):
        result = linux_vm.run_ssh(server, text, timeout=timeout)
        with (output / 'server.jsonl').open('a') as log:
            log.write(json.dumps({'command': text, 'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}) + '\n')
        result.check_returncode()
        return result.stdout

    def server_state(label):
        """Copies the server's notebook, recycle bin included, until its section parses,
        keeping it as `label/notebook`; returns the section's exported model."""
        deadline = time.monotonic() + 120
        while True:
            target = output / label
            shutil.rmtree(target, ignore_errors=True)
            (target / 'notebook').mkdir(parents=True)
            archive = subprocess.run(linux_vm.ssh_argv(server, f"tar cf - -C '/srv/agent/{SHARE}' ."), capture_output=True, check=True, timeout=60).stdout
            subprocess.run(['tar', 'xf', '-', '-C', target / 'notebook'], input=archive, check=True)
            result = subprocess.run([EXPORTER, target / 'notebook/synthetic.one', target / 'model'], capture_output=True, text=True)
            if result.returncode == 0:
                return json.loads((target / 'model/document.json').read_text())
            if time.monotonic() > deadline:
                raise RuntimeError(f'{label}: {result.stderr}')
            time.sleep(1)

    linux_vm.create_instance(server)
    try:
        linux_vm.launch(server)
        linux_vm.wait_instance(server, 600)
        (output / 'linux.json').write_text(json.dumps(linux_vm.load_instance(server), indent=2))
        ssh(f'mkdir /srv/agent/{SHARE}')
        for name in ('synthetic.one', 'Open Notebook.onetoc2'):
            with (FIXTURE / name).open('rb') as stream:
                subprocess.run(linux_vm.ssh_argv(server, f"cat > '/srv/agent/{SHARE}/{name}'"), stdin=stream, check=True)
        ssh(f'chmod u+w /srv/agent/{SHARE}/*')
        with ExitStack() as stack:
            labels = ['a', 'b']
            for label in labels:
                (output / label).mkdir()

            def start(label):
                manager = clone(output / label)
                name = manager.__enter__()
                return label, name, manager
            with ThreadPoolExecutor(max_workers=2) as pool:
                started = list(pool.map(start, labels))
            for _, _, manager in started:
                stack.push(manager)
            clients = {}
            for label, name, _ in started:
                folder = output / label
                for local in scripts.iterdir():
                    remote = 'cold-current.ps1' if local.name == 'cold.ps1' else local.name
                    result = windows.do_put(local, 'C:\\one-tests\\' + remote, name)
                    if result.get('error'): raise RuntimeError(result['error'])
                command(name, 'mkdir C:\\one-tests\\runs\\capture', folder)
                command(name, 'powershell -NoProfile -ExecutionPolicy Bypass -File C:\\one-tests\\network.ps1 -LabMac ' + vm.lab_mac(name), folder)
                result = windows.do_spawn('cmd /c powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\one-tests\\collaborate.ps1 -Root C:\\one-tests\\runs\\capture -SharedPath \\\\192.168.77.1\\agent\\' + SHARE + ' -CloneHost ONE-' + name.upper() + ' > C:\\one-tests\\runs\\capture\\controller.log 2>&1', name)
                if result.get('error'): raise RuntimeError(result['error'])
                deadline = time.monotonic() + 180
                while windows.do_cmd('if exist C:\\one-tests\\runs\\capture\\ready (echo ready)', target=name).get('stdout', '').find('ready') < 0:
                    if time.monotonic() > deadline: raise RuntimeError('The collaboration controller did not open the notebook')
                    time.sleep(1)
                clients[label] = {'name': name, 'folder': folder, 'sequence': 0}
            (output / 'clients.json').write_text(json.dumps({label: client['name'] for label, client in clients.items()}, indent=2))

            def action(client, kind, **parameters):
                client['sequence'] += 1
                sequence = client['sequence']
                local = client['folder'] / f'command-{sequence:04}.json'
                local.write_text(json.dumps({'action': kind, **parameters}, ensure_ascii=False))
                inbox = f'C:\\one-tests\\runs\\capture\\inbox\\{sequence:04}'
                result = windows.do_put(local, inbox + '.tmp', client['name'])
                if result.get('error'): raise RuntimeError(result['error'])
                command(client['name'], f'move {inbox}.tmp {inbox}.json', client['folder'])
                remote = f'C:\\one-tests\\runs\\capture\\outbox\\{sequence}'
                deadline = time.monotonic() + 600
                while True:
                    result = windows.do_cmd(f'if exist {remote}\\error (type {remote}\\error & exit /b 1) else (if exist {remote}\\done (echo complete))', target=client['name'])
                    if result.get('exit') != 0: raise RuntimeError(str(result))
                    if 'complete' in result.get('stdout', ''): break
                    if time.monotonic() > deadline: raise TimeoutError(f'{kind} did not complete')
                    time.sleep(.5)
                if kind != 'snapshot':
                    return None
                hierarchy = client['folder'] / f'hierarchy-{sequence:04}.xml'
                windows.do_get(remote + '\\hierarchy.xml', hierarchy, client['name'])
                pages = [node for node in ET.fromstring(hierarchy.read_text(encoding='utf-8-sig').strip()).iter() if node.tag.endswith('}Page')]
                for i in range(len(pages)):
                    windows.do_get(remote + f'\\page-{i}.xml', client['folder'] / f'snapshot-{sequence:04}-{i}.xml', client['name'])
                return pages

            def settle(client, check, label):
                deadline = time.monotonic() + 300
                while True:
                    action(client, 'sync')
                    if check(label): return
                    if time.monotonic() > deadline: raise TimeoutError(label)
                    time.sleep(2)

            lab = Lab()
            lab.a, lab.b, lab.output = clients['a'], clients['b'], output
            lab.action, lab.state, lab.settle = action, server_state, settle
            scenario(lab)

            for client in clients.values():
                try:
                    action(client, 'close')
                except Exception as error:
                    (client['folder'] / 'close-failure.txt').write_text(str(error))
            server_state('final')
    finally:
        if linux_vm.running(server):
            linux_vm.shutdown(server, 120)
        linux_vm.delete_instance(server)
        (output / 'linux-teardown.json').write_text(json.dumps({'absent': not linux_vm.instance_path(server).exists()}) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='mode', required=True)
    scenarios = {'sync': edits, 'pages': pages, 'restore': restore}
    for mode in scenarios:
        lab = commands.add_parser(mode)
        lab.add_argument('linux')
        lab.add_argument('output', type=Path)
    reopen = commands.add_parser('cold')
    reopen.add_argument('notebook', type=Path)
    reopen.add_argument('output', type=Path)
    reopen.add_argument('--delete', action='store_true')
    args = parser.parse_args()
    if args.mode in scenarios:
        sync(args.linux, args.output, scenarios[args.mode])
    else:
        cold(args.notebook, args.output, args.delete)
    print('Captured', args.output)
