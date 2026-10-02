import base64
from datetime import datetime, timezone
import hashlib
from pathlib import Path
import runpy
import tempfile
import unittest

release = runpy.run_path(str(Path(__file__).resolve().parent / 'release.py'))


def utc(text):
    return datetime.fromisoformat(text).replace(tzinfo=timezone.utc)


class ReleaseTest(unittest.TestCase):
    def test_a_build_counts_the_commits_of_its_day_in_los_angeles(self):
        # 07:34 UTC on the 29th is 00:34 in Los Angeles; 05:00 UTC on the 30th is 22:00 on the 29th.
        commits = [utc('2026-09-30T05:00:00'), utc('2026-09-29T19:23:00'), utc('2026-09-29T07:34:00'),
                   utc('2026-09-29T06:29:00'), utc('2026-09-28T12:43:00')]
        self.assertEqual(release['derive'](commits[0], commits), ('2026-09-29', 3))
        self.assertEqual(release['derive'](commits[3], commits[3:]), ('2026-09-28', 2))
        self.assertEqual(release['name'](('2026-09-29', 10)), '2026-09-29-r10')
        self.assertEqual(release['parse']('2026-09-29-r10'), ('2026-09-29', 10))
        self.assertEqual(release['folder'](('2026-09-29', 10)), '2026-09-29.r10')

    def test_a_commit_counts_once_by_its_prefix_or_once_per_bullet(self):
        entries = release['entries']
        self.assertEqual(entries('fix(update): accept an archive of exactly its size\n\nureq refuses.\n'),
                         [('fix', 'Accept an archive of exactly its size')])
        self.assertEqual(entries('docs: center the row'), [('other', 'Center the row')])
        self.assertEqual(entries('Initial import'), [('other', 'Initial import')])
        self.assertEqual(entries('feat!: macOS icon'), [('feature', 'macOS icon')])
        self.assertEqual(
            entries('feat: icon, builds\n\nIntro.\n\n- macOS ships an icon so Tahoe\n  shows it.\n'
                    '* pinch zoom.\n\nAssisted-by: claude-opus-5.5\n'),
            [('feature', 'macOS ships an icon so Tahoe shows it'), ('feature', 'Pinch zoom')])

    def test_changes_start_after_the_first_build_and_carry_their_commits_versions(self):
        first = release['FIRST']
        commits = {first: ([], utc('2026-09-30T10:00:00'), 'fix: published first'),
                   'b': ([first], utc('2026-09-30T11:00:00'), 'feat: pinch zoom'),
                   'c': (['b'], utc('2026-09-30T12:00:00'), 'fix: two\n\n- one\n- two\n')}
        self.assertEqual(release['changes'](commits, 'c'), [
            {'version': '2026-09-30-r2', 'kind': 'feature', 'title': 'Pinch zoom'},
            {'version': '2026-09-30-r3', 'kind': 'fix', 'title': 'One'},
            {'version': '2026-09-30-r3', 'kind': 'fix', 'title': 'Two'}])

    def test_latest_only_moves_forward(self):
        latest = {'macos-aarch64': '2026-09-29-r9', 'linux-x86_64': '2026-09-30-r1'}
        moved = release['newest'](latest, {'macos-aarch64': {}, 'linux-x86_64': {}, 'macos-10.6': {}},
                                  ('2026-09-29', 10))
        self.assertEqual(moved, {'macos-aarch64': '2026-09-29-r10', 'linux-x86_64': '2026-09-30-r1',
                                 'macos-10.6': '2026-09-29-r10'})
        self.assertEqual(release['newest'](latest, {'macos-aarch64': {}}, ('2026-09-29', 9)), latest)

    def test_minisig_signs_the_files_blake2b_then_that_with_its_comment(self):
        minisign, scope = release['minisign'], release['minisign'].__globals__
        messages = []

        def sign(paths):
            messages.extend(Path(path).read_bytes() for path in paths)
            return [bytes([len(messages)]).hex() * 64 for _ in paths]

        real, scope['sign'] = scope['sign'], sign
        try:
            with tempfile.TemporaryDirectory() as folder:
                file = Path(folder) / 'snowbound-linux-x86_64'
                file.write_bytes(b'an executable')
                minisign([file])
                untrusted, signature, trusted, global_signature = Path(f'{file}.minisig').read_text().splitlines()
        finally:
            scope['sign'] = real
        key_id = base64.b64decode(release['MINISIGN'].read_text().splitlines()[1])[2:10]
        self.assertTrue(untrusted.startswith('untrusted comment: '))
        self.assertEqual(base64.b64decode(signature), b'ED' + key_id + bytes([1]) * 64)
        self.assertRegex(trusted, r'^trusted comment: timestamp:\d+\tfile:snowbound-linux-x86_64\thashed$')
        self.assertEqual(base64.b64decode(global_signature), bytes([2]) * 64)
        self.assertEqual(messages, [hashlib.blake2b(b'an executable').digest(),
                                    bytes([1]) * 64 + trusted.removeprefix('trusted comment: ').encode()])

    def test_build_macos_signs_each_mac_app(self):
        build_mac, scope = release['build_mac'], release['build_mac'].__globals__
        commands = []

        def record(command, **_):
            commands.append(list(map(str, command)))
            if command[0] == 'ditto':
                Path(command[-1]).touch()

        run, scope['run'] = scope['run'], record
        try:
            def signing(platform, developer_id, notarize):
                commands.clear()
                with tempfile.TemporaryDirectory() as stage:
                    build_mac(platform, Path(stage), developer_id, notarize, Path(stage) / 'symbols.zip')
                    build = commands[0]
                    return (build[build.index(f'{stage}/Snowbound.dSYM') + 1:],
                            [command[1] for command in commands[1:]])

            self.assertEqual(signing('macos-aarch64', True, ['--key-id', 'K']),
                             (['--sign', 'developer-id', '--sign-identity', release['IDENTITY'], '--arch', 'aarch64'],
                              ['-c', '-c', 'notarytool', 'stapler', '-c']))
            self.assertEqual(signing('macos-x86_64', True, ['--key-id', 'K']),
                             (['--sign', 'developer-id', '--sign-identity', release['IDENTITY'], '--arch', 'x86_64'],
                              ['-c', '-c', 'notarytool', 'stapler', '-c']))
            self.assertEqual(signing('macos-aarch64', False, None), (['--sign', 'ad-hoc', '--arch', 'aarch64'], ['-c', '-c']))
            self.assertEqual(signing('macos-10.6', True, ['--key-id', 'K']), (['--snow-leopard'], ['-c', '-c']))
        finally:
            scope['run'] = run


if __name__ == '__main__':
    unittest.main()
