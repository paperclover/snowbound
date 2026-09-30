from datetime import datetime, timezone
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

    def test_latest_only_moves_forward(self):
        latest = {'macos-aarch64': '2026-09-29-r9', 'linux-x86_64': '2026-09-30-r1'}
        moved = release['newest'](latest, {'macos-aarch64': {}, 'linux-x86_64': {}, 'macos-10.6': {}},
                                  ('2026-09-29', 10))
        self.assertEqual(moved, {'macos-aarch64': '2026-09-29-r10', 'linux-x86_64': '2026-09-30-r1',
                                 'macos-10.6': '2026-09-29-r10'})
        self.assertEqual(release['newest'](latest, {'macos-aarch64': {}}, ('2026-09-29', 9)), latest)

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
                    build_mac(platform, Path(stage), developer_id, notarize)
                    build = commands[0]
                    return (build[build.index(f'{stage}/Snowbound.app') + 1:],
                            [command[1] for command in commands[1:]])

            self.assertEqual(signing('macos-aarch64', True, ['--key-id', 'K']),
                             (['--sign', 'developer-id', '--sign-identity', release['IDENTITY'], '--arch', 'aarch64'],
                              ['-c', 'notarytool', 'stapler', '-c']))
            self.assertEqual(signing('macos-x86_64', True, ['--key-id', 'K']),
                             (['--sign', 'developer-id', '--sign-identity', release['IDENTITY'], '--arch', 'x86_64'],
                              ['-c', 'notarytool', 'stapler', '-c']))
            self.assertEqual(signing('macos-aarch64', False, None), (['--sign', 'ad-hoc', '--arch', 'aarch64'], ['-c']))
            self.assertEqual(signing('macos-10.6', True, ['--key-id', 'K']), (['--snow-leopard'], ['-c']))
        finally:
            scope['run'] = run


if __name__ == '__main__':
    unittest.main()
