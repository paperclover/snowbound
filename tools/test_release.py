from datetime import datetime, timezone
from pathlib import Path
import runpy
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


if __name__ == '__main__':
    unittest.main()
