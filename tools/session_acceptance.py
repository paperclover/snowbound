#!/usr/bin/env python3
"""Edit a notebook on the owned Samba share through the application's session, relaunch,
then cold-open the result in a disposable OneNote clone and compare its text."""
import argparse
import html
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools/w7'))
sys.path.insert(0, str(ROOT / 'tools'))
import linux_vm  # noqa: E402

FIXTURE = ROOT / 'corpus/outline-edit/before/notebook'
NS = {'one': 'http://schemas.microsoft.com/office/onenote/2010/onenote'}


def native_texts(read):
    pages = {}
    for path in sorted(read.glob('page-*.xml')):
        root = ET.parse(path).getroot()
        texts = []
        for element in root.iter('{%s}T' % NS['one']):
            texts.append(html.unescape(re.sub(r'<[^>]*>', '', element.text or '')))
        pages[root.get('name')] = texts
    return pages


def compare(expected, native):
    """The client's edits carry its launch label; other text is compared by the corpus lanes."""
    mismatches = []
    for title, texts in expected.items():
        body = [text for text in native[title] if text != title]
        wanted = [text for text in texts if 'launch' in text]
        assert wanted, title
        for text in wanted:
            if text not in body:
                mismatches.append({'page': title, 'missing': text, 'native': body})
    return mismatches


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--linux', help='Disposable Linux VM owned by this run')
    parser.add_argument('--expected-pages', type=int, default=15)
    parser.add_argument('--verify-only', action='store_true',
                        help='Compare an existing capture in OUTPUT without the lab.')
    args = parser.parse_args()
    output = args.output.resolve()
    if not args.verify_only and not args.linux:
        parser.error('--linux names the disposable Linux VM this run owns')
    if args.verify_only:
        expected = {}
        for launch in ('first', 'second'):
            for line in (output / f'{launch}.log').read_text().splitlines():
                if line.startswith('{') and '"published"' in line:
                    event = json.loads(line)
                    expected[event['title']] = event['texts']
        native = native_texts(output / 'result/cold/read')
        mismatches = compare(expected, native)
        (output / 'verification.json').write_text(json.dumps(
            {'expected': expected, 'native': {title: native[title] for title in expected}, 'mismatches': mismatches},
            indent=2, ensure_ascii=False))
        assert not mismatches, mismatches
        print(f'Session acceptance verified: {len(expected)} pages', flush=True)
        return
    output.mkdir(parents=True)
    server = args.linux
    mount = output / 'mount'
    mount.mkdir()
    client = ROOT / 'target/debug/examples/session_client'
    subprocess.run(['cargo', 'build', '-p', 'notebook', '--example', 'session_client'], cwd=ROOT, check=True)
    record = {'server': server, 'fixture': str(FIXTURE.relative_to(ROOT)), 'launches': []}

    def unmount():
        for options in ([], ['-f']):
            if not os.path.ismount(mount):
                return
            subprocess.run(['/sbin/umount', *options, str(mount)], capture_output=True, text=True, timeout=60)
        if os.path.ismount(mount):
            raise RuntimeError('The owned SMB mount remains attached')

    try:
        if not linux_vm.instance_path(server).exists():
            linux_vm.create_instance(server)
            linux_vm.launch(server)
        linux_vm.wait_instance(server, 600)
        config = linux_vm.load_instance(server)
        (output / 'linux.json').write_text(json.dumps(config, indent=2))
        linux_vm.run_ssh(server, 'mkdir /srv/agent/m4-session').check_returncode()
        with tarfile.open(output / 'input.tar', 'w', dereference=True) as archive:
            for name in ('synthetic.one', 'Open Notebook.onetoc2'):
                archive.add(FIXTURE / name, arcname=name)
        with (output / 'input.tar').open('rb') as stream:
            subprocess.run(linux_vm.ssh_argv(server, 'tar xf - -C /srv/agent/m4-session'), stdin=stream, check=True)
        linux_vm.run_ssh(server, 'chmod u+w /srv/agent/m4-session/*').check_returncode()
        subprocess.run(['/sbin/mount_smbfs', '-N', f'//guest@127.0.0.1:{config["samba_port"]}/agent', mount],
                       check=True, stdin=subprocess.DEVNULL)
        section = mount / 'm4-session/synthetic.one'
        assert section.read_bytes() == (FIXTURE / 'synthetic.one').read_bytes()
        cache = output / 'cache'
        expected = {}
        for launch in ('First launch', 'Second launch'):
            result = subprocess.run([client, section, cache, launch], capture_output=True, text=True, timeout=300)
            (output / f'{launch.split()[0].lower()}.log').write_text(result.stdout + result.stderr)
            result.check_returncode()
            events = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
            assert events[0]['event'] == 'opened' and events[0]['pending'] == 0, events[0]
            published = [e for e in events if e['event'] == 'published']
            assert published and all(e['revision'] for e in published), published
            for e in published:
                assert e['stored'] == e['texts'], e
                expected[e['title']] = e['texts']
            record['launches'].append({'label': launch, 'published': published})
        unmount()
        result = output / 'result'
        (result / 'notebook').mkdir(parents=True)
        with (output / 'result.tar').open('wb') as stream:
            subprocess.run(linux_vm.ssh_argv(server, 'tar cf - -C /srv/agent/m4-session .'), stdout=stream, check=True)
        with tarfile.open(output / 'result.tar') as archive:
            archive.extractall(result / 'notebook', filter='data')
        subprocess.run([sys.executable, ROOT / 'tools/native_runner.py', result / 'notebook', result / 'cold',
                        '--expected-pages', str(args.expected_pages), '--collect-notebook'], check=True)
        native = native_texts(result / 'cold/read')
        mismatches = compare(expected, native)
        record['native'] = {title: native[title] for title in expected}
        record['mismatches'] = mismatches
        (output / 'record.json').write_text(json.dumps(record, indent=2, ensure_ascii=False))
        assert not mismatches, mismatches
        print(f'Session acceptance passed: {len(expected)} pages edited twice, reopened natively', flush=True)
    finally:
        unmount()
        if linux_vm.instance_path(server).exists():
            if linux_vm.running(server):
                linux_vm.shutdown(server, 60)
            linux_vm.delete_instance(server)
        shutil.rmtree(mount, ignore_errors=True)


if __name__ == '__main__':
    main()
